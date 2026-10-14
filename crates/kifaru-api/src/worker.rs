//! The outbox publisher.
//!
//! # Scope
//!
//! This drains `outbox_events` for transfers that have already committed. It is
//! the producer side of the transactional outbox: the transfer wrote its event in
//! the same transaction as its money movement, so by the time an event exists the
//! transfer is already durable. Nothing here can fail a transfer.
//!
//! # Delivery guarantee
//!
//! **At-least-once.** An event is leased, published, then marked delivered. A
//! crash in between produces a redelivery once the lease expires. There is no
//! exactly-once here and none is claimed. Consumers dedupe via
//! [`kifaru_storage::PgOutboxStore::try_record_consumption`].
//!
//! The worker is safe to run in several replicas at once: claiming uses
//! `FOR UPDATE SKIP LOCKED`, so replicas take disjoint batches instead of
//! contending.

use kifaru_storage::models::OutboxState;
use kifaru_storage::{OutboxEvent, PgOutboxStore, StorageError};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Tuning for the publish loop.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Identifies the lease holder. Appears in `outbox_events.locked_by`, so a
    /// stuck lease can be traced to a specific replica.
    pub worker_id: String,
    pub batch_size: i64,
    /// How long a claim is protected before another worker may retry it. Must
    /// exceed the longest expected publish, or events will be redelivered while
    /// still in flight.
    pub lease: Duration,
    pub max_attempts: i32,
    pub poll_interval: Duration,
    /// First retry delay; doubled per attempt, capped by `backoff_max`.
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub consumer_name: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", std::process::id()),
            batch_size: 50,
            lease: Duration::from_secs(30),
            max_attempts: 8,
            poll_interval: Duration::from_millis(500),
            backoff_base: Duration::from_secs(2),
            backoff_max: Duration::from_secs(300),
            consumer_name: "simulated-consumer".to_string(),
        }
    }
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            worker_id: env_or("KIFARU_WORKER_ID", &defaults.worker_id),
            batch_size: parse_or("KIFARU_WORKER_BATCH_SIZE", defaults.batch_size),
            lease: secs("KIFARU_WORKER_LEASE_SECONDS", 30),
            max_attempts: parse_or("KIFARU_WORKER_MAX_ATTEMPTS", defaults.max_attempts),
            poll_interval: millis(
                "KIFARU_WORKER_POLL_INTERVAL_MS",
                defaults.poll_interval.as_millis() as i64,
            ),
            backoff_base: secs("KIFARU_WORKER_BACKOFF_SECONDS", 2),
            backoff_max: secs("KIFARU_WORKER_BACKOFF_MAX_SECONDS", 300),
            consumer_name: env_or("KIFARU_WORKER_CONSUMER", &defaults.consumer_name),
        }
    }

    /// Exponential backoff for a given attempt, saturating and capped.
    ///
    /// `attempt` is 1-based. Saturating arithmetic matters: a hostile or merely
    /// long-lived `attempts` value must not overflow a release build into a
    /// panic or, worse, a negative interval.
    pub fn backoff_for(&self, attempt: i32) -> Duration {
        // Clamp the exponent to 31: `1u32 << 32` is a shift overflow and panics
        // in a debug build, which is exactly the kind of production panic a
        // hostile or merely long-lived attempt count should not be able to cause.
        let exponent = attempt.saturating_sub(1).clamp(0, 31) as u32;
        let scaled = self.backoff_base.saturating_mul(1u32 << exponent);
        scaled.min(self.backoff_max)
    }

    /// Reject configuration that would corrupt the delivery contract.
    pub fn validate(&self) -> Result<(), WorkerConfigError> {
        if self.batch_size <= 0 {
            return Err(WorkerConfigError::Invalid("batch_size must be positive"));
        }
        if self.max_attempts <= 0 {
            return Err(WorkerConfigError::Invalid(
                "max_attempts must be positive; a worker that never retries is not at-least-once",
            ));
        }
        if self.lease.is_zero() {
            return Err(WorkerConfigError::Invalid(
                "lease must be positive; a zero lease makes every event instantly redeliverable",
            ));
        }
        if self.poll_interval.is_zero() {
            return Err(WorkerConfigError::Invalid("poll_interval must be positive"));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkerConfigError {
    #[error("invalid worker configuration: {0}")]
    Invalid(&'static str),
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn parse_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Seconds from the environment, clamped at zero so a negative value cannot
/// become a nonsensical duration.
fn secs(key: &str, default: i64) -> Duration {
    Duration::from_secs(parse_or::<i64>(key, default).max(0) as u64)
}

fn millis(key: &str, default: i64) -> Duration {
    Duration::from_millis(parse_or::<i64>(key, default).max(0) as u64)
}

/// Why a publish failed.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("transient: {0}")]
    Transient(String),
    #[error("permanent: {0}")]
    Permanent(String),
}

/// Destination for claimed events.
///
/// Kept as a trait so failure injection is a test concern rather than something
/// the production path can be talked into.
#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError>;
}

/// The default publisher: logs the event and considers it delivered.
///
/// Honest about what it is. KIFARU has no broker, so "publishing" means
/// recording that the event would have been handed to one. The durable part
/// being demonstrated is the outbox itself — the claim, the lease, the retry, and
/// the dedupe on the consumer side.
pub struct LoggingPublisher;

#[async_trait::async_trait]
impl EventPublisher for LoggingPublisher {
    async fn publish(&self, event: &OutboxEvent) -> Result<(), PublishError> {
        tracing::info!(
            event_id = %event.id,
            event_type = %event.event_type,
            aggregate_id = %event.aggregate_id,
            attempt = event.attempts,
            "published outbox event"
        );
        Ok(())
    }
}

/// A consumer that dedupes by `event_id` through `consumer_inbox`.
///
/// Stands in for a downstream service. Its value is that it exercises the real
/// consumer-side dedupe: delivering the same event twice performs the side
/// effect once.
pub struct DedupeConsumer {
    store: PgOutboxStore,
    consumer: String,
}

impl DedupeConsumer {
    pub fn new(store: PgOutboxStore, consumer: impl Into<String>) -> Self {
        Self {
            store,
            consumer: consumer.into(),
        }
    }

    /// Deliver an event to this consumer, returning whether it was a duplicate.
    pub async fn consume(&self, event: &OutboxEvent) -> Result<bool, StorageError> {
        let first = self
            .store
            .try_record_consumption(&self.consumer, event.id)
            .await?;
        if first {
            tracing::info!(
                event_id = %event.id,
                consumer = %self.consumer,
                "consumer processed event"
            );
        } else {
            tracing::info!(
                event_id = %event.id,
                consumer = %self.consumer,
                "duplicate delivery skipped"
            );
        }
        Ok(!first)
    }
}

/// What one pass over the queue did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchReport {
    pub claimed: usize,
    pub delivered: usize,
    pub retried: usize,
    pub dead: usize,
}

pub struct OutboxWorker {
    store: PgOutboxStore,
    publisher: Arc<dyn EventPublisher>,
    config: WorkerConfig,
}

impl OutboxWorker {
    pub fn new(
        store: PgOutboxStore,
        publisher: Arc<dyn EventPublisher>,
        config: WorkerConfig,
    ) -> Result<Self, WorkerConfigError> {
        config.validate()?;
        Ok(Self {
            store,
            publisher,
            config,
        })
    }

    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    /// Claim one batch and publish it.
    ///
    /// Publish failures are handled per-event: one poison event must not abort
    /// the batch and strand the rest. A failure to *record* that failure does
    /// abort, because the lease is then still held and the event would silently
    /// stall until it expired.
    pub async fn run_once(&self) -> Result<BatchReport, StorageError> {
        let mut report = BatchReport::default();

        let events = self
            .store
            .claim_batch(
                &self.config.worker_id,
                self.config.lease,
                self.config.batch_size,
            )
            .await?;
        report.claimed = events.len();

        for event in events {
            match self.publisher.publish(&event).await {
                Ok(()) => {
                    self.store.mark_delivered(event.id).await?;
                    report.delivered += 1;
                }
                Err(err) => {
                    let state = self
                        .store
                        .record_failure(
                            event.id,
                            &err.to_string(),
                            self.config.max_attempts,
                            self.config.backoff_for(event.attempts),
                        )
                        .await?;

                    match state {
                        OutboxState::Dead => {
                            report.dead += 1;
                            tracing::error!(
                                event_id = %event.id,
                                event_type = %event.event_type,
                                attempts = event.attempts,
                                error = %err,
                                "outbox event exhausted its attempts and was dead-lettered"
                            );
                        }
                        OutboxState::Pending => {
                            report.retried += 1;
                            tracing::warn!(
                                event_id = %event.id,
                                event_type = %event.event_type,
                                attempts = event.attempts,
                                error = %err,
                                "outbox event failed and will be retried"
                            );
                        }
                        // `record_failure` only ever returns Pending or Dead.
                        OutboxState::Delivered => {
                            unreachable!("record_failure cannot return Delivered");
                        }
                    }
                }
            }
        }

        Ok(report)
    }

    /// Poll until `shutdown` resolves.
    ///
    /// An empty batch is not an error and not a reason to spin: the loop sleeps
    /// for `poll_interval` either way, so an idle queue costs one cheap query
    /// per interval.
    pub async fn run_until(&self, shutdown: impl Future<Output = ()>) -> Result<(), StorageError> {
        tokio::pin!(shutdown);

        tracing::info!(
            worker_id = %self.config.worker_id,
            batch_size = self.config.batch_size,
            lease_seconds = self.config.lease.as_secs(),
            max_attempts = self.config.max_attempts,
            "outbox worker started"
        );

        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => {
                    tracing::info!(worker_id = %self.config.worker_id, "shutdown requested, stopping");
                    return Ok(());
                }
                result = self.run_once() => {
                    match result {
                        Ok(report) if report.claimed > 0 => {
                            tracing::info!(
                                claimed = report.claimed,
                                delivered = report.delivered,
                                retried = report.retried,
                                dead = report.dead,
                                "outbox batch processed"
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            // A database blip must not kill the worker: the lease
                            // protects every claimed event, so backing off is
                            // always safe.
                            tracing::error!(error = %err, "outbox batch failed; will retry");
                        }
                    }
                    tokio::time::sleep(self.config.poll_interval).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> WorkerConfig {
        WorkerConfig {
            worker_id: "test".into(),
            batch_size: 10,
            lease: Duration::from_secs(30),
            max_attempts: 3,
            poll_interval: Duration::from_millis(10),
            backoff_base: Duration::from_secs(2),
            backoff_max: Duration::from_secs(60),
            consumer_name: "test".into(),
        }
    }

    #[test]
    fn backoff_doubles_per_attempt() {
        let c = config();
        assert_eq!(c.backoff_for(1), Duration::from_secs(2));
        assert_eq!(c.backoff_for(2), Duration::from_secs(4));
        assert_eq!(c.backoff_for(3), Duration::from_secs(8));
    }

    #[test]
    fn backoff_is_capped() {
        let c = config();
        assert_eq!(c.backoff_for(30), c.backoff_max);
        assert_eq!(c.backoff_for(1_000), c.backoff_max);
    }

    #[test]
    fn backoff_saturates_instead_of_overflowing() {
        let c = config();
        // A hostile attempt count must not panic or wrap to a negative interval.
        assert_eq!(c.backoff_for(i32::MAX), c.backoff_max);
        assert_eq!(c.backoff_for(i32::MIN), Duration::from_secs(2));
        assert_eq!(c.backoff_for(0), Duration::from_secs(2));
    }

    #[test]
    fn a_zero_lease_is_rejected() {
        let mut c = config();
        c.lease = Duration::ZERO;
        assert!(c.validate().is_err());
    }

    #[test]
    fn a_zero_budget_is_rejected() {
        let mut c = config();
        c.max_attempts = 0;
        assert!(c.validate().is_err());

        let mut c = config();
        c.batch_size = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn the_default_configuration_is_valid() {
        assert!(WorkerConfig::default().validate().is_ok());
    }
}
