//! User settings persisted as TOML in `$XDG_CONFIG_HOME/<DIR_NAME>/settings.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::branding;
use crate::error::{AppError, Result};
use crate::recording::camera::{CameraLayout, Corner, Shape};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub microphone: bool,
    /// PipeWire `node.name` of the microphone; `None` follows the system default.
    pub microphone_device: Option<String>,
    /// Linear gain applied to the microphone (1.0 = unchanged).
    pub microphone_volume: f64,
    /// Also record what the speakers play (monitor of the default output).
    pub system_audio: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            microphone: true,
            microphone_device: None,
            microphone_volume: 1.0,
            system_audio: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CameraSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CameraSize {
    pub const ALL: [CameraSize; 3] = [Self::Small, Self::Medium, Self::Large];

    /// Bubble height as a fraction of the recording height.
    pub fn height_fraction(self) -> f64 {
        match self {
            Self::Small => 0.18,
            Self::Medium => 0.25,
            Self::Large => 0.33,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CameraShape {
    /// 4:3 rectangle; cheapest (compositing stays in I420).
    #[default]
    Rectangle,
    /// Round bubble; needs per-pixel alpha (≈ +20 % CPU on 1080p).
    Circle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CameraCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CameraSettings {
    pub enabled: bool,
    /// PipeWire `node.name` of the camera; `None` lets PipeWire choose.
    pub device: Option<String>,
    pub size: CameraSize,
    pub shape: CameraShape,
    pub corner: CameraCorner,
    pub mirror: bool,
}

impl CameraSettings {
    pub fn layout(&self) -> CameraLayout {
        CameraLayout {
            corner: match self.corner {
                CameraCorner::TopLeft => Corner::TopLeft,
                CameraCorner::TopRight => Corner::TopRight,
                CameraCorner::BottomLeft => Corner::BottomLeft,
                CameraCorner::BottomRight => Corner::BottomRight,
            },
            height_fraction: self.size.height_fraction(),
            shape: match self.shape {
                CameraShape::Rectangle => Shape::Rectangle,
                CameraShape::Circle => Shape::Circle,
            },
            mirror: self.mirror,
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
    pub audio: AudioSettings,
    pub camera: CameraSettings,
    pub storage: StorageSettings,
}

impl Settings {
    pub const MIN_FPS: u32 = 5;
    pub const MAX_FPS: u32 = 60;
    pub const MAX_VOLUME: f64 = 4.0;

    /// Clamps values a user could have hand-edited into nonsense.
    pub fn sanitized(mut self) -> Self {
        self.video.fps = self.video.fps.clamp(Self::MIN_FPS, Self::MAX_FPS);
        let volume = self.audio.microphone_volume;
        self.audio.microphone_volume = if volume.is_finite() {
            volume.clamp(0.0, Self::MAX_VOLUME)
        } else {
            1.0
        };
        let blank = |d: &Option<String>| d.as_ref().is_some_and(|d| d.trim().is_empty());
        if blank(&self.audio.microphone_device) {
            self.audio.microphone_device = None;
        }
        if blank(&self.camera.device) {
            self.camera.device = None;
        }
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

    /// Saves to the default location.
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path().ok_or_else(|| AppError::Config {
            path: PathBuf::from("~/.config"),
            message: "no config directory available".into(),
        })?;
        self.save_to(&path)
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
    fn audio_defaults_record_default_microphone_only() {
        let a = Settings::default().audio;
        assert!(a.microphone);
        assert!(a.microphone_device.is_none());
        assert!(!a.system_audio);
        assert_eq!(a.microphone_volume, 1.0);
    }

    #[test]
    fn audio_values_are_sanitized() {
        let s: Settings =
            toml::from_str("[audio]\nmicrophone_volume = 9.0\nmicrophone_device = \"  \"\n")
                .expect("valid toml");
        let s = s.sanitized();
        assert_eq!(s.audio.microphone_volume, Settings::MAX_VOLUME);
        assert!(s.audio.microphone_device.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/settings.toml");
        let mut s = Settings::default();
        s.audio.system_audio = true;
        s.audio.microphone_device = Some("alsa_input.test".into());
        s.save_to(&path).expect("save");
        assert_eq!(Settings::load_from(&path).expect("load"), s);
    }

    #[test]
    fn corrupt_file_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "video = [").expect("write");
        assert!(matches!(
            Settings::load_from(&path),
            Err(AppError::Config { .. })
        ));
    }

    #[test]
    fn camera_defaults_are_off_bottom_right_rectangle() {
        let c = Settings::default().camera;
        assert!(!c.enabled);
        assert_eq!(c.corner, CameraCorner::BottomRight);
        assert_eq!(c.shape, CameraShape::Rectangle);
        assert_eq!(c.size, CameraSize::Medium);
        let layout = c.layout();
        assert_eq!(layout.corner, Corner::BottomRight);
        assert_eq!(layout.height_fraction, 0.25);
    }

    #[test]
    fn camera_settings_parse_from_toml() {
        let s: Settings = toml::from_str(
            "[camera]\nenabled = true\nsize = \"large\"\nshape = \"circle\"\ncorner = \"top-left\"\n",
        )
        .expect("valid toml");
        assert!(s.camera.enabled);
        assert_eq!(s.camera.layout().shape, Shape::Circle);
        assert_eq!(s.camera.layout().corner, Corner::TopLeft);
        assert_eq!(s.camera.layout().height_fraction, 0.33);
    }

    #[test]
    fn camera_sizes_are_ordered() {
        let f: Vec<f64> = CameraSize::ALL
            .iter()
            .map(|s| s.height_fraction())
            .collect();
        assert!(f.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn quality_ordering() {
        assert!(Quality::Low.bits_per_pixel() < Quality::Medium.bits_per_pixel());
        assert!(Quality::Medium.bits_per_pixel() < Quality::High.bits_per_pixel());
    }
}
