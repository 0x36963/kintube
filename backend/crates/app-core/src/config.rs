use std::net::SocketAddr;

use anyhow::{Context, Result};

/// Runtime configuration, read from environment variables (see `backend/.env.example`).
#[derive(Clone, Debug)]
pub struct Config {
    /// Product name used in emails. Placeholder until the real name is chosen.
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
            cookie_secure: var_or("COOKIE_SECURE", "true")
                .parse()
                .context("COOKIE_SECURE must be true or false")?,
            client_ip_header: std::env::var("CLIENT_IP_HEADER")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.to_ascii_lowercase()),
            auth_rate_limit_per_minute: var_or("AUTH_RATE_LIMIT_PER_MINUTE", "20")
                .parse()
                .context("AUTH_RATE_LIMIT_PER_MINUTE must be a positive integer")?,
        })
    }
}

fn var(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing required env var {name}"))
}

fn var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}
