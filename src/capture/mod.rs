//! Media sources: screen and camera (portals + PipeWire) and audio
//! (PipeWire via the PulseAudio protocol).

pub mod audio;
pub mod camera;
pub mod portal;

pub use portal::{CaptureRequest, ScreenCastSession, SourceKind};
