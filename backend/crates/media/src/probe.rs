//! `ffprobe` metadata, normalized.

use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// Seconds.
    pub duration: f64,
    /// Encoded frame size, before rotation.
    pub coded_width: u32,
    pub coded_height: u32,
    /// Display rotation in degrees: 0, 90, 180 or 270.
    pub rotation: u32,
    pub video_codec: String,
    pub pix_fmt: Option<String>,
    pub color_transfer: Option<String>,
    pub fps: Option<f64>,
    pub audio_codec: Option<String>,
    /// When the video was recorded, from container metadata, if plausible.
    pub creation_time: Option<DateTime<Utc>>,
    pub format_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("no playable video stream")]
    NotAVideo,
    #[error("ffprobe failed: {0}")]
    Failed(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Probe {
    /// Size as displayed, after applying rotation.
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.coded_height, self.coded_width)
        } else {
            (self.coded_width, self.coded_height)
        }
    }

    pub fn short_side(&self) -> u32 {
        let (w, h) = self.display_size();
        w.min(h)
    }

    /// PQ (HDR10, Dolby Vision profile 8.4 base) or HLG, e.g. iPhone HDR video.
    pub fn is_hdr(&self) -> bool {
        matches!(
            self.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        )
    }
}

pub async fn run(path: &Path) -> Result<Probe, ProbeError> {
    let out = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::info!(stderr = %stderr.trim(), "ffprobe could not read file");
        return Err(ProbeError::NotAVideo);
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    streams: Vec<RawStream>,
    format: Option<RawFormat>,
}

#[derive(Deserialize)]
struct RawStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    color_transfer: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    tags: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    disposition: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    #[serde(default)]
    tags: serde_json::Map<String, serde_json::Value>,
}

pub fn parse(json: &str) -> Result<Probe, ProbeError> {
    let raw: Raw = serde_json::from_str(json).map_err(|e| ProbeError::Failed(e.to_string()))?;
    let is_cover_art =
        |s: &RawStream| s.disposition.get("attached_pic").and_then(|v| v.as_i64()) == Some(1);
    let video = raw
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video") && !is_cover_art(s))
        .ok_or(ProbeError::NotAVideo)?;
    let (Some(coded_width), Some(coded_height)) = (video.width, video.height) else {
        return Err(ProbeError::NotAVideo);
    };
    if coded_width == 0 || coded_height == 0 {
        return Err(ProbeError::NotAVideo);
    }
    let audio = raw
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"));
    let format = raw.format.as_ref();

    let duration = format
        .and_then(|f| f.duration.as_deref())
        .or(video.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or(ProbeError::NotAVideo)?;

    let rotation = video
        .side_data_list
        .iter()
        .find_map(|sd| sd.get("rotation").and_then(|r| r.as_f64()))
        .or_else(|| {
            video
                .tags
                .get("rotate")
                .and_then(|r| r.as_str())
                .and_then(|r| r.parse().ok())
        })
        .map(|r| ((r.round() as i64).rem_euclid(360)) as u32)
        .unwrap_or(0);

    let fps = [&video.avg_frame_rate, &video.r_frame_rate]
        .into_iter()
        .filter_map(|r| r.as_deref().and_then(parse_rate))
        .next();

    // Apple's tag keeps the local time zone; the generic one is UTC.
    let tag = |key: &str| {
        format
            .and_then(|f| f.tags.get(key))
            .or_else(|| video.tags.get(key))
            .and_then(|v| v.as_str())
    };
    let creation_time = tag("com.apple.quicktime.creationdate")
        .and_then(parse_time)
        .or_else(|| tag("creation_time").and_then(parse_time))
        .filter(|t| *t > plausible_after());

    Ok(Probe {
        duration,
        coded_width,
        coded_height,
        rotation,
        video_codec: video.codec_name.clone().unwrap_or_else(|| "unknown".into()),
        pix_fmt: video.pix_fmt.clone(),
        color_transfer: video.color_transfer.clone(),
        fps,
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
        creation_time,
        format_name: format
            .and_then(|f| f.format_name.clone())
            .unwrap_or_default(),
    })
}

fn parse_rate(rate: &str) -> Option<f64> {
    let (n, d) = rate.split_once('/')?;
    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z"))
        .map(|t| t.with_timezone(&Utc))
        .ok()
}

/// Cameras with unset clocks write 1970/1904 dates; ignore anything this early.
fn plausible_after() -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(1990, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
}

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE_HDR_PORTRAIT: &str = r#"{
      "streams": [
        {"codec_type": "video", "codec_name": "hevc", "width": 3840, "height": 2160, "pix_fmt": "yuv420p10le",
         "color_transfer": "arib-std-b67", "avg_frame_rate": "30000/1001", "r_frame_rate": "30/1",
         "side_data_list": [{"side_data_type": "Display Matrix", "rotation": -90}]},
        {"codec_type": "audio", "codec_name": "aac"},
        {"codec_type": "data", "codec_name": "none"}
      ],
      "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "12.345",
                 "tags": {"creation_time": "2024-07-04T13:30:00.000000Z",
                          "com.apple.quicktime.creationdate": "2024-07-04T15:30:00+0200"}}
    }"#;

    #[test]
    fn parses_an_iphone_hdr_portrait_clip() {
        let p = parse(IPHONE_HDR_PORTRAIT).unwrap();
        assert_eq!(p.rotation, 270);
        assert_eq!(p.display_size(), (2160, 3840));
        assert_eq!(p.short_side(), 2160);
        assert!(p.is_hdr());
        assert_eq!(p.video_codec, "hevc");
        assert_eq!(p.audio_codec.as_deref(), Some("aac"));
        assert!((p.fps.unwrap() - 29.97).abs() < 0.01);
        assert!((p.duration - 12.345).abs() < 1e-9);
        assert_eq!(
            p.creation_time.unwrap().to_rfc3339(),
            "2024-07-04T13:30:00+00:00"
        );
    }

    #[test]
    fn handles_legacy_rotate_tags_cover_art_and_bogus_dates() {
        let json = r#"{
          "streams": [
            {"codec_type": "video", "codec_name": "mjpeg", "width": 600, "height": 600, "disposition": {"attached_pic": 1}},
            {"codec_type": "video", "codec_name": "h264", "width": 1280, "height": 720, "avg_frame_rate": "0/0",
             "r_frame_rate": "25/1", "duration": "3.0", "tags": {"rotate": "180"}}
          ],
          "format": {"format_name": "mov", "tags": {"creation_time": "1970-01-01T00:00:00.000000Z"}}
        }"#;
        let p = parse(json).unwrap();
        assert_eq!(p.video_codec, "h264", "skips the cover-art stream");
        assert_eq!((p.rotation, p.display_size()), (180, (1280, 720)));
        assert_eq!(p.fps, Some(25.0));
        assert_eq!(p.duration, 3.0, "falls back to stream duration");
        assert_eq!(p.creation_time, None);
        assert!(!p.is_hdr());
    }

    #[test]
    fn rejects_files_without_video() {
        let audio_only = r#"{"streams": [{"codec_type": "audio", "codec_name": "mp3"}], "format": {"duration": "200"}}"#;
        assert!(matches!(parse(audio_only), Err(ProbeError::NotAVideo)));
        let no_duration = r#"{"streams": [{"codec_type": "video", "codec_name": "h264", "width": 10, "height": 10}], "format": {}}"#;
        assert!(matches!(parse(no_duration), Err(ProbeError::NotAVideo)));
    }
}
