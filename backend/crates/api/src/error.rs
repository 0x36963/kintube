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
