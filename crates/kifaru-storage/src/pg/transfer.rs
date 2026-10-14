//! The atomic transfer adapter.
//!
//! # The guarantee this file exists to provide
//!
//! Everything a transfer changes — both account balances, the transfer row, both
//! ledger entries, the outbox event, and the idempotency record — is written
//! inside **one** PostgreSQL transaction. Either all of it is durable or none of
//! it is. There is no code path in which a balance moves without a ledger entry,
//! or a ledger entry exists without its transfer.
//!
//! # Concurrency
//!
//! Deadlock avoidance is by lock ordering: both accounts are locked with a single
//! `SELECT ... WHERE id = ANY($1) ORDER BY id FOR UPDATE`. Because PostgreSQL
//! takes row locks in the order the executor produces rows, and the plan is
//! ordered by primary key, two concurrent transfers touching the same pair of
//! accounts always acquire locks in the same order. `READ COMMITTED` (the
//! default) is sufficient; the explicit row locks are what make it correct.
//!
//! # Idempotency
//!
//! The key is claimed with `INSERT ... ON CONFLICT DO NOTHING`. If the insert
//! wins, this request executes. If it loses, a row already exists, and the
//! subsequent `SELECT ... FOR UPDATE` blocks until the competing transaction
//! commits — at which point the committed outcome is replayed. Two concurrent
//! requests with the same key therefore cannot both execute, and the second one
//! cannot observe an in-flight row as if it were finished.
//!
//! # Why a rejected transfer is still recorded
//!
//! A business rejection (insufficient funds, inactive account, currency
//! mismatch) commits a `failed` transfer row and the idempotency record, but
//! moves **no** money and writes **no** ledger entries. Recording the rejection
//! is what makes a retry deterministic: the same key returns the same answer
//! instead of being re-evaluated against balances that may have changed in the
//! meantime. The transfer row is only written when both accounts exist, because
//! the schema's foreign key would otherwise reject it.

use crate::error::StorageError;
use crate::models::{AccountRow, IdempotencyRow, LedgerEntryRow, TransferRow};
use async_trait::async_trait;
use chrono::Utc;
use kifaru_application::ports::{
    ExecuteOutcome, ExecuteTransfer, Rejection, TransferReader, TransferRepository,
};
use kifaru_domain::{
    Account, FailureReason, LedgerTransaction, Transfer, TransferError, TransferId, TransferStatus,
};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

/// HTTP status recorded against a committed transfer.
const STATUS_CREATED: i16 = 201;
/// HTTP status recorded against a rejected transfer.
const STATUS_UNPROCESSABLE: i16 = 422;

#[derive(Clone)]
pub struct PgTransferRepository {
    pool: PgPool,
}

impl PgTransferRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl TransferRepository for PgTransferRepository {
    type Err = StorageError;

