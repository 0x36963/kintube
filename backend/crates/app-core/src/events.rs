//! Live media status updates, sent from workers to API servers over Postgres
//! `NOTIFY media_events` and on to browsers as server-sent events.

use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;
use uuid::Uuid;

pub const CHANNEL: &str = "media_events";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaEvent {
    pub owner_id: Uuid,
    pub media_id: Uuid,
    /// `processing`, `ready`, `failed`, or `deleted`.
    pub status: String,
    /// 0.0–1.0 while processing.
    pub progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn publish(db: impl PgExecutor<'_>, event: &MediaEvent) -> Result<(), sqlx::Error> {
    sqlx::query("select pg_notify($1, $2)")
        .bind(CHANNEL)
        .bind(serde_json::to_string(event).expect("event serializes"))
        .execute(db)
        .await?;
    Ok(())
}
