//! The `kifaru` binary.
//!
//! Subcommands are parsed by hand rather than with a dependency: there are four
//! of them, the grammar is trivial, and `clap` would be the largest dependency
//! in the workspace for no behavioural gain.

use kifaru_api::{Config, LogFormat, http, migrate};
use kifaru_storage::sqlx::postgres::PgPoolOptions;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
kifaru — transfer reliability platform

USAGE:
    kifaru <COMMAND>

COMMANDS:
    serve       Run the HTTP API
    migrate     Apply database migrations and exit
    worker      Run the outbox publisher            (not implemented yet)
    reconcile   Run the reconciliation engine       (not implemented yet)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str);

    match command {
        Some("serve") => run(run_serve()),
        Some("migrate") => run(run_migrate()),
        Some("worker") => {
            eprintln!(
                "kifaru: `worker` is not implemented yet. The outbox event is written \
                 transactionally with every transfer, but nothing drains it in this build."
            );
            ExitCode::from(2)
        }
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
