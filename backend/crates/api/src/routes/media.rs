//! The signed-in user's library: list, detail (with playback URLs), edit, trash.

use app_core::media::{self, Asset, Media};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ErrorBody},
    extract::CurrentUser,
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_media))
        .routes(routes!(get_media, update_media, trash_media))
        .routes(routes!(restore_media))
        .routes(routes!(list_trash))
        .routes(routes!(purge_media))
}

/// A video (photos arrive in M3) as shown in grids and lists.
#[derive(Serialize, ToSchema)]
pub struct MediaItem {
    pub id: Uuid,
    /// `video` or `photo`.
    pub kind: String,
    /// `uploading`, `processing`, `ready` or `failed`.
    pub status: String,
    pub title: String,
    pub description: String,
    pub original_filename: String,
    pub bytes: i64,
    /// Display size, after rotation. Known once processed.
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub duration_ms: Option<i64>,
    pub hdr: Option<bool>,
    /// When it was recorded, if the file says.
    pub taken_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub trashed_at: Option<DateTime<Utc>>,
    /// 0–1 while processing.
    pub processing_progress: f32,
    /// Why processing failed, for the owner.
    pub processing_error: Option<String>,
    /// Poster images, smallest first, as signed URLs. Empty until processed.
    pub posters: Vec<Poster>,
}

#[derive(Serialize, ToSchema)]
pub struct Poster {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, ToSchema)]
pub struct Playback {
    /// HLS master playlist (signed URL; segments inherit the token).
    pub hls_url: String,
    /// WebVTT track of scrub-preview thumbnails.
    pub thumbnails_url: Option<String>,
    pub renditions: Vec<RenditionInfo>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct RenditionInfo {
    /// e.g. `1080p`.
    pub label: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, ToSchema)]
pub struct MediaDetail {
    pub media: MediaItem,
    /// Present once the video is ready.
    pub playback: Option<Playback>,
}

#[derive(Serialize, ToSchema)]
pub struct MediaPage {
    pub items: Vec<MediaItem>,
    /// Pass as `cursor` to get the next page; absent on the last page.
    pub next_cursor: Option<String>,
}

/// Builds signed media URLs: `{MEDIA_BASE_URL}/{token}/m/{media_id}/{path}`.
pub struct MediaUrls<'a> {
    state: &'a AppState,
    now: i64,
}

