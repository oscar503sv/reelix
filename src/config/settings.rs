//! User settings persisted as TOML in `$XDG_CONFIG_HOME/<DIR_NAME>/settings.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::branding;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Low,
    #[default]
    Medium,
    High,
}

impl Quality {
    /// Target bits per pixel per frame. Screen content compresses much better
    /// than camera footage, so these are deliberately low.
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            Self::Low => 0.04,
            Self::Medium => 0.08,
            Self::High => 0.15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EncoderPreference {
    /// Hardware if available, software otherwise.
    #[default]
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub fps: u32,
    pub quality: Quality,
    pub encoder: EncoderPreference,
    pub show_cursor: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            fps: 30,
            quality: Quality::default(),
            encoder: EncoderPreference::default(),
            show_cursor: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StorageSettings {
    /// `None` means the default `~/Videos/<DIR_NAME>`.
    pub recording_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub video: VideoSettings,
    pub storage: StorageSettings,
}

impl Settings {
    pub const MIN_FPS: u32 = 5;
    pub const MAX_FPS: u32 = 60;

    /// Clamps values a user could have hand-edited into nonsense.
    pub fn sanitized(mut self) -> Self {
        self.video.fps = self.video.fps.clamp(Self::MIN_FPS, Self::MAX_FPS);
        self
    }

    pub fn default_path() -> Option<PathBuf> {
        let (qualifier, organization, application) = branding::PROJECT_DIRS;
        directories::ProjectDirs::from(qualifier, organization, application)
            .map(|d| d.config_dir().join("settings.toml"))
    }

    /// Loads settings from `path`. A missing file yields defaults; a corrupt
    /// file is reported so the caller can warn and fall back to defaults.
    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Settings>(&text)
                .map(Settings::sanitized)
                .map_err(|e| AppError::Config {
                    path: path.to_owned(),
                    message: e.to_string(),
                }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::io(path, e)),
        }
    }

    /// Loads from the default location, logging and falling back to defaults
    /// on any problem. Never fails: a broken config must not prevent startup.
    pub fn load_or_default() -> Self {
        let Some(path) = Self::default_path() else {
            tracing::warn!("no config directory available; using default settings");
            return Self::default();
        };
        Self::load_from(&path).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to load settings; using defaults");
            Self::default()
        })
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| AppError::Config {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
        fs::write(path, text).map_err(|e| AppError::io(path, e))
    }

    /// Directory where recordings are stored.
    pub fn recording_dir(&self) -> PathBuf {
        if let Some(dir) = &self.storage.recording_dir {
            return dir.clone();
        }
        let videos = directories::UserDirs::new()
            .and_then(|u| u.video_dir().map(Path::to_path_buf))
            .or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().join("Videos")))
            .unwrap_or_else(|| PathBuf::from("."));
        videos.join(branding::DIR_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::default();
        assert_eq!(s.video.fps, 30);
        assert_eq!(s.video.quality, Quality::Medium);
        assert_eq!(s.video.encoder, EncoderPreference::Auto);
        assert!(s.recording_dir().ends_with(branding::DIR_NAME));
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let s: Settings = toml::from_str("[video]\nfps = 60\n").expect("valid toml");
        assert_eq!(s.video.fps, 60);
        assert_eq!(s.video.quality, Quality::Medium);
        assert!(s.storage.recording_dir.is_none());
    }

    #[test]
    fn fps_is_clamped() {
        let s: Settings = toml::from_str("[video]\nfps = 500\n").expect("valid toml");
        assert_eq!(s.sanitized().video.fps, Settings::MAX_FPS);
    }

    #[test]
    fn custom_recording_dir_is_respected() {
        let mut s = Settings::default();
        s.storage.recording_dir = Some(PathBuf::from("/data/rec"));
        assert_eq!(s.recording_dir(), PathBuf::from("/data/rec"));
    }

    #[test]
    fn quality_ordering() {
        assert!(Quality::Low.bits_per_pixel() < Quality::Medium.bits_per_pixel());
        assert!(Quality::Medium.bits_per_pixel() < Quality::High.bits_per_pixel());
    }
}
