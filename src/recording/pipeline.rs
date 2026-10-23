//! GStreamer recording pipeline.
//!
//! ```text
//! pipewiresrc (portal fd + node id)
//!   → videoconvert (BGRx → I420/NV12, multithreaded, no dithering)
//!   → videoscale + capsfilter (output size fixed to the first negotiated size)
//!   → videorate max-rate=fps (drop-only: never duplicates frames)
//!   → queue (leaky, bounded: drops frames instead of growing RAM)
//!   → H.264 encoder → h264parse ─┐
//!                                ├→ mp4mux → filesink
//!   audio branch (audio.rs) ─────┘
//! ```
//!
//! GNOME delivers frames only on screen damage (plus a keep-alive), so the
//! output is variable frame rate capped at `fps`. This keeps CPU usage low
//! on static screens.
//!
//! All sources are timestamped against the system clock, so audio and video
//! share one timeline and cannot drift apart.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gst::prelude::*;

use super::audio::{self, AacEncoderKind, AudioInput};
use super::encoder::{EncoderKind, target_bitrate};
use crate::error::{AppError, Result};

const SCREEN_SOURCE_NAME: &str = "screen-source";
const VIDEO_QUEUE_NAME: &str = "encode-queue";
/// Standard 90 kHz video clock.
const VIDEO_TIMESCALE: u32 = 90_000;

/// Where video frames come from.
#[derive(Debug, Clone)]
pub enum VideoInput {
    /// A ScreenCast portal stream. `pipewiresrc` dups the fd; the caller
    /// must keep the original open while building.
    PipeWire { fd: RawFd, node_id: u32 },
    /// Any live element producing raw video (tests, future sources).
    Element(gst::Element),
}

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub inputs: Vec<AudioInput>,
    pub encoder: AacEncoderKind,
}

/// Settings needed to build a pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub video: VideoInput,
    pub audio: Option<AudioConfig>,
    pub output: PathBuf,
    pub fps: u32,
    pub bits_per_pixel: f64,
    pub encoder: EncoderKind,
}

/// Notifications delivered on the GLib main thread.
#[derive(Debug)]
pub enum PipelineEvent {
    /// Pipeline reached PLAYING.
    Started,
    /// A capture source disappeared (e.g. the user stopped sharing from
    /// GNOME's top bar). The pipeline is finalizing what it has.
    SourceLost {
        source: String,
    },
    /// Microphone loudness, 0.0–1.0, roughly every 100 ms.
    AudioLevel(f64),
    /// EOS reached the sink: the MP4 file is complete.
    Finalized,
    Error(AppError),
}

