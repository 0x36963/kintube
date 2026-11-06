//! Media items: the upload lifecycle, library queries, trash, and processing results.
//!
//! Lifecycle: `uploading` → (complete) → `processing` → `ready` | `failed`.
//! Quota: starting an upload *reserves* its size; completing moves it to *used*;
//! aborting releases it. Purging a trashed item frees its used bytes.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::{
    events::{self, MediaEvent},
    jobs,
    storage::{Storage, UploadedPart, media_prefix, original_key},
};

/// Hard cap per file, whatever the remaining quota (PLAN.md §4).
pub const MAX_FILE_BYTES: i64 = 50 * 1024 * 1024 * 1024;
/// Max number of parts in an S3 multipart upload.
const MAX_PARTS: i64 = 10_000;
/// Trashed items are purged automatically after this long.
pub const TRASH_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// Uploads not completed within this long are aborted and their reservation released.
pub const STALE_UPLOAD_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("not found")]
    NotFound,
    #[error("only video files can be uploaded right now")]
    UnsupportedType,
    #[error("files can be at most 50 GB")]
    FileTooLarge,
    #[error("the file is empty")]
    EmptyFile,
    #[error("not enough storage left for this file")]
    StorageQuotaExceeded,
    #[error("the upload is missing parts")]
    UploadIncomplete,
    #[error("{0}")]
    InvalidState(&'static str),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Media {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub kind: String,
    pub status: String,
    pub title: String,
    pub description: String,
    pub original_filename: String,
    pub mime: String,
    pub bytes: i64,
    pub upload_id: Option<String>,
    pub part_size: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub duration_ms: Option<i64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hdr: Option<bool>,
    pub taken_at: Option<DateTime<Utc>>,
    pub processing_progress: f32,
    pub processing_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub ready_at: Option<DateTime<Utc>>,
    pub trashed_at: Option<DateTime<Utc>>,
}

/// Column list for `Media`, as a macro so queries can be built with `concat!` and stay
/// compile-time string literals (sqlx 0.9 rejects runtime-built SQL).
macro_rules! media_columns {
    () => {
        "id, owner_id, kind, status, title, description, original_filename, mime, bytes, \
         upload_id, part_size, width, height, duration_ms, video_codec, audio_codec, hdr, taken_at, \
         processing_progress, processing_error, created_at, updated_at, ready_at, trashed_at"
    };
}

impl Media {
    pub fn part_count(&self) -> i64 {
        let part_size = self.part_size.unwrap_or(self.bytes).max(1);
        (self.bytes + part_size - 1) / part_size
    }

