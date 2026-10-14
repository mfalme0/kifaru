//! Errors returned by the transfer application service.
//!
//! These are the *decisions* the service makes. The API layer maps them to HTTP
//! status codes; nothing below this crate knows what HTTP is.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransferServiceError {
    /// The request was malformed before any database work happened: zero or
    /// negative amount, a self-transfer, or a missing identifier. Nothing was
    /// persisted and nothing was charged to the idempotency key.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The idempotency key was already used for a *different* request body.
    /// Recorded as a distinct variant because it maps to `409 Conflict` and is
    /// the one idempotency outcome a client must not blindly retry.
    #[error("idempotency key already used with a different request body")]
    IdempotencyConflict,

    #[error("account not found: {account_id}")]
    AccountNotFound { account_id: String },

    #[error("insufficient funds: balance {balance} minor units, required {required} minor units")]
    InsufficientFunds { balance: i64, required: i64 },

    #[error("account is not active: {account_id}")]
    AccountNotActive { account_id: String },

    #[error("account currency does not match the transfer currency")]
    CurrencyMismatch,

    /// A database-level failure that could not be retried to success.
    ///
    /// The message is preserved for diagnostics but is deliberately an opaque
    /// string: the application layer must not pattern-match on driver errors.
    #[error("storage failure: {0}")]
    Storage(String),
}
