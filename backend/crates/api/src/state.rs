use std::{net::IpAddr, num::NonZeroU32, sync::Arc, time::Duration};

use app_core::{Config, Mailer, events::MediaEvent, media_token::MediaTokens, storage::Storage};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sqlx::{PgPool, postgres::PgListener};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub mailer: Mailer,
    pub config: Arc<Config>,
    pub storage: Storage,
    pub tokens: MediaTokens,
    /// Media status updates from workers, fanned out to `/api/events` streams.
    pub events: broadcast::Sender<MediaEvent>,
    /// Per-IP limit on `/api/auth/*`, on top of the per-email limit in `app_core::auth`.
    pub auth_limiter: Arc<DefaultKeyedRateLimiter<IpAddr>>,
}

impl AppState {
    pub fn new(db: PgPool, mailer: Mailer, config: Config) -> Self {
        let per_minute =
            NonZeroU32::new(config.auth_rate_limit_per_minute).unwrap_or(NonZeroU32::MIN);
        let (events, _) = broadcast::channel(1024);
        Self {
            db,
            mailer,
            storage: Storage::new(&config.s3),
            tokens: MediaTokens::new(config.media.token_secret.clone()),
            config: Arc::new(config),
            events,
            auth_limiter: Arc::new(RateLimiter::keyed(Quota::per_minute(per_minute))),
        }
    }

    /// Background tasks: rate-limiter cleanup and the Postgres → SSE event bridge.
    pub fn spawn_background(&self) {
        let limiter = self.auth_limiter.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                limiter.retain_recent();
            }
        });
        tokio::spawn(forward_events(self.db.clone(), self.events.clone()));
    }
}

/// Relays `NOTIFY media_events` into the in-process broadcast channel, reconnecting forever.
pub async fn forward_events(db: PgPool, tx: broadcast::Sender<MediaEvent>) {
    loop {
        let result = async {
            let mut listener = PgListener::connect_with(&db).await?;
            listener.listen(app_core::events::CHANNEL).await?;
            loop {
                let notification = listener.recv().await?;
                match serde_json::from_str::<MediaEvent>(notification.payload()) {
                    // No receivers is fine: nobody is watching right now.
                    Ok(event) => drop(tx.send(event)),
                    Err(err) => tracing::warn!(error = %err, "malformed media event"),
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), sqlx::Error>(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "event listener disconnected; reconnecting");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
