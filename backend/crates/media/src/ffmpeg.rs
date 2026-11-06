//! Building and running ffmpeg commands.

use std::{collections::VecDeque, path::Path, process::Stdio, time::Duration};

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    plan::{POSTER_SIZES, Rendition, SpritePlan},
    probe::Probe,
};

#[derive(Debug, thiserror::Error)]
pub enum FfmpegError {
    #[error("ffmpeg exited with {status}: {stderr}")]
    Failed { status: String, stderr: String },
    #[error("ffmpeg timed out after {0:?}")]
    TimedOut(Duration),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// HDR (PQ/HLG) → SDR BT.709 for universal playback. Rotation is applied by ffmpeg's
/// autorotate before the filter graph.
const TONEMAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
                       tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

fn normalize(probe: &Probe) -> &'static str {
    if probe.is_hdr() {
        TONEMAP
    } else {
        "format=yuv420p"
    }
}

/// One pass that decodes the source once and writes the whole HLS ladder plus the scrub
/// sprite sheets. Run with the working directory set to the `hls/` output folder; writes
/// `master.m3u8`, `v{n}/index.m3u8`, `v{n}/init_{n}.mp4`, `v{n}/seg_NNNN.m4s` and
/// `../sprites/sheet_NNN.webp`.
pub fn hls_and_sprites_args(
    input: &Path,
    probe: &Probe,
    ladder: &[Rendition],
    sprites: &SpritePlan,
) -> Vec<String> {
    let n = ladder.len();
    let mut graph = format!("[0:v]{},split={}", normalize(probe), n + 1);
    for i in 0..n {
        graph.push_str(&format!("[s{i}]"));
    }
    graph.push_str("[sp]");
    for (i, r) in ladder.iter().enumerate() {
        graph.push_str(&format!(";[s{i}]scale={}:{}[v{i}]", r.width, r.height));
    }
    // One image per thumbnail, tiled into sheets afterwards by `tile_sprites_args`. Tiling
    // here would make this output silent until a whole sheet fills, and ffmpeg's progress
    // report doesn't advance until every output has written something.
    graph.push_str(&format!(
        ";[sp]fps=1/{},scale={}:{}[spr]",
        sprites.interval, sprites.tile_width, sprites.tile_height
    ));

    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-v", "error", "-i"]
        .into_iter()
        .map(String::from)
        .collect();
    args.push(input.display().to_string());
    args.extend(["-filter_complex".into(), graph]);
    for i in 0..n {
        args.extend(["-map".into(), format!("[v{i}]")]);
    }
    let has_audio = probe.audio_codec.is_some();
    if has_audio {
        for _ in 0..n {
            args.extend(["-map".into(), "0:a:0".into()]);
        }
    }
    args.extend(["-c:v", "libx264", "-profile:v", "high", "-crf", "21"].map(String::from));
    for (i, r) in ladder.iter().enumerate() {
        args.extend([
            format!("-preset:v:{i}"),
            r.preset.into(),
            format!("-maxrate:v:{i}"),
            format!("{}k", r.maxrate_kbps),
            format!("-bufsize:v:{i}"),
            format!("{}k", r.maxrate_kbps * 2),
        ]);
    }
    if probe.fps.is_some_and(|f| f > 60.5) {
        args.extend(["-r".into(), "60".into()]);
    }
    if has_audio {
        args.extend(["-c:a", "aac", "-b:a", "128k", "-ac", "2"].map(String::from));
    }
    let stream_map = (0..n)
        .map(|i| {
            if has_audio {
                format!("v:{i},a:{i}")
            } else {
                format!("v:{i}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    args.extend(
        [
            // Keyframe every 2 s on every rung, so segments align and quality switches are clean.
            "-force_key_frames",
            "expr:gte(t,n_forced*2)",
            "-sc_threshold",
            "0",
            "-f",
            "hls",
            "-hls_time",
            "6",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_type",
            "fmp4",
            "-hls_flags",
            "independent_segments",
            "-hls_segment_filename",
            "v%v/seg_%04d.m4s",
            "-hls_fmp4_init_filename",
            "init.mp4",
            "-master_pl_name",
            "master.m3u8",
            "-var_stream_map",
        ]
        .map(String::from),
    );
    args.push(stream_map);
    args.push("v%v/index.m3u8".into());
    // Second output: scrub thumbnails from the same decode (lossless, tiled next).
    args.extend(
        [
            "-map",
            "[spr]",
            "-c:v",
            "png",
            "-f",
            "image2",
            "../sprites/thumbs/%04d.png",
        ]
        .map(String::from),
    );
    args
}

/// Tiles the individual thumbnails into WebP sprite sheets. Cheap: at most ~200 tiny
/// images. Run in the `sprites/` folder; writes `sheet_NNN.webp`.
pub fn tile_sprites_args(sprites: &SpritePlan) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-v",
        "error",
        "-framerate",
        "1",
        "-i",
        "thumbs/%04d.png",
        "-vf",
    ]
    .map(String::from)
    .to_vec();
    args.push(format!("tile={}x{}", sprites.columns, sprites.rows));
    args.extend(
        [
            "-c:v",
            "libwebp",
            "-quality",
            "70",
            "-f",
            "image2",
            "sheet_%03d.webp",
        ]
        .map(String::from),
    );
    args
}

/// Poster frame in three widths. The `thumbnail` filter picks the most representative
/// of the next 30 frames, which avoids blurry or black frames. Run in the `poster/` folder.
pub fn poster_args(input: &Path, probe: &Probe, at: f64) -> Vec<String> {
    let [small, medium, large] = POSTER_SIZES;
    let (w, h) = probe.display_size();
    // Size the long side for portrait video, so posters are never huge.
    let fit = |px: u32| {
        if w >= h {
            format!("{px}:-2")
        } else {
            format!("-2:{px}")
        }
    };
    let large_fit = if w >= h {
        format!("'min({large},iw)':-2")
    } else {
        format!("-2:'min({large},ih)'")
    };
    let graph = format!(
        "[0:v]thumbnail=n=30,{},scale={large_fit},split=3[l][m][s];[m]scale={}[m2];[s]scale={}[s2]",
        normalize(probe),
        fit(medium),
        fit(small)
    );
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-v", "error", "-ss"]
        .map(String::from)
        .to_vec();
    args.push(format!("{at:.3}"));
    args.push("-i".into());
    args.push(input.display().to_string());
    args.extend(["-filter_complex".into(), graph]);
    for (label, size) in [("[l]", large), ("[m2]", medium), ("[s2]", small)] {
        args.extend(
            [
                "-map",
                label,
                "-frames:v",
                "1",
                "-c:v",
                "libwebp",
                "-quality",
                "80",
            ]
            .map(String::from),
        );
        args.push(format!("{size}.webp"));
    }
    args
}

/// Runs ffmpeg in `cwd`, reporting progress (0.0–1.0 of `duration` seconds). Kills ffmpeg
/// on timeout or if the future is dropped.
pub async fn run(
    args: &[String],
    cwd: &Path,
    duration: f64,
    timeout: Duration,
    mut on_progress: impl FnMut(f32),
) -> Result<(), FfmpegError> {
    let mut child = tokio::process::Command::new("ffmpeg")
        .args(args)
        .args(["-progress", "pipe:1", "-stats_period", "0.25", "-nostats"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let stderr_task = tokio::spawn(async move {
        // Keep only the tail: errors are at the end, and inputs can be very chatty.
        let mut tail = VecDeque::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tail.len() == 30 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let work = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(us) = line
                .strip_prefix("out_time_us=")
                .and_then(|v| v.parse::<f64>().ok())
                && duration > 0.0
            {
                on_progress(((us / 1e6) / duration).clamp(0.0, 1.0) as f32);
            }
        }
        child.wait().await
    };
    let status = match tokio::time::timeout(timeout, work).await {
        Ok(status) => status?,
        Err(_) => return Err(FfmpegError::TimedOut(timeout)),
    };
    let stderr = stderr_task.await.unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(FfmpegError::Failed {
            status: status.to_string(),
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ladder, sprite_plan, tests::probe};

    #[test]
    fn hls_args_map_audio_per_rendition_and_set_per_rung_rates() {
        let p = probe(1920, 1080, 0, 30.0, 8.0);
        let args =
            hls_and_sprites_args(Path::new("/in/original"), &p, &ladder(&p), &sprite_plan(&p));
        let joined = args.join(" ");
        assert!(joined.contains("split=5[s0][s1][s2][s3][sp]"), "{joined}");
        assert!(joined.contains("[s0]scale=1920:1080[v0]"));
        assert!(joined.contains("[sp]fps=1/1,scale=160:90[spr]"));
        assert!(joined.ends_with("-map [spr] -c:v png -f image2 ../sprites/thumbs/%04d.png"));
        assert!(
            tile_sprites_args(&sprite_plan(&p))
                .join(" ")
                .contains("-i thumbs/%04d.png -vf tile=10x10")
        );
        assert_eq!(args.iter().filter(|a| *a == "0:a:0").count(), 4);
        assert!(joined.contains("-maxrate:v:3 700k -bufsize:v:3 1400k"));
        assert!(joined.contains("-var_stream_map v:0,a:0 v:1,a:1 v:2,a:2 v:3,a:3 v%v/index.m3u8"));
        assert!(!joined.contains("zscale"), "SDR input isn't tonemapped");
    }

    #[test]
    fn silent_hdr_video_is_tonemapped_and_maps_no_audio() {
        let mut p = probe(3840, 2160, 90, 30.0, 4.0);
        p.audio_codec = None;
        p.color_transfer = Some("smpte2084".into());
        let joined =
            hls_and_sprites_args(Path::new("in"), &p, &ladder(&p), &sprite_plan(&p)).join(" ");
        assert!(joined.contains("tonemap=tonemap=hable"));
        assert!(!joined.contains("0:a:0") && !joined.contains("-c:a"));
        assert!(joined.contains("-var_stream_map v:0 v:1 v:2 v:3 v:4 v:5 "));
        assert!(
            joined.contains("[s0]scale=2160:3840[v0]"),
            "portrait after rotation"
        );
    }

    #[test]
    fn poster_sizes_by_long_side_for_portrait() {
        let joined =
            poster_args(Path::new("in"), &probe(1920, 1080, 90, 30.0, 10.0), 2.0).join(" ");
        assert!(joined.contains("scale=-2:'min(1280,ih)'"), "{joined}");
        assert!(joined.contains("[m]scale=-2:640[m2];[s]scale=-2:320[s2]"));
        assert!(joined.contains("-ss 2.000 -i in"));
    }
}
