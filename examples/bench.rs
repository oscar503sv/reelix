//! Reproducible recording benchmark: runs the production `RecordingPipeline`
//! with synthetic live sources and reports CPU, peak RSS, dropped frames,
//! encode latency and real bitrate.
//!
//! ```bash
//! cargo run --release --example bench -- --seconds 10 --encoder software
//! cargo run --release --example bench -- --camera circle --audio
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use gst::glib;
use gst::prelude::*;
use reelix::config::{EncoderPreference, Quality};
use reelix::recording::audio::{AacEncoderKind, AudioInput, AudioSourceElement};
use reelix::recording::camera::{CameraInput, CameraLayout, CameraSource, Corner, Shape};
use reelix::recording::encoder::target_bitrate;
use reelix::recording::pipeline::output_size;
use reelix::recording::{
    AudioConfig, EncoderKind, PipelineConfig, PipelineEvent, RecordingPipeline, VideoInput,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EncoderArg {
    Auto,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CameraArg {
    None,
    Rectangle,
    Circle,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum QualityArg {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Content {
    /// Small moving object on a static background (typical screen).
    Screen,
    /// Every pixel changes every frame (worst case).
    Motion,
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    #[arg(long, value_enum, default_value_t = EncoderArg::Auto)]
    encoder: EncoderArg,
    #[arg(long, value_enum, default_value_t = CameraArg::None)]
    camera: CameraArg,
    #[arg(long)]
    audio: bool,
    #[arg(long, value_enum, default_value_t = QualityArg::Medium)]
    quality: QualityArg,
    #[arg(long, value_enum, default_value_t = Content::Screen)]
    content: Content,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    /// Downscale the 1080p source to this height (e.g. 720).
    #[arg(long)]
    max_height: Option<u32>,
}

/// A live test source with fixed caps, wrapped in a bin with one src pad.
fn live_source(factory: &str, caps: &str, configure: impl FnOnce(&gst::Element)) -> gst::Element {
    let bin = gst::Bin::new();
    let src = gst::ElementFactory::make(factory)
        .property("is-live", true)
        .build()
        .expect("test source");
    configure(&src);
    let filter = gst::ElementFactory::make("capsfilter")
        .property("caps", caps.parse::<gst::Caps>().expect("caps"))
        .build()
        .expect("capsfilter");
    bin.add_many([&src, &filter]).expect("add");
    src.link(&filter).expect("link");
    let pad = filter.static_pad("src").expect("src pad");
    bin.add_pad(&gst::GhostPad::with_target(&pad).expect("ghost"))
        .expect("add pad");
    bin.upcast()
}

/// (user + system CPU seconds, peak RSS in KiB) of this process.
fn process_usage() -> (f64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after the parenthesised command name; utime/stime are 14/15.
    let after = stat.rsplit(')').next().unwrap_or("");
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| {
        fields
            .get(i)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    // USER_HZ is 100 on Linux x86_64; indices are offset by the two fields
    // before the parenthesis.
    let cpu = (ticks(11) + ticks(12)) / 100.0;
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let peak = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    (cpu, peak)
}

fn main() {
    let args = Args::parse();
    gst::init().expect("GStreamer");

    let quality = match args.quality {
        QualityArg::Low => Quality::Low,
        QualityArg::Medium => Quality::Medium,
        QualityArg::High => Quality::High,
    };
    let encoder = EncoderKind::detect(match args.encoder {
        EncoderArg::Auto => EncoderPreference::Auto,
        EncoderArg::Software => EncoderPreference::Software,
        EncoderArg::Hardware => EncoderPreference::Hardware,
    })
    .expect("encoder");

    let pattern = match args.content {
        Content::Screen => "ball",
        Content::Motion => "zone-plate",
    };
    let screen = live_source(
        "videotestsrc",
        &format!(
            "video/x-raw,format=BGRx,width=1920,height=1080,framerate={}/1",
            args.fps
        ),
        |s| {
            s.set_property_from_str("pattern", pattern);
            if matches!(args.content, Content::Motion) {
                s.set_property("kt", 2i32);
            }
        },
    );
    let camera = match args.camera {
        CameraArg::None => None,
        shape => Some(CameraInput {
            source: CameraSource::Element(live_source(
                "videotestsrc",
                "video/x-raw,format=YUY2,width=640,height=480,framerate=30/1",
                |s| s.set_property_from_str("pattern", "ball"),
            )),
            layout: CameraLayout {
                corner: Corner::BottomRight,
                height_fraction: 0.25,
                shape: if matches!(shape, CameraArg::Circle) {
                    Shape::Circle
                } else {
                    Shape::Rectangle
                },
                mirror: false,
            },
        }),
    };
    let audio = args.audio.then(|| AudioConfig {
        inputs: vec![AudioInput {
            source: AudioSourceElement::Element(
                gst::ElementFactory::make("audiotestsrc")
                    .property("is-live", true)
                    .build()
                    .expect("audiotestsrc"),
            ),
            volume: 1.0,
            meter: true,
        }],
        encoder: AacEncoderKind::detect().expect("AAC encoder"),
    });

    let dir = std::env::temp_dir().join(format!("reelix-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let output = dir.join("bench.mp4");
    let config = PipelineConfig {
        video: VideoInput::Element(screen),
        audio,
        camera,
        output: output.clone(),
        fps: args.fps,
        bits_per_pixel: quality.bits_per_pixel(),
        encoder,
        max_height: args.max_height,
    };

    let main_loop = glib::MainLoop::new(None, false);
    let finalized = Rc::new(Cell::new(false));
    let pipeline = {
        let main_loop = main_loop.clone();
        let finalized = Rc::clone(&finalized);
        Rc::new(
            RecordingPipeline::new(&config, move |event| match event {
                PipelineEvent::Finalized => {
                    finalized.set(true);
                    main_loop.quit();
                }
                PipelineEvent::Error(e) => {
                    eprintln!("error: {e}");
                    main_loop.quit();
                }
                _ => {}
            })
            .expect("pipeline"),
        )
    };

    let (cpu_before, _) = process_usage();
    let started = Instant::now();
    pipeline.start().expect("start");
    {
        let pipeline = Rc::clone(&pipeline);
        glib::timeout_add_local_once(Duration::from_secs(args.seconds), move || pipeline.finish());
    }
    {
        let main_loop = main_loop.clone();
        glib::timeout_add_local_once(Duration::from_secs(args.seconds + 20), move || {
            main_loop.quit()
        });
    }
    main_loop.run();
    let wall = started.elapsed().as_secs_f64();
    let (cpu_after, peak_rss) = process_usage();
    let stats = pipeline.stats();
    pipeline.abort();

    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let bitrate = size as f64 * 8.0 / args.seconds as f64 / 1e6;
    let (out_w, out_h) = output_size(1920, 1080, args.max_height);
    let target = f64::from(target_bitrate(
        out_w,
        out_h,
        args.fps,
        quality.bits_per_pixel(),
    )) / 1e6;
    println!(
        "encoder={encoder} camera={:?} audio={} quality={:?} content={:?} fps={} output={out_w}x{out_h}",
        args.camera, args.audio, args.quality, args.content, args.fps
    );
    println!(
        "  CPU {:5.1}% of one core | peak RSS {:4} MB | frames {} (dropped {}, {:.1}%)",
        (cpu_after - cpu_before) / wall * 100.0,
        peak_rss / 1024,
        stats.encoded,
        stats.dropped,
        stats.drop_ratio() * 100.0
    );
    println!(
        "  encode latency avg {:.1} ms / max {:.1} ms | bitrate {:.2} Mbit/s (target {:.2}) | finalized {}",
        stats.avg_latency.as_secs_f64() * 1000.0,
        stats.max_latency.as_secs_f64() * 1000.0,
        bitrate,
        target,
        finalized.get()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