    async fn execute(&self, request: ExecuteTransfer) -> Result<ExecuteOutcome, StorageError> {
        let mut tx = self.pool.begin().await?;

        // ---- 1. Claim the idempotency key ---------------------------------
        let claimed = sqlx::query(
            r#"INSERT INTO idempotency_keys (client_id, key, request_hash)
               VALUES ($1, $2, $3)
               ON CONFLICT (client_id, key) DO NOTHING
               RETURNING client_id"#,
        )
        .bind(&request.client_id)
        .bind(&request.idempotency_key)
        .bind(request.request_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?;

        if claimed.is_none() {
            return resolve_existing(&mut tx, &request).await;
        }

        // ---- 2. Lock both accounts in ascending id order -------------------
        let mut ids = vec![
            request.transfer.source.as_uuid(),
            request.transfer.destination.as_uuid(),
        ];
        ids.sort_unstable();
        ids.dedup();

        let rows = sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, status, currency, balance_minor, version, created_at
               FROM accounts
               WHERE id = ANY($1)
               ORDER BY id
               FOR UPDATE"#,
        )
        .bind(ids.as_slice())
        .fetch_all(&mut *tx)
        .await?;

        let mut source: Option<Account> = None;
        let mut destination: Option<Account> = None;
        for row in rows {
            let account = Account::try_from(row)?;
            if account.id == request.transfer.source {
                source = Some(account);
            } else if account.id == request.transfer.destination {
                destination = Some(account);
            }
        }

        // A missing account cannot be recorded as a failed transfer: the schema
        // foreign key would reject the row. So this rolls the whole thing back,
        // releasing the idempotency key, and a corrected request is not blocked.
        let (Some(mut source), Some(mut destination)) = (source, destination) else {
            return Ok(ExecuteOutcome::AccountNotFound);
        };

        // ---- 3. Validate against the locked, current balances --------------
        let amount = request.transfer.amount;
        let violation = source
            .ensure_can_debit(amount)
            .err()
            .or_else(|| destination.ensure_can_credit(amount).err());

        if let Some(err) = violation {
            let rejection = rejection_for(&err, &source, &destination);
            let mut rejected = request.transfer.clone();
            rejected
                .fail(FailureReason::new(err.to_string()), Utc::now())
                .map_err(|e| StorageError::RowInvariant(e.to_string()))?;

            // The rejection is recorded verbatim so that a retry with the same
            // key reproduces this exact error instead of re-evaluating against
            // balances that may have changed since.
            let body = serde_json::to_value(&rejection).map_err(|e| {
                StorageError::RowInvariant(format!("could not encode rejection: {e}"))
            })?;

            insert_transfer(&mut tx, &rejected).await?;
            finalize_idempotency(
                &mut tx,
                &request,
                &rejected.id,
                STATUS_UNPROCESSABLE,
                Some(&body),
            )
            .await?;
            tx.commit().await?;

            return Ok(ExecuteOutcome::Rejected {
                transfer_id: rejected.id,
                rejection,
            });
        }

        // ---- 4. Apply the movement atomically ------------------------------
        let settled_at = Utc::now();
        // Both calls were pre-validated above; a failure here is a logic bug,
        // not a business outcome.
        source.debit(amount).map_err(domain_row_error)?;
        destination.credit(amount).map_err(domain_row_error)?;

        let mut completed = request.transfer.clone();
        completed
            .complete(settled_at)
            .map_err(|e| StorageError::RowInvariant(e.to_string()))?;

        update_balance(&mut tx, &source).await?;
        update_balance(&mut tx, &destination).await?;
        insert_transfer(&mut tx, &completed).await?;
        insert_ledger(&mut tx, &request.ledger).await?;

        let payload = outbox_payload(&completed);
        insert_outbox(&mut tx, &completed, &payload).await?;
        finalize_idempotency(
            &mut tx,
            &request,
            &completed.id,
            STATUS_CREATED,
            Some(&payload),
        )
        .await?;

        tx.commit().await?;

        Ok(ExecuteOutcome::Committed {
            transfer_id: completed.id,
            committed_at: settled_at,
        })
    }

    fn is_retriable(err: &StorageError) -> bool {
        err.is_retriable()
    }
}

#[async_trait]
impl TransferReader for PgTransferRepository {
    type Err = StorageError;

