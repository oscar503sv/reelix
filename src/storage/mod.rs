mod metadata;
pub mod naming;
mod recordings;

pub use metadata::{RecordingMetadata, format_duration};
pub use recordings::{Library, Recording};
