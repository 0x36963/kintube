use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    error::{ApiError, ErrorBody},
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(health))
}

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: String,
}

/// Liveness + database connectivity.
#[utoipa::path(
    get, path = "/api/health", tag = "system",
    responses(
        (status = 200, body = Health),
        (status = 503, body = ErrorBody, description = "Database unreachable"),
    )
)]
async fn health(State(state): State<AppState>) -> Result<Json<Health>, ApiError> {
    sqlx::query("select 1")
        .execute(&state.db)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "health check: database unreachable");
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "db_unavailable",
                "Database unreachable.",
            )
        })?;
    Ok(Json(Health {
        status: "ok".into(),
    }))
}
