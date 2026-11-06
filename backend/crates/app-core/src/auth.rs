//! Passwordless email sign-in (magic link + 6-digit code) and server-side sessions.
//!
//! Secrets (link tokens, codes, session tokens) are never stored: only their SHA-256
//! hashes are. Sign-up and sign-in are the same flow; an account is created the first
//! time an email address is verified.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    Config, Mailer, User,
    mailer::Email,
    users::{self},
};

pub const LOGIN_TTL_MINUTES: i64 = 15;
pub const MAX_CODE_ATTEMPTS: i32 = 5;
pub const MAX_LOGIN_EMAILS_PER_HOUR: i64 = 5;
/// Sessions expire after this much inactivity…
pub const SESSION_IDLE_DAYS: i64 = 30;
/// …and unconditionally after this long.
pub const SESSION_MAX_DAYS: i64 = 365;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("that doesn't look like a valid email address")]
    InvalidEmail,
    #[error("too many sign-in emails requested; try again later")]
    RateLimited,
    #[error("this sign-in link or code is invalid or has expired")]
    InvalidOrExpired,
    #[error("incorrect code")]
    IncorrectCode,
    #[error("this account has been disabled")]
    AccountDisabled,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Mail(#[from] anyhow::Error),
}

/// Trims and lowercases an email address and applies a deliberately loose sanity check.
/// The real validation is that the user can receive mail at it.
pub fn normalize_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = email.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then_some(email)
}

/// Emails a magic link and a code. Returns the login id the client pairs with the code.
pub async fn start_email_login(
    db: &PgPool,
    mailer: &Mailer,
    config: &Config,
    raw_email: &str,
    ip: Option<&str>,
) -> Result<Uuid, AuthError> {
    let email = normalize_email(raw_email).ok_or(AuthError::InvalidEmail)?;

    let recent: i64 = sqlx::query_scalar(
        "select count(*) from login_tokens where email = $1 and created_at > now() - interval '1 hour'",
    )
    .bind(&email)
    .fetch_one(db)
    .await?;
    if recent >= MAX_LOGIN_EMAILS_PER_HOUR {
        return Err(AuthError::RateLimited);
    }

    let login_id = Uuid::now_v7();
    let token = random_token();
    let code = format!("{:06}", rand::random_range(0..1_000_000u32));

    sqlx::query(
        "insert into login_tokens (id, email, token_hash, code_hash, expires_at, ip)
         values ($1, $2, $3, $4, now() + make_interval(mins => $5), $6)",
    )
    .bind(login_id)
    .bind(&email)
    .bind(sha256(token.as_bytes()))
    .bind(code_hash(login_id, &code))
    .bind(LOGIN_TTL_MINUTES as i32)
    .bind(ip)
    .execute(db)
    .await?;

    // The token travels in the URL fragment: it never reaches a server log or a Referer
    // header, and link-preview bots that GET the URL can't consume it (the SPA POSTs it).
    let link = format!("{}/auth/verify#token={token}", config.app_origin);
    mailer
        .send(login_email(config, &email, &link, &code))
        .await?;

    Ok(login_id)
}

pub async fn verify_login_token(db: &PgPool, token: &str) -> Result<User, AuthError> {
    let email: Option<String> = sqlx::query_scalar(
        "update login_tokens set used_at = now()
         where token_hash = $1 and used_at is null and expires_at > now()
         returning email",
    )
    .bind(sha256(token.as_bytes()))
    .fetch_optional(db)
    .await?;

    let email = email.ok_or(AuthError::InvalidOrExpired)?;
    sign_in(db, &email).await
}

pub async fn verify_login_code(db: &PgPool, login_id: Uuid, code: &str) -> Result<User, AuthError> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AuthError::IncorrectCode);
    }

    // Count the attempt before checking it, atomically, so parallel guesses can't
    // exceed the limit. Exhausted, expired and used logins all return no row.
    let row: Option<(Vec<u8>, String)> = sqlx::query_as(
        "update login_tokens set attempts = attempts + 1
         where id = $1 and used_at is null and expires_at > now() and attempts < $2
         returning code_hash, email",
    )
    .bind(login_id)
    .bind(MAX_CODE_ATTEMPTS)
    .fetch_optional(db)
    .await?;

    let (stored_hash, email) = row.ok_or(AuthError::InvalidOrExpired)?;
    if !constant_time_eq(&stored_hash, &code_hash(login_id, code)) {
        return Err(AuthError::IncorrectCode);
    }

    let consumed =
        sqlx::query("update login_tokens set used_at = now() where id = $1 and used_at is null")
            .bind(login_id)
            .execute(db)
            .await?
            .rows_affected();
    if consumed != 1 {
        return Err(AuthError::InvalidOrExpired);
    }

    sign_in(db, &email).await
}

