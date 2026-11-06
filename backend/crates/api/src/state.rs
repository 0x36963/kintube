use std::{net::IpAddr, num::NonZeroU32, sync::Arc, time::Duration};

use app_core::{Config, Mailer};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub mailer: Mailer,
    pub config: Arc<Config>,
    /// Per-IP limit on `/api/auth/*`, on top of the per-email limit in `app_core::auth`.
    pub auth_limiter: Arc<DefaultKeyedRateLimiter<IpAddr>>,
}

impl AppState {
    pub fn new(db: PgPool, mailer: Mailer, config: Config) -> Self {
        let per_minute =
            NonZeroU32::new(config.auth_rate_limit_per_minute).unwrap_or(NonZeroU32::MIN);
        let quota = Quota::per_minute(per_minute);
        Self {
            db,
            mailer,
            config: Arc::new(config),
            auth_limiter: Arc::new(RateLimiter::keyed(quota)),
        }
    }

    /// Periodically drops idle rate-limiter keys so memory doesn't grow with unique IPs.
    pub fn spawn_maintenance(&self) {
        let limiter = self.auth_limiter.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                limiter.retain_recent();
            }
        });
    }
}
