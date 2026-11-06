//! Server-sent events: live processing progress for the signed-in user's media.

use std::{convert::Infallible, time::Duration};

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use serde::Serialize;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{AppState, error::ErrorBody, extract::CurrentUser};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(events))
}

/// Payload of a `media` event.
#[derive(Serialize, ToSchema)]
pub struct MediaStatusEvent {
    pub media_id: Uuid,
    /// `processing`, `ready`, `failed`, or `deleted`.
    pub status: String,
    pub progress: f32,
    pub error: Option<String>,
}

/// Event stream (`text/event-stream`). Events: `media` (a [`MediaStatusEvent`]) and
/// `resync` (updates were missed; refetch).
#[utoipa::path(
    get, path = "/api/events", tag = "media",
    responses(
        (status = 200, content_type = "text/event-stream", body = MediaStatusEvent),
        (status = 401, body = ErrorBody),
    )
)]
async fn events(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let owner = user.id;
    let stream = BroadcastStream::new(state.events.subscribe()).filter_map(move |msg| async move {
        match msg {
            Ok(ev) if ev.owner_id == owner => Event::default()
                .event("media")
                .json_data(MediaStatusEvent {
                    media_id: ev.media_id,
                    status: ev.status,
                    progress: ev.progress,
                    error: ev.error,
                })
                .ok()
                .map(Ok),
            Ok(_) => None,
            Err(BroadcastStreamRecvError::Lagged(_)) => {
                Some(Ok(Event::default().event("resync").data("{}")))
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}
