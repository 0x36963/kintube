//! Full processing of one video: poster → HLS ladder + sprites → VTT.
//!
//! Output layout under `out_dir` (mirrors object storage under `m/{media_id}/`):
//! `poster/{320,640,1280}.webp`, `hls/master.m3u8`, `hls/v{n}/…`,
//! `sprites/sheet_NNN.webp`, `sprites/sprites.vtt`.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    ffmpeg::{self, FfmpegError},
    plan::{self, POSTER_SIZES, Rendition, SpritePlan},
    probe::Probe,
    sprites,
};

/// Longest video we accept (PLAN.md §4).
pub const MAX_DURATION_SECS: f64 = 4.0 * 3600.0;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("video is longer than 4 hours")]
    TooLong,
    #[error("ffmpeg could not decode the video: {0}")]
    Undecodable(String),
    #[error(transparent)]
    Ffmpeg(#[from] FfmpegError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl PipelineError {
    /// Message for the owner when processing can't succeed, or `None` if it might on retry.
    pub fn user_message(&self) -> Option<&'static str> {
        match self {
            Self::TooLong => Some("Videos can be at most 4 hours long."),
            Self::Undecodable(_) => Some(
                "We couldn't decode this video. The file may be damaged or use an unsupported format.",
            ),
            Self::Ffmpeg(_) | Self::Io(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub ladder: Vec<Rendition>,
    pub sprites: SpritePlan,
    pub sprite_sheets: u32,
    /// (width, height) of each poster file, smallest first.
    pub posters: Vec<(u32, u32, &'static str)>,
    pub weighted_seconds: i64,
    /// Every file written, relative to `out_dir`.
    pub files: Vec<PathBuf>,
}

/// Progress split between stages (the ladder dominates).
const POSTER_SHARE: f32 = 0.03;

pub async fn process_video(
    input: &Path,
    probe: &Probe,
    out_dir: &Path,
    mut on_progress: impl FnMut(f32),
) -> Result<Output, PipelineError> {
    if probe.duration > MAX_DURATION_SECS {
        return Err(PipelineError::TooLong);
    }
    let ladder = plan::ladder(probe);
    let sprite_plan = plan::sprite_plan(probe);
    // Generous: 4K at "veryfast" on a modest box runs well under 5× real time.
    let timeout = Duration::from_secs_f64((probe.duration * 10.0).max(600.0));

    for dir in ["poster", "hls", "sprites/thumbs"] {
        tokio::fs::create_dir_all(out_dir.join(dir)).await?;
    }

    on_progress(0.0);
    let poster = ffmpeg::poster_args(input, probe, plan::poster_time(probe));
    ffmpeg::run(
        &poster,
        &out_dir.join("poster"),
        1.0,
        Duration::from_secs(300),
        |_| {},
    )
    .await
    .map_err(classify)?;
    on_progress(POSTER_SHARE);

    let args = ffmpeg::hls_and_sprites_args(input, probe, &ladder, &sprite_plan);
    ffmpeg::run(&args, &out_dir.join("hls"), probe.duration, timeout, |p| {
        on_progress(POSTER_SHARE + p * (1.0 - POSTER_SHARE));
    })
    .await
    .map_err(classify)?;

    let tile = ffmpeg::tile_sprites_args(&sprite_plan);
    ffmpeg::run(
        &tile,
        &out_dir.join("sprites"),
        1.0,
        Duration::from_secs(120),
        |_| {},
    )
    .await
    .map_err(classify)?;
    tokio::fs::remove_dir_all(out_dir.join("sprites/thumbs")).await?;

    let files = list_files(out_dir).await?;
    let sheets = files
        .iter()
        .filter(|f| f.starts_with("sprites") && f.extension().is_some_and(|e| e == "webp"))
        .count() as u32;
    let vtt = sprites::vtt(&sprite_plan, probe.duration, sheets);
    tokio::fs::write(out_dir.join("sprites/sprites.vtt"), vtt).await?;

    let mut posters = Vec::new();
    for size in POSTER_SIZES {
        let name: &'static str = match size {
            320 => "poster/320.webp",
            640 => "poster/640.webp",
            _ => "poster/1280.webp",
        };
        if let Some(dims) = image_size(&out_dir.join(name)).await {
            posters.push((dims.0, dims.1, name));
        }
    }

    on_progress(1.0);
    Ok(Output {
        weighted_seconds: plan::weighted_seconds(probe, &ladder),
        ladder,
        sprites: sprite_plan,
        sprite_sheets: sheets,
        posters,
        files: list_files(out_dir).await?,
    })
}

/// A failing decode means a bad file (permanent); anything else may be transient.
fn classify(err: FfmpegError) -> PipelineError {
    match err {
        FfmpegError::Failed { ref stderr, .. }
            if [
                "Invalid data found",
                "could not find codec",
                "Decoder",
                "moov atom not found",
                "Error while decoding",
            ]
            .iter()
            .any(|needle| stderr.contains(needle)) =>
        {
            PipelineError::Undecodable(stderr.lines().last().unwrap_or_default().to_owned())
        }
        other => PipelineError::Ffmpeg(other),
    }
}

async fn list_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Width and height of an image via ffprobe.
async fn image_size(path: &Path) -> Option<(u32, u32)> {
    let out = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (w, h) = text.trim().split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
