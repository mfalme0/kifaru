//! Account creation and lookup.
//!
//! Opening an account writes exactly one row at a zero balance. It deliberately
//! writes **no** ledger entry: there is no movement, so a genesis posting would
//! be a single-sided entry and would break the balance-equals-ledger property
//! (INV-4) for the new account. Money enters the system only through a transfer,
//! which is always double-entry.

use crate::error::StorageError;
use crate::models::AccountRow;
use async_trait::async_trait;
use chrono::Utc;
use kifaru_application::ports::AccountDirectory;
use kifaru_domain::{Account, AccountId, Currency};
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgAccountDirectory {
    pool: PgPool,
}

impl PgAccountDirectory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl AccountDirectory for PgAccountDirectory {
    type Err = StorageError;

    async fn open(&self, currency: Currency) -> Result<Account, StorageError> {
        let account = Account::open(currency, Utc::now());
        sqlx::query(
            r#"INSERT INTO accounts (id, status, currency, balance_minor, version, created_at)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(account.id.as_uuid())
        .bind(account.status.to_string())
        .bind(account.currency.code())
        .bind(account.balance.minor_units())
        .bind(account.version)
        .bind(account.created_at)
        .execute(&self.pool)
        .await?;

        Ok(account)
    }

    async fn find(&self, id: AccountId) -> Result<Option<Account>, StorageError> {
        let row = sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, status, currency, balance_minor, version, created_at
               FROM accounts
               WHERE id = $1"#,
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;

        row.map(Account::try_from).transpose()
    }
}
