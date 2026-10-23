//! Runs the production `RecordingPipeline` with live test sources in place
//! of the portal stream and microphones: start, record, stop via EOS
//! injection, then verify the MP4 tracks.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gst::glib;
use gst::prelude::*;
use reelix::config::EncoderPreference;
use reelix::recording::audio::{AacEncoderKind, AudioInput, AudioSourceElement};
use reelix::recording::camera::{CameraInput, CameraLayout, CameraSource, Corner, Shape};
use reelix::recording::{
    AudioConfig, EncoderKind, PipelineConfig, PipelineEvent, RecordingPipeline, VideoInput,
};

const RECORD_FOR: Duration = Duration::from_secs(3);

fn live(factory: &str) -> gst::Element {
    gst::ElementFactory::make(factory)
        .property("is-live", true)
        .build()
        .expect("test source exists")
}

fn audio_input(meter: bool, wave: &str) -> AudioInput {
    let src = live("audiotestsrc");
    src.set_property_from_str("wave", wave);
    AudioInput {
        source: AudioSourceElement::Element(src),
        volume: 0.8,
        meter,
    }
}

#[derive(Default)]
struct Observed {
    started: bool,
    finalized: bool,
    levels: usize,
    errors: Vec<String>,
}

/// Each test gets its own GLib main context: tests run in parallel threads
/// and must not share the global default context.
fn record(audio_inputs: Vec<AudioInput>, output: &Path) -> Observed {
    let video = live("videotestsrc");
    video.set_property_from_str("pattern", "ball");
    record_with(video, audio_inputs, None, output)
}

fn record_with(
    video: gst::Element,
    audio_inputs: Vec<AudioInput>,
    camera: Option<CameraInput>,
    output: &Path,
) -> Observed {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| record_in_context(video, audio_inputs, camera, output, &context))
        .expect("acquire main context")
}

fn record_in_context(
    video: gst::Element,
    audio_inputs: Vec<AudioInput>,
    camera: Option<CameraInput>,
    output: &Path,
    context: &glib::MainContext,
) -> Observed {
    let config = PipelineConfig {
        video: VideoInput::Element(video),
        audio: (!audio_inputs.is_empty()).then(|| AudioConfig {
            inputs: audio_inputs,
            encoder: AacEncoderKind::detect().expect("AAC encoder"),
        }),
        camera,
        output: output.to_owned(),
        fps: 30,
        bits_per_pixel: 0.08,
        encoder: EncoderKind::detect(EncoderPreference::Auto).expect("H.264 encoder"),
    };

    let main_loop = glib::MainLoop::new(Some(context), false);
    let observed = Rc::new(RefCell::new(Observed::default()));
    let pipeline = {
        let observed = Rc::clone(&observed);
        let main_loop = main_loop.clone();
        RecordingPipeline::new(&config, move |event| {
            let mut o = observed.borrow_mut();
            match event {
                PipelineEvent::Started => o.started = true,
                PipelineEvent::AudioLevel(_) => o.levels += 1,
                PipelineEvent::Finalized => {
                    o.finalized = true;
                    main_loop.quit();
                }
                PipelineEvent::Error(e) => {
                    o.errors.push(e.to_string());
                    main_loop.quit();
                }
                PipelineEvent::SourceLost { source } => o.errors.push(format!("lost {source}")),
            }
        })
        .expect("pipeline builds")
    };
    let pipeline = Rc::new(pipeline);
    pipeline.start().expect("pipeline starts");

    // Timers must be attached to this test's context explicitly:
    // `timeout_add_local*` would use the global default context.
    let stop_timer = {
        let pipeline = Rc::clone(&pipeline);
        once_after(RECORD_FOR, context, move || pipeline.finish())
    };
    // Safety net so a hung pipeline fails the test instead of blocking CI.
    let safety_timer = {
        let main_loop = main_loop.clone();
        once_after(RECORD_FOR * 5, context, move || main_loop.quit())
    };
    main_loop.run();
    stop_timer.abort();
    safety_timer.abort();
    assert_eq!(pipeline.resolution(), Some((320, 240)));
    pipeline.abort();
    observed.take()
}

