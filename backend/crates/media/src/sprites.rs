//! WebVTT thumbnail track for scrub previews (`sheet_001.webp#xywh=x,y,w,h` cues).

use std::fmt::Write;

use crate::plan::SpritePlan;

pub fn sheet_name(index: u32) -> String {
    format!("sheet_{:03}.webp", index + 1)
}

/// `available_sheets` guards against ffmpeg emitting fewer frames than planned
/// (the `fps` filter rounds): cues never point past the last sheet that exists.
pub fn vtt(plan: &SpritePlan, duration: f64, available_sheets: u32) -> String {
    let count = plan.count.min(available_sheets * plan.per_sheet());
    let mut out = String::from("WEBVTT\n");
    for i in 0..count {
        let start = f64::from(i) * plan.interval;
        let end = (start + plan.interval).min(duration);
        if start >= duration {
            break;
        }
        let sheet = i / plan.per_sheet();
        let cell = i % plan.per_sheet();
        let x = (cell % plan.columns) * plan.tile_width;
        let y = (cell / plan.columns) * plan.tile_height;
        let _ = write!(
            out,
            "\n{} --> {}\n{}#xywh={x},{y},{},{}\n",
            timestamp(start),
            timestamp(end),
            sheet_name(sheet),
            plan.tile_width,
            plan.tile_height
        );
    }
    out
}

fn timestamp(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(count: u32, interval: f64) -> SpritePlan {
        SpritePlan {
            interval,
            tile_width: 160,
            tile_height: 90,
            columns: 10,
            rows: 10,
            count,
        }
    }

    #[test]
    fn cues_walk_the_grid_and_span_sheets() {
        let vtt = vtt(&plan(102, 1.0), 101.5, 2);
        assert!(vtt.starts_with(
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nsheet_001.webp#xywh=0,0,160,90\n"
        ));
        assert!(vtt.contains("00:00:11.000 --> 00:00:12.000\nsheet_001.webp#xywh=160,90,160,90\n"));
        assert!(
            vtt.contains("00:01:39.000 --> 00:01:40.000\nsheet_001.webp#xywh=1440,810,160,90\n")
        );
        // The last cue starts on sheet 2 and ends at the video's end, not a full interval.
        assert!(
            vtt.ends_with("00:01:41.000 --> 00:01:41.500\nsheet_002.webp#xywh=160,0,160,90\n"),
            "{vtt}"
        );
    }

    #[test]
    fn never_references_missing_sheets() {
        let vtt = vtt(&plan(150, 1.0), 150.0, 1);
        assert!(!vtt.contains("sheet_002"));
        assert_eq!(vtt.matches("-->").count(), 100);
    }

    #[test]
    fn long_timestamps() {
        assert_eq!(timestamp(3725.25), "01:02:05.250");
    }
}