pub struct RecordingPipeline {
    pipeline: gst::Pipeline,
    resolution: Arc<Mutex<Option<(u32, u32)>>>,
    finishing: Arc<AtomicBool>,
    has_audio: bool,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl RecordingPipeline {
    /// Builds the pipeline (in NULL state). `on_event` is invoked on the
    /// thread-default GLib main context, i.e. the GTK UI thread.
    pub fn new(
        config: &PipelineConfig,
        on_event: impl Fn(PipelineEvent) + 'static,
    ) -> Result<Self> {
        super::init()?;

        let pipeline = gst::Pipeline::with_name("reelix-recording");
        pipeline.use_clock(Some(&gst::SystemClock::obtain()));
        let format = config.encoder.preferred_format();

        let source = make_video_source(&config.video)?;

        let convert = make("videoconvert", "convert")?;
        convert.set_property_from_str("chroma-mode", "none");
        convert.set_property_from_str("dither", "none");
        convert.set_property_from_str("matrix-mode", "output-only");
        convert.set_property("n-threads", conversion_threads());

        let scale = make("videoscale", "scale")?;
        scale.set_property("add-borders", true);

        let size_filter = make("capsfilter", "size-filter")?;
        size_filter.set_property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", format)
                .build(),
        );

        let rate = make("videorate", "rate")?;
        rate.set_property("max-rate", i32::try_from(config.fps).unwrap_or(30));
        rate.set_property("skip-to-first", true);

        let queue = make("queue", VIDEO_QUEUE_NAME)?;
        queue.set_property("max-size-buffers", 5u32);
        queue.set_property("max-size-bytes", 0u32);
        queue.set_property("max-size-time", 0u64);
        queue.set_property_from_str("leaky", "downstream");

        let initial_bitrate = target_bitrate(1920, 1080, config.fps, config.bits_per_pixel);
        let encoder = config.encoder.build(initial_bitrate, config.fps)?;
        let parse = make("h264parse", "parse")?;

        let mux = make("mp4mux", "mux")?;
        // Keep the index at the start of the file and refresh it every 10 s:
        // if the app crashes the MP4 is still playable up to that point.
        mux.set_property(
            "reserved-max-duration",
            gst::ClockTime::from_seconds(4 * 3600).nseconds(),
        );
        mux.set_property(
            "reserved-moov-update-period",
            gst::ClockTime::from_seconds(10).nseconds(),
        );

        let sink = make("filesink", "sink")?;
        sink.set_property("location", config.output.to_string_lossy().as_ref());

        let chain = [
            &source,
            &convert,
            &scale,
            &size_filter,
            &rate,
            &queue,
            &encoder,
            &parse,
            &mux,
            &sink,
        ];
        pipeline
            .add_many(chain)
            .map_err(|e| AppError::Pipeline(e.to_string()))?;
        gst::Element::link_many(chain).map_err(|e| AppError::Pipeline(e.to_string()))?;
        // Variable frame rate timestamps need a fine timescale; the automatic
        // one is derived from the (absent) frame rate and loses precision.
        if let Some(mux_pad) = parse.static_pad("src").and_then(|p| p.peer()) {
            mux_pad.set_property("trak-timescale", VIDEO_TIMESCALE);
        }

        let mut capture_sources = vec![SCREEN_SOURCE_NAME.to_owned()];
        let has_audio = match &config.audio {
            Some(audio_config) if !audio_config.inputs.is_empty() => {
                capture_sources.extend(audio::build_branch(
                    &pipeline,
                    &audio_config.inputs,
                    audio_config.encoder,
                    &mux,
                )?);
                true
            }
            _ => false,
        };

        let resolution = Arc::new(Mutex::new(None));
        install_size_lock(
            &source,
            &size_filter,
            &encoder,
            config,
            format,
            Arc::clone(&resolution),
        )?;

        let finishing = Arc::new(AtomicBool::new(false));
        let bus_watch =
            install_bus_watch(&pipeline, Arc::clone(&finishing), capture_sources, on_event)?;

        tracing::info!(
            encoder = %config.encoder,
            audio = has_audio,
            output = %config.output.display(),
            "recording pipeline built"
        );

        Ok(Self {
            pipeline,
            resolution,
            finishing,
            has_audio,
            _bus_watch: bus_watch,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .map(|_| ())
            .map_err(|e| AppError::Pipeline(format!("failed to start pipeline: {e}")))
    }

    /// Requests a clean stop. The file is complete once
    /// [`PipelineEvent::Finalized`] is delivered.
    pub fn finish(&self) {
        request_finish(&self.pipeline, &self.finishing);
    }

    /// Tears the pipeline down immediately without finalizing.
    pub fn abort(&self) {
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            tracing::warn!(error = %e, "failed to stop pipeline");
        }
    }

    pub fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// Output resolution, once the first frame has been negotiated.
    pub fn resolution(&self) -> Option<(u32, u32)> {
        self.resolution.lock().ok().and_then(|r| *r)
    }

    /// Current recording position (running time of the pipeline).
    pub fn position(&self) -> Option<Duration> {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|t| Duration::from_nanos(t.nseconds()))
    }
}

impl Drop for RecordingPipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn make(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|_| AppError::MissingElement(factory.to_owned()))
}

fn make_video_source(input: &VideoInput) -> Result<gst::Element> {
    match input {
        VideoInput::PipeWire { fd, node_id } => {
            let source = make("pipewiresrc", SCREEN_SOURCE_NAME)?;
            source.set_property("fd", *fd);
            source.set_property("path", node_id.to_string());
            // Same settings GNOME Shell's built-in recorder uses.
            source.set_property("do-timestamp", true);
            source.set_property("keepalive-time", 1000i32);
            Ok(source)
        }
        VideoInput::Element(element) => {
            element.set_property("name", SCREEN_SOURCE_NAME);
            Ok(element.clone())
        }
    }
}

fn conversion_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(1).min(4))
        .unwrap_or(1)
}

/// Pushes EOS into each branch's encode queue rather than into the sources.
/// This works whether the sources are healthy or have just failed, because
/// EOS travels downstream from the queues to the muxer regardless of
/// upstream state. The muxer finalizes once every branch has seen EOS.
fn request_finish(pipeline: &gst::Pipeline, finishing: &AtomicBool) {
    if finishing.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::info!("finalizing recording");
    let mut delivered = false;
    for name in [VIDEO_QUEUE_NAME, audio::AUDIO_QUEUE_NAME] {
        let Some(pad) = pipeline.by_name(name).and_then(|q| q.static_pad("sink")) else {
            continue;
        };
        if pad.send_event(gst::event::Eos::new()) {
            delivered = true;
        } else {
            tracing::warn!(queue = name, "queue rejected EOS");
        }
    }
    if !delivered {
        tracing::warn!("no queue accepted EOS; sending it to the pipeline");
        pipeline.send_event(gst::event::Eos::new());
    }
}

