//! The `kifaru` binary.
//!
//! Subcommands are parsed by hand rather than with a dependency: there are four
//! of them, the grammar is trivial, and `clap` would be the largest dependency
//! in the workspace for no behavioural gain.

use kifaru_api::{Config, EventPublisher, LogFormat, PublishError, http, migrate};
use kifaru_storage::PgOutboxStore;
use kifaru_storage::sqlx::postgres::PgPoolOptions;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
kifaru — transfer reliability platform

USAGE:
    kifaru <COMMAND>

COMMANDS:
    serve          Run the HTTP API
    migrate        Apply database migrations and exit
    worker         Drain the transactional outbox until interrupted
    outbox-status  Print outbox event counts by state and exit
    reconcile      Run the reconciliation engine       (not implemented yet)

Run `worker` alongside `serve` to publish committed transfers. Delivery is
AT-LEAST-ONCE: consumers deduplicate on event_id.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);

    match command {
        Some("serve") => run(run_serve()),
        Some("migrate") => run(run_migrate()),
        Some("worker") => run(run_worker()),
        Some("outbox-status") => run(run_outbox_status()),
        Some("reconcile") => {
            eprintln!("kifaru: `reconcile` is not implemented yet.");
            ExitCode::from(2)
        }
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("kifaru: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kifaru: {err}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(cfg: &Config) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    match cfg.log_format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
        LogFormat::Pretty => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}

async fn connect(cfg: &Config) -> Result<kifaru_storage::sqlx::PgPool, Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&cfg.database_url)
        .await?;
    Ok(pool)
}

#[tokio::main]
async fn run_serve() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg);

    tracing::info!(
        database = %cfg.redacted_database_url(),
        bind = %cfg.bind,
        auto_migrate = cfg.auto_migrate,
        "starting kifaru"
    );

    let pool = connect(&cfg).await?;

    if cfg.auto_migrate {
        migrate(&pool).await?;
        tracing::info!("migrations applied");
    }

    let state = Arc::new(http::AppState::new(pool, &cfg));
    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;

    tracing::info!(addr = %listener.local_addr()?, "listening");

    axum::serve(listener, app).await?;
    Ok(())
}

#[tokio::main]
async fn run_migrate() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg);

    let pool = connect(&cfg).await?;
    migrate(&pool).await?;
    tracing::info!("migrations applied");
    println!("migrations applied");
    Ok(())
}

#[tokio::main]
async fn run_worker() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env()?;
    init_tracing(&cfg);

    let worker_config = kifaru_api::WorkerConfig::from_env();
    let pool = connect(&cfg).await?;

    let store = PgOutboxStore::new(pool.clone());
    let publisher: Arc<dyn EventPublisher> = match publisher_kind() {
        PublisherKind::Logging => Arc::new(kifaru_api::LoggingPublisher),
        PublisherKind::DedupeConsumer => {
            // After publishing, the event is also handed to a consumer that
            // dedupes on event_id. This is what makes a redelivery harmless.
            Arc::new(ConsumerPublishingPublisher::new(
                PgOutboxStore::new(pool.clone()),
                worker_config.consumer_name.clone(),
            ))
        }
    };

    let worker = kifaru_api::OutboxWorker::new(store, publisher, worker_config)?;

    tracing::info!(
        database = %cfg.redacted_database_url(),
        publisher = %publisher_kind(),
        "starting kifaru worker"
    );

    worker
        .run_until(async {
            if let Err(err) = tokio::signal::ctrl_c().await {
                tracing::error!(error = %err, "failed to listen for shutdown signal");
            }
            tracing::info!("interrupt received");
        })
        .await?;

    Ok(())
}

#[tokio::main]
async fn run_outbox_status() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::from_env()?;
    let pool = connect(&cfg).await?;

    let store = PgOutboxStore::new(pool);
    let counts = store.count_by_state().await?;

    println!("outbox events by state:");
    for state in [
        kifaru_storage::OutboxState::Pending,
        kifaru_storage::OutboxState::Delivered,
        kifaru_storage::OutboxState::Dead,
    ] {
        let count = counts
            .iter()
            .find(|(s, _)| *s == state)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        println!("  {:<10} {}", state.as_str(), count);
    }

    Ok(())
}

enum PublisherKind {
    Logging,
    DedupeConsumer,
}

impl std::fmt::Display for PublisherKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PublisherKind::Logging => "logging",
            PublisherKind::DedupeConsumer => "dedupe-consumer",
        })
    }
}

fn publisher_kind() -> PublisherKind {
    match std::env::var("KIFARU_WORKER_PUBLISHER").as_deref() {
        Ok("dedupe-consumer") => PublisherKind::DedupeConsumer,
        _ => PublisherKind::Logging,
    }
}

/// Publishes to the log, then hands the event to a deduplicating consumer.
///
/// The consumer step is what proves the at-least-once contract end to end: if a
/// lease expires mid-publish and the event is redelivered, the consumer records
/// nothing the second time.
struct ConsumerPublishingPublisher {
    store: PgOutboxStore,
    consumer: String,
}

impl ConsumerPublishingPublisher {
    fn new(store: PgOutboxStore, consumer: impl Into<String>) -> Self {
        Self {
            store,
            consumer: consumer.into(),
        }
    }
}

#[async_trait::async_trait]
impl EventPublisher for ConsumerPublishingPublisher {
    async fn publish(&self, event: &kifaru_storage::OutboxEvent) -> Result<(), PublishError> {
        tracing::info!(
            event_id = %event.id,
            event_type = %event.event_type,
            aggregate_id = %event.aggregate_id,
            attempt = event.attempts,
            "published outbox event"
        );

        self.store
            .try_record_consumption(&self.consumer, event.id)
            .await
            .map(|first| {
                if first {
                    tracing::info!(event_id = %event.id, consumer = %self.consumer, "consumer processed event");
                } else {
                    tracing::info!(event_id = %event.id, consumer = %self.consumer, "duplicate delivery skipped");
                }
            })
            .map_err(|e| PublishError::Transient(format!("consumer inbox unavailable: {e}")))
    }
}
