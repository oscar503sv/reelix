//! Media sources. Phase 1 only has the screen; microphone and camera land
//! in phases 2 and 3.

pub mod portal;

pub use portal::{CaptureRequest, ScreenCastSession, SourceKind};
