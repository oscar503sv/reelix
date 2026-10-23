//! Media sources: screen (portal + PipeWire) and audio (PipeWire via the
//! PulseAudio protocol). The camera lands in phase 3.

pub mod audio;
pub mod portal;

pub use portal::{CaptureRequest, ScreenCastSession, SourceKind};