    pub fn event(&self) -> MediaEvent {
        MediaEvent {
            owner_id: self.owner_id,
            media_id: self.id,
            status: self.status.clone(),
            progress: self.processing_progress,
            error: self.processing_error.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Asset {
    pub media_id: Uuid,
    pub kind: String,
    pub path: String,
    pub bytes: i64,
    pub meta: Value,
}

#[derive(Debug, Clone, Copy, Serialize, sqlx::FromRow)]
pub struct StorageUsage {
    pub storage_limit_bytes: i64,
    pub storage_used_bytes: i64,
    pub storage_reserved_bytes: i64,
    pub processing_minutes_limit: i32,
    pub processing_seconds_used: i64,
}

// ---------------------------------------------------------------------------------------
// Uploads

pub struct NewUpload<'a> {
    pub owner_id: Uuid,
    pub filename: &'a str,
    pub mime: &'a str,
    pub bytes: i64,
    /// Preferred part size; grown if needed to stay within S3's part limit.
    pub part_size: i64,
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "qt", "mkv", "webm", "avi", "mts", "m2ts", "ts", "3gp", "3g2", "mpg",
    "mpeg", "wmv", "flv", "ogv",
];

fn is_video(filename: &str, mime: &str) -> bool {
    if mime.starts_with("video/") {
        return true;
    }
    // Browsers report an empty or generic type for some containers (e.g. .mkv, .mts).
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    (mime.is_empty() || mime == "application/octet-stream")
        && VIDEO_EXTENSIONS.contains(&ext.as_str())
}

/// Part size for a file: the preferred size, grown (in whole MiB) if the file would
/// otherwise need more than 10,000 parts.
pub fn part_size_for(bytes: i64, preferred: i64) -> i64 {
    const MIB: i64 = 1024 * 1024;
    let min_for_limit = (bytes + MAX_PARTS - 1) / MAX_PARTS;
    let rounded = (min_for_limit + MIB - 1) / MIB * MIB;
    preferred.max(rounded)
}

fn title_from_filename(filename: &str) -> String {
    let stem = filename
        .rsplit_once('.')
        .map_or(filename, |(stem, _)| stem)
        .trim();
    let title: String = if stem.is_empty() { filename } else { stem }
        .chars()
        .take(200)
        .collect();
    if title.is_empty() {
        "Untitled".into()
    } else {
        title
    }
}

pub async fn begin_upload(
    db: &PgPool,
    storage: &Storage,
    new: NewUpload<'_>,
) -> Result<Media, MediaError> {
    if !is_video(new.filename, new.mime) {
        return Err(MediaError::UnsupportedType);
    }
    if new.bytes <= 0 {
        return Err(MediaError::EmptyFile);
    }
    if new.bytes > MAX_FILE_BYTES {
        return Err(MediaError::FileTooLarge);
    }
    let mime = if new.mime.is_empty() {
        "application/octet-stream"
    } else {
        new.mime
    };
    let media_id = Uuid::now_v7();
    let part_size = part_size_for(new.bytes, new.part_size);

    let mut tx = db.begin().await?;
    let reserved = sqlx::query(
        "update users set storage_reserved_bytes = storage_reserved_bytes + $2
         where id = $1 and storage_used_bytes + storage_reserved_bytes + $2 <= storage_limit_bytes",
    )
    .bind(new.owner_id)
    .bind(new.bytes)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if reserved != 1 {
        return Err(MediaError::StorageQuotaExceeded);
    }

    let key = original_key(media_id);
    let upload_id = storage.create_multipart_upload(&key, mime).await?;
    let media = sqlx::query_as::<_, Media>(concat!("insert into media (id, owner_id, kind, status, title, original_filename, mime, bytes, upload_id, part_size)
         values ($1, $2, 'video', 'uploading', $3, $4, $5, $6, $7, $8)
         returning ", media_columns!(), ""))
    .bind(media_id)
    .bind(new.owner_id)
    .bind(title_from_filename(new.filename))
    .bind(new.filename.chars().take(255).collect::<String>())
    .bind(mime)
    .bind(new.bytes)
    .bind(&upload_id)
    .bind(part_size)
    .fetch_one(&mut *tx)
    .await;

    match media {
        Ok(media) => {
            tx.commit().await?;
            Ok(media)
        }
        Err(err) => {
            drop(tx); // rolls back the reservation
            let _ = storage.abort_multipart_upload(&key, &upload_id).await;
            Err(err.into())
        }
    }
}

/// An in-progress upload and the part numbers already stored, so a client can resume.
pub async fn upload_progress(
    db: &PgPool,
    storage: &Storage,
    owner_id: Uuid,
    media_id: Uuid,
) -> Result<(Media, Vec<UploadedPart>), MediaError> {
    let media = get_uploading(db, owner_id, media_id).await?;
    let upload_id = media
        .upload_id
        .as_deref()
        .ok_or(MediaError::InvalidState("upload has no id"))?;
    let parts = storage
        .list_parts(&original_key(media_id), upload_id)
        .await?;
    Ok((media, parts))
}

pub async fn presign_parts(
    db: &PgPool,
    storage: &Storage,
    owner_id: Uuid,
    media_id: Uuid,
    part_numbers: &[i32],
) -> Result<Vec<(i32, String)>, MediaError> {
    let media = get_uploading(db, owner_id, media_id).await?;
    let upload_id = media
        .upload_id
        .as_deref()
        .ok_or(MediaError::InvalidState("upload has no id"))?;
    let key = original_key(media_id);
    let max = media.part_count();
    let mut urls = Vec::with_capacity(part_numbers.len());
    for &n in part_numbers {
        if n < 1 || i64::from(n) > max {
            return Err(MediaError::InvalidState("part number out of range"));
        }
        urls.push((
            n,
            storage
                .presign_upload_part(&key, upload_id, n, Duration::from_secs(3600))
                .await?,
        ));
    }
    Ok(urls)
}

/// Finishes an upload once every part is stored and queues processing.
/// Idempotent: completing an already-completed upload returns the item.
pub async fn complete_upload(
    db: &PgPool,
    storage: &Storage,
    owner_id: Uuid,
    media_id: Uuid,
) -> Result<Media, MediaError> {
    let media = get_owned(db, owner_id, media_id).await?;
    if media.status != "uploading" {
        return Ok(media);
    }
    let upload_id = media
        .upload_id
        .clone()
        .ok_or(MediaError::InvalidState("upload has no id"))?;
    let key = original_key(media_id);

    let parts = storage.list_parts(&key, &upload_id).await?;
    if !parts_complete(&parts, media.bytes, media.part_size.unwrap_or(media.bytes)) {
        return Err(MediaError::UploadIncomplete);
    }
    storage
        .complete_multipart_upload(&key, &upload_id, &parts)
        .await?;

    let mut tx = db.begin().await?;
    let media = sqlx::query_as::<_, Media>(concat!(
        "update media set status = 'processing', upload_id = null, updated_at = now()
         where id = $1 and status = 'uploading'
         returning ",
        media_columns!(),
        ""
    ))
    .bind(media_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(media) = media else {
        // A concurrent request completed it first.
        return get_owned(db, owner_id, media_id).await;
    };
    sqlx::query(
        "update users set storage_reserved_bytes = storage_reserved_bytes - $2,
                          storage_used_bytes = storage_used_bytes + $2
         where id = $1",
    )
    .bind(owner_id)
    .bind(media.bytes)
    .execute(&mut *tx)
    .await?;
    jobs::enqueue(
        &mut *tx,
        jobs::kind::PROCESS_VIDEO,
        Some(owner_id),
        json!({ "media_id": media_id }),
    )
    .await?;
    events::publish(&mut *tx, &media.event()).await?;
    tx.commit().await?;
    Ok(media)
}

/// True if parts 1..=n are all present, every part but the last is exactly `part_size`,
/// and the sizes add up to the declared file size.
fn parts_complete(parts: &[UploadedPart], bytes: i64, part_size: i64) -> bool {
    let expected = (bytes + part_size - 1) / part_size.max(1);
    parts.len() as i64 == expected
        && parts.iter().enumerate().all(|(i, p)| {
            p.number as i64 == i as i64 + 1 && (p.number as i64 == expected || p.size == part_size)
        })
        && parts.iter().map(|p| p.size).sum::<i64>() == bytes
}

pub async fn abort_upload(
    db: &PgPool,
    storage: &Storage,
    owner_id: Uuid,
    media_id: Uuid,
) -> Result<(), MediaError> {
    let media = get_uploading(db, owner_id, media_id).await?;
    if let Some(upload_id) = &media.upload_id {
        storage
            .abort_multipart_upload(&original_key(media_id), upload_id)
            .await?;
    }
    release_upload(db, &media).await
}

async fn release_upload(db: &PgPool, media: &Media) -> Result<(), MediaError> {
    let mut tx = db.begin().await?;
    let deleted = sqlx::query("delete from media where id = $1 and status = 'uploading'")
        .bind(media.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 1 {
        sqlx::query(
            "update users set storage_reserved_bytes = storage_reserved_bytes - $2 where id = $1",
        )
        .bind(media.owner_id)
        .bind(media.bytes)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Aborts uploads nobody finished, releasing their reservations. Returns how many.
pub async fn abort_stale_uploads(db: &PgPool, storage: &Storage) -> Result<usize, MediaError> {
    let stale = sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media
         where status = 'uploading' and created_at < now() - make_interval(secs => $1)
         limit 500"
    ))
    .bind(STALE_UPLOAD_AGE.as_secs_f64())
    .fetch_all(db)
    .await?;
    for media in &stale {
        if let Some(upload_id) = &media.upload_id {
            storage
                .abort_multipart_upload(&original_key(media.id), upload_id)
                .await?;
        }
        release_upload(db, media).await?;
    }
    Ok(stale.len())
}

// ---------------------------------------------------------------------------------------
// Library

pub async fn get_owned(
    db: impl PgExecutor<'_>,
    owner_id: Uuid,
    media_id: Uuid,
) -> Result<Media, MediaError> {
    sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media where id = $1 and owner_id = $2"
    ))
    .bind(media_id)
    .bind(owner_id)
    .fetch_optional(db)
    .await?
    .ok_or(MediaError::NotFound)
}

async fn get_uploading(db: &PgPool, owner_id: Uuid, media_id: Uuid) -> Result<Media, MediaError> {
    let media = get_owned(db, owner_id, media_id).await?;
    if media.status != "uploading" {
        return Err(MediaError::InvalidState("upload already finished"));
    }
    Ok(media)
}

pub async fn get_by_id(
    db: impl PgExecutor<'_>,
    media_id: Uuid,
) -> Result<Option<Media>, sqlx::Error> {
    sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media where id = $1"
    ))
    .bind(media_id)
    .fetch_optional(db)
    .await
}

/// Keyset cursor for library pages: `(created_at, id)` of the last item seen.
pub type Cursor = (DateTime<Utc>, Uuid);

/// The owner's library (excluding uploads in progress and trash), newest first.
pub async fn list_library(
    db: &PgPool,
    owner_id: Uuid,
    after: Option<Cursor>,
    limit: i64,
) -> Result<Vec<Media>, sqlx::Error> {
    let (ts, id) = after.unzip();
    sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media
         where owner_id = $1 and trashed_at is null and status <> 'uploading'
           and ($2::timestamptz is null or (created_at, id) < ($2, $3))
         order by created_at desc, id desc
         limit $4"
    ))
    .bind(owner_id)
    .bind(ts)
    .bind(id)
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn list_trash(db: &PgPool, owner_id: Uuid) -> Result<Vec<Media>, sqlx::Error> {
    sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media
         where owner_id = $1 and trashed_at is not null
         order by trashed_at desc limit 1000"
    ))
    .bind(owner_id)
    .fetch_all(db)
    .await
}

