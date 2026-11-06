//! End-to-end processing of synthetic clips with the real ffmpeg (must be on PATH).

use std::path::Path;

use media::{PipelineError, probe, process_video};

/// Generates a test clip with ffmpeg's built-in sources.
async fn make_clip(dir: &Path, name: &str, args: &[&str]) -> std::path::PathBuf {
    let path = dir.join(name);
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(args)
        .arg(&path)
        .status()
        .await
        .expect("ffmpeg is installed");
    assert!(status.success(), "generating {name}");
    path
}

#[tokio::test]
async fn landscape_clip_with_audio() {
    let tmp = tempfile::tempdir().unwrap();
    let input = make_clip(
        tmp.path(),
        "in.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata",
            "creation_time=2024-07-04T15:30:00Z",
        ],
    )
    .await;
    let probe = probe::run(&input).await.unwrap();
    assert_eq!(
        probe.creation_time.unwrap().to_rfc3339(),
        "2024-07-04T15:30:00+00:00"
    );

    let out_dir = tmp.path().join("out");
    let mut progress = Vec::new();
    let out = process_video(&input, &probe, &out_dir, |p| progress.push(p))
        .await
        .unwrap();

    assert_eq!(
        out.ladder.iter().map(|r| r.short_side).collect::<Vec<_>>(),
        vec![720, 480, 360]
    );
    assert_eq!(out.weighted_seconds, 20);
    assert!(
        progress.windows(2).all(|w| w[1] >= w[0]),
        "progress never goes backwards"
    );
    // Regression: progress must arrive *during* the transcode, not only at the end.
    let midway = progress.iter().filter(|p| **p > 0.1 && **p < 0.9).count();
    assert!(
        midway >= 2,
        "only {midway} progress reports mid-transcode: {progress:?}"
    );
    assert_eq!(progress.last(), Some(&1.0));

    let master = std::fs::read_to_string(out_dir.join("hls/master.m3u8")).unwrap();
    assert_eq!(master.matches("#EXT-X-STREAM-INF").count(), 3);
    assert!(
        master.contains("RESOLUTION=1280x720") && master.contains("mp4a.40.2"),
        "{master}"
    );
    for v in 0..3 {
        assert!(out_dir.join(format!("hls/v{v}/index.m3u8")).exists());
        assert!(out_dir.join(format!("hls/v{v}/init_{v}.mp4")).exists());
    }
    assert_eq!(
        out.posters.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>(),
        vec![(320, 180), (640, 360), (1280, 720)]
    );
    assert_eq!(out.sprite_sheets, 1);
    let vtt = std::fs::read_to_string(out_dir.join("sprites/sprites.vtt")).unwrap();
    assert_eq!(vtt.matches("-->").count(), 20);
    assert!(
        !out.files
            .iter()
            .any(|f| f.to_string_lossy().contains("thumbs")),
        "intermediate thumbnails are cleaned up"
    );
    assert!(
        out.files
            .iter()
            .any(|f| f.ends_with("sprites/sheet_001.webp"))
    );
}

#[tokio::test]
async fn rotated_hdr_hevc_without_audio_becomes_portrait_sdr() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = make_clip(
        tmp.path(),
        "raw.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1920x1080:rate=30",
            "-t",
            "3",
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p10le",
            "-x265-params",
            "log-level=error:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            "-tag:v",
            "hvc1",
        ],
    )
    .await;
    // Stream-copy with a display matrix, like a phone held upright.
    let input = make_clip(
        tmp.path(),
        "rotated.mov",
        &[
            "-display_rotation",
            "90",
            "-i",
            raw.to_str().unwrap(),
            "-c",
            "copy",
        ],
    )
    .await;
    let probe = probe::run(&input).await.unwrap();
    assert!(probe.is_hdr());
    assert_eq!(probe.display_size(), (1080, 1920));
    assert_eq!(probe.audio_codec, None);

    let out_dir = tmp.path().join("out");
    let out = process_video(&input, &probe, &out_dir, |_| {})
        .await
        .unwrap();
    let master = std::fs::read_to_string(out_dir.join("hls/master.m3u8")).unwrap();
    assert!(master.contains("RESOLUTION=1080x1920"), "{master}");
    assert!(!master.contains("mp4a"));
    assert_eq!(
        out.posters.last().map(|p| (p.0, p.1)),
        Some((720, 1280)),
        "portrait poster sized by height"
    );

    let transfer = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=color_transfer",
            "-of",
            "csv=p=0",
        ])
        .arg(out_dir.join("hls/v0/init_0.mp4"))
        .output()
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&transfer.stdout).trim(),
        "bt709",
        "tonemapped to SDR"
    );
}

#[tokio::test]
async fn garbage_is_rejected_with_a_user_facing_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("fake.mp4");
    std::fs::write(&path, b"definitely not a video").unwrap();
    assert!(matches!(
        probe::run(&path).await,
        Err(probe::ProbeError::NotAVideo)
    ));

    // Probe succeeds on a truncated file, but decoding fails: permanent, with a message.
    let input = make_clip(
        tmp.path(),
        "ok.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ],
    )
    .await;
    let bytes = std::fs::read(&input).unwrap();
    let truncated = tmp.path().join("truncated.mp4");
    std::fs::write(&truncated, &bytes[..bytes.len() / 3]).unwrap();
    if let Ok(probe) = probe::run(&truncated).await {
        match process_video(&truncated, &probe, &tmp.path().join("out"), |_| {}).await {
            Err(err @ PipelineError::Undecodable(_)) => assert!(err.user_message().is_some()),
            // ffmpeg may also just stop early and produce a short, valid video. Both are acceptable;
            // what must never happen is a retryable error for a damaged file.
            Ok(_) => {}
            Err(other) => panic!("expected a permanent failure, got {other:?}"),
        }
    }
}
