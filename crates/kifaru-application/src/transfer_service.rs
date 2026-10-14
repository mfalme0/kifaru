//! Transfer application service implementing atomic transfers with idempotency.

use chrono::Utc;
use kifaru_domain::{AccountId, Money, TransferId, TransferStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferCommand {
    pub source: AccountId,
    pub destination: AccountId,
    pub amount: Money,
    pub client_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct TransferResult {
    pub transfer_id: TransferId,
    pub status: TransferStatus,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum TransferServiceError {
    #[error("domain error: {0}")]
    Domain(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("idempotency conflict")]
    IdempotencyConflict,
}