pub async fn assets(db: &PgPool, media_ids: &[Uuid]) -> Result<Vec<Asset>, sqlx::Error> {
    sqlx::query_as::<_, Asset>(
        "select media_id, kind, path, bytes, meta from media_assets where media_id = any($1)",
    )
    .bind(media_ids)
    .fetch_all(db)
    .await
}

pub async fn update_details(
    db: &PgPool,
    owner_id: Uuid,
    media_id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Media, MediaError> {
    sqlx::query_as::<_, Media>(concat!("update media set title = coalesce($3, title), description = coalesce($4, description), updated_at = now()
         where id = $1 and owner_id = $2
         returning ", media_columns!(), ""))
    .bind(media_id)
    .bind(owner_id)
    .bind(title)
    .bind(description)
    .fetch_optional(db)
    .await?
    .ok_or(MediaError::NotFound)
}

pub async fn trash(db: &PgPool, owner_id: Uuid, media_id: Uuid) -> Result<Media, MediaError> {
    let media = get_owned(db, owner_id, media_id).await?;
    if media.status == "uploading" {
        return Err(MediaError::InvalidState("cancel the upload instead"));
    }
    set_trashed(db, owner_id, media_id, true).await
}

pub async fn restore(db: &PgPool, owner_id: Uuid, media_id: Uuid) -> Result<Media, MediaError> {
    set_trashed(db, owner_id, media_id, false).await
}

async fn set_trashed(
    db: &PgPool,
    owner_id: Uuid,
    media_id: Uuid,
    trashed: bool,
) -> Result<Media, MediaError> {
    sqlx::query_as::<_, Media>(concat!("update media set trashed_at = case when $3 then coalesce(trashed_at, now()) end, updated_at = now()
         where id = $1 and owner_id = $2
         returning ", media_columns!(), ""))
    .bind(media_id)
    .bind(owner_id)
    .bind(trashed)
    .fetch_optional(db)
    .await?
    .ok_or(MediaError::NotFound)
}

/// Permanently deletes a trashed item: its files, its row, and its quota usage.
pub async fn purge(
    db: &PgPool,
    storage: &Storage,
    owner_id: Uuid,
    media_id: Uuid,
) -> Result<(), MediaError> {
    let media = get_owned(db, owner_id, media_id).await?;
    if media.trashed_at.is_none() {
        return Err(MediaError::InvalidState("move it to the trash first"));
    }
    purge_media(db, storage, &media).await
}

async fn purge_media(db: &PgPool, storage: &Storage, media: &Media) -> Result<(), MediaError> {
    storage.delete_prefix(&media_prefix(media.id)).await?;
    let mut tx = db.begin().await?;
    let deleted = sqlx::query("delete from media where id = $1")
        .bind(media.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 1 {
        sqlx::query("update users set storage_used_bytes = greatest(storage_used_bytes - $2, 0) where id = $1")
            .bind(media.owner_id)
            .bind(media.bytes)
            .execute(&mut *tx)
            .await?;
        events::publish(
            &mut *tx,
            &MediaEvent {
                status: "deleted".into(),
                ..media.event()
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Purges items that have been in the trash longer than [`TRASH_RETENTION`].
pub async fn purge_expired_trash(db: &PgPool, storage: &Storage) -> Result<usize, MediaError> {
    let expired = sqlx::query_as::<_, Media>(concat!(
        "select ",
        media_columns!(),
        " from media
         where trashed_at < now() - make_interval(secs => $1)
         limit 500"
    ))
    .bind(TRASH_RETENTION.as_secs_f64())
    .fetch_all(db)
    .await?;
    for media in &expired {
        purge_media(db, storage, media).await?;
    }
    Ok(expired.len())
}

pub async fn usage(db: &PgPool, user_id: Uuid) -> Result<StorageUsage, sqlx::Error> {
    sqlx::query_as::<_, StorageUsage>(
        "select u.storage_limit_bytes, u.storage_used_bytes, u.storage_reserved_bytes, u.processing_minutes_limit,
                coalesce(p.weighted_seconds, 0) as processing_seconds_used
         from users u
         left join processing_usage p on p.user_id = u.id and p.month = date_trunc('month', now() at time zone 'utc')::date
         where u.id = $1",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
}

// ---------------------------------------------------------------------------------------
// Processing (called by workers)

/// Everything processing learned about a video.
#[derive(Debug, Clone)]
pub struct ProcessedVideo {
    pub width: i32,
    pub height: i32,
    pub duration_ms: i64,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub hdr: bool,
    pub taken_at: Option<DateTime<Utc>>,
    /// Source seconds × resolution weight, charged to the monthly processing budget.
    pub weighted_seconds: i64,
    pub assets: Vec<NewAsset>,
}

#[derive(Debug, Clone)]
pub struct NewAsset {
    pub kind: &'static str,
    pub path: String,
    pub bytes: i64,
    pub meta: Value,
}

/// Records progress (0.0–1.0) and notifies the owner's open pages.
pub async fn set_progress(db: &PgPool, media_id: Uuid, progress: f32) -> Result<(), sqlx::Error> {
    let media = sqlx::query_as::<_, Media>(concat!(
        "update media set processing_progress = $2, updated_at = now()
         where id = $1 and status = 'processing'
         returning ",
        media_columns!(),
        ""
    ))
    .bind(media_id)
    .bind(progress.clamp(0.0, 1.0))
    .fetch_optional(db)
    .await?;
    if let Some(media) = media {
        events::publish(db, &media.event()).await?;
    }
    Ok(())
}

pub async fn mark_ready(
    db: &PgPool,
    media_id: Uuid,
    result: &ProcessedVideo,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("delete from media_assets where media_id = $1")
        .bind(media_id)
        .execute(&mut *tx)
        .await?;
    for asset in &result.assets {
        sqlx::query("insert into media_assets (media_id, kind, path, bytes, meta) values ($1, $2, $3, $4, $5)")
            .bind(media_id)
            .bind(asset.kind)
            .bind(&asset.path)
            .bind(asset.bytes)
            .bind(&asset.meta)
            .execute(&mut *tx)
            .await?;
    }
    let media = sqlx::query_as::<_, Media>(concat!("update media set status = 'ready', processing_progress = 1, processing_error = null,
             width = $2, height = $3, duration_ms = $4, video_codec = $5, audio_codec = $6, hdr = $7,
             taken_at = $8, ready_at = now(), updated_at = now()
         where id = $1
         returning ", media_columns!(), ""))
    .bind(media_id)
    .bind(result.width)
    .bind(result.height)
    .bind(result.duration_ms)
    .bind(&result.video_codec)
    .bind(&result.audio_codec)
    .bind(result.hdr)
    .bind(result.taken_at)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "insert into processing_usage (user_id, month, weighted_seconds)
         values ($1, date_trunc('month', now() at time zone 'utc')::date, $2)
         on conflict (user_id, month) do update set weighted_seconds = processing_usage.weighted_seconds + excluded.weighted_seconds",
    )
    .bind(media.owner_id)
    .bind(result.weighted_seconds)
    .execute(&mut *tx)
    .await?;
    events::publish(&mut *tx, &media.event()).await?;
    tx.commit().await
}

/// Marks processing as failed with a reason the owner will see.
pub async fn mark_failed(db: &PgPool, media_id: Uuid, reason: &str) -> Result<(), sqlx::Error> {
    let media = sqlx::query_as::<_, Media>(concat!(
        "update media set status = 'failed', processing_error = $2, updated_at = now()
         where id = $1 and status = 'processing'
         returning ",
        media_columns!(),
        ""
    ))
    .bind(media_id)
    .bind(reason)
    .fetch_optional(db)
    .await?;
    if let Some(media) = media {
        events::publish(db, &media.event()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(number: i32, size: i64) -> UploadedPart {
        UploadedPart {
            number,
            size,
            etag: format!("\"{number}\""),
        }
    }

    #[test]
    fn part_size_grows_only_when_needed() {
        const MIB: i64 = 1024 * 1024;
        assert_eq!(part_size_for(100 * MIB, 16 * MIB), 16 * MIB);
        assert_eq!(part_size_for(4 * 1024 * MIB, 16 * MIB), 16 * MIB);
        // 50 GiB / 10,000 parts → 5.12 MiB, rounded up to 6 MiB, still below 16.
        assert_eq!(part_size_for(MAX_FILE_BYTES, 16 * MIB), 16 * MIB);
        assert_eq!(part_size_for(MAX_FILE_BYTES, 5 * MIB), 6 * MIB);
        assert!(MAX_FILE_BYTES / part_size_for(MAX_FILE_BYTES, 5 * MIB) <= MAX_PARTS);
    }

    #[test]
    fn upload_completeness() {
        assert!(parts_complete(
            &[part(1, 10), part(2, 10), part(3, 5)],
            25,
            10
        ));
        assert!(parts_complete(&[part(1, 7)], 7, 10), "single short part");
        assert!(
            !parts_complete(&[part(1, 10), part(3, 5)], 25, 10),
            "missing middle part"
        );
        assert!(
            !parts_complete(&[part(1, 10), part(2, 10)], 25, 10),
            "missing last part"
        );
        assert!(
            !parts_complete(&[part(1, 10), part(2, 9), part(3, 6)], 25, 10),
            "wrong part size"
        );
        assert!(
            !parts_complete(&[part(1, 10), part(2, 10), part(3, 6)], 25, 10),
            "too many bytes"
        );
    }

    #[test]
    fn accepts_videos_by_type_or_extension() {
        assert!(is_video("clip.mov", "video/quicktime"));
        assert!(is_video("clip.mkv", ""));
        assert!(is_video("CLIP.MTS", "application/octet-stream"));
        assert!(!is_video("photo.jpg", "image/jpeg"));
        assert!(!is_video("notes.txt", ""));
        assert!(
            !is_video("sneaky.mp4", "text/html"),
            "explicit non-video type wins"
        );
    }

    #[test]
    fn titles_come_from_filenames() {
        assert_eq!(title_from_filename("Beach day.MOV"), "Beach day");
        assert_eq!(title_from_filename("IMG_0042"), "IMG_0042");
        assert_eq!(title_from_filename(".mov"), ".mov");
    }
}
