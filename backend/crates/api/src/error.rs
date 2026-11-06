use app_core::auth::AuthError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

/// Every error response has this shape: `{"error": {"code": "...", "message": "..."}}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable, machine-readable identifier, e.g. `unauthenticated`.
    pub code: String,
    /// Human-readable explanation, safe to show to the user.
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "You need to sign in.",
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Something went wrong on our side.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.to_owned(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        Self::internal()
    }
}

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        let message = err.to_string();
        match err {
            AuthError::InvalidEmail => Self::new(StatusCode::BAD_REQUEST, "invalid_email", message),
            AuthError::RateLimited => {
                Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
            }
            AuthError::InvalidOrExpired => {
                Self::new(StatusCode::BAD_REQUEST, "login_invalid", message)
            }
            AuthError::IncorrectCode => {
                Self::new(StatusCode::BAD_REQUEST, "incorrect_code", message)
            }
            AuthError::AccountDisabled => {
                Self::new(StatusCode::FORBIDDEN, "account_disabled", message)
            }
            AuthError::Db(err) => err.into(),
            AuthError::Mail(err) => {
                tracing::error!(error = ?err, "failed to send email");
                Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "email_unavailable",
                    "We couldn't send the email right now. Please try again shortly.",
                )
            }
        }
    }
}

impl From<app_core::media::MediaError> for ApiError {
    fn from(err: app_core::media::MediaError) -> Self {
        use app_core::media::MediaError as E;
        let message = err.to_string();
        let message = {
            let mut c = message.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>() + ".")
                .unwrap_or_default()
        };
        match err {
            E::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "That video doesn't exist.",
            ),
            E::UnsupportedType => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_type",
                message,
            ),
            E::FileTooLarge => Self::new(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large", message),
            E::EmptyFile => Self::new(StatusCode::BAD_REQUEST, "empty_file", message),
            E::StorageQuotaExceeded => {
                Self::new(StatusCode::CONFLICT, "storage_quota_exceeded", message)
            }
            E::UploadIncomplete => Self::new(StatusCode::CONFLICT, "upload_incomplete", message),
            E::InvalidState(_) => Self::new(StatusCode::CONFLICT, "invalid_state", message),
            E::Db(err) => err.into(),
            E::Storage(err) => {
                tracing::error!(error = ?err, "object storage error");
                Self::new(
                    StatusCode::BAD_GATEWAY,
                    "storage_unavailable",
                    "Storage is temporarily unavailable. Please try again.",
                )
            }
        }
    }
}

impl ApiError {
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }
}
