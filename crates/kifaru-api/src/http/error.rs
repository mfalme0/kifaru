//! Mapping application and domain failures onto HTTP.
//!
//! The status code and the machine-readable `code` are decided here and nowhere
//! else, so the contract is greppable in one file. Internal failures never leak
//! their message to the client: the detail is logged, and the client gets a
//! correlation-free generic error.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kifaru_application::TransferServiceError;
use serde_json::json;

use super::dto::{ErrorBody, ErrorDetail};

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
            details: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
            details: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            // Keep the operator-facing detail in the logs, out of the response.
            tracing::error!(code = self.code, detail = %self.message, "request failed");
            let body = ErrorBody {
                error: ErrorDetail {
                    code: "internal_error",
                    message: "an internal error occurred".to_string(),
                    details: None,
                },
            };
            return (self.status, Json(body)).into_response();
        }

        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
                details: self.details,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<TransferServiceError> for ApiError {
    fn from(err: TransferServiceError) -> Self {
        match err {
            TransferServiceError::InvalidRequest(message) => ApiError::bad_request(message),

            // 409: the key is taken by a *different* request. Retrying will never
            // help; the client must use a new key.
            TransferServiceError::IdempotencyConflict => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
                message: "this idempotency key was already used with a different request body"
                    .to_string(),
                details: None,
            },

            TransferServiceError::AccountNotFound { account_id } => Self {
                status: StatusCode::NOT_FOUND,
                code: "account_not_found",
                message: "an account referenced by this transfer does not exist".to_string(),
                details: Some(json!({ "account_id": account_id })),
            },

            // 422: the request is well-formed and the account exists, but the
            // business rule refuses it. Retrying unchanged will not help.
            TransferServiceError::InsufficientFunds { balance, required } => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "insufficient_funds",
                message: "the source account does not have enough funds".to_string(),
                details: Some(json!({ "balance_minor": balance, "required_minor": required })),
            },

            TransferServiceError::AccountNotActive { account_id } => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "account_not_active",
                message: "an account involved in this transfer is not active".to_string(),
                details: Some(json!({ "account_id": account_id })),
            },

            TransferServiceError::CurrencyMismatch => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "currency_mismatch",
                message: "account currency does not match the transfer currency".to_string(),
                details: None,
            },

            TransferServiceError::Storage(message) => ApiError::internal(message),
        }
    }
}