    async fn find_transfer(&self, id: TransferId) -> Result<Option<Transfer>, StorageError> {
        let row = sqlx::query_as::<_, TransferRow>(
            r#"SELECT id, source_account_id, destination_account_id, amount_minor, currency,
                      status, failure_reason, created_at, updated_at
               FROM transfers
               WHERE id = $1"#,
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;

        row.map(Transfer::try_from).transpose()
    }

    async fn ledger_for_transfer(
        &self,
        id: TransferId,
    ) -> Result<Vec<kifaru_domain::LedgerEntry>, StorageError> {
        let rows = sqlx::query_as::<_, LedgerEntryRow>(
            r#"SELECT id, transfer_id, account_id, direction, amount_minor, currency, created_at
               FROM ledger_entries
               WHERE transfer_id = $1
               ORDER BY direction"#,
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(kifaru_domain::LedgerEntry::try_from)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Idempotency resolution
// ---------------------------------------------------------------------------

/// A key already exists. Decide between replay and conflict.
async fn resolve_existing(
    tx: &mut Transaction<'_, Postgres>,
    request: &ExecuteTransfer,
) -> Result<ExecuteOutcome, StorageError> {
    // If the competing transaction is still in flight, this blocks until it
    // finishes. That is the point: we must not read a half-finished row.
    let existing = sqlx::query_as::<_, IdempotencyRow>(
        r#"SELECT request_hash, transfer_id, response_status, response_body
           FROM idempotency_keys
           WHERE client_id = $1 AND key = $2
           FOR UPDATE"#,
    )
    .bind(&request.client_id)
    .bind(&request.idempotency_key)
    .fetch_one(&mut **tx)
    .await?;

    // Same key, different body: refuse rather than guess which body the caller
    // meant. This is the only 409 path.
    if existing.request_hash != request.request_hash {
        return Ok(ExecuteOutcome::IdempotencyConflict);
    }

    // A committed row without a transfer would mean a row written by a code
    // path that does not exist in this build. Refuse rather than invent a result.
    let Some(transfer_id) = existing.transfer_id else {
        return Ok(ExecuteOutcome::IdempotencyConflict);
    };

    let row = sqlx::query_as::<_, TransferRow>(
        r#"SELECT id, source_account_id, destination_account_id, amount_minor, currency,
                  status, failure_reason, created_at, updated_at
           FROM transfers
           WHERE id = $1"#,
    )
    .bind(transfer_id)
    .fetch_one(&mut **tx)
    .await?;

    let transfer = Transfer::try_from(row)?;

    // Reconstruct the original outcome. For a rejected transfer the recorded
    // body carries the typed rejection, so the caller gets the same error the
    // original attempt produced.
    let rejection = if transfer.status == TransferStatus::Failed {
        match existing.response_body.as_ref() {
            Some(body) => Some(
                serde_json::from_value::<Rejection>(body.clone()).map_err(|e| {
                    StorageError::RowInvariant(format!("recorded rejection is unreadable: {e}"))
                })?,
            ),
            // A failed transfer with no recorded reason cannot be replayed
            // faithfully. Refuse rather than invent an outcome.
            None => return Ok(ExecuteOutcome::IdempotencyConflict),
        }
    } else {
        None
    };

    Ok(ExecuteOutcome::Replayed {
        transfer_id: TransferId::from_uuid(transfer_id),
        status: transfer.status,
        created_at: transfer.created_at,
        rejection,
    })
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

async fn update_balance(conn: &mut PgConnection, account: &Account) -> Result<(), StorageError> {
    let affected = sqlx::query(
        r#"UPDATE accounts
           SET balance_minor = $1, version = version + 1
           WHERE id = $2"#,
    )
    .bind(account.balance.minor_units())
    .bind(account.id.as_uuid())
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if affected != 1 {
        // The row is locked by this transaction, so it cannot have vanished.
        return Err(StorageError::RowInvariant(format!(
            "expected to update 1 account row for {}, updated {affected}",
            account.id
        )));
    }
    Ok(())
}

async fn insert_transfer(conn: &mut PgConnection, transfer: &Transfer) -> Result<(), StorageError> {
    sqlx::query(
        r#"INSERT INTO transfers (
               id, source_account_id, destination_account_id, amount_minor, currency,
               status, failure_reason, created_at, updated_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(transfer.id.as_uuid())
    .bind(transfer.source.as_uuid())
    .bind(transfer.destination.as_uuid())
    .bind(transfer.amount.minor_units())
    .bind(transfer.amount.currency().code())
    .bind(transfer.status.to_string())
    .bind(
        transfer
            .failure_reason
            .as_ref()
            .map(|r| r.as_str().to_owned()),
    )
    .bind(transfer.created_at)
    .bind(transfer.updated_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_ledger(
    conn: &mut PgConnection,
    ledger: &LedgerTransaction,
) -> Result<(), StorageError> {
    // Re-derive INV-2/INV-3 from the fields before writing them, so a bug that
    // built an unbalanced pair is caught here rather than committed.
    ledger
        .validate()
        .map_err(|e| StorageError::RowInvariant(e.to_string()))?;

    for entry in ledger.entries() {
        sqlx::query(
            r#"INSERT INTO ledger_entries (
                   id, transfer_id, account_id, direction, amount_minor, currency, created_at
               ) VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(entry.id)
        .bind(entry.transfer_id.as_uuid())
        .bind(entry.account_id.as_uuid())
        .bind(entry.direction.to_string())
        .bind(entry.amount.minor_units())
        .bind(entry.amount.currency().code())
        .bind(entry.created_at)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn insert_outbox(
    conn: &mut PgConnection,
    transfer: &Transfer,
    payload: &serde_json::Value,
) -> Result<(), StorageError> {
    sqlx::query(
        r#"INSERT INTO outbox_events (
               id, aggregate_id, event_type, payload, state, next_attempt_at
           ) VALUES ($1, $2, $3, $4, 'pending', now())"#,
    )
    .bind(Uuid::now_v7())
    .bind(transfer.id.as_uuid())
    .bind("transfer.completed")
    .bind(payload)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn finalize_idempotency(
    conn: &mut PgConnection,
    request: &ExecuteTransfer,
    transfer_id: &TransferId,
    status: i16,
    body: Option<&serde_json::Value>,
) -> Result<(), StorageError> {
    sqlx::query(
        r#"UPDATE idempotency_keys
           SET transfer_id = $1, response_status = $2, response_body = $3, completed_at = now()
           WHERE client_id = $4 AND key = $5"#,
    )
    .bind(transfer_id.as_uuid())
    .bind(status)
    .bind(body)
    .bind(&request.client_id)
    .bind(&request.idempotency_key)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Mapping helpers
// ---------------------------------------------------------------------------

fn rejection_for(err: &TransferError, source: &Account, destination: &Account) -> Rejection {
    match err {
        TransferError::InsufficientFunds { balance, required } => Rejection::InsufficientFunds {
            balance: *balance,
            required: *required,
        },
        TransferError::SourceNotActive(_) => Rejection::AccountNotActive {
            account_id: source.id.to_string(),
        },
        TransferError::DestinationNotActive(_) => Rejection::AccountNotActive {
            account_id: destination.id.to_string(),
        },
        TransferError::AccountCurrencyMismatch { .. } => Rejection::CurrencyMismatch,
        // Nothing else can arise from `ensure_can_debit`/`ensure_can_credit` on
        // freshly locked rows, but a total match is required, so anything that
        // somehow does is treated as a refusal rather than an execution.
        _ => Rejection::CurrencyMismatch,
    }
}

fn domain_row_error(err: TransferError) -> StorageError {
    StorageError::RowInvariant(format!("validated transfer still failed in memory: {err}"))
}

fn outbox_payload(transfer: &Transfer) -> serde_json::Value {
    serde_json::json!({
        "transfer_id": transfer.id.to_string(),
        "source_account_id": transfer.source.to_string(),
        "destination_account_id": transfer.destination.to_string(),
        "amount_minor": transfer.amount.minor_units(),
        "currency": transfer.amount.currency().code(),
        "status": transfer.status.to_string(),
        "created_at": transfer.created_at.to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use kifaru_domain::{AccountId, Currency, Money};

    fn completed() -> Transfer {
        let mut t = Transfer::new(
            AccountId::new(),
            AccountId::new(),
            Money::positive(1_234, Currency::Kes).unwrap(),
            Utc.with_ymd_and_hms(2026, 10, 14, 9, 0, 0).unwrap(),
        )
        .unwrap();
        t.complete(Utc.with_ymd_and_hms(2026, 10, 14, 9, 0, 1).unwrap())
            .unwrap();
        t
    }

    #[test]
    fn outbox_payload_carries_the_whole_movement() {
        let t = completed();
        let payload = outbox_payload(&t);
        assert_eq!(payload["amount_minor"], 1_234);
        assert_eq!(payload["currency"], "KES");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["transfer_id"], t.id.to_string());
        assert_eq!(payload["source_account_id"], t.source.to_string());
        assert_eq!(payload["destination_account_id"], t.destination.to_string());
    }

    #[test]
    fn retriable_classification_is_narrow() {
        let invariant = StorageError::RowInvariant("x".into());
        assert!(!invariant.is_retriable());
    }
}
