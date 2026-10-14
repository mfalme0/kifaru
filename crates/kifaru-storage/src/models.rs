//! Row types and fallible conversions into domain entities.
//!
//! Every conversion that parses a string column returns a `Result`. The previous
//! draft of this file defaulted an unrecognised `status` to `Suspended`; that is
//! exactly the kind of silent coercion that turns a schema bug into a security
//! incident, so it is gone.

use crate::error::StorageError;
use chrono::{DateTime, Utc};
use kifaru_domain::{
    Account, AccountId, AccountStatus, Currency, EntryDirection, FailureReason, LedgerEntry, Money,
    Transfer, TransferId, TransferStatus,
};
use uuid::Uuid;

/// `accounts` row.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AccountRow {
    pub id: Uuid,
    pub status: String,
    pub currency: String,
    pub balance_minor: i64,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<AccountRow> for Account {
    type Error = StorageError;

    fn try_from(row: AccountRow) -> Result<Self, Self::Error> {
        let status = parse_account_status(&row.status)?;
        let currency = parse_currency(&row.currency)?;
        // Negative balances are impossible by CHECK constraint (INV-1). If one
        // is ever observed, refuse to build the entity rather than panic.
        let balance = Money::from_minor_units(row.balance_minor, currency).map_err(|e| {
            StorageError::RowInvariant(format!("accounts.balance_minor={}: {e}", row.balance_minor))
        })?;
        Ok(Account::from_parts(
            AccountId::from_uuid(row.id),
            status,
            currency,
            balance,
            row.version,
            row.created_at,
        ))
    }
}

/// `transfers` row.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct TransferRow {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub destination_account_id: Uuid,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<TransferRow> for Transfer {
    type Error = StorageError;

    fn try_from(row: TransferRow) -> Result<Self, Self::Error> {
        let status = parse_transfer_status(&row.status)?;
        let currency = parse_currency(&row.currency)?;
        let amount = Money::from_minor_units(row.amount_minor, currency).map_err(|e| {
            StorageError::RowInvariant(format!("transfers.amount_minor={}: {e}", row.amount_minor))
        })?;
        Ok(Transfer::from_parts(
            TransferId::from_uuid(row.id),
            AccountId::from_uuid(row.source_account_id),
            AccountId::from_uuid(row.destination_account_id),
            amount,
            status,
            row.failure_reason.map(FailureReason::new),
            row.created_at,
            row.updated_at,
        ))
    }
}

/// `ledger_entries` row.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct LedgerEntryRow {
    pub id: Uuid,
    pub transfer_id: Uuid,
    pub account_id: Uuid,
    pub direction: String,
    pub amount_minor: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<LedgerEntryRow> for LedgerEntry {
    type Error = StorageError;

    fn try_from(row: LedgerEntryRow) -> Result<Self, Self::Error> {
        let direction = parse_entry_direction(&row.direction)?;
        let currency = parse_currency(&row.currency)?;
        let amount = Money::from_minor_units(row.amount_minor, currency).map_err(|e| {
            StorageError::RowInvariant(format!(
                "ledger_entries.amount_minor={}: {e}",
                row.amount_minor
            ))
        })?;
        Ok(LedgerEntry::from_parts(
            row.id,
            TransferId::from_uuid(row.transfer_id),
            AccountId::from_uuid(row.account_id),
            direction,
            amount,
            row.created_at,
        ))
    }
}

/// `idempotency_keys` row, projected to just what the resolve path needs.
#[derive(sqlx::FromRow, Debug)]
pub struct IdempotencyRow {
    pub request_hash: Vec<u8>,
    pub transfer_id: Option<Uuid>,
    pub response_status: Option<i16>,
    /// For a committed success this is the outbox payload (informational). For a
    /// recorded rejection it is the serialised [`kifaru_application::Rejection`],
    /// which is what lets a retry reproduce the original typed error.
    pub response_body: Option<serde_json::Value>,
}

/// `outbox_events` row.
#[derive(sqlx::FromRow, Debug, Clone)]
pub struct OutboxEventRow {
    pub id: Uuid,
    pub aggregate_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub state: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Lifecycle of a single outbox event.
///
/// `Pending` is the only state a worker may claim. The other two are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutboxState {
    Pending,
    Delivered,
    Dead,
}

impl OutboxState {
    pub const fn as_str(self) -> &'static str {
        match self {
            OutboxState::Pending => "pending",
            OutboxState::Delivered => "delivered",
            OutboxState::Dead => "dead",
        }
    }

    /// Parse the `state` column.
    ///
    /// An unrecognised value is an error rather than a default: silently
    /// treating an unknown state as `pending` would make a dead-lettered event
    /// look claimable again.
    pub fn parse(raw: &str) -> Result<Self, StorageError> {
        match raw {
            "pending" => Ok(OutboxState::Pending),
            "delivered" => Ok(OutboxState::Delivered),
            "dead" => Ok(OutboxState::Dead),
            other => Err(StorageError::RowInvariant(format!(
                "unknown outbox state {other:?}"
            ))),
        }
    }
}

pub(crate) fn parse_account_status(raw: &str) -> Result<AccountStatus, StorageError> {
    match raw {
        "active" => Ok(AccountStatus::Active),
        "suspended" => Ok(AccountStatus::Suspended),
        "closed" => Ok(AccountStatus::Closed),
        other => Err(StorageError::RowInvariant(format!(
            "unknown account status {other:?}"
        ))),
    }
}

pub(crate) fn parse_transfer_status(raw: &str) -> Result<TransferStatus, StorageError> {
    match raw {
        "pending" => Ok(TransferStatus::Pending),
        "completed" => Ok(TransferStatus::Completed),
        "failed" => Ok(TransferStatus::Failed),
        other => Err(StorageError::RowInvariant(format!(
            "unknown transfer status {other:?}"
        ))),
    }
}

pub(crate) fn parse_entry_direction(raw: &str) -> Result<EntryDirection, StorageError> {
    match raw {
        "debit" => Ok(EntryDirection::Debit),
        "credit" => Ok(EntryDirection::Credit),
        other => Err(StorageError::RowInvariant(format!(
            "unknown ledger direction {other:?}"
        ))),
    }
}

pub(crate) fn parse_currency(raw: &str) -> Result<Currency, StorageError> {
    match raw {
        "KES" => Ok(Currency::Kes),
        other => Err(StorageError::RowInvariant(format!(
            "unknown currency {other:?}"
        ))),
    }
}
