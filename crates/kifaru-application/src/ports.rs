//! Ports (traits) the application layer depends on.
//!
//! # Why the transaction boundary lives in the adapter
//!
//! A transfer must commit its balance updates, transfer row, ledger entries, and
//! outbox event as **one** database transaction. If the port surface exposed
//! per-entity methods that each opened their own connection, that guarantee would
//! be structurally impossible to keep — the caller could not express "all of
//! this, or none of it" through the trait.
//!
//! So [`TransferRepository::execute`] is deliberately coarse: it takes a fully
//! validated domain request and its contract is *atomicity*. The adapter owns the
//! SQL and the `BEGIN`/`COMMIT`. The application service owns validation,
//! idempotency orchestration, and retry policy.
//!
//! This also keeps `sqlx` out of the application crate entirely, so the service
//! logic can be tested against an in-memory implementation of the trait.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kifaru_domain::{
    Account, AccountId, Currency, LedgerEntry, LedgerTransaction, Transfer, TransferId,
    TransferStatus,
};
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Everything needed to attempt one transfer, already validated by the domain.
///
/// The caller has already built `transfer` and `ledger` through domain
/// constructors, so the pairing is balanced and the amount is strictly positive
/// before the adapter is ever called. The adapter does not re-derive business
/// rules; it enforces concurrency and persistence.
#[derive(Debug, Clone)]
pub struct ExecuteTransfer {
    pub transfer: Transfer,
    pub ledger: LedgerTransaction,
    /// Who is making the request. Scopes the idempotency key.
    ///
    /// In Phase 1 this is a server-issued development identity and is **not** a
    /// security control. See `docs/threat-model.md`.
    pub client_id: String,
    pub idempotency_key: String,
    /// Hash of the canonical request payload. Used to detect the same key
    /// reused with a *different* body, which must be rejected rather than
    /// silently executed.
    pub request_hash: Vec<u8>,
}

/// A business rejection, recorded so that a retry replays the *same* answer.
///
/// A rejected transfer still gets a row (status `failed`) and an idempotency
/// record, so re-sending the same key does not re-evaluate the request against
/// balances that may have changed since. Serialised into
/// `idempotency_keys.response_body` verbatim, which is what makes the replay
/// reconstruct the original typed error instead of a generic one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "rejection", rename_all = "snake_case")]
pub enum Rejection {
    InsufficientFunds { balance: i64, required: i64 },
    AccountNotActive { account_id: String },
    CurrencyMismatch,
}

/// The result of one atomic transfer attempt.
///
/// Every variant except a database-level error represents a decision that was
/// reached *inside* the transaction, so the caller never has to reason about
/// partial state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteOutcome {
    /// The transfer committed. Balances, transfer, ledger entries, and the
    /// outbox event are all durable.
    Committed {
        transfer_id: TransferId,
        committed_at: DateTime<Utc>,
    },
    /// This exact request already committed. The stored result is replayed and
    /// **no** financial effect occurs (INV-6).
    Replayed {
        transfer_id: TransferId,
        status: TransferStatus,
        created_at: DateTime<Utc>,
        /// `Some` when the original attempt was rejected, so the caller can
        /// reproduce the original error rather than treating it as a success.
        rejection: Option<Rejection>,
    },
    /// The transfer was refused and the refusal was recorded. No money moved and
    /// no ledger entries were written.
    Rejected {
        transfer_id: TransferId,
        rejection: Rejection,
    },
    /// The same idempotency key was used with a different payload. Rejected.
    IdempotencyConflict,
    /// One of the accounts referenced does not exist.
    AccountNotFound,
}

impl ExecuteOutcome {
    pub fn transfer_id(&self) -> Option<TransferId> {
        match self {
            ExecuteOutcome::Committed { transfer_id, .. }
            | ExecuteOutcome::Replayed { transfer_id, .. }
            | ExecuteOutcome::Rejected { transfer_id, .. } => Some(*transfer_id),
            _ => None,
        }
    }

    /// The recorded rejection, if this outcome is a refusal.
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            ExecuteOutcome::Rejected { rejection, .. } => Some(rejection),
            ExecuteOutcome::Replayed { rejection, .. } => rejection.as_ref(),
            _ => None,
        }
    }

    pub fn transfer_status(&self) -> Option<TransferStatus> {
        match self {
            ExecuteOutcome::Committed { .. } => Some(TransferStatus::Completed),
            ExecuteOutcome::Replayed { status, .. } => Some(*status),
            ExecuteOutcome::Rejected { .. } => Some(TransferStatus::Failed),
            _ => None,
        }
    }

    pub fn committed_at(&self) -> Option<DateTime<Utc>> {
        match self {
            ExecuteOutcome::Committed { committed_at, .. } => Some(*committed_at),
            ExecuteOutcome::Replayed { created_at, .. } => Some(*created_at),
            _ => None,
        }
    }
}

/// Atomic transfer execution.
#[async_trait]
pub trait TransferRepository: Send + Sync {
    type Err: Error + Send + Sync + 'static;

    /// Attempt the transfer as a single atomic unit.
    ///
    /// Implementations MUST commit balance updates, the transfer row, both
    /// ledger entries, the outbox event, and the idempotency record together, or
    /// commit none of them.
    async fn execute(&self, request: ExecuteTransfer) -> Result<ExecuteOutcome, Self::Err>;

    /// Whether an error from [`Self::execute`] is safe to retry.
    ///
    /// Only serialization failures and deadlocks qualify. A retry re-runs the
    /// **entire** transaction. An error with an uncertain commit outcome must
    /// never report `true`; it must be resolved through idempotency identity
    /// instead.
    fn is_retriable(err: &Self::Err) -> bool;
}

/// Read-side queries. Separate from the write port because these need no
/// transaction and are safe to serve from a pool directly.
#[async_trait]
pub trait TransferReader: Send + Sync {
    type Err: Error + Send + Sync + 'static;

    async fn find_transfer(&self, id: TransferId) -> Result<Option<Transfer>, Self::Err>;

    async fn ledger_for_transfer(&self, id: TransferId) -> Result<Vec<LedgerEntry>, Self::Err>;
}

/// Account creation and lookup.
///
/// Kept separate from [`TransferRepository`] because opening an account is not
/// part of the transfer transaction, and mixing the two would force every test
/// that only needs a transfer to satisfy an unrelated interface.
#[async_trait]
pub trait AccountDirectory: Send + Sync {
    type Err: Error + Send + Sync + 'static;

    /// Open a new account at zero balance.
    async fn open(&self, currency: Currency) -> Result<Account, Self::Err>;

    async fn find(&self, id: AccountId) -> Result<Option<Account>, Self::Err>;
}

/// A requested transfer, before the domain has built any entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitTransfer {
    pub source: AccountId,
    pub destination: AccountId,
    pub amount_minor: i64,
    pub client_id: String,
    pub idempotency_key: String,
}

impl SubmitTransfer {
    /// Canonical byte encoding of the request body, used as the idempotency
    /// fingerprint.
    ///
    /// Built explicitly rather than by serialising the struct so that the
    /// fingerprint cannot silently change when a field is renamed, reordered, or
    /// gains a derive. Two requests produce the same fingerprint if and only if
    /// they would have the same financial effect.
    pub fn canonical_fingerprint(&self) -> Vec<u8> {
        format!(
            "v1|src={}|dst={}|amount={}|client={}|key={}",
            self.source, self.destination, self.amount_minor, self.client_id, self.idempotency_key
        )
        .into_bytes()
    }
}
