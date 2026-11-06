use app_core::{User, auth};
use axum::{Json, extract::State, http::StatusCode};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ErrorBody},
    extract::{ClientInfo, session_cookie_name},
    routes::account::Me,
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(start_email_login))
        .routes(routes!(verify_email_code))
        .routes(routes!(verify_email_link))
        .routes(routes!(logout))
}

/// Cookie lifetime. The server-side session enforces the real (sliding) expiry;
/// this only needs to outlive it. Browsers cap cookies at 400 days.
const COOKIE_MAX_AGE_DAYS: i64 = 400;

#[derive(Deserialize, ToSchema)]
pub struct StartEmailLogin {
    pub email: String,
}

#[derive(Serialize, ToSchema)]
pub struct StartEmailLoginResponse {
    /// Pair with the emailed code in `/api/auth/email/verify-code`.
    pub login_id: Uuid,
}

#[derive(Deserialize, ToSchema)]
pub struct VerifyEmailCode {
    pub login_id: Uuid,
    pub code: String,
}

#[derive(Deserialize, ToSchema)]
pub struct VerifyEmailLink {
    /// The `token` from the emailed link's URL fragment.
    pub token: String,
}

/// Email a sign-in link and code. Creates the account on first successful verification.
#[utoipa::path(
    post, path = "/api/auth/email/start", tag = "auth",
    request_body = StartEmailLogin,
    responses(
        (status = 200, body = StartEmailLoginResponse),
        (status = 400, body = ErrorBody, description = "`invalid_email`"),
        (status = 429, body = ErrorBody, description = "`rate_limited`"),
    )
)]
async fn start_email_login(
    State(state): State<AppState>,
    client: ClientInfo,
    Json(body): Json<StartEmailLogin>,
) -> Result<Json<StartEmailLoginResponse>, ApiError> {
    let login_id = auth::start_email_login(
        &state.db,
        &state.mailer,
        &state.config,
        &body.email,
        client.ip.as_deref(),
    )
    .await?;
    Ok(Json(StartEmailLoginResponse { login_id }))
}

/// Sign in with the 6-digit code from the email. Sets the session cookie.
#[utoipa::path(
    post, path = "/api/auth/email/verify-code", tag = "auth",
    request_body = VerifyEmailCode,
    responses(
        (status = 200, body = Me),
        (status = 400, body = ErrorBody, description = "`incorrect_code` or `login_invalid`"),
        (status = 403, body = ErrorBody, description = "`account_disabled`"),
    )
)]
async fn verify_email_code(
    State(state): State<AppState>,
    client: ClientInfo,
    jar: CookieJar,
    Json(body): Json<VerifyEmailCode>,
) -> Result<(CookieJar, Json<Me>), ApiError> {
    let user = auth::verify_login_code(&state.db, body.login_id, &body.code).await?;
    start_session(&state, client, jar, user).await
}

/// Sign in with the token from the emailed magic link. Sets the session cookie.
#[utoipa::path(
    post, path = "/api/auth/email/verify-link", tag = "auth",
    request_body = VerifyEmailLink,
    responses(
        (status = 200, body = Me),
        (status = 400, body = ErrorBody, description = "`login_invalid`"),
        (status = 403, body = ErrorBody, description = "`account_disabled`"),
    )
)]
async fn verify_email_link(
    State(state): State<AppState>,
    client: ClientInfo,
    jar: CookieJar,
    Json(body): Json<VerifyEmailLink>,
) -> Result<(CookieJar, Json<Me>), ApiError> {
    let user = auth::verify_login_token(&state.db, &body.token).await?;
    start_session(&state, client, jar, user).await
}

/// End the current session.
#[utoipa::path(
    post, path = "/api/auth/logout", tag = "auth",
    responses((status = 204, description = "Signed out (also when there was no session)"))
)]
async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), ApiError> {
    let name = session_cookie_name(state.config.cookie_secure);
    if let Some(cookie) = jar.get(name) {
        auth::delete_session(&state.db, cookie.value()).await?;
    }
    let jar = jar.remove(session_cookie(&state, String::new()));
    Ok((jar, StatusCode::NO_CONTENT))
}

async fn start_session(
    state: &AppState,
    client: ClientInfo,
    jar: CookieJar,
    user: User,
) -> Result<(CookieJar, Json<Me>), ApiError> {
    let session = auth::create_session(
        &state.db,
        user.id,
        client.user_agent.as_deref(),
        client.ip.as_deref(),
    )
    .await?;
    tracing::info!(user_id = %user.id, "signed in");
    Ok((
        jar.add(session_cookie(state, session.token)),
        Json(user.into()),
    ))
}

fn session_cookie(state: &AppState, value: String) -> Cookie<'static> {
    let secure = state.config.cookie_secure;
    Cookie::build((session_cookie_name(secure), value))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::days(COOKIE_MAX_AGE_DAYS))
        .build()
}
