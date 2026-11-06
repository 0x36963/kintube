use app_core::{User, media};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ErrorBody},
    extract::CurrentUser,
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(me))
        .routes(routes!(usage))
}

/// The signed-in user's own account.
#[derive(Serialize, ToSchema)]
pub struct Me {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    /// `user` or `admin`.
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl From<User> for Me {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            display_name: user.display_name,
            role: user.role,
            created_at: user.created_at,
        }
    }
}

#[utoipa::path(
    get, path = "/api/me", tag = "account",
    responses(
        (status = 200, body = Me),
        (status = 401, body = ErrorBody),
    )
)]
async fn me(CurrentUser(user): CurrentUser) -> Json<Me> {
    Json(user.into())
}

/// Storage and monthly processing usage against the account's limits.
#[derive(Serialize, ToSchema)]
pub struct Usage {
    pub storage_limit_bytes: i64,
    /// Originals of uploaded media (derived files don't count).
    pub storage_used_bytes: i64,
    /// Held for uploads in progress.
    pub storage_reserved_bytes: i64,
    /// Weighted minutes per calendar month (UTC): ×1 up to 1080p, ×2 for 1440p, ×4 for 4K.
    pub processing_minutes_limit: i32,
    pub processing_minutes_used: i64,
}

#[utoipa::path(
    get, path = "/api/me/usage", tag = "account",
    responses((status = 200, body = Usage), (status = 401, body = ErrorBody))
)]
async fn usage(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<Usage>, ApiError> {
    let u = media::usage(&state.db, user.id).await?;
    Ok(Json(Usage {
        storage_limit_bytes: u.storage_limit_bytes,
        storage_used_bytes: u.storage_used_bytes,
        storage_reserved_bytes: u.storage_reserved_bytes,
        processing_minutes_limit: u.processing_minutes_limit,
        processing_minutes_used: (u.processing_seconds_used + 59) / 60,
    }))
}
