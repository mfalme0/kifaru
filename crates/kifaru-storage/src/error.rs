//! PostgreSQL adapters and error classification.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error(transparent)]
    Domain(#[from] DomainError),
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("insufficient funds")]
    InsufficientFunds,
    #[error("account not active")]
    AccountNotActive,
    #[error("currency mismatch")]
    CurrencyMismatch,
    #[error("unknown domain error: {0}")]
    Unknown(String),
}

/// Map SQLx errors to domain errors for retry classification.
///
/// The transfer service must only retry on serialization failures and deadlocks:
/// - SQLSTATE 40001: serialization failure
/// - SQLSTATE 40P01: deadlock detected
pub fn is_retriable_db_error(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err
        && let Some(code) = db_err.code()
    {
        return code == "40001" || code == "40P01";
    }
    false
}
