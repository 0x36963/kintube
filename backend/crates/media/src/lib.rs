//! Video processing with ffmpeg: probing, the HLS ladder, posters, and scrub sprites.
//!
//! Pure planning logic (`plan`, `sprites`, `probe::parse`) is unit-tested; the runners
//! shell out to `ffprobe`/`ffmpeg`, which must be on `PATH`.

pub mod ffmpeg;
pub mod pipeline;
pub mod plan;
pub mod probe;
pub mod sprites;

pub use pipeline::{Output, PipelineError, process_video};
pub use probe::Probe;
