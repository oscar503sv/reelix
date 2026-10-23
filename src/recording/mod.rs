pub mod audio;
pub mod camera;
pub mod encoder;
pub mod pipeline;
pub mod props;
pub mod remux;
pub mod stats;

pub use encoder::EncoderKind;
pub use pipeline::{AudioConfig, PipelineConfig, PipelineEvent, RecordingPipeline, VideoInput};

/// Initializes GStreamer. Idempotent and cheap after the first call; every
/// entry point that touches GStreamer goes through here.
pub fn init() -> crate::error::Result<()> {
    gst::init().map_err(|e| crate::error::AppError::GstInit(e.to_string()))
}
