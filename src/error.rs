//! Structured errors.
//!
//! Every error carries a technical description (for logs, via `Display`) and
//! a user-facing explanation (via [`AppError::user_message`]).

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("screen cast portal error: {0}")]
    Portal(String),

    #[error("screen selection was cancelled by the user")]
    SelectionCancelled,

    #[error("the portal returned no streams")]
    NoStream,

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

    /// Short title and longer explanation suitable for a dialog.
    pub fn user_message(&self) -> (String, String) {
        match self {
            Self::Portal(_) => (
                "No se pudo acceder a la pantalla".into(),
                "Comprueba que xdg-desktop-portal y xdg-desktop-portal-gnome están \
                 instalados y que la sesión es Wayland."
                    .into(),
            ),
            Self::SelectionCancelled => (
                "Grabación cancelada".into(),
                "No se seleccionó ninguna pantalla ni ventana.".into(),
            ),
            Self::NoStream => (
                "No se pudo acceder a la pantalla".into(),
                "El portal de captura no devolvió ninguna fuente. Inténtalo de nuevo.".into(),
            ),
            Self::GstInit(_) | Self::MissingElement(_) => (
                "Faltan componentes multimedia".into(),
                "Instala GStreamer y sus plugins: \
                 sudo dnf install pipewire-gstreamer gstreamer1-plugins-good \
                 gstreamer1-plugins-bad-free gstreamer1-plugin-openh264"
                    .into(),
            ),
            Self::NoEncoder(_) => (
                "No hay codificador de vídeo disponible".into(),
                "Instala gstreamer1-plugin-openh264 o, para aceleración por hardware \
                 en Intel, intel-media-driver de RPM Fusion."
                    .into(),
            ),
            Self::NoAudioEncoder(_) => (
                "No hay codificador de audio disponible".into(),
                "Instala fdk-aac-free y gstreamer1-plugins-bad-free, o \
                 gstreamer1-plugin-libav."
                    .into(),
            ),
            Self::Pipeline(_) | Self::Streaming { .. } => (
                "La grabación falló".into(),
                "Se produjo un error en el pipeline multimedia. Ejecuta con \
                 RUST_LOG=debug para ver los detalles."
                    .into(),
            ),
            Self::Io { path, .. } => (
                "Error de archivo".into(),
                format!("No se pudo leer o escribir {}.", path.display()),
            ),
            Self::Config { path, .. } => (
                "Configuración inválida".into(),
                format!(
                    "El archivo {} no es válido. Se usará la configuración por defecto.",
                    path.display()
                ),
            ),
            Self::Metadata(_) => (
                "Metadatos inválidos".into(),
                "No se pudieron leer o escribir los metadatos de la grabación.".into(),
            ),
            Self::InvalidTransition(_) => (
                "Operación no disponible".into(),
                "Esta acción no es posible en el estado actual.".into(),
            ),
            Self::RecordingNotFound(name) => (
                "Grabación no encontrada".into(),
                format!("No existe la grabación «{name}»."),
            ),
        }
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;
