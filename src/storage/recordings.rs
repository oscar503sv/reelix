//! The local recordings library: a directory of `.mp4` files with optional
//! `.json` sidecars.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use super::metadata::RecordingMetadata;
use super::naming::{self, METADATA_EXTENSION, VIDEO_EXTENSION};
use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub video_path: PathBuf,
    pub metadata: Option<RecordingMetadata>,
    pub size_bytes: u64,
    pub modified: Option<DateTime<Local>>,
}

impl Recording {
    pub fn title(&self) -> String {
        self.metadata
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.stem())
    }

    pub fn stem(&self) -> String {
        self.video_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn created_at(&self) -> Option<DateTime<Local>> {
        self.metadata
            .as_ref()
            .map(|m| m.created_at)
            .or(self.modified)
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| AppError::io(&self.dir, e))
    }

    /// Reserves a fresh output path for a recording starting now.
    pub fn new_recording_path(&self, now: &DateTime<Local>) -> Result<PathBuf> {
        self.ensure_dir()?;
        Ok(naming::unique_video_path(
            &self.dir,
            &naming::recording_stem(now),
        ))
    }

    /// Lists recordings, newest first. Unreadable metadata is logged and
    /// ignored rather than hiding the video.
    pub fn list(&self) -> Result<Vec<Recording>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AppError::io(&self.dir, e)),
        };

        let mut recordings: Vec<Recording> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == VIDEO_EXTENSION))
            .map(|video_path| self.load(video_path))
            .collect();

        recordings.sort_by_key(|r| std::cmp::Reverse(r.created_at()));
        Ok(recordings)
    }

    fn load(&self, video_path: PathBuf) -> Recording {
        let meta_path = naming::metadata_path_for(&video_path);
        let metadata = if meta_path.exists() {
            RecordingMetadata::read(&meta_path)
                .inspect_err(|e| tracing::warn!(path = %meta_path.display(), error = %e, "ignoring bad metadata"))
                .ok()
        } else {
            None
        };
        let fs_meta = fs::metadata(&video_path).ok();
        Recording {
            size_bytes: fs_meta.as_ref().map_or(0, |m| m.len()),
            modified: fs_meta
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Local>::from),
            metadata,
            video_path,
        }
    }

    /// Finds a recording by file stem (as printed by `--list`).
    pub fn find(&self, stem: &str) -> Result<Recording> {
        let video = self.dir.join(format!("{stem}.{VIDEO_EXTENSION}"));
        if !video.is_file() {
            return Err(AppError::RecordingNotFound(stem.to_owned()));
        }
        Ok(self.load(video))
    }

    /// Changes the display title. The file name is kept stable so external
    /// references to the file keep working.
    pub fn rename(&self, recording: &Recording, title: &str) -> Result<Recording> {
        let mut meta = recording
            .metadata
            .clone()
            .unwrap_or_else(|| RecordingMetadata {
                title: String::new(),
                created_at: recording.created_at().unwrap_or_else(Local::now),
                duration: 0.0,
                resolution: None,
                fps: 0,
                audio: false,
                camera: false,
                encoder: None,
            });
        meta.title = title.trim().to_owned();
        meta.write(&naming::metadata_path_for(&recording.video_path))?;
        Ok(Recording {
            metadata: Some(meta),
            ..recording.clone()
        })
    }

    /// Deletes the video and its sidecar.
    pub fn delete(&self, recording: &Recording) -> Result<()> {
        let video = &recording.video_path;
        fs::remove_file(video).map_err(|e| AppError::io(video, e))?;
        let meta = video.with_extension(METADATA_EXTENSION);
        match fs::remove_file(&meta) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::io(meta, e)),
        }
    }
}