fn once_after(
    delay: Duration,
    context: &glib::MainContext,
    f: impl FnOnce() + 'static,
) -> glib::JoinHandle<()> {
    // `timeout_future` attaches to the thread-default context, which is
    // this test's context inside `with_thread_default`.
    context.spawn_local(async move {
        glib::timeout_future(delay).await;
        f();
    })
}

/// Returns (video seconds, audio seconds) by demuxing the file. Only the
/// tracks that exist are linked, so a missing track never blocks preroll.
fn track_durations(path: &Path) -> (Option<f64>, Option<f64>) {
    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().as_ref())
        .build()
        .expect("filesrc");
    let demux = gst::ElementFactory::make("qtdemux")
        .build()
        .expect("qtdemux");
    pipeline.add_many([&src, &demux]).expect("add");
    src.link(&demux).expect("link");

    let pipeline_weak = pipeline.downgrade();
    demux.connect_pad_added(move |_, pad| {
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .build()
            .expect("fakesink");
        pipeline.add(&sink).expect("add sink");
        let _ = sink.sync_state_with_parent();
        if let Some(sink_pad) = sink.static_pad("sink") {
            let _ = pad.link(&sink_pad);
        }
    });

    // With no sinks yet, PAUSED completes immediately; wait until qtdemux
    // has parsed the header and exposed every track.
    let (tx, rx) = std::sync::mpsc::channel();
    demux.connect_no_more_pads(move |_| {
        let _ = tx.send(());
    });
    pipeline.set_state(gst::State::Paused).expect("pause");
    rx.recv_timeout(Duration::from_secs(10))
        .expect("qtdemux exposed its tracks");
    let duration = |prefix: &str| {
        demux
            .src_pads()
            .into_iter()
            .find(|p| p.name().starts_with(prefix))
            .and_then(|p| p.query_duration::<gst::ClockTime>())
            .map(|d| d.seconds_f64())
    };
    let result = (duration("video"), duration("audio"));
    let channels = demux
        .src_pads()
        .into_iter()
        .find(|p| p.name().starts_with("audio"))
        .and_then(|p| p.current_caps())
        .and_then(|c| c.structure(0).and_then(|s| s.get::<i32>("channels").ok()));
    if let Some(channels) = channels {
        assert_eq!(channels, 2, "audio track must be stereo");
    }
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Decodes the audio track and returns its highest peak in dBFS.
fn decoded_peak_db(path: &Path) -> f64 {
    let pipeline = gst::parse::launch(&format!(
        "filesrc location=\"{}\" ! qtdemux name=d d.audio_0 ! decodebin ! audioconvert \
         ! level name=lvl post-messages=true ! fakesink sync=false",
        path.display()
    ))
    .expect("parse")
    .downcast::<gst::Pipeline>()
    .expect("pipeline");
    pipeline.set_state(gst::State::Playing).expect("play");
    let bus = pipeline.bus().expect("bus");
    let mut peak = f64::NEG_INFINITY;
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(20)) {
        match msg.view() {
            gst::MessageView::Element(e) => {
                let Some(s) = e.structure().filter(|s| s.name() == "level") else {
                    continue;
                };
                if let Ok(values) = s.get::<glib::ValueArray>("peak") {
                    for v in values.iter().filter_map(|v| v.get::<f64>().ok()) {
                        peak = peak.max(v);
                    }
                }
            }
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(err) => panic!("decode failed: {}", err.error()),
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    peak
}

fn gst_ready() -> bool {
    if gst::init().is_err() {
        eprintln!("skipping: GStreamer not available");
        return false;
    }
    true
}

#[test]
fn records_video_with_mixed_audio() {
    if !gst_ready() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("av.mp4");
    let observed = record(
        vec![audio_input(true, "sine"), audio_input(false, "pink-noise")],
        &output,
    );

    assert!(observed.errors.is_empty(), "errors: {:?}", observed.errors);
    assert!(observed.started && observed.finalized);
    assert!(observed.levels >= 10, "level messages: {}", observed.levels);

    let (video, audio) = track_durations(&output);
    let video = video.expect("video track");
    let audio = audio.expect("audio track");
    let expected = RECORD_FOR.as_secs_f64();
    assert!((video - expected).abs() < 0.6, "video lasted {video}s");
    assert!((audio - expected).abs() < 0.6, "audio lasted {audio}s");
    assert!(
        (video - audio).abs() < 0.3,
        "A/V mismatch: {video}s vs {audio}s"
    );
}

#[test]
fn records_video_only_without_audio_inputs() {
    if !gst_ready() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("v.mp4");
    let observed = record(Vec::new(), &output);

    assert!(observed.errors.is_empty(), "errors: {:?}", observed.errors);
    assert!(observed.finalized);
    assert_eq!(observed.levels, 0);
    let (video, audio) = track_durations(&output);
    assert!(video.is_some());
    assert!(audio.is_none());
}

#[test]
fn loud_mix_is_limited_below_full_scale() {
    if !gst_ready() {
        return;
    }
    let tone = |freq: f64| {
        let src = live("audiotestsrc");
        src.set_property("freq", freq);
        src.set_property("volume", 1.0f64);
        AudioInput {
            source: AudioSourceElement::Element(src),
            volume: 1.0,
            meter: false,
        }
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("loud.mp4");
    // Two full-scale tones sum to 2× full scale (+6 dBFS) before limiting.
    let observed = record(vec![tone(440.0), tone(660.0)], &output);
    assert!(observed.errors.is_empty(), "errors: {:?}", observed.errors);

    let peak = decoded_peak_db(&output);
    // Small allowance for AAC overshoot.
    assert!(peak < 0.3, "decoded peak {peak:.2} dBFS: the mix clips");
}

/// Solid-colour live source (videotestsrc defaults to 320×240).
fn solid(argb: u32) -> gst::Element {
    let src = live("videotestsrc");
    src.set_property_from_str("pattern", "solid-color");
    src.set_property("foreground-color", argb);
    src
}

/// Decodes the video track and returns the last frame as I420 planes.
struct Frame {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Frame {
    /// (Y, U, V) at pixel (x, y).
    fn yuv(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let (x, y) = (x as usize, y as usize);
        let (w, h) = (self.width, self.height);
        let u_plane = w * h;
        let v_plane = u_plane + (w / 2) * (h / 2);
        let chroma = (y / 2) * (w / 2) + x / 2;
        (
            self.data[y * w + x],
            self.data[u_plane + chroma],
            self.data[v_plane + chroma],
        )
    }
}

fn last_frame(path: &Path) -> Frame {
    use std::sync::{Arc, Mutex};
    let pipeline = gst::parse::launch(&format!(
        "filesrc location=\"{}\" ! qtdemux name=d d.video_0 ! decodebin ! videoconvert \
         ! video/x-raw,format=I420 ! fakesink name=sink signal-handoffs=true sync=false",
        path.display()
    ))
    .expect("parse")
    .downcast::<gst::Pipeline>()
    .expect("pipeline");
    type DecodedFrame = (gst::Caps, Vec<u8>);
    let last: Arc<Mutex<Option<DecodedFrame>>> = Arc::default();
    {
        let last = Arc::clone(&last);
        pipeline
            .by_name("sink")
            .expect("sink")
            .connect("handoff", false, move |args| {
                let buffer = args[1].get::<gst::Buffer>().ok()?;
                let pad = args[2].get::<gst::Pad>().ok()?;
                let caps = pad.current_caps()?;
                let map = buffer.map_readable().ok()?;
                if let Ok(mut slot) = last.lock() {
                    *slot = Some((caps, map.as_slice().to_vec()));
                }
                None
            });
    }
    pipeline.set_state(gst::State::Playing).expect("play");
    let bus = pipeline.bus().expect("bus");
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(20)) {
        match msg.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(err) => panic!("decode failed: {}", err.error()),
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    let (caps, data) = last.lock().expect("lock").take().expect("decoded a frame");
    let s = caps.structure(0).expect("caps structure");
    Frame {
        width: s.get::<i32>("width").expect("width") as usize,
        height: s.get::<i32>("height").expect("height") as usize,
        data,
    }
}

fn is_red(yuv: (u8, u8, u8)) -> bool {
    // BT.601 red ≈ (81, 90, 240); allow for compression.
    yuv.2 > 200 && yuv.1 < 130
}

fn is_green(yuv: (u8, u8, u8)) -> bool {
    // BT.601 green ≈ (145, 54, 34).
    yuv.0 > 110 && yuv.1 < 90 && yuv.2 < 80
}

fn record_camera(shape: Shape) -> Frame {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("camera.mp4");
    let layout = CameraLayout {
        corner: Corner::BottomRight,
        height_fraction: 0.25,
        shape,
        mirror: false,
    };
    let camera = CameraInput {
        source: CameraSource::Element(solid(0xff00ff00)),
        layout,
    };
    let observed = record_with(solid(0xffff0000), Vec::new(), Some(camera), &output);
    assert!(observed.errors.is_empty(), "errors: {:?}", observed.errors);
    assert!(observed.finalized);
    last_frame(&output)
}

#[test]
fn camera_rectangle_is_composited_bottom_right() {
    if !gst_ready() {
        return;
    }
    let frame = record_camera(Shape::Rectangle);
    assert_eq!((frame.width, frame.height), (320, 240));
    let layout = CameraLayout {
        corner: Corner::BottomRight,
        height_fraction: 0.25,
        shape: Shape::Rectangle,
        mirror: false,
    };
    let r = layout.bubble_rect(320, 240);
    let centre = frame.yuv(r.x + r.width / 2, r.y + r.height / 2);
    assert!(is_green(centre), "bubble centre {centre:?}");
    assert!(is_green(frame.yuv(r.x + 3, r.y + 3)), "rectangle corner");
    assert!(
        is_red(frame.yuv(20, 20)),
        "screen area {:?}",
        frame.yuv(20, 20)
    );
    assert!(
        is_red(frame.yuv(r.x - 6, r.y + r.height / 2)),
        "left of bubble"
    );
}

#[test]
fn camera_circle_masks_the_corners() {
    if !gst_ready() {
        return;
    }
    let frame = record_camera(Shape::Circle);
    let layout = CameraLayout {
        corner: Corner::BottomRight,
        height_fraction: 0.25,
        shape: Shape::Circle,
        mirror: false,
    };
    let r = layout.bubble_rect(320, 240);
    let centre = frame.yuv(r.x + r.width / 2, r.y + r.height / 2);
    assert!(is_green(centre), "bubble centre {centre:?}");
    // The square's corner lies outside the circle: the screen shows through.
    let corner = frame.yuv(r.x + 3, r.y + 3);
    assert!(
        is_red(corner),
        "circle corner should be transparent: {corner:?}"
    );
    assert!(is_red(frame.yuv(20, 20)));
}

/// Same composition with the real webcam through PipeWire. Needs a camera,
/// so it only runs on demand: `cargo test -- --ignored real_camera`.
#[test]
#[ignore = "needs a real camera"]
fn real_camera_composition() {
    if !gst_ready() {
        return;
    }
    for shape in [Shape::Rectangle, Shape::Circle] {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("real-camera.mp4");
        let camera = CameraInput {
            source: CameraSource::Element(
                gst::ElementFactory::make("pipewiresrc")
                    .build()
                    .expect("pipewiresrc"),
            ),
            layout: CameraLayout {
                corner: Corner::BottomRight,
                height_fraction: 0.25,
                shape,
                mirror: true,
            },
        };
        let observed = record_with(solid(0xffff0000), Vec::new(), Some(camera), &output);
        assert!(
            observed.errors.is_empty(),
            "{shape:?}: {:?}",
            observed.errors
        );
        assert!(observed.finalized, "{shape:?} not finalized");
        let (video, _) = track_durations(&output);
        assert!(video.is_some_and(|d| d > 2.0), "{shape:?}: video {video:?}");
    }
}
