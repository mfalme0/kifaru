//! Main storage module.

pub mod accounts;
pub mod error;
pub mod models;

pub use accounts::AccountRepo;
pub use error::{DomainError, StorageError, is_retriable_db_error};
