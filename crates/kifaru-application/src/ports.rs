//! Port definitions (traits) for storage and side effects.

use async_trait::async_trait;
use kifaru_domain::{
    Account, AccountId, LedgerEntry, LedgerTransaction, Money, Transfer, TransferId,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use uuid::Uuid;

#[async_trait]
pub trait AccountRepository: Send + Sync {
    type Err: Error + Send + Sync + 'static;
    async fn find_by_id(&self, id: AccountId) -> Result<Option<Account>, Self::Err>;
    async fn find_by_ids_for_update(&self, ids: &[AccountId]) -> Result<Vec<Account>, Self::Err>;
    async fn update_balances(
        &self,
        debits: &[DebitUpdate],
        credits: &[CreditUpdate],
    ) -> Result<(), Self::Err>;
}

#[derive(Clone, Debug)]
pub struct DebitUpdate {
    pub account_id: AccountId,
    pub new_balance: Money,
    pub version: i64,
}

#[derive(Clone, Debug)]
pub struct CreditUpdate {
    pub account_id: AccountId,
    pub new_balance: Money,
    pub version: i64,
}

#[async_trait]
pub trait LedgerRepository: Send + Sync {
    type Err: Error + Send + Sync + 'static;
    async fn insert_ledger(&self, tx: &LedgerTransaction) -> Result<(), Self::Err>;
    async fn entries_for_account(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<LedgerEntry>, Self::Err>;
}

#[async_trait]
pub trait IdempotencyRepository: Send + Sync {
    type Err: Error + Send + Sync + 'static;
    async fn lookup(&self, key: &IdempotencyKey) -> Result<Option<IdempotencyRecord>, Self::Err>;
    async fn store_result(
        &self,
        key: &IdempotencyKey,
        result: &StoredResult,
    ) -> Result<(), Self::Err>;
    async fn begin_request(&self, key: &IdempotencyKey, hash: &[u8]) -> Result<(), Self::Err>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdempotencyKey {
    pub client_id: String,
    pub key: String,
}

#[derive(Clone, Debug)]
pub struct IdempotencyRecord {
    pub transfer_id: Option<TransferId>,
    pub response_status: Option<i32>,
    pub response_body: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub struct StoredResult {
    pub transfer_id: TransferId,
    pub response_status: i32,
    pub response_body: serde_json::Value,
}

#[async_trait]
pub trait OutboxPublisher: Send + Sync {
    type Err: Error + Send + Sync + 'static;
    async fn publish_transfer_completed(
        &self,
        transfer: &Transfer,
        debit: &LedgerEntry,
        credit: &LedgerEntry,
    ) -> Result<Uuid, Self::Err>;
}
