use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{AppState, error::ApiError, extract::client_ip};

/// CSRF defense: state-changing requests must come from the app's own origin.
/// Browsers always send `Origin` on cross-origin and on non-GET same-origin fetches,
/// and pages can't forge it. Combined with `SameSite=Lax` session cookies.
pub async fn require_same_origin(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());
        if origin != Some(state.config.app_origin.as_str()) {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "bad_origin",
                "Request origin not allowed.",
            )
            .into_response();
        }
    }
    next.run(req).await
}

/// Per-IP rate limit for the unauthenticated auth endpoints.
pub async fn rate_limit_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if req.uri().path().starts_with("/api/auth/") {
        // No IP (shouldn't happen in production) → share one bucket rather than skip limiting.
        let ip = client_ip(&state, req.headers(), req.extensions())
            .unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));
        if state.auth_limiter.check_key(&ip).is_err() {
            return ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many requests. Please wait a minute and try again.",
            )
            .into_response();
        }
    }
    next.run(req).await
}
