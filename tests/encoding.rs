//! End-to-end encode test: the same encoder configuration the app uses,
//! fed by `videotestsrc` instead of a portal stream, must produce a valid
//! MP4. Skipped (with a message) if GStreamer or an encoder is missing.

use gst::prelude::*;
use reelix::config::EncoderPreference;
use reelix::recording::EncoderKind;
use reelix::recording::encoder::target_bitrate;

fn encode_test_clip(kind: EncoderKind, output: &std::path::Path) -> Result<(), String> {
    let pipeline = gst::Pipeline::new();
    let make = |f: &str| {
        gst::ElementFactory::make(f)
            .build()
            .map_err(|e| e.to_string())
    };

    let src = make("videotestsrc")?;
    src.set_property("num-buffers", 60i32);
    let src_caps = make("capsfilter")?;
    // GNOME's screen cast delivers BGRx; start from the same format.
    src_caps.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "BGRx")
            .field("width", 1280i32)
            .field("height", 720i32)
            .field("framerate", gst::Fraction::new(30, 1))
            .build(),
    );
    let convert = make("videoconvert")?;
    let fmt_caps = make("capsfilter")?;
    fmt_caps.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", kind.preferred_format())
            .build(),
    );
    let encoder = kind
        .build(target_bitrate(1280, 720, 30, 0.08), 30, 0.08)
        .map_err(|e| e.to_string())?;
    let parse = make("h264parse")?;
    let mux = make("mp4mux")?;
    let sink = make("filesink")?;
    sink.set_property("location", output.to_string_lossy().as_ref());

    let chain = [
        &src, &src_caps, &convert, &fmt_caps, &encoder, &parse, &mux, &sink,
    ];
    pipeline.add_many(chain).map_err(|e| e.to_string())?;
    gst::Element::link_many(chain).map_err(|e| e.to_string())?;

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| e.to_string())?;
    let bus = pipeline.bus().ok_or("no bus")?;
    let msg = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(30),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let result = match msg.as_ref().map(|m| m.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(err)) => Err(format!("{} ({:?})", err.error(), err.debug())),
        _ => Err("timed out waiting for EOS".into()),
    };
    let _ = pipeline.set_state(gst::State::Null);
    result
}

#[test]
fn detected_encoder_produces_playable_mp4() {
    if gst::init().is_err() {
        eprintln!("skipping: GStreamer not available");
        return;
    }
    let kind = match EncoderKind::detect(EncoderPreference::Auto) {
        Ok(kind) => kind,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("clip.mp4");
    encode_test_clip(kind, &output).expect("encode");

    // Verify the file by demuxing it.
    let caps = demuxed_video_caps(&output).expect("qtdemux should parse the generated file");
    assert!(caps.starts_with("video/x-h264"), "unexpected caps: {caps}");
}

/// Demuxes the file and reports the caps of its first video track.
fn demuxed_video_caps(path: &std::path::Path) -> Result<String, String> {
    let pipeline = gst::parse::launch(&format!(
        "filesrc location=\"{}\" ! qtdemux name=d d.video_0 ! fakesink name=sink",
        path.display()
    ))
    .map_err(|e| e.to_string())?
    .downcast::<gst::Pipeline>()
    .map_err(|_| "not a pipeline".to_string())?;
    let sink = pipeline.by_name("sink").ok_or("no sink")?;
    let pad = sink.static_pad("sink").ok_or("no pad")?;
    pipeline
        .set_state(gst::State::Paused)
        .map_err(|e| e.to_string())?;
    let _ = pipeline.state(gst::ClockTime::from_seconds(10));
    let caps = pad.current_caps().map(|c| c.to_string());
    let _ = pipeline.set_state(gst::State::Null);
    caps.ok_or_else(|| "no caps negotiated".into())
}
