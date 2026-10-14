//! Application layer: ports and the transfer service.
//!
//! Deliberately free of `sqlx`, HTTP, and any storage detail. The only external
//! dependency of substance is `async-trait`, needed because the ports are used
//! behind `dyn` in the HTTP layer.

pub mod error;
pub mod ports;
pub mod transfer_service;

pub use error::TransferServiceError;
pub use ports::{
    AccountDirectory, ExecuteOutcome, ExecuteTransfer, Rejection, SubmitTransfer, TransferReader,
    TransferRepository,
};
pub use transfer_service::{DEFAULT_MAX_ATTEMPTS, TransferResult, TransferService};
