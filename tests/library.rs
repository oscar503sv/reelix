//! Integration tests for the on-disk recordings library.

use chrono::{Duration, Local};
use reelix::storage::{Library, RecordingMetadata, naming};

fn write_recording(lib: &Library, minutes_ago: i64, title: Option<&str>) -> std::path::PathBuf {
    let when = Local::now() - Duration::minutes(minutes_ago);
    let video = lib.new_recording_path(&when).expect("path");
    std::fs::write(&video, b"fake mp4").expect("write video");
    if let Some(title) = title {
        RecordingMetadata {
            title: title.into(),
            created_at: when,
            duration: 42.0,
            resolution: Some("1920x1080".into()),
            fps: 30,
            audio: false,
            camera: false,
            encoder: Some("openh264enc".into()),
        }
        .write(&naming::metadata_path_for(&video))
        .expect("write metadata");
    }
    video
}

#[test]
fn missing_directory_lists_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path().join("does-not-exist"));
    assert!(lib.list().expect("list").is_empty());
}

#[test]
fn lists_newest_first_and_ignores_other_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    write_recording(&lib, 60, Some("Old"));
    write_recording(&lib, 1, Some("New"));
    std::fs::write(dir.path().join("notes.txt"), "x").expect("write");

    let titles: Vec<String> = lib
        .list()
        .expect("list")
        .iter()
        .map(|r| r.title())
        .collect();
    assert_eq!(titles, ["New", "Old"]);
}

#[test]
fn recording_without_metadata_uses_file_stem() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    let video = write_recording(&lib, 0, None);
    let rec = &lib.list().expect("list")[0];
    assert_eq!(rec.metadata, None);
    assert_eq!(
        rec.title(),
        video.file_stem().and_then(|s| s.to_str()).expect("stem")
    );
}

#[test]
fn corrupt_metadata_does_not_hide_video() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    let video = write_recording(&lib, 0, None);
    std::fs::write(naming::metadata_path_for(&video), "{not json").expect("write");
    let list = lib.list().expect("list");
    assert_eq!(list.len(), 1);
    assert!(list[0].metadata.is_none());
}

#[test]
fn rename_updates_title_but_keeps_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    let video = write_recording(&lib, 0, Some("Demo"));
    let rec = lib.list().expect("list").remove(0);

    let renamed = lib.rename(&rec, "  Demo FastAPI ").expect("rename");
    assert_eq!(renamed.title(), "Demo FastAPI");
    assert!(video.exists());

    let reloaded = lib.find(&rec.stem()).expect("find");
    assert_eq!(reloaded.title(), "Demo FastAPI");
    assert_eq!(reloaded.metadata.expect("meta").duration, 42.0);
}

#[test]
fn rename_creates_metadata_when_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    let video = write_recording(&lib, 0, None);
    let rec = lib.list().expect("list").remove(0);
    lib.rename(&rec, "Nueva").expect("rename");
    assert!(naming::metadata_path_for(&video).exists());
}

#[test]
fn delete_removes_video_and_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = Library::new(dir.path());
    let video = write_recording(&lib, 0, Some("Bye"));
    let rec = lib.list().expect("list").remove(0);
    lib.delete(&rec).expect("delete");
    assert!(!video.exists());
    assert!(!naming::metadata_path_for(&video).exists());
    assert!(lib.find(&rec.stem()).is_err());
}
