//! `process_video` job: original → HLS ladder, poster, sprites, all stored under `m/{id}/`.

use std::time::{Duration, Instant};

use anyhow::Context;
use app_core::{
    jobs::Job,
    media::{self, NewAsset, ProcessedVideo},
    storage::{content_type_for, media_prefix, original_key},
};
use futures::{StreamExt, TryStreamExt};
use serde_json::json;
use uuid::Uuid;

use crate::{Ctx, JobError};

pub fn media_id(job: &Job) -> Option<Uuid> {
    job.payload.get("media_id")?.as_str()?.parse().ok()
}

pub async fn run(ctx: &Ctx, job: &Job) -> Result<(), JobError> {
    let media_id =
        media_id(job).ok_or_else(|| JobError::Permanent("job has no media_id".into()))?;
    let Some(item) = media::get_by_id(&ctx.db, media_id).await? else {
        tracing::info!(%media_id, "media was deleted before processing; nothing to do");
        return Ok(());
    };
    if item.status != "processing" {
        tracing::info!(%media_id, status = %item.status, "media isn't awaiting processing; skipping");
        return Ok(());
    }

    let tmp = tempfile::TempDir::new_in(&ctx.tmp_root)?;
    let input = tmp.path().join("original");
    let bytes = ctx
        .storage
        .download_to_file(&original_key(media_id), &input)
        .await
        .context("downloading original")?;
    tracing::debug!(%media_id, bytes, "downloaded original");

    let probe = match ::media::probe::run(&input).await {
        Ok(probe) => probe,
        Err(::media::probe::ProbeError::NotAVideo) => {
            return Err(JobError::Permanent(
                "We couldn't find a video in this file.".into(),
            ));
        }
        Err(err) => return Err(JobError::Retryable(err.into())),
    };

    // Monthly processing budget (weighted minutes, PLAN.md §4).
    let ladder = ::media::plan::ladder(&probe);
    let cost = ::media::plan::weighted_seconds(&probe, &ladder);
    let usage = media::usage(&ctx.db, item.owner_id).await?;
    if usage.processing_seconds_used + cost > i64::from(usage.processing_minutes_limit) * 60 {
        return Err(JobError::Permanent(
            "This month's video processing allowance is used up. It resets on the 1st.".into(),
        ));
    }

    // ffmpeg reports progress many times a second; write to the DB at most once a second.
    let (progress_tx, mut progress_rx) = tokio::sync::watch::channel(0.0f32);
    let db = ctx.db.clone();
    let reporter = tokio::spawn(async move {
        let mut last = -1.0f32;
        while progress_rx.changed().await.is_ok() {
            let p = *progress_rx.borrow_and_update();
            if p - last >= 0.01 || p >= 1.0 {
                last = p;
                if let Err(err) = media::set_progress(&db, media_id, p * 0.95).await {
                    tracing::warn!(error = %err, "could not record progress");
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let out_dir = tmp.path().join("out");
    let started = Instant::now();
    let result = ::media::process_video(&input, &probe, &out_dir, |p| {
        let _ = progress_tx.send(p);
    })
    .await;
    drop(progress_tx);
    let _ = reporter.await;
    let out = match result {
        Ok(out) => out,
        Err(err) => {
            return Err(match err.user_message() {
                Some(message) => JobError::Permanent(message.into()),
                None => JobError::Retryable(err.into()),
            });
        }
    };
    tracing::info!(
        %media_id,
        duration_s = probe.duration,
        renditions = out.ladder.len(),
        speed = format!("{:.2}x", probe.duration / started.elapsed().as_secs_f64()),
        "transcoded"
    );

    // Upload everything; segments are independent, so do several at once.
    let prefix = media_prefix(media_id);
    let rels: Vec<String> = out
        .files
        .iter()
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .collect();
    let sizes: Vec<(String, u64)> = futures::stream::iter(rels)
        .map(|rel| {
            // Owned per upload: these futures outlive any borrow of `ctx` the stream could hold.
            let storage = ctx.storage.clone();
            let path = out_dir.join(&rel);
            let key = format!("{prefix}{rel}");
            async move {
                let size = tokio::fs::metadata(&path).await?.len();
                storage
                    .put_file(&key, &path, content_type_for(&rel))
                    .await?;
                Ok::<_, anyhow::Error>((rel, size))
            }
        })
        .buffer_unordered(8)
        .try_collect()
        .await?;
    media::set_progress(&ctx.db, media_id, 0.99).await?;

    let bytes_under = |dir: &str| {
        sizes
            .iter()
            .filter(|(rel, _)| rel.starts_with(dir))
            .map(|(_, s)| *s as i64)
            .sum::<i64>()
    };
    let (width, height) = probe.display_size();
    let largest_poster = out
        .posters
        .last()
        .map(|p| p.2)
        .unwrap_or("poster/1280.webp");
    let result = ProcessedVideo {
        width: width as i32,
        height: height as i32,
        duration_ms: (probe.duration * 1000.0).round() as i64,
        video_codec: probe.video_codec.clone(),
        audio_codec: probe.audio_codec.clone(),
        hdr: probe.is_hdr(),
        taken_at: probe.creation_time,
        weighted_seconds: out.weighted_seconds,
        assets: vec![
            NewAsset {
                kind: "hls",
                path: "hls/master.m3u8".into(),
                bytes: bytes_under("hls/"),
                meta: json!({
                    "renditions": out.ladder.iter().map(|r| json!({
                        "width": r.width, "height": r.height, "label": format!("{}p", r.short_side),
                    })).collect::<Vec<_>>(),
                }),
            },
            NewAsset {
                kind: "poster",
                path: largest_poster.into(),
                bytes: bytes_under("poster/"),
                meta: json!({
                    "sizes": out.posters.iter().map(|(w, h, path)| json!({ "width": w, "height": h, "path": path })).collect::<Vec<_>>(),
                }),
            },
            NewAsset {
                kind: "sprites",
                path: "sprites/sprites.vtt".into(),
                bytes: bytes_under("sprites/"),
                meta: json!({
                    "interval": out.sprites.interval,
                    "tile_width": out.sprites.tile_width,
                    "tile_height": out.sprites.tile_height,
                    "sheets": out.sprite_sheets,
                }),
            },
        ],
    };
    media::mark_ready(&ctx.db, media_id, &result).await?;
    Ok(())
}
