//! PostgreSQL storage adapters.
//!
//! This is the only crate that talks to the database. It maps rows to domain
//! entities, owns the transaction boundaries that make a transfer atomic, and
//! classifies errors for retry. It contains no business rules — those live in
//! `kifaru-domain` and are re-applied here, inside the transaction, against
//! locked rows.

#![forbid(unsafe_code)]

pub mod error;
pub mod models;
pub mod pg;

pub use error::{StorageError, is_retriable_db_error};
pub use models::OutboxState;
pub use pg::{OutboxEvent, PgAccountDirectory, PgOutboxStore, PgTransferRepository};

/// Re-exported so integration tests and the API crate can name the exact driver
/// version this crate was compiled against, instead of depending on a second
/// copy that might drift.
pub use sqlx;
