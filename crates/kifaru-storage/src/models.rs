//! Database models and conversion helpers.

use chrono::{DateTime, Utc};
use kifaru_domain::{
    Account, AccountId, AccountStatus, Currency, EntryDirection, LedgerEntry, Money, TransferId,
};
use sqlx::types::Uuid;

/// Row from accounts table
#[derive(sqlx::FromRow, Debug)]
pub struct AccountRow {
    pub id: Uuid,
    pub status: String,
    pub currency: String,
    pub balance_minor: i64,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl From<AccountRow> for Account {
    fn from(row: AccountRow) -> Self {
        let status = match row.status.as_str() {
            "active" => AccountStatus::Active,
            "suspended" => AccountStatus::Suspended,
            "closed" => AccountStatus::Closed,
            _ => AccountStatus::Suspended,
        };
        let currency = match row.currency.as_str() {
            "KES" => Currency::Kes,
            _ => Currency::configured(),
        };
        let balance = Money::from_minor_units(row.balance_minor, currency).expect("valid balance");
        Account::from_parts(
            AccountId::from_uuid(row.id),
            status,
            currency,
            balance,
            row.version,
            row.created_at,
        )
    }
}

/// Row from ledger_entries table
#[derive(sqlx::FromRow, Debug)]
pub struct LedgerEntryRow {
    pub id: Uuid,
    pub transfer_id: Uuid,
    pub account_id: Uuid,
    pub direction: String,
    pub amount_minor: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

impl From<LedgerEntryRow> for LedgerEntry {
    fn from(row: LedgerEntryRow) -> Self {
        let direction = match row.direction.as_str() {
            "debit" => EntryDirection::Debit,
            "credit" => EntryDirection::Credit,
            _ => EntryDirection::Debit,
        };
        let currency = match row.currency.as_str() {
            "KES" => Currency::Kes,
            _ => Currency::configured(),
        };
        let amount = Money::from_minor_units(row.amount_minor, currency).expect("valid amount");
        LedgerEntry::from_parts(
            row.id,
            TransferId::from_uuid(row.transfer_id),
            AccountId::from_uuid(row.account_id),
            direction,
            amount,
            row.created_at,
        )
    }
}
