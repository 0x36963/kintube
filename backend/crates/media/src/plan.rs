//! What to produce for a given source: the HLS ladder, processing weight, sprite and
//! poster parameters. Pure functions, so the decisions are easy to test.

use crate::probe::Probe;

/// One HLS rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendition {
    /// Nominal size ("1080p"), measured on the short side so vertical video works.
    pub short_side: u32,
    pub width: u32,
    pub height: u32,
    /// x264 VBV cap; quality is CRF-driven below it.
    pub maxrate_kbps: u32,
    pub preset: &'static str,
}

/// (short side, max bitrate at ≤30 fps, x264 preset). Bigger rungs use a faster preset:
/// they dominate encode time (PLAN.md §3.2).
const RUNGS: [(u32, u32, &str); 6] = [
    (2160, 16_000, "veryfast"),
    (1440, 9_000, "veryfast"),
    (1080, 5_000, "faster"),
    (720, 2_800, "faster"),
    (480, 1_200, "faster"),
    (360, 700, "faster"),
];

/// Sources within this fraction of a rung still get it (e.g. 1920×1072 → 1080p).
const RUNG_TOLERANCE: f64 = 0.95;

pub fn ladder(probe: &Probe) -> Vec<Rendition> {
    let (w, h) = probe.display_size();
    let short = w.min(h);
    let high_fps = probe.fps.is_some_and(|f| f > 31.0);
    let mut rungs: Vec<Rendition> = RUNGS
        .iter()
        .filter(|(rung, _, _)| f64::from(short) >= f64::from(*rung) * RUNG_TOLERANCE)
        .map(|&(rung, kbps, preset)| {
            let (width, height) = scaled(w, h, rung);
            Rendition {
                short_side: rung,
                width,
                height,
                maxrate_kbps: if high_fps { kbps * 3 / 2 } else { kbps },
                preset,
            }
        })
        .collect();
    if rungs.is_empty() {
        // Smaller than the smallest rung: one rendition at the source size.
        let (width, height) = (even(w), even(h));
        rungs.push(Rendition {
            short_side: short,
            width,
            height,
            maxrate_kbps: 700,
            preset: "faster",
        });
    }
    rungs
}

/// Scale so the short side equals `short`, keeping aspect ratio, with even dimensions.
fn scaled(w: u32, h: u32, short: u32) -> (u32, u32) {
    if w >= h {
        (
            nearest_even(f64::from(w) * f64::from(short) / f64::from(h)),
            short,
        )
    } else {
        (
            short,
            nearest_even(f64::from(h) * f64::from(short) / f64::from(w)),
        )
    }
}

/// Encoders need even dimensions. Scaled sizes round to the nearest even number
/// (16:9 at 480 → 853.3 → 854, the standard width).
fn nearest_even(x: f64) -> u32 {
    ((x / 2.0).round() as u32).max(1) * 2
}

/// Source-size renditions round down, so we never upscale.
fn even(x: u32) -> u32 {
    (x.max(2) / 2) * 2
}

/// Multiplier for the monthly processing budget, by the largest rendition produced.
pub fn processing_weight(ladder: &[Rendition]) -> u32 {
    match ladder.iter().map(|r| r.short_side).max().unwrap_or(0) {
        s if s >= 2160 => 4,
        s if s >= 1440 => 2,
        _ => 1,
    }
}

/// Seconds charged to the processing budget.
pub fn weighted_seconds(probe: &Probe, ladder: &[Rendition]) -> i64 {
    (probe.duration.ceil() as i64) * i64::from(processing_weight(ladder))
}

/// Scrub-preview sprite sheets.
#[derive(Debug, Clone, PartialEq)]
pub struct SpritePlan {
    /// Seconds between thumbnails.
    pub interval: f64,
    pub tile_width: u32,
    pub tile_height: u32,
    pub columns: u32,
    pub rows: u32,
    /// Total thumbnails across all sheets.
    pub count: u32,
}

/// At most ~200 thumbnails, at least one per second, 160 px on the long side.
pub fn sprite_plan(probe: &Probe) -> SpritePlan {
    let interval = (probe.duration / 200.0).ceil().max(1.0);
    let (w, h) = probe.display_size();
    let (tile_width, tile_height) = if w >= h {
        (160, nearest_even(160.0 * f64::from(h) / f64::from(w)))
    } else {
        (nearest_even(160.0 * f64::from(w) / f64::from(h)), 160)
    };
    SpritePlan {
        interval,
        tile_width,
        tile_height,
        columns: 10,
        rows: 10,
        count: (probe.duration / interval).ceil().max(1.0) as u32,
    }
}

