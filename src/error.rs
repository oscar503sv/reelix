//! Structured errors.
//!
//! Every error carries a technical description (for logs, via `Display`) and
//! a user-facing explanation (via [`AppError::user_message`]).

use std::path::PathBuf;

use thiserror::Error;

use crate::i18n::{fill, gettext};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("screen cast portal error: {0}")]
    Portal(String),

    #[error("screen selection was cancelled by the user")]
    SelectionCancelled,

    #[error("the portal returned no streams")]
    NoStream,

    #[error("no camera is available")]
    NoCamera,

    #[error("camera access was denied")]
    CameraDenied,

    #[error("GStreamer initialization failed: {0}")]
    GstInit(String),

    #[error("required GStreamer element `{0}` is not installed")]
    MissingElement(String),

    #[error("no usable H.264 encoder found (tried: {0})")]
    NoEncoder(String),

    #[error("no usable AAC encoder found (tried: {0})")]
    NoAudioEncoder(String),

    #[error("failed to build pipeline: {0}")]
    Pipeline(String),

    #[error("pipeline error from {source_element}: {message} ({debug})")]
    Streaming {
        source_element: String,
        message: String,
        debug: String,
    },

    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid configuration file {path}: {message}")]
    Config { path: PathBuf, message: String },

    #[error("metadata error: {0}")]
    Metadata(#[from] serde_json::Error),

    #[error("invalid state transition: {0}")]
    InvalidTransition(String),

    #[error("recording not found: {0}")]
    RecordingNotFound(String),
}

impl AppError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Short title and longer explanation suitable for a dialog, translated.
    pub fn user_message(&self) -> (String, String) {
        let (title, body) = match self {
            Self::Portal(_) => (
                gettext("Could not access the screen"),
                gettext(
                    "Check that xdg-desktop-portal and xdg-desktop-portal-gnome are installed \
                     and that the session is Wayland.",
                ),
            ),
            Self::SelectionCancelled => (
                gettext("Recording cancelled"),
                gettext("No screen or window was selected."),
            ),
            Self::NoStream => (
                gettext("Could not access the screen"),
                gettext("The screen capture portal returned no source. Please try again."),
            ),
            Self::NoCamera => (
                gettext("No camera available"),
                gettext("No camera was detected. Recording will continue without it."),
            ),
            Self::CameraDenied => (
                gettext("Camera access denied"),
                gettext("You can allow it in Settings → Privacy → Camera."),
            ),
            Self::GstInit(_) | Self::MissingElement(_) => (
                gettext("Multimedia components are missing"),
                gettext(
                    "Install GStreamer and its plugins: sudo dnf install pipewire-gstreamer \
                     gstreamer1-plugins-good gstreamer1-plugins-bad-free \
                     gstreamer1-plugin-openh264",
                ),
            ),
            Self::NoEncoder(_) => (
                gettext("No video encoder available"),
                gettext(
                    "Install gstreamer1-plugin-openh264 or, for hardware acceleration on \
                     Intel, intel-media-driver from RPM Fusion.",
                ),
            ),
            Self::NoAudioEncoder(_) => (
                gettext("No audio encoder available"),
                gettext(
                    "Install fdk-aac-free and gstreamer1-plugins-bad-free, or \
                     gstreamer1-plugin-libav.",
                ),
            ),
            Self::Pipeline(_) | Self::Streaming { .. } => (
                gettext("Recording failed"),
                gettext(
                    "An error occurred in the multimedia pipeline. Run with RUST_LOG=debug \
                     to see the details.",
                ),
            ),
            Self::Io { path, .. } => (
                gettext("File error"),
                fill(
                    &gettext("Could not read or write {path}."),
                    &[("path", &path.display())],
                ),
            ),
            Self::Config { path, .. } => (
                gettext("Invalid settings"),
                fill(
                    &gettext("The file {path} is not valid. The default settings will be used."),
                    &[("path", &path.display())],
                ),
            ),
            Self::Metadata(_) => (
                gettext("Invalid metadata"),
                gettext("The recording metadata could not be read or written."),
            ),
            Self::InvalidTransition(_) => (
                gettext("Action not available"),
                gettext("This action is not possible right now."),
            ),
            Self::RecordingNotFound(name) => (
                gettext("Recording not found"),
                fill(
                    &gettext("The recording “{name}” does not exist."),
                    &[("name", name)],
                ),
            ),
        };
        (title, body)
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;
