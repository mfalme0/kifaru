//! Transactional-outbox persistence.
//!
//! # The guarantee
//!
//! At-least-once delivery. An event is claimed under a lease, published, and
//! only then marked delivered. A worker that dies between "publish" and "mark
//! delivered" causes a redelivery after the lease expires — which is exactly the
//! behaviour a consumer must be built to tolerate, and why
//! [`PgOutboxStore::try_record_consumption`] exists.
//!
//! **There is no exactly-once here and none is claimed.** Making delivery
//! effectively-once is the consumer's job, enforced by `consumer_inbox`.
//!
//! # Claiming
//!
//! One statement does claim-and-lease:
//!
//! ```sql
//! WITH candidates AS (
//!     SELECT id FROM outbox_events
//!     WHERE  state = 'pending'
//!        AND  next_attempt_at <= now()
//!        AND (locked_until IS NULL OR locked_until < now())
//!     ORDER BY next_attempt_at, created_at
//!     LIMIT $1
//!     FOR UPDATE SKIP LOCKED
//! )
//! UPDATE ... FROM candidates ...
//! ```
//!
//! `SKIP LOCKED` is what lets several workers drain the same queue concurrently
//! without coordination: a worker skips rows another worker already holds rather
//! than blocking behind it. Claiming and leasing in one statement means there is
//! no window in which a row is selected but not leased, and therefore no window
//! in which two workers could both believe they own the same event.

use crate::error::StorageError;
use crate::models::{OutboxEventRow, OutboxState};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// An event claimed by a worker, together with its attempt count.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEvent {
    pub id: Uuid,
    pub aggregate_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    /// Attempts *including* the claim that produced this row. After the first
    /// claim this is `1`, so it is already incremented when a failure is
    /// recorded.
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
}

impl OutboxEvent {
    pub fn from_row(row: OutboxEventRow) -> Result<Self, StorageError> {
        // Claimed rows are pending by construction; parse rather than trust, so
        // a schema drift is caught here instead of being silently ignored.
        OutboxState::parse(&row.state)?;
        Ok(Self {
            id: row.id,
            aggregate_id: row.aggregate_id,
            event_type: row.event_type,
            payload: row.payload,
            attempts: row.attempts,
            created_at: row.created_at,
        })
    }
}

#[derive(Clone)]
pub struct PgOutboxStore {
    pool: PgPool,
}

impl PgOutboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Claim up to `limit` claimable events, leasing them to `worker_id`.
    ///
    /// An empty result means the queue is drained (or nothing is due yet); it is
    /// not an error.
    pub async fn claim_batch(
        &self,
        worker_id: &str,
        lease: std::time::Duration,
        limit: i64,
    ) -> Result<Vec<OutboxEvent>, StorageError> {
        if limit <= 0 {
            return Ok(Vec::new());
        }

        let rows: Vec<OutboxEventRow> = sqlx::query_as(
            r#"WITH candidates AS (
                   SELECT id
                   FROM outbox_events
                   WHERE state = 'pending'
                     AND next_attempt_at <= now()
                     AND (locked_until IS NULL OR locked_until < now())
                   ORDER BY next_attempt_at, created_at
                   LIMIT $1
                   FOR UPDATE SKIP LOCKED
               )
               UPDATE outbox_events AS o
               SET locked_until = now() + make_interval(secs => $2::double precision),
                   locked_by     = $3,
                   attempts      = o.attempts + 1
               FROM candidates
               WHERE o.id = candidates.id
               RETURNING o.id, o.aggregate_id, o.event_type, o.payload, o.state,
                         o.attempts, o.last_error, o.created_at, o.delivered_at"#,
        )
        .bind(limit)
        .bind(lease.as_secs_f64())
        .bind(worker_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(OutboxEvent::from_row).collect()
    }

    /// Mark an event delivered and release its lease.
    ///
    /// `last_error` is cleared: the event no longer has a failure, and leaving a
    /// stale reason on a delivered event would misreport its history.
    pub async fn mark_delivered(&self, id: Uuid) -> Result<(), StorageError> {
        let affected = sqlx::query(
            r#"UPDATE outbox_events
               SET state        = 'delivered',
                   delivered_at = now(),
                   locked_until = NULL,
                   locked_by     = NULL,
                   last_error    = NULL
               WHERE id = $1"#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        expect_one(affected, id, "mark_delivered")
    }

    /// Record a failed delivery attempt.
    ///
    /// If the event has exhausted `max_attempts` it is moved to `dead` (and the
    /// database refuses to let a dead event exist without this error — see
    /// migration 0002). Otherwise it returns to `pending`, releases its lease,
    /// and is rescheduled after `backoff`.
    ///
    /// Returns the resulting state so the caller can log or count it.
    pub async fn record_failure(
        &self,
        id: Uuid,
        error: &str,
        max_attempts: i32,
        backoff: std::time::Duration,
    ) -> Result<OutboxState, StorageError> {
        let row: (String,) = sqlx::query_as(
            r#"UPDATE outbox_events
               SET state        = CASE WHEN attempts >= $2 THEN 'dead' ELSE 'pending' END,
                   last_error    = $3,
                   locked_until  = NULL,
                   locked_by     = NULL,
                   next_attempt_at = CASE
                       WHEN attempts >= $2 THEN next_attempt_at
                       ELSE now() + make_interval(secs => $4::double precision)
                   END
               WHERE id = $1
               RETURNING state"#,
        )
        .bind(id)
        .bind(max_attempts)
        .bind(error)
        .bind(backoff.as_secs_f64())
        .fetch_one(&self.pool)
        .await?;

        OutboxState::parse(&row.0)
    }

    /// Attempt to record that `consumer` has processed `event_id`.
    ///
    /// Returns `true` if this is the first time this consumer has seen the event
    /// (so the side effect should run), and `false` if it is a redelivery (so
    /// the side effect must be skipped).
    ///
    /// This is the consumer-side half of the at-least-once contract. Without it,
    /// at-least-once is just at-least-twice.
    pub async fn try_record_consumption(
        &self,
        consumer: &str,
        event_id: Uuid,
    ) -> Result<bool, StorageError> {
        let inserted = sqlx::query(
            r#"INSERT INTO consumer_inbox (consumer, event_id)
               VALUES ($1, $2)
               ON CONFLICT (consumer, event_id) DO NOTHING
               RETURNING consumer"#,
        )
        .bind(consumer)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(inserted.is_some())
    }

    /// Whether `consumer` has already processed `event_id`.
    pub async fn has_consumed(&self, consumer: &str, event_id: Uuid) -> Result<bool, StorageError> {
        let seen: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM consumer_inbox WHERE consumer = $1 AND event_id = $2)",
        )
        .bind(consumer)
        .bind(event_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(seen)
    }

    /// Event counts per state, for logging and readiness checks.
    pub async fn count_by_state(&self) -> Result<Vec<(OutboxState, i64)>, StorageError> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT state, count(*) FROM outbox_events GROUP BY state")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|(raw, count)| Ok((OutboxState::parse(&raw)?, count)))
            .collect()
    }
}

/// Every outbox row is addressed by a primary key that the claim returned, so a
/// zero-row update means the row was removed or the id was wrong — never a
/// benign no-op.
fn expect_one(affected: u64, id: Uuid, operation: &str) -> Result<(), StorageError> {
    if affected == 1 {
        Ok(())
    } else {
        Err(StorageError::RowInvariant(format!(
            "{operation} on outbox event {id} affected {affected} rows, expected exactly 1"
        )))
    }
}