impl SpritePlan {
    pub fn per_sheet(&self) -> u32 {
        self.columns * self.rows
    }

    pub fn sheets(&self) -> u32 {
        self.count.div_ceil(self.per_sheet())
    }
}

/// Where to look for a poster frame: a fifth of the way in (openings are often shaky),
/// at most a minute.
pub fn poster_time(probe: &Probe) -> f64 {
    if probe.duration < 2.0 {
        0.0
    } else {
        (probe.duration * 0.2).min(60.0)
    }
}

/// Poster widths produced (for the long side of portrait video, the height).
pub const POSTER_SIZES: [u32; 3] = [320, 640, 1280];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn probe(w: u32, h: u32, rotation: u32, fps: f64, duration: f64) -> Probe {
        Probe {
            duration,
            coded_width: w,
            coded_height: h,
            rotation,
            video_codec: "h264".into(),
            pix_fmt: Some("yuv420p".into()),
            color_transfer: None,
            fps: Some(fps),
            audio_codec: Some("aac".into()),
            creation_time: None,
            format_name: "mov".into(),
        }
    }

    fn sizes(l: &[Rendition]) -> Vec<(u32, u32)> {
        l.iter().map(|r| (r.width, r.height)).collect()
    }

    #[test]
    fn landscape_4k_gets_every_rung() {
        let l = ladder(&probe(3840, 2160, 0, 30.0, 10.0));
        assert_eq!(
            sizes(&l),
            vec![
                (3840, 2160),
                (2560, 1440),
                (1920, 1080),
                (1280, 720),
                (854, 480),
                (640, 360)
            ]
        );
        assert_eq!(processing_weight(&l), 4);
    }

    #[test]
    fn rotated_portrait_video_is_laddered_on_its_short_side() {
        let l = ladder(&probe(1920, 1080, 90, 30.0, 10.0));
        assert_eq!(
            sizes(&l),
            vec![(1080, 1920), (720, 1280), (480, 854), (360, 640)]
        );
        assert_eq!(processing_weight(&l), 1);
    }

    #[test]
    fn near_miss_sizes_keep_their_rung_and_odd_sizes_become_even() {
        let l = ladder(&probe(1920, 1072, 0, 30.0, 10.0));
        assert_eq!(l[0].short_side, 1080);
        assert_eq!(
            sizes(&ladder(&probe(2560, 1440, 0, 30.0, 1.0)))[0],
            (2560, 1440)
        );
        assert_eq!(
            processing_weight(&ladder(&probe(2560, 1440, 0, 30.0, 1.0))),
            2
        );
    }

    #[test]
    fn tiny_sources_get_a_single_rendition_at_source_size() {
        let l = ladder(&probe(321, 241, 0, 15.0, 10.0));
        assert_eq!(sizes(&l), vec![(320, 240)]);
    }

    #[test]
    fn high_frame_rate_gets_more_bits() {
        let l30 = ladder(&probe(1920, 1080, 0, 30.0, 1.0));
        let l60 = ladder(&probe(1920, 1080, 0, 59.94, 1.0));
        assert_eq!(l60[0].maxrate_kbps, l30[0].maxrate_kbps * 3 / 2);
    }

    #[test]
    fn weighted_seconds_round_up_and_scale() {
        let p = probe(3840, 2160, 0, 30.0, 59.2);
        assert_eq!(weighted_seconds(&p, &ladder(&p)), 60 * 4);
    }

    #[test]
    fn sprites_cap_at_about_200_thumbnails() {
        let short = sprite_plan(&probe(1920, 1080, 0, 30.0, 8.0));
        assert_eq!((short.interval, short.count, short.sheets()), (1.0, 8, 1));
        assert_eq!((short.tile_width, short.tile_height), (160, 90));

        let hour = sprite_plan(&probe(1920, 1080, 0, 30.0, 3600.0));
        assert_eq!((hour.interval, hour.count, hour.sheets()), (18.0, 200, 2));

        let portrait = sprite_plan(&probe(1920, 1080, 270, 30.0, 8.0));
        assert_eq!((portrait.tile_width, portrait.tile_height), (90, 160));
    }

    #[test]
    fn poster_avoids_the_shaky_start() {
        assert_eq!(poster_time(&probe(1, 1, 0, 30.0, 1.0)), 0.0);
        assert_eq!(poster_time(&probe(1, 1, 0, 30.0, 10.0)), 2.0);
        assert_eq!(poster_time(&probe(1, 1, 0, 30.0, 3600.0)), 60.0);
    }
}
