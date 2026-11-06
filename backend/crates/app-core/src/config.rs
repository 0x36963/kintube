use std::net::SocketAddr;

use anyhow::{Context, Result, ensure};

/// API server configuration, read from environment variables (see `backend/.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    /// Product name used in emails.
    pub app_name: String,
    /// Public origin of the web app, e.g. `https://app.example.com`. Used to build
    /// links in emails and as the only accepted `Origin` for state-changing requests.
    pub app_origin: String,
    pub database_url: String,
    pub bind_addr: SocketAddr,
    /// e.g. `smtp://localhost:1025` (Mailpit) or `smtps://user:pass@smtp.provider.com`.
    pub smtp_url: String,
    pub email_from: String,
    /// `false` only for plain-http local development.
    pub cookie_secure: bool,
    /// Header carrying the real client IP when behind a trusted proxy
    /// (`cf-connecting-ip` behind Cloudflare). Unset = use the TCP peer address.
    pub client_ip_header: Option<String>,
    /// Per-IP requests per minute on `/api/auth/*`.
    pub auth_rate_limit_per_minute: u32,
    pub s3: S3Config,
    pub media: MediaConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            app_name: var_or("APP_NAME", "kintube"),
            app_origin: var("APP_ORIGIN")?.trim_end_matches('/').to_owned(),
            database_url: var("DATABASE_URL")?,
            bind_addr: var_or("BIND_ADDR", "127.0.0.1:8787")
                .parse()
                .context("BIND_ADDR must be host:port")?,
            smtp_url: var("SMTP_URL")?,
            email_from: var("EMAIL_FROM")?,
            cookie_secure: parse_var("COOKIE_SECURE", "true")?,
            client_ip_header: std::env::var("CLIENT_IP_HEADER")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.to_ascii_lowercase()),
            auth_rate_limit_per_minute: parse_var("AUTH_RATE_LIMIT_PER_MINUTE", "20")?,
            s3: S3Config::from_env()?,
            media: MediaConfig::from_env()?,
        })
    }
}

/// S3-compatible object storage: Cloudflare R2 in production, RustFS in development.
#[derive(Clone, Debug)]
pub struct S3Config {
    /// Endpoint the API and workers talk to.
    pub endpoint: String,
    /// Endpoint embedded in presigned URLs handed to browsers. Usually the same as
    /// `endpoint`; differs when servers reach storage over a private address.
    pub public_endpoint: String,
    /// `auto` for R2.
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Path-style URLs (`endpoint/bucket/key`). Needed for local S3 servers.
    pub force_path_style: bool,
}

impl S3Config {
    pub fn from_env() -> Result<Self> {
        let endpoint = var("S3_ENDPOINT")?.trim_end_matches('/').to_owned();
        Ok(Self {
            public_endpoint: std::env::var("S3_PUBLIC_ENDPOINT")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.trim_end_matches('/').to_owned())
                .unwrap_or_else(|| endpoint.clone()),
            endpoint,
            region: var_or("S3_REGION", "auto"),
            bucket: var("S3_BUCKET")?,
            access_key_id: var("S3_ACCESS_KEY_ID")?,
            secret_access_key: var("S3_SECRET_ACCESS_KEY")?,
            force_path_style: parse_var("S3_FORCE_PATH_STYLE", "false")?,
        })
    }
}

/// Upload limits and private media delivery.
#[derive(Clone, Debug)]
pub struct MediaConfig {
    /// Base URL for tokenized media URLs: the Cloudflare media gateway in production,
    /// the API's own `/media` route in development. A separate, cookieless origin.
    pub base_url: String,
    /// HMAC key shared with the media gateway. At least 32 bytes.
    pub token_secret: Vec<u8>,
    /// Multipart upload part size. S3 requires at least 5 MiB for all but the last part.
    pub upload_part_size: u64,
    /// Serve `/media/…` from the API itself (development). In production the Cloudflare
    /// media gateway serves it from R2 instead.
    pub serve_from_api: bool,
}

impl MediaConfig {
    pub fn from_env() -> Result<Self> {
        let token_secret = var("MEDIA_TOKEN_SECRET")?.into_bytes();
        ensure!(
            token_secret.len() >= 32,
            "MEDIA_TOKEN_SECRET must be at least 32 bytes"
        );
        let part_mib: u64 = parse_var("UPLOAD_PART_SIZE_MIB", "16")?;
        ensure!(part_mib >= 5, "UPLOAD_PART_SIZE_MIB must be at least 5");
        Ok(Self {
            base_url: var("MEDIA_BASE_URL")?.trim_end_matches('/').to_owned(),
            token_secret,
            upload_part_size: part_mib * 1024 * 1024,
            serve_from_api: parse_var("MEDIA_SERVE_FROM_API", "false")?,
        })
    }
}

fn var(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing required env var {name}"))
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn parse_var<T: std::str::FromStr>(name: &str, default: &str) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    var_or(name, default)
        .parse()
        .with_context(|| format!("invalid value for {name}"))
}