/// Window captures can be resized mid-recording, and some windows have odd
/// dimensions that 4:2:0 encoders reject. The first negotiated size is
/// rounded down to even numbers and locked in the capsfilter; later resizes
/// are scaled (with borders) into that frame so the MP4 never changes caps.
/// The bitrate is computed from that real size.
fn install_size_lock(
    source: &gst::Element,
    size_filter: &gst::Element,
    encoder: &gst::Element,
    config: &PipelineConfig,
    format: &'static str,
    resolution: Arc<Mutex<Option<(u32, u32)>>>,
) -> Result<()> {
    let pad = source
        .static_pad("src")
        .ok_or_else(|| AppError::Pipeline("video source has no src pad".into()))?;
    let size_filter = size_filter.downgrade();
    let encoder = encoder.downgrade();
    let (kind, fps, bpp) = (config.encoder, config.fps, config.bits_per_pixel);

    pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
        let Some(gst::PadProbeData::Event(event)) = &info.data else {
            return gst::PadProbeReturn::Ok;
        };
        let gst::EventView::Caps(caps_event) = event.view() else {
            return gst::PadProbeReturn::Ok;
        };
        let Ok(mut locked) = resolution.lock() else {
            return gst::PadProbeReturn::Remove;
        };
        if locked.is_some() {
            return gst::PadProbeReturn::Remove;
        }
        let Some(s) = caps_event.caps().structure(0) else {
            return gst::PadProbeReturn::Ok;
        };
        let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
            return gst::PadProbeReturn::Ok;
        };
        let even = |v: i32| (u32::try_from(v).unwrap_or(2).max(2)) & !1;
        let (w, h) = (even(w), even(h));
        tracing::info!(width = w, height = h, caps = %caps_event.caps(), "video stream negotiated");

        if let Some(filter) = size_filter.upgrade() {
            filter.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("format", format)
                    .field("width", i32::try_from(w).unwrap_or(i32::MAX))
                    .field("height", i32::try_from(h).unwrap_or(i32::MAX))
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .build(),
            );
        }
        if let Some(enc) = encoder.upgrade() {
            kind.set_bitrate(&enc, target_bitrate(w, h, fps, bpp));
        }
        *locked = Some((w, h));
        gst::PadProbeReturn::Remove
    })
    .ok_or_else(|| AppError::Pipeline("failed to install caps probe".into()))?;
    Ok(())
}

fn install_bus_watch(
    pipeline: &gst::Pipeline,
    finishing: Arc<AtomicBool>,
    capture_sources: Vec<String>,
    on_event: impl Fn(PipelineEvent) + 'static,
) -> Result<gst::bus::BusWatchGuard> {
    let bus = pipeline
        .bus()
        .ok_or_else(|| AppError::Pipeline("pipeline has no bus".into()))?;
    let weak_pipeline = pipeline.downgrade();
    let mut started = false;

    bus.add_watch_local(move |_, msg| {
        use gst::MessageView;
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return gst::glib::ControlFlow::Break;
        };
        match msg.view() {
            MessageView::StateChanged(sc)
                if !started
                    && sc.current() == gst::State::Playing
                    && msg.src() == Some(pipeline.upcast_ref::<gst::Object>()) =>
            {
                started = true;
                on_event(PipelineEvent::Started);
            }
            MessageView::Element(element) => {
                let from_meter = msg.src().is_some_and(|s| s.name() == audio::LEVEL_NAME);
                if let Some(level) = element
                    .structure()
                    .filter(|_| from_meter)
                    .and_then(audio::parse_level)
                {
                    on_event(PipelineEvent::AudioLevel(level));
                }
            }
            MessageView::Eos(_) => {
                tracing::info!("EOS reached sink; recording finalized");
                on_event(PipelineEvent::Finalized);
            }
            MessageView::Error(err) => {
                let src_name = msg.src().map(|s| s.name().to_string()).unwrap_or_default();
                let details = err.debug().map(|d| d.to_string()).unwrap_or_default();
                let is_capture_source = capture_sources.contains(&src_name);
                if is_capture_source && finishing.load(Ordering::SeqCst) {
                    // Sources commonly complain once they are being shut
                    // down (e.g. "target not found" after the portal closes).
                    tracing::debug!(source = %src_name, error = %err.error(), "ignoring source error while finalizing");
                } else if is_capture_source && started {
                    // A failing source mid-recording almost always means the
                    // screen cast was revoked or a device vanished; keep what
                    // we have instead of failing the whole recording.
                    tracing::warn!(source = %src_name, error = %err.error(), %details, "capture source lost; finalizing");
                    request_finish(&pipeline, &finishing);
                    on_event(PipelineEvent::SourceLost { source: src_name });
                } else {
                    tracing::error!(source = %src_name, error = %err.error(), %details, "pipeline error");
                    on_event(PipelineEvent::Error(AppError::Streaming {
                        source_element: src_name,
                        message: err.error().to_string(),
                        debug: details,
                    }));
                }
            }
            MessageView::Warning(w) => {
                tracing::warn!(
                    source = ?msg.src().map(|s| s.name()),
                    warning = %w.error(),
                    debug = ?w.debug(),
                    "pipeline warning"
                );
            }
            _ => {}
        }
        gst::glib::ControlFlow::Continue
    })
    .map_err(|e| AppError::Pipeline(format!("failed to watch bus: {e}")))
}
