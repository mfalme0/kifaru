//! Database-backed test harness.
//!
//! Shared by every crate's integration tests so there is exactly one definition
//! of how a test obtains a pool, applies migrations, opens accounts, and submits
//! transfers.
//!
//! # Migrations
//!
//! [`Harness::new`] applies the repository `migrations/` directory through sqlx,
//! so a test run against an empty database works. sqlx takes a PostgreSQL
//! advisory lock around migrations, so concurrent test processes are safe.
//!
//! # Isolation
//!
//! Tests run in parallel (that is `cargo nextest`'s default, and it runs each
//! test in its own process), so nothing here may depend on global state. Every
//! test creates its own accounts and its own idempotency keys, and all
//! assertions are scoped to identifiers it created. No test asserts a global row
//! count.

use kifaru_application::ports::{AccountDirectory, SubmitTransfer, TransferReader};
use kifaru_application::{TransferResult, TransferService, TransferServiceError};
use kifaru_domain::{Account, AccountId, Currency, TransferId};
use kifaru_storage::sqlx;
use kifaru_storage::sqlx::postgres::PgPoolOptions;
use kifaru_storage::sqlx::{Connection, Executor, PgConnection, PgPool};
use kifaru_storage::{OutboxState, PgAccountDirectory, PgOutboxStore, PgTransferRepository};
use uuid::Uuid;

/// Apply the embedded migrations into whatever schema the pool points at.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

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

/// Connect to a **fresh, isolated schema**.
///
/// # Why not just share the default schema
///
/// The outbox is a single global queue. If tests shared it, one test's worker
/// would drain another test's events, and every assertion about claim counts or
/// queue depth would be a race that happens to pass on a quiet machine. Running
/// the suite in parallel would make it flaky rather than fast.
///
/// Giving each test its own schema removes that entire class of failure while
/// keeping the suite parallel. Every object in `migrations/` is unqualified, so
/// PostgreSQL creates it in the first schema on the search path — which means
/// [`migrate`] lays down the full schema per test, for free, in a few
/// milliseconds.
///
/// The schema is dropped again when the [`Harness`] is dropped.
pub async fn connect() -> (PgPool, String) {
    let url = database_url();

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&url)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "could not connect to PostgreSQL at {url}: {e}. \
                 Start it with `docker compose up -d`."
            )
        });

    let schema = format!("test_{}", Uuid::new_v4().simple());

    sqlx::query(&format!(r#"CREATE SCHEMA "{schema}""#))
        .execute(&admin)
        .await
        .expect("create isolated test schema");
    admin.close().await;

    // Applied to every connection in the pool, not just the first.
    let search_path = format!(r#"SET search_path TO "{schema}""#);
    let pool = PgPoolOptions::new()
        .max_connections(25)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .after_connect(move |conn, _meta| {
            let statement = search_path.clone();
            Box::pin(async move {
                conn.execute(statement.as_str()).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap_or_else(|e| panic!("could not connect to PostgreSQL at {url}: {e}"));

    migrate(&pool)
        .await
        .expect("apply migrations into the isolated test schema");

    (pool, schema)
}

/// Everything the tests assert about one outbox row.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRow {
    pub state: OutboxState,
    pub attempts: i32,
    pub locked_by: Option<String>,
    pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
    pub delivered_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_attempt_at: chrono::DateTime<chrono::Utc>,
}

/// Everything a test needs, wired together the way the binary wires it.
pub struct Harness {
    pub pool: PgPool,
    pub accounts: PgAccountDirectory,
    pub repo: PgTransferRepository,
    pub outbox: PgOutboxStore,
    pub service: TransferService<PgTransferRepository>,
    /// The private schema this harness owns, dropped on cleanup.
    schema: String,
}

impl Harness {
    pub async fn new() -> Self {
        let (pool, schema) = connect().await;

        let accounts = PgAccountDirectory::new(pool.clone());
        let repo = PgTransferRepository::new(pool.clone());
        let outbox = PgOutboxStore::new(pool.clone());
        let service = TransferService::new(repo.clone());

        Self {
            pool,
            accounts,
            repo,
            outbox,
            service,
            schema,
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

    pub async fn ledger_len(&self, transfer_id: TransferId) -> usize {
        self.repo
            .ledger_for_transfer(transfer_id)
            .await
            .expect("ledger")
            .len()
    }

    pub async fn outbox_count(&self, transfer_id: TransferId) -> i64 {
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

    // -- outbox -------------------------------------------------------------

    pub async fn outbox_row(&self, id: Uuid) -> OutboxRow {
        let (
            state,
            attempts,
            locked_by,
            locked_until,
            last_error,
            delivered_at,
            next_attempt_at,
        ) = sqlx::query_as::<_, (
            String,
            i32,
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            chrono::DateTime<chrono::Utc>,
        )>(
            "SELECT state, attempts, locked_by, locked_until, last_error, delivered_at, next_attempt_at
             FROM outbox_events WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .expect("outbox row");

        OutboxRow {
            state: OutboxState::parse(&state).expect("valid outbox state"),
            attempts,
            locked_by,
            locked_until,
            last_error,
            delivered_at,
            next_attempt_at,
        }
    }

    pub async fn outbox_id_for(&self, transfer_id: TransferId) -> Uuid {
        sqlx::query_scalar("SELECT id FROM outbox_events WHERE aggregate_id = $1")
            .bind(transfer_id.as_uuid())
            .fetch_one(&self.pool)
            .await
            .expect("outbox event for transfer")
    }

    /// Insert a standalone pending event, for tests that are about the outbox
    /// itself rather than about how a transfer produced one.
    pub async fn insert_pending_event(&self, event_type: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO outbox_events (id, aggregate_id, event_type, payload, state, attempts, next_attempt_at, created_at)
             VALUES ($1, $2, $3, '{}'::jsonb, 'pending', 0, now(), now())",
        )
        .bind(id)
        .bind(Uuid::now_v7())
        .bind(event_type)
        .execute(&self.pool)
        .await
        .expect("insert pending event");
        id
    }

    pub async fn inbox_count(&self, consumer: &str, event_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM consumer_inbox WHERE consumer = $1 AND event_id = $2",
        )
        .bind(consumer)
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .expect("inbox count")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let url = database_url();
        let schema = std::mem::take(&mut self.schema);
        if schema.is_empty() {
            return;
        }

        // Best effort. If the runtime has already shut down the schema is left
        // behind, which is harmless: it is uniquely named and never read again.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            if let Ok(mut conn) = PgConnection::connect(&url).await {
                let _ = sqlx::query(&format!(r#"DROP SCHEMA IF EXISTS "{schema}" CASCADE"#))
                    .execute(&mut conn)
                    .await;
            }
        });
    }
}

/// A fresh idempotency key. Unique per call so parallel tests never collide.
pub fn key(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// A distinct development `client_id`.
///
/// The service boundary takes `client_id` as an argument precisely so tests can
/// vary it and prove that idempotency keys are scoped per tenant.
pub fn client(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
