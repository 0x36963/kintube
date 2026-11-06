use app_core::User;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{AppState, error::ErrorBody, extract::CurrentUser};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(me))
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
