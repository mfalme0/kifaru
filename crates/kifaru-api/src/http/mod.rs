//! HTTP surface.
//!
//! The transport layer validates *shape* and delegates every decision to
//! [`kifaru_application::TransferService`]. It contains no business rules: if a
//! rule about money lives in this file, it is in the wrong place.

pub mod dto;
pub mod error;

use crate::config::Config;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use dto::{
    AccountResponse, CreateAccountRequest, HealthResponse, LedgerResponse, TransferRequest,
    TransferResponse,
};
use error::ApiError;
use kifaru_application::TransferService;
use kifaru_application::ports::{AccountDirectory, SubmitTransfer, TransferReader};
use kifaru_domain::{AccountId, Currency, TransferId};
use kifaru_storage::{PgAccountDirectory, PgTransferRepository, StorageError};
use std::sync::Arc;
use uuid::Uuid;

/// Header a client uses to scope their idempotency keys.
pub const HEADER_CLIENT_ID: &str = "x-client-id";
/// Header carrying the idempotency key. Required on `POST /v1/transfers`.
pub const HEADER_IDEMPOTENCY_KEY: &str = "idempotency-key";

#[derive(Clone)]
pub struct AppState {
    pub accounts: PgAccountDirectory,
    pub repo: PgTransferRepository,
    pub service: Arc<TransferService<PgTransferRepository>>,
    /// Fallback identity when [`HEADER_CLIENT_ID`] is absent.
    ///
    /// **Not authentication.** See `docs/threat-model.md`.
    pub dev_client_id: String,
}

impl AppState {
    pub fn new(pool: sqlx::PgPool, config: &Config) -> Self {
        let accounts = PgAccountDirectory::new(pool.clone());
        let repo = PgTransferRepository::new(pool);
        Self {
            accounts,
            repo: repo.clone(),
            service: Arc::new(TransferService::new(repo)),
            dev_client_id: config.dev_client_id.clone(),
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/accounts", post(create_account))
        .route("/v1/accounts/{id}", get(get_account))
        .route("/v1/transfers", post(submit_transfer))
        .route("/v1/transfers/{id}", get(get_transfer))
        .route("/v1/transfers/{id}/ledger", get(get_transfer_ledger))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Liveness. Does not touch the database, so it stays 200 even when the database
/// is down — that is the difference between liveness and readiness.
async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        database: "unchecked",
    })
}

/// Readiness. Fails when the database is unreachable, because the process cannot
/// serve a transfer without it.
async fn readyz(State(state): State<Arc<AppState>>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(state.repo.pool())
        .await
        .map_err(|e| ApiError::internal(format!("database not ready: {e}")))?;
    Ok(Json(HealthResponse {
        status: "ok",
        database: "ok",
    }))
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    body: Option<Json<CreateAccountRequest>>,
) -> Result<(StatusCode, Json<AccountResponse>), ApiError> {
    if let Some(Json(request)) = &body
        && let Some(requested) = &request.currency
        && requested != Currency::configured().code()
    {
        return Err(ApiError::bad_request(format!(
            "unsupported currency {requested:?}; this instance is configured for {}",
            Currency::configured()
        )));
    }

    let account = state
        .accounts
        .open(Currency::configured())
        .await
        .map_err(storage_error)?;

    Ok((StatusCode::CREATED, Json(AccountResponse::from(&account))))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AccountResponse>, ApiError> {
    let id = parse_account_id(&id)?;
    let account = state
        .accounts
        .find(id)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| ApiError::not_found(format!("no account {id}")))?;
    Ok(Json(AccountResponse::from(&account)))
}

async fn submit_transfer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<TransferRequest>,
) -> Result<(StatusCode, Json<TransferResponse>), ApiError> {
    let idempotency_key = required_header(&headers, HEADER_IDEMPOTENCY_KEY)?.to_string();
    let client_id = client_id(&headers, &state.dev_client_id);

    let source = parse_account_id(&request.source_account_id)?;
    let destination = parse_account_id(&request.destination_account_id)?;

    let result = state
        .service
        .submit(SubmitTransfer {
            source,
            destination,
            amount_minor: request.amount_minor,
            client_id,
            idempotency_key,
        })
        .await?;

    // A fresh execution is 201 Created; a replay is 200 OK, because the client
    // did not create anything this time.
    let status = if result.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };

    Ok((
        status,
        Json(TransferResponse {
            transfer_id: result.transfer_id.to_string(),
            status: result.status.to_string(),
            source_account_id: result.source.to_string(),
            destination_account_id: result.destination.to_string(),
            amount_minor: result.amount_minor,
            currency: Currency::configured().code().to_string(),
            settled_at: result.settled_at,
            replayed: Some(result.replayed),
        }),
    ))
}

async fn get_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TransferResponse>, ApiError> {
    let id = parse_transfer_id(&id)?;
    let transfer = state
        .repo
        .find_transfer(id)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| ApiError::not_found(format!("no transfer {id}")))?;
    Ok(Json(TransferResponse::from_domain(&transfer)))
}

async fn get_transfer_ledger(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<LedgerResponse>, ApiError> {
    let id = parse_transfer_id(&id)?;

    // Distinguish "transfer does not exist" from "transfer exists with no
    // entries". A transfer with no entries means it was recorded as failed.
    if state
        .repo
        .find_transfer(id)
        .await
        .map_err(storage_error)?
        .is_none()
    {
        return Err(ApiError::not_found(format!("no transfer {id}")));
    }

    let entries = state
        .repo
        .ledger_for_transfer(id)
        .await
        .map_err(storage_error)?;

    Ok(Json(LedgerResponse {
        transfer_id: id.to_string(),
        entries: entries.iter().map(Into::into).collect(),
    }))
}

// ---------------------------------------------------------------------------
// Extraction helpers
// ---------------------------------------------------------------------------

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .ok_or_else(|| ApiError::bad_request(format!("missing required header {name}")))?
        .to_str()
        .map_err(|_| ApiError::bad_request(format!("header {name} is not valid ASCII")))
        .and_then(|value| {
            if value.trim().is_empty() {
                Err(ApiError::bad_request(format!("header {name} is empty")))
            } else {
                Ok(value)
            }
        })
}

fn client_id(headers: &HeaderMap, fallback: &str) -> String {
    headers
        .get(HEADER_CLIENT_ID)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn parse_account_id(raw: &str) -> Result<AccountId, ApiError> {
    Uuid::parse_str(raw)
        .map(AccountId::from_uuid)
        .map_err(|_| ApiError::bad_request(format!("{raw:?} is not a valid account id")))
}

fn parse_transfer_id(raw: &str) -> Result<TransferId, ApiError> {
    Uuid::parse_str(raw)
        .map(TransferId::from_uuid)
        .map_err(|_| ApiError::bad_request(format!("{raw:?} is not a valid transfer id")))
}

fn storage_error(err: StorageError) -> ApiError {
    ApiError::internal(err.to_string())
}
