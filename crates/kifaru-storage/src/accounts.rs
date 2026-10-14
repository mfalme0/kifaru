//! Database access layer for accounts.

use crate::{StorageError, models::AccountRow};
use kifaru_domain::{Account, AccountId};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AccountRepo {
    pool: PgPool,
}

impl AccountRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_id(&self, id: AccountId) -> Result<Option<Account>, StorageError> {
        let row = sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, status, currency, balance_minor, version, created_at
               FROM accounts
               WHERE id = $1"#,
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.into()))
    }
}
