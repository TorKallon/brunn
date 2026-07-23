use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{message}")]
    Public {
        status: StatusCode,
        code: &'static str,
        message: String,
        details: Option<Value>,
    },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
    request_id: String,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ApiError {
    pub fn public(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::Public {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self::Public {
            status,
            code,
            message: message.into(),
            details: Some(details),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::public(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    pub fn unauthenticated() -> Self {
        Self::public(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "a valid bearer token is required",
        )
    }

    pub fn capability(capability: &str) -> Self {
        Self::with_details(
            StatusCode::FORBIDDEN,
            "capability_denied",
            "the credential does not grant this operation",
            json!({"required_capability": capability}),
        )
    }

    pub fn not_found(kind: &'static str, reference: &str) -> Self {
        Self::with_details(
            StatusCode::NOT_FOUND,
            kind,
            "the requested reference was not found",
            json!({"ref": reference}),
        )
    }

    pub fn conflict(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self::with_details(StatusCode::CONFLICT, code, message, details)
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = format!("req_{}", Uuid::now_v7().simple());
        let (status, code, message, details) = match self {
            Self::Public {
                status,
                code,
                message,
                details,
            } => (status, code, message, details),
            other => {
                error!(error = ?other, request_id, "unhandled API error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "an internal error occurred".to_owned(),
                    None,
                )
            }
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code,
                    message,
                    details,
                },
                request_id,
            }),
        )
            .into_response()
    }
}
