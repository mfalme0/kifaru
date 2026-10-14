//! Request and response bodies.
//!
//! Wire types are kept separate from domain types on purpose: renaming a domain
//! field must not silently change the API, and a domain type must not gain a
//! serialisation shape because one transport happens to want one.

use chrono::{DateTime, Utc};
use kifaru_domain::{Account, EntryDirection, LedgerEntry, Transfer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAccountRequest {
    /// Optional. Phase 1 has exactly one configured currency, so this may only
    /// be omitted or `"KES"`.
    #[serde(default)]
    pub currency: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AccountResponse {
    pub id: String,
    pub status: String,
    pub currency: String,
    /// Balance in minor units (cents for KES). Never a float, never major units.
    pub balance_minor: i64,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl From<&Account> for AccountResponse {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.to_string(),
            status: account.status.to_string(),
            currency: account.currency.code().to_string(),
            balance_minor: account.balance.minor_units(),
            version: account.version,
            created_at: account.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    pub source_account_id: String,
    pub destination_account_id: String,
    /// Minor units. Must be strictly positive.
    pub amount_minor: i64,
}

#[derive(Debug, Serialize)]
pub struct TransferResponse {
    pub transfer_id: String,
    pub status: String,
    pub source_account_id: String,
    pub destination_account_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub settled_at: DateTime<Utc>,
    /// Only present on the response to a `POST`; a `GET` has no notion of a
    /// replay, so the field is omitted rather than reported as `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed: Option<bool>,
}

impl TransferResponse {
    pub fn from_domain(transfer: &Transfer) -> Self {
        Self {
            transfer_id: transfer.id.to_string(),
            status: transfer.status.to_string(),
            source_account_id: transfer.source.to_string(),
            destination_account_id: transfer.destination.to_string(),
            amount_minor: transfer.amount.minor_units(),
            currency: transfer.amount.currency().code().to_string(),
            settled_at: transfer.updated_at,
            replayed: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LedgerEntryResponse {
    pub id: String,
    pub transfer_id: String,
    pub account_id: String,
    pub direction: String,
    /// Always positive. `direction` carries the sign.
    pub amount_minor: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

impl From<&LedgerEntry> for LedgerEntryResponse {
    fn from(entry: &LedgerEntry) -> Self {
        Self {
            id: entry.id.to_string(),
            transfer_id: entry.transfer_id.to_string(),
            account_id: entry.account_id.to_string(),
            direction: match entry.direction {
                EntryDirection::Debit => "debit".to_string(),
                EntryDirection::Credit => "credit".to_string(),
            },
            amount_minor: entry.amount.minor_units(),
            currency: entry.amount.currency().code().to_string(),
            created_at: entry.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LedgerResponse {
    pub transfer_id: String,
    pub entries: Vec<LedgerEntryResponse>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    /// Stable, machine-readable. Clients branch on this, not on `message`.
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub database: &'static str,
}
