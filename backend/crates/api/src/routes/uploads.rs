//! Resumable uploads, straight from the browser to object storage.
//!
//! 1. `POST /api/uploads` reserves quota and starts an S3 multipart upload.
//! 2. `POST /api/uploads/{id}/parts` presigns part URLs; the browser `PUT`s bytes to them.
//! 3. `GET /api/uploads/{id}` lists stored parts, so an interrupted upload can resume.
//! 4. `POST /api/uploads/{id}/complete` assembles the file and queues processing.
//!    The server reads part ETags from storage, so browsers never need to.

use app_core::media::{self, NewUpload};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ErrorBody},
    extract::CurrentUser,
    routes::media::{MediaItem, MediaUrls},
};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_upload))
        .routes(routes!(upload_status, abort_upload))
        .routes(routes!(presign_parts))
        .routes(routes!(complete_upload))
}

#[derive(Deserialize, ToSchema)]
pub struct CreateUpload {
    pub filename: String,
    /// Bytes.
    pub size: i64,
    /// The browser's `File.type`; may be empty.
    #[serde(default)]
    pub mime_type: String,
}

#[derive(Serialize, ToSchema)]
pub struct UploadInfo {
    pub media_id: Uuid,
    /// Every part but the last must be exactly this many bytes.
    pub part_size: i64,
    pub part_count: i64,
    /// Part numbers (1-based) already stored. Empty for a new upload.
    pub uploaded_parts: Vec<i32>,
}

/// Start an upload. Fails with `storage_quota_exceeded` if the file doesn't fit.
#[utoipa::path(
    post, path = "/api/uploads", tag = "uploads", request_body = CreateUpload,
    responses(
        (status = 201, body = UploadInfo),
        (status = 409, body = ErrorBody, description = "`storage_quota_exceeded`"),
        (status = 413, body = ErrorBody, description = "`file_too_large`"),
        (status = 415, body = ErrorBody, description = "`unsupported_type`"),
    )
)]
async fn create_upload(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateUpload>,
) -> Result<(StatusCode, Json<UploadInfo>), ApiError> {
    let filename = body.filename.trim();
    if filename.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_filename",
            "The file needs a name.",
        ));
    }
    let item = media::begin_upload(
        &state.db,
        &state.storage,
        NewUpload {
            owner_id: user.id,
            filename,
            mime: &body.mime_type,
            bytes: body.size,
            part_size: state.config.media.upload_part_size as i64,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(UploadInfo {
            media_id: item.id,
            part_size: item.part_size.unwrap_or(item.bytes),
            part_count: item.part_count(),
            uploaded_parts: vec![],
        }),
    ))
}

/// Which parts are already stored. Use it to resume after a refresh or network loss.
#[utoipa::path(
    get, path = "/api/uploads/{id}", tag = "uploads",
    responses((status = 200, body = UploadInfo), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
async fn upload_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<UploadInfo>, ApiError> {
    let (item, parts) = media::upload_progress(&state.db, &state.storage, user.id, id).await?;
    Ok(Json(UploadInfo {
        media_id: item.id,
        part_size: item.part_size.unwrap_or(item.bytes),
        part_count: item.part_count(),
        uploaded_parts: parts.iter().map(|p| p.number).collect(),
    }))
}

#[derive(Deserialize, ToSchema)]
pub struct PresignParts {
    /// Up to 100 part numbers.
    pub part_numbers: Vec<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct PresignedPart {
    pub part_number: i32,
    /// `PUT` the part's bytes here. Valid for an hour.
    pub url: String,
}

#[derive(Serialize, ToSchema)]
pub struct PresignedParts {
    pub parts: Vec<PresignedPart>,
}

#[utoipa::path(
    post, path = "/api/uploads/{id}/parts", tag = "uploads", request_body = PresignParts,
    responses((status = 200, body = PresignedParts), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
async fn presign_parts(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<PresignParts>,
) -> Result<Json<PresignedParts>, ApiError> {
    if body.part_numbers.is_empty() || body.part_numbers.len() > 100 {
        return Err(ApiError::bad_request(
            "invalid_parts",
            "Request between 1 and 100 parts at a time.",
        ));
    }
    let urls =
        media::presign_parts(&state.db, &state.storage, user.id, id, &body.part_numbers).await?;
    Ok(Json(PresignedParts {
        parts: urls
            .into_iter()
            .map(|(part_number, url)| PresignedPart { part_number, url })
            .collect(),
    }))
}

/// Finish the upload and queue processing. Idempotent.
#[utoipa::path(
    post, path = "/api/uploads/{id}/complete", tag = "uploads",
    responses(
        (status = 200, body = MediaItem),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "`upload_incomplete`"),
    )
)]
async fn complete_upload(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MediaItem>, ApiError> {
    let item = media::complete_upload(&state.db, &state.storage, user.id, id).await?;
    Ok(Json(
        MediaUrls::new(&state).items(vec![item]).await?.remove(0),
    ))
}

/// Cancel an upload and release its reserved storage.
#[utoipa::path(
    delete, path = "/api/uploads/{id}", tag = "uploads",
    responses((status = 204), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody))
)]
async fn abort_upload(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    media::abort_upload(&state.db, &state.storage, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
