//! JSON sidecar metadata stored next to each recording.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub title: String,
    pub created_at: DateTime<Local>,
    /// Duration in seconds.
    pub duration: f64,
    /// `"1920x1080"`, or `None` if unknown.
    #[serde(default)]
    pub resolution: Option<String>,
    pub fps: u32,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub camera: bool,
    /// GStreamer element used to encode the video, e.g. `openh264enc`.
    #[serde(default)]
    pub encoder: Option<String>,
}

impl RecordingMetadata {
    pub fn read(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a
    /// truncated JSON file behind.
    pub fn write(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(|e| AppError::io(&tmp, e))?;
        fs::rename(&tmp, path).map_err(|e| AppError::io(path, e))
    }
}

/// Formats a duration in seconds as `m:ss` or `h:mm:ss`.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// How long ago a recording was made, for the library's day label
/// ("Today", "Yesterday", "Oct 20", "Dec 31, 2025"; the UI translates it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeDay {
    Today,
    Yesterday,
    ThisYear,
    OtherYear,
}

pub fn relative_day(date: chrono::NaiveDate, today: chrono::NaiveDate) -> RelativeDay {
    use chrono::Datelike;
    match (today - date).num_days() {
        0 => RelativeDay::Today,
        1 => RelativeDay::Yesterday,
        _ if date.year() == today.year() => RelativeDay::ThisYear,
        _ => RelativeDay::OtherYear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RecordingMetadata {
        RecordingMetadata {
            title: "Demo".into(),
            created_at: Local::now(),
            duration: 123.4,
            resolution: Some("1920x1080".into()),
            fps: 30,
            audio: false,
            camera: false,
            encoder: Some("openh264enc".into()),
        }
    }

    #[test]
    fn roundtrip_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.json");
        let meta = sample();
        meta.write(&path).expect("write");
        assert_eq!(RecordingMetadata::read(&path).expect("read"), meta);
        assert!(!dir.path().join("a.json.tmp").exists());
    }

    #[test]
    fn parses_spec_example_with_missing_optional_fields() {
        let json = r#"{
            "title": "Demo",
            "created_at": "2026-10-22T21:05:00+02:00",
            "duration": 123,
            "resolution": "1920x1080",
            "fps": 30,
            "audio": true,
            "camera": true
        }"#;
        let meta: RecordingMetadata = serde_json::from_str(json).expect("valid");
        assert_eq!(meta.duration, 123.0);
        assert!(meta.audio && meta.camera);
        assert_eq!(meta.encoder, None);
    }

    #[test]
    fn relative_days() {
        use chrono::NaiveDate;
        let today = NaiveDate::from_ymd_opt(2026, 10, 23).expect("date");
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).expect("date");
        assert_eq!(relative_day(day(2026, 10, 23), today), RelativeDay::Today);
        assert_eq!(
            relative_day(day(2026, 10, 22), today),
            RelativeDay::Yesterday
        );
        assert_eq!(
            relative_day(day(2026, 10, 20), today),
            RelativeDay::ThisYear
        );
        assert_eq!(
            relative_day(day(2025, 12, 31), today),
            RelativeDay::OtherYear
        );
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(151.0), "2:31");
        assert_eq!(format_duration(3725.0), "1:02:05");
        assert_eq!(format_duration(-3.0), "0:00");
    }
}