impl<'a> MediaUrls<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self {
            state,
            now: Utc::now().timestamp(),
        }
    }

    pub fn url(&self, media_id: Uuid, path: &str) -> String {
        let token = self.state.tokens.issue(media_id, self.now);
        format!(
            "{}/{token}/m/{media_id}/{path}",
            self.state.config.media.base_url
        )
    }

    pub fn item(&self, media: Media, assets: &[Asset]) -> MediaItem {
        let posters = assets
            .iter()
            .find(|a| a.media_id == media.id && a.kind == "poster")
            .and_then(|a| a.meta.get("sizes"))
            .and_then(|s| s.as_array())
            .map(|sizes| {
                sizes
                    .iter()
                    .filter_map(|s| {
                        Some(Poster {
                            url: self.url(media.id, s.get("path")?.as_str()?),
                            width: s.get("width")?.as_u64()? as u32,
                            height: s.get("height")?.as_u64()? as u32,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        MediaItem {
            id: media.id,
            kind: media.kind,
            status: media.status,
            title: media.title,
            description: media.description,
            original_filename: media.original_filename,
            bytes: media.bytes,
            width: media.width,
            height: media.height,
            duration_ms: media.duration_ms,
            hdr: media.hdr,
            taken_at: media.taken_at,
            created_at: media.created_at,
            trashed_at: media.trashed_at,
            processing_progress: media.processing_progress,
            processing_error: media.processing_error,
            posters,
        }
    }

    fn playback(&self, media: &Media, assets: &[Asset]) -> Option<Playback> {
        if media.status != "ready" {
            return None;
        }
        let hls = assets.iter().find(|a| a.kind == "hls")?;
        Some(Playback {
            hls_url: self.url(media.id, &hls.path),
            thumbnails_url: assets
                .iter()
                .find(|a| a.kind == "sprites")
                .map(|a| self.url(media.id, &a.path)),
            renditions: hls
                .meta
                .get("renditions")
                .and_then(|r| serde_json::from_value(r.clone()).ok())
                .unwrap_or_default(),
        })
    }

    pub async fn items(&self, media: Vec<Media>) -> Result<Vec<MediaItem>, ApiError> {
        let ids: Vec<Uuid> = media.iter().map(|m| m.id).collect();
        let assets = media::assets(&self.state.db, &ids).await?;
        Ok(media.into_iter().map(|m| self.item(m, &assets)).collect())
    }
}

#[derive(Deserialize, IntoParams)]
pub struct ListParams {
    /// From the previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size, 1–100 (default 24).
    pub limit: Option<i64>,
}

fn encode_cursor(m: &Media) -> String {
    format!("{}_{}", m.created_at.timestamp_micros(), m.id)
}

fn decode_cursor(cursor: &str) -> Option<media::Cursor> {
    let (micros, id) = cursor.split_once('_')?;
    Some((
        DateTime::from_timestamp_micros(micros.parse().ok()?)?,
        id.parse().ok()?,
    ))
}

/// The library, newest first. Includes items still processing or that failed.
#[utoipa::path(
    get, path = "/api/media", tag = "media", params(ListParams),
    responses((status = 200, body = MediaPage), (status = 401, body = ErrorBody))
)]
async fn list_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<MediaPage>, ApiError> {
    let limit = params.limit.unwrap_or(24).clamp(1, 100);
    let after = match params.cursor.as_deref() {
        Some(c) => Some(
            decode_cursor(c)
                .ok_or_else(|| ApiError::bad_request("bad_cursor", "Invalid cursor."))?,
        ),
        None => None,
    };
    let mut rows = media::list_library(&state.db, user.id, after, limit + 1).await?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(encode_cursor)
    } else {
        None
    };
    let items = MediaUrls::new(&state).items(rows).await?;
    Ok(Json(MediaPage { items, next_cursor }))
}

/// One item, with playback URLs once it's ready.
#[utoipa::path(
    get, path = "/api/media/{id}", tag = "media",
    responses((status = 200, body = MediaDetail), (status = 404, body = ErrorBody))
)]
async fn get_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MediaDetail>, ApiError> {
    let item = media::get_owned(&state.db, user.id, id).await?;
    let assets = media::assets(&state.db, &[id]).await?;
    let urls = MediaUrls::new(&state);
    let playback = urls.playback(&item, &assets);
    Ok(Json(MediaDetail {
        media: urls.item(item, &assets),
        playback,
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateMedia {
    /// 1–200 characters.
    pub title: Option<String>,
    /// Up to 5,000 characters.
    pub description: Option<String>,
}

#[utoipa::path(
    patch, path = "/api/media/{id}", tag = "media", request_body = UpdateMedia,
    responses((status = 200, body = MediaItem), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody))
)]
async fn update_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMedia>,
) -> Result<Json<MediaItem>, ApiError> {
    let title = body.title.as_deref().map(str::trim);
    if title.is_some_and(|t| t.is_empty() || t.chars().count() > 200) {
        return Err(ApiError::bad_request(
            "invalid_title",
            "Titles must be 1–200 characters.",
        ));
    }
    if body
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > 5000)
    {
        return Err(ApiError::bad_request(
            "invalid_description",
            "Descriptions can be at most 5,000 characters.",
        ));
    }
    let item =
        media::update_details(&state.db, user.id, id, title, body.description.as_deref()).await?;
    Ok(Json(
        MediaUrls::new(&state).items(vec![item]).await?.remove(0),
    ))
}

/// Move to the trash. Restorable for 30 days, then deleted for good.
#[utoipa::path(
    delete, path = "/api/media/{id}", tag = "media",
    responses((status = 200, body = MediaItem), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
async fn trash_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MediaItem>, ApiError> {
    let item = media::trash(&state.db, user.id, id).await?;
    Ok(Json(
        MediaUrls::new(&state).items(vec![item]).await?.remove(0),
    ))
}

#[utoipa::path(
    post, path = "/api/media/{id}/restore", tag = "media",
    responses((status = 200, body = MediaItem), (status = 404, body = ErrorBody))
)]
async fn restore_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MediaItem>, ApiError> {
    let item = media::restore(&state.db, user.id, id).await?;
    Ok(Json(
        MediaUrls::new(&state).items(vec![item]).await?.remove(0),
    ))
}

#[derive(Serialize, ToSchema)]
pub struct TrashList {
    pub items: Vec<MediaItem>,
    /// Days items stay in the trash before being deleted for good.
    pub retention_days: u32,
}

#[utoipa::path(
    get, path = "/api/trash", tag = "media",
    responses((status = 200, body = TrashList), (status = 401, body = ErrorBody))
)]
async fn list_trash(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<TrashList>, ApiError> {
    let rows = media::list_trash(&state.db, user.id).await?;
    Ok(Json(TrashList {
        items: MediaUrls::new(&state).items(rows).await?,
        retention_days: (media::TRASH_RETENTION.as_secs() / 86_400) as u32,
    }))
}

/// Delete a trashed item for good, freeing its storage.
#[utoipa::path(
    delete, path = "/api/trash/{id}", tag = "media",
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
async fn purge_media(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    media::purge(&state.db, &state.storage, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
