use std::net::{IpAddr, SocketAddr};

use app_core::{User, auth};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts},
};
use axum_extra::extract::CookieJar;

use crate::{AppState, error::ApiError};

/// Name of the session cookie. The `__Host-` prefix makes browsers refuse the cookie
/// unless it is `Secure`, `Path=/` and has no `Domain`, so it can't be set or
/// overridden by a sibling subdomain. Plain-http dev can't use the prefix.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure { "__Host-sid" } else { "sid" }
}

/// The signed-in user. Rejects with 401 when there is no valid session.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(session_cookie_name(state.config.cookie_secure))
            .map(|c| c.value().to_owned())
            .ok_or_else(ApiError::unauthenticated)?;
        auth::session_user(&state.db, &token)
            .await?
            .map(CurrentUser)
            .ok_or_else(ApiError::unauthenticated)
    }
}

/// Client IP: from the configured trusted-proxy header if set, else the TCP peer.
pub fn client_ip(
    state: &AppState,
    parts_headers: &HeaderMap,
    extensions: &axum::http::Extensions,
) -> Option<IpAddr> {
    if let Some(header) = &state.config.client_ip_header {
        return parts_headers
            .get(header)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
    }
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

/// Request metadata recorded on login attempts and sessions.
pub struct ClientInfo {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            ip: client_ip(state, &parts.headers, &parts.extensions).map(|ip| ip.to_string()),
            user_agent: parts
                .headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        })
    }
}