async fn sign_in(db: &PgPool, email: &str) -> Result<User, AuthError> {
    let user = users::find_or_create_by_email(db, email).await?;
    if user.deleted_at.is_some() {
        return Err(AuthError::AccountDisabled);
    }
    Ok(user)
}

pub struct NewSession {
    /// Raw token for the cookie. Not stored server-side.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn create_session(
    db: &PgPool,
    user_id: Uuid,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<NewSession, sqlx::Error> {
    let token = random_token();
    let expires_at = Utc::now() + Duration::days(SESSION_IDLE_DAYS);
    sqlx::query(
        "insert into sessions (id, token_hash, user_id, expires_at, user_agent, ip)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(sha256(token.as_bytes()))
    .bind(user_id)
    .bind(expires_at)
    .bind(user_agent.map(|ua| ua.chars().take(512).collect::<String>()))
    .bind(ip)
    .execute(db)
    .await?;
    Ok(NewSession { token, expires_at })
}

/// Resolves a session cookie to its user, sliding the idle expiry forward
/// (at most every 10 minutes, to avoid a write per request).
pub async fn session_user(db: &PgPool, token: &str) -> Result<Option<User>, sqlx::Error> {
    let hash = sha256(token.as_bytes());
    let user = sqlx::query_as::<_, User>(
        "select u.id, u.email, u.display_name, u.role, u.created_at, u.deleted_at
         from sessions s join users u on u.id = s.user_id
         where s.token_hash = $1
           and s.expires_at > now()
           and s.created_at > now() - make_interval(days => $2)
           and u.deleted_at is null",
    )
    .bind(&hash)
    .bind(SESSION_MAX_DAYS as i32)
    .fetch_optional(db)
    .await?;

    if user.is_some() {
        sqlx::query(
            "update sessions set last_seen_at = now(), expires_at = now() + make_interval(days => $2)
             where token_hash = $1 and last_seen_at < now() - interval '10 minutes'",
        )
        .bind(&hash)
        .bind(SESSION_IDLE_DAYS as i32)
        .execute(db)
        .await?;
    }
    Ok(user)
}

pub async fn delete_session(db: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from sessions where token_hash = $1")
        .bind(sha256(token.as_bytes()))
        .execute(db)
        .await?;
    Ok(())
}

fn login_email(config: &Config, to: &str, link: &str, code: &str) -> Email {
    let app = &config.app_name;
    let text = format!(
        "Sign in to {app}\n\n\
         Open this link to sign in:\n{link}\n\n\
         Or enter this code: {code}\n\n\
         The link and code expire in {LOGIN_TTL_MINUTES} minutes. \
         If you didn't ask to sign in, you can ignore this email.\n"
    );
    let html = format!(
        r#"<div style="font-family:system-ui,sans-serif;max-width:480px;margin:auto;padding:24px;color:#111">
<h1 style="font-size:20px">Sign in to {app}</h1>
<p><a href="{link}" style="display:inline-block;background:#111;color:#fff;padding:12px 20px;border-radius:8px;text-decoration:none">Sign in</a></p>
<p>Or enter this code:</p>
<p style="font-size:28px;letter-spacing:6px;font-weight:600">{code}</p>
<p style="color:#666;font-size:13px">The link and code expire in {LOGIN_TTL_MINUTES} minutes. If you didn't ask to sign in, you can ignore this email.</p>
</div>"#
    );
    Email {
        to: to.to_owned(),
        subject: format!("{code} is your {app} sign-in code"),
        text,
        html,
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn code_hash(login_id: Uuid, code: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(login_id.as_bytes());
    hasher.update(code.as_bytes());
    hasher.finalize().to_vec()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_emails() {
        assert_eq!(
            normalize_email("  Ada@Example.COM "),
            Some("ada@example.com".into())
        );
        for bad in [
            "",
            "ada",
            "ada@",
            "@example.com",
            "ada@example",
            "a da@example.com",
            "ada@.com",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad:?} should be rejected");
        }
    }

    #[test]
    fn tokens_are_unique_and_url_safe() {
        let (a, b) = (random_token(), random_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }
}
