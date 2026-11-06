//! HTTP API. `main.rs` is a thin CLI around [`app`] and [`openapi`]; tests drive [`app`] directly.

pub mod error;
pub mod extract;
pub mod middleware;
pub mod routes;
pub mod state;

use axum::{Json, Router, routing::get};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(title = "kintube API", description = "Private media library API."),
    tags(
        (name = "auth", description = "Passwordless sign-in and sessions"),
        (name = "account", description = "The signed-in user"),
        (name = "system", description = "Health and metadata"),
    )
)]
struct ApiDoc;

fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(routes::system::router())
        .merge(routes::auth::router())
        .merge(routes::account::router())
}

/// The full application router, ready to serve.
pub fn app(state: AppState) -> Router {
    let (router, api) = api_router().split_for_parts();
    router
        .route("/api/openapi.json", get(move || async move { Json(api) }))
        // Layers run outermost-last: trace → same-origin check → auth rate limit → handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_same_origin,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The OpenAPI document, used to generate the TypeScript client.
pub fn openapi() -> utoipa::openapi::OpenApi {
    api_router().into_openapi()
}
