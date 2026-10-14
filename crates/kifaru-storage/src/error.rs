//! Storage errors and the retry classification the application layer depends on.

use thiserror::Error;

/// Anything that can go wrong below the application layer.
///
/// Deliberately a small, closed set. The application service only ever needs to
/// answer one question about an error — *is it safe to retry?* — and everything
/// else is diagnostics.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// A row loaded from the database does not satisfy the domain's invariants.
    ///
    /// This is never "expected". It means either corruption or a code path that
    /// wrote a value the domain forbids. It is reported instead of being
    /// coerced, because silently mapping an unknown status onto a default is how
    /// a closed account starts accepting money again.
    #[error("row violates a domain invariant: {0}")]
    RowInvariant(String),
}

impl StorageError {
    /// Whether retrying the **entire** transaction is safe.
    ///
    /// Only these two SQLSTATEs qualify:
    /// - `40001` serialization failure
    /// - `40P01` deadlock detected
    ///
    /// Both guarantee the transaction did not commit, so a retry cannot double-
    /// apply. Nothing else is retriable — in particular, a connection error may
    /// have an *unknown* commit outcome, and retrying it blindly is how a
    /// payment gets applied twice. Those must be resolved by idempotency
    /// identity, not by optimistic retry.
    pub fn is_retriable(&self) -> bool {
        match self {
            StorageError::Db(e) => is_retriable_db_error(e),
            StorageError::RowInvariant(_) => false,
        }
    }
}

/// Classify a raw driver error. See [`StorageError::is_retriable`].
pub fn is_retriable_db_error(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            matches!(db_err.code().as_deref(), Some("40001") | Some("40P01"))
        }
        _ => false,
    }
}
