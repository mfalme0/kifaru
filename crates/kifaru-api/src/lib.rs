//! HTTP transport for KIFARU.
//!
//! Contains the `kifaru` binary and the Axum router. No business rules live
//! here: this crate validates request shape, maps failures to status codes, and
//! delegates every decision to `kifaru-application`.

#![forbid(unsafe_code)]

pub mod config;
pub mod http;
pub mod worker;

pub use config::{Config, ConfigError, LogFormat};
pub use http::{AppState, router};
pub use worker::{
    BatchReport, DedupeConsumer, EventPublisher, LoggingPublisher, OutboxWorker, PublishError,
    WorkerConfig, WorkerConfigError,
};

/// Apply pending migrations from the repository `migrations/` directory.
///
/// Embedded at compile time, so a binary can set up its own schema without the
/// source tree being present at runtime.
pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}
