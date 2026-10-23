//! Converts the fragmented MP4 written while recording into a regular MP4
//! with the index at the front ("faststart"), without re-encoding.
//!
//! Why: recording fragmented keeps the file playable if the app dies, with no
//! wasted space (mp4mux's reserved-moov alternative padded every file with
//! ~8 MB per track). The regular faststart MP4 is the most compatible result
//! (browsers, editors, progressive playback).
//!
//! ```text
//! filesrc → qtdemux ─┬→ queue → h264parse ─┬→ mp4mux faststart → filesink (tmp) → rename
//!                    └→ queue → aacparse ──┘
//! ```
//! The parsers are required: linking qtdemux straight into mp4mux stalls.

use std::path::{Path, PathBuf};

use gst::prelude::*;

use crate::error::{AppError, Result};

/// A running remux. Keep it alive until `on_done` has been called.
pub struct Remux {
    pipeline: gst::Pipeline,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl Remux {
    /// Starts remuxing `path` in place. `on_done` runs on the thread-default
    /// main context once the file has been replaced (or on failure, leaving
    /// the original fragmented file untouched — it is still valid).
    pub fn start(path: &Path, on_done: impl FnOnce(Result<()>) + 'static) -> Result<Self> {
        super::init()?;
        let tmp = tmp_path(path);
        let pipeline = gst::Pipeline::with_name("reelix-remux");
        let make = |factory: &str| {
            gst::ElementFactory::make(factory)
                .build()
                .map_err(|_| AppError::MissingElement(factory.to_owned()))
        };
        let src = make("filesrc")?;
        src.set_property("location", path.to_string_lossy().as_ref());
        let demux = make("qtdemux")?;
        let mux = make("mp4mux")?;
        mux.set_property("faststart", true);
        let sink = make("filesink")?;
        sink.set_property("location", tmp.to_string_lossy().as_ref());
        pipeline
            .add_many([&src, &demux, &mux, &sink])
            .map_err(|e| AppError::Pipeline(e.to_string()))?;
        src.link(&demux)
            .map_err(|e| AppError::Pipeline(e.to_string()))?;
        mux.link(&sink)
            .map_err(|e| AppError::Pipeline(e.to_string()))?;

        let pipeline_weak = pipeline.downgrade();
        let mux_weak = mux.downgrade();
        demux.connect_pad_added(move |_, pad| {
            let (Some(pipeline), Some(mux)) = (pipeline_weak.upgrade(), mux_weak.upgrade()) else {
                return;
            };
            if let Err(e) = link_stream(&pipeline, pad, &mux) {
                tracing::error!(error = %e, pad = %pad.name(), "remux: cannot link stream");
            }
        });

        let mut on_done = Some(on_done);
        let original = path.to_owned();
        let bus = pipeline
            .bus()
            .ok_or_else(|| AppError::Pipeline("remux pipeline has no bus".into()))?;
        let pipeline_for_bus = pipeline.downgrade();
        let watch = bus
            .add_watch_local(move |_, msg| {
                let result = match msg.view() {
                    gst::MessageView::Eos(_) => Some(replace(&tmp, &original)),
                    gst::MessageView::Error(err) => {
                        let _ = std::fs::remove_file(&tmp);
                        Some(Err(AppError::Pipeline(format!(
                            "remux failed: {} ({:?})",
                            err.error(),
                            err.debug()
                        ))))
                    }
                    _ => None,
                };
                match result {
                    Some(result) => {
                        if let Some(pipeline) = pipeline_for_bus.upgrade() {
                            let _ = pipeline.set_state(gst::State::Null);
                        }
                        if let Some(on_done) = on_done.take() {
                            on_done(result);
                        }
                        gst::glib::ControlFlow::Break
                    }
                    None => gst::glib::ControlFlow::Continue,
                }
            })
            .map_err(|e| AppError::Pipeline(format!("remux bus: {e}")))?;

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| AppError::Pipeline(format!("remux start: {e}")))?;
        tracing::info!(path = %path.display(), "remuxing to faststart MP4");
        Ok(Self {
            pipeline,
            _bus_watch: watch,
        })
    }
}

impl Drop for Remux {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".remux.tmp");
    path.with_file_name(name)
}

fn replace(tmp: &Path, original: &Path) -> Result<()> {
    std::fs::rename(tmp, original).map_err(|e| AppError::io(original, e))
}

/// demux pad → queue → parser → mux request pad.
fn link_stream(pipeline: &gst::Pipeline, pad: &gst::Pad, mux: &gst::Element) -> Result<()> {
    let caps = pad
        .current_caps()
        .or_else(|| Some(pad.query_caps(None)))
        .ok_or_else(|| AppError::Pipeline("demux pad without caps".into()))?;
    let media = caps
        .structure(0)
        .map(|s| s.name().to_string())
        .unwrap_or_default();
    let parser = match media.as_str() {
        "video/x-h264" => "h264parse",
        "audio/mpeg" => "aacparse",
        other => {
            tracing::warn!(media = other, "remux: skipping unknown stream");
            return Ok(());
        }
    };
    let make = |factory: &str| {
        gst::ElementFactory::make(factory)
            .build()
            .map_err(|_| AppError::MissingElement(factory.to_owned()))
    };
    let queue = make("queue")?;
    let parse = make(parser)?;
    pipeline
        .add_many([&queue, &parse])
        .map_err(|e| AppError::Pipeline(e.to_string()))?;
    queue
        .link(&parse)
        .map_err(|e| AppError::Pipeline(e.to_string()))?;
    parse
        .link(mux)
        .map_err(|e| AppError::Pipeline(format!("remux: {media} into muxer: {e}")))?;
    for element in [&queue, &parse] {
        element
            .sync_state_with_parent()
            .map_err(|e| AppError::Pipeline(e.to_string()))?;
    }
    let queue_sink = queue
        .static_pad("sink")
        .ok_or_else(|| AppError::Pipeline("queue without sink pad".into()))?;
    pad.link(&queue_sink)
        .map(|_| ())
        .map_err(|e| AppError::Pipeline(format!("remux: link {media}: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_path_is_next_to_the_file() {
        assert_eq!(
            tmp_path(Path::new("/v/recording-1.mp4")),
            PathBuf::from("/v/recording-1.mp4.remux.tmp")
        );
    }
}
