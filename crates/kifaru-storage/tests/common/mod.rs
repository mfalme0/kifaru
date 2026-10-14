//! Shared helpers for the database-backed integration tests.
//!
//! These tests need a live PostgreSQL reachable at `DATABASE_URL` (or the value
//! in the repository `.env`). They are *not* unit tests: they exist to prove the
//! invariants that only the database can settle — atomicity, row-level lock
//! ordering, constraint enforcement, and idempotent replay.
//!
//! Isolation strategy: every test creates its own accounts and its own
//! idempotency keys, and every assertion is scoped to those identifiers. No test
//! asserts a global row count, so the suite is safe to run in parallel (which is
//! `cargo nextest`'s default).

#![allow(dead_code)]

use kifaru_application::ports::{AccountDirectory, SubmitTransfer, TransferReader};
use kifaru_application::{TransferResult, TransferService, TransferServiceError};
use kifaru_domain::{Account, AccountId, Currency};
use kifaru_storage::sqlx;
use kifaru_storage::sqlx::PgPool;
use kifaru_storage::sqlx::postgres::PgPoolOptions;
use kifaru_storage::{PgAccountDirectory, PgTransferRepository};
use uuid::Uuid;

/// Resolve the connection string: environment first, then the repo `.env`.
///
/// The password is never duplicated here. If neither is available the tests fail
/// loudly, because silently skipping would turn "invariant not proven" into
/// "invariant assumed".
pub fn database_url() -> String {
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.trim().is_empty()
    {
        return url;
    }

    let mut dir = std::env::current_dir().expect("current dir");
    loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            let contents = std::fs::read_to_string(&candidate).expect("read .env");
            for line in contents.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("DATABASE_URL=") {
                    let value = rest.trim().trim_matches('"');
                    if !value.is_empty() {
                        return value.to_string();
                    }
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }

    panic!(
        "no DATABASE_URL. Set it in the environment, or add it to the repository .env, \
         before running the integration tests."
    );
}

pub async fn connect() -> PgPool {
    PgPoolOptions::new()
        .max_connections(25)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&database_url())
        .await
        .unwrap_or_else(|e| {
            panic!(
                "could not connect to PostgreSQL at {}: {e}. \
                 Start it with `docker compose up -d`.",
                database_url()
            )
        })
}

/// Everything a test needs, wired together the way the binary wires it.
pub struct Harness {
    pub pool: PgPool,
    pub accounts: PgAccountDirectory,
    pub repo: PgTransferRepository,
    pub service: TransferService<PgTransferRepository>,
}

impl Harness {
    pub async fn new() -> Self {
        let pool = connect().await;
        let accounts = PgAccountDirectory::new(pool.clone());
        let repo = PgTransferRepository::new(pool.clone());
        let service = TransferService::new(repo.clone());
        Self {
            pool,
            accounts,
            repo,
            service,
        }
    }

    pub async fn open_account(&self) -> Account {
        self.accounts
            .open(Currency::Kes)
            .await
            .expect("open account")
    }

    /// Open an account and give it an opening balance.
    pub async fn open_funded(&self, minor_units: i64) -> Account {
        let account = self.open_account().await;
        self.seed_opening_balance(account.id, minor_units).await;
        account
    }

    /// Test-only opening balance.
    ///
    /// This is the single place in the whole system where a balance is
    /// established without a corresponding balanced ledger transaction, and it
    /// exists only in tests. It is unavoidable: the schema forbids a negative
    /// balance (INV-1), so a closed ledger has no representation for money
    /// entering from outside. Production code has no path that mints money.
    ///
    /// Tests must therefore state INV-4 as
    /// `stored == opening_balance + derived_from_ledger`, which reduces to
    /// `stored == derived` whenever the opening balance is zero.
    pub async fn seed_opening_balance(&self, id: AccountId, minor_units: i64) {
        let affected = sqlx::query("UPDATE accounts SET balance_minor = $1 WHERE id = $2")
            .bind(minor_units)
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .expect("seed opening balance")
            .rows_affected();
        assert_eq!(affected, 1, "expected to seed exactly one account row");
    }

    pub async fn stored_balance(&self, id: AccountId) -> i64 {
        self.accounts
            .find(id)
            .await
            .expect("find account")
            .expect("account exists")
            .balance
            .minor_units()
    }

    /// Balance recomputed from the ledger, signed, exactly as reconciliation
    /// would compute it.
    pub async fn derived_balance(&self, id: AccountId) -> i64 {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT direction, amount_minor FROM ledger_entries WHERE account_id = $1",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .expect("read ledger");

        rows.iter()
            .map(|(direction, amount)| {
                if direction == "debit" {
                    -amount
                } else {
                    *amount
                }
            })
            .sum()
    }

    /// INV-4 for one account: the authoritative balance equals the opening
    /// balance plus everything the ledger says happened.
    pub async fn assert_inv4(&self, id: AccountId, opening_balance: i64) {
        let stored = self.stored_balance(id).await;
        let derived = self.derived_balance(id).await;
        assert_eq!(
            stored,
            opening_balance + derived,
            "INV-4 violated for {id}: stored {stored}, opening {opening_balance}, derived {derived}"
        );
        assert!(stored >= 0, "INV-1 violated for {id}: balance {stored}");
    }

    pub async fn ledger_len(&self, transfer_id: kifaru_domain::TransferId) -> usize {
        self.repo
            .ledger_for_transfer(transfer_id)
            .await
            .expect("ledger")
            .len()
    }

    pub async fn outbox_count(&self, transfer_id: kifaru_domain::TransferId) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE aggregate_id = $1")
            .bind(transfer_id.as_uuid())
            .fetch_one(&self.pool)
            .await
            .expect("outbox count")
    }

    /// The recorded status for a key, or `None` when no row exists.
    ///
    /// `None` is the meaningful "the key was released" state, so this must not
    /// treat a missing row as an error.
    pub async fn idempotency_status(&self, client_id: &str, key: &str) -> Option<i16> {
        sqlx::query_scalar(
            "SELECT response_status FROM idempotency_keys WHERE client_id = $1 AND key = $2",
        )
        .bind(client_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .expect("idempotency status")
    }

    pub async fn submit(
        &self,
        source: AccountId,
        destination: AccountId,
        amount_minor: i64,
        client_id: &str,
        key: &str,
    ) -> Result<TransferResult, TransferServiceError> {
        self.service
            .submit(SubmitTransfer {
                source,
                destination,
                amount_minor,
                client_id: client_id.to_string(),
                idempotency_key: key.to_string(),
            })
            .await
    }
}

/// A fresh key. Unique per call so parallel tests never collide.
pub fn key(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// A distinct development `client_id`.
pub fn client(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
