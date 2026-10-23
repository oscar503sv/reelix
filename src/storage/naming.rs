//! File naming for recordings: `<prefix>-YYYY-MM-DD-HHMMSS[-N].mp4`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone};

use crate::branding;

pub const VIDEO_EXTENSION: &str = "mp4";
pub const METADATA_EXTENSION: &str = "json";

/// File stem for a recording started at `when`, e.g. `recording-2026-10-22-210500`.
pub fn recording_stem<Tz: TimeZone>(when: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    format!(
        "{}-{}",
        branding::RECORDING_PREFIX,
        when.format("%Y-%m-%d-%H%M%S")
    )
}

/// Returns a video path in `dir` that does not exist yet. If two recordings
/// start within the same second a numeric suffix is appended.
pub fn unique_video_path(dir: &Path, stem: &str) -> PathBuf {
    let candidate = dir.join(format!("{stem}.{VIDEO_EXTENSION}"));
    if !candidate.exists() {
        return candidate;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.{VIDEO_EXTENSION}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate) // unreachable: the iterator is infinite
}

/// Sidecar metadata path for a video: same stem, `.json` extension.
pub fn metadata_path_for(video: &Path) -> PathBuf {
    video.with_extension(METADATA_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    #[test]
    fn stem_format_matches_spec() {
        let tz = FixedOffset::east_opt(2 * 3600).expect("valid offset");
        let when = tz
            .with_ymd_and_hms(2026, 10, 22, 21, 5, 0)
            .single()
            .expect("valid date");
        assert_eq!(recording_stem(&when), "recording-2026-10-22-210500");
    }

    #[test]
    fn metadata_path_replaces_extension() {
        let video = Path::new("/videos/recording-2026-10-22-210500.mp4");
        assert_eq!(
            metadata_path_for(video),
            PathBuf::from("/videos/recording-2026-10-22-210500.json")
        );
    }

    #[test]
    fn unique_path_appends_suffix_on_collision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = unique_video_path(dir.path(), "recording-x");
        assert_eq!(
            first.file_name().and_then(|s| s.to_str()),
            Some("recording-x.mp4")
        );
        std::fs::write(&first, b"").expect("write");
        let second = unique_video_path(dir.path(), "recording-x");
        assert_eq!(
            second.file_name().and_then(|s| s.to_str()),
            Some("recording-x-2.mp4")
        );
    }
}
