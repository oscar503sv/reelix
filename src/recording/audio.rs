//! Audio branch of the recording pipeline.
//!
//! ```text
//! source₀ → audioconvert → audioresample → 48 kHz stereo → volume [→ level] ─┐
//! source₁ → …                                                               ├→ audiomixer ─┐
//!                                       (only when there is more than one) ─┘              │
//!              limiter (hard-knee) ← ──────────────────────────────────────────────────────┘
//!                → queue (bounded, leaky) → AAC encoder → mp4mux
//! ```

use std::fmt;

use gst::prelude::*;

use crate::branding;
use crate::error::{AppError, Result};

pub const AUDIO_SOURCE_PREFIX: &str = "audio-source";
pub const AUDIO_QUEUE_NAME: &str = "audio-queue";
pub const LEVEL_NAME: &str = "mic-level";

const SAMPLE_RATE: i32 = 48_000;
const CHANNELS: i32 = 2;
const AAC_BITRATE: u32 = 128_000;
/// Limiter: samples above this linear level are compressed…
const LIMITER_THRESHOLD: f32 = 0.6;
/// …by this ratio. A mix peaking at 3× full scale still ends below 0 dBFS:
/// 0.6 + (3.0 − 0.6) × 0.15 = 0.96. Tuned on a real mic + system recording:
/// peak +6.0 → −1.8 dBFS with RMS only −0.4 dB.
const LIMITER_RATIO: f32 = 0.15;

/// Where an audio stream comes from.
#[derive(Debug, Clone)]
pub enum AudioSourceElement {
    /// A PulseAudio-protocol source (served by `pipewire-pulse`). `None` is
    /// the default input; `@DEFAULT_MONITOR@` captures what the speakers play.
    Pulse { device: Option<String> },
    /// Any live element producing raw audio (tests, future sources).
    Element(gst::Element),
}

#[derive(Debug, Clone)]
pub struct AudioInput {
    pub source: AudioSourceElement,
    /// Linear gain, 1.0 = unchanged.
    pub volume: f64,
    /// Post level messages for this input (drives the UI VU meter).
    pub meter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AacEncoderKind {
    /// Fraunhofer FDK AAC (`fdk-aac-free` in Fedora): best quality.
    Fdk,
    /// FFmpeg's native AAC encoder (`gstreamer1-plugin-libav`).
    Libav,
    VoAac,
}

impl AacEncoderKind {
    pub const ALL: [AacEncoderKind; 3] = [Self::Fdk, Self::Libav, Self::VoAac];

    pub fn factory_name(self) -> &'static str {
        match self {
            Self::Fdk => "fdkaacenc",
            Self::Libav => "avenc_aac",
            Self::VoAac => "voaacenc",
        }
    }

    pub fn choose(is_available: impl Fn(&str) -> bool) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|k| is_available(k.factory_name()))
            .ok_or_else(|| {
                AppError::NoAudioEncoder(
                    Self::ALL
                        .iter()
                        .map(|k| k.factory_name())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            })
    }

    pub fn detect() -> Result<Self> {
        super::init()?;
        Self::choose(|name| gst::ElementFactory::find(name).is_some())
    }

    fn build(self) -> Result<gst::Element> {
        let element = make(self.factory_name(), "audio-encoder")?;
        // The property type differs between encoders (int vs int64); parsing
        // from a string works for all of them.
        element.set_property_from_str("bitrate", &AAC_BITRATE.to_string());
        Ok(element)
    }
}

impl fmt::Display for AacEncoderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.factory_name())
    }
}

/// Builds the audio branch inside `pipeline` and links it to `mux`.
/// Returns the names of the created source elements.
pub fn build_branch(
    pipeline: &gst::Pipeline,
    inputs: &[AudioInput],
    encoder: AacEncoderKind,
    mux: &gst::Element,
) -> Result<Vec<String>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }

    let queue = make("queue", AUDIO_QUEUE_NAME)?;
    queue.set_property("max-size-buffers", 0u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", gst::ClockTime::SECOND.nseconds());
    queue.set_property_from_str("leaky", "downstream");
    let aac = encoder.build()?;
    // Loud speech, or speech mixed with system audio, easily exceeds full
    // scale; a compressor keeps peaks from clipping on playback while
    // leaving normal levels untouched. Hard knee on purpose: audiodynamic's
    // soft-knee curve does not compress samples that are already above 1.0.
    let limiter = make("audiodynamic", "audio-limiter")?;
    limiter.set_property_from_str("mode", "compressor");
    limiter.set_property_from_str("characteristics", "hard-knee");
    limiter.set_property("threshold", LIMITER_THRESHOLD);
    limiter.set_property("ratio", LIMITER_RATIO);
    add_and_link(pipeline, &[&limiter, &queue, &aac])?;
    aac.link(mux)
        .map_err(|e| AppError::Pipeline(format!("cannot link audio to muxer: {e}")))?;

    let mixer = if inputs.len() > 1 {
        let mixer = make("audiomixer", "audio-mixer")?;
        // audiomixer negotiates its output with the encoder (which would pick
        // mono) and converts inputs to match; pin the output to stereo.
        let mixer_caps = make("capsfilter", "audio-mixer-caps")?;
        mixer_caps.set_property("caps", raw_audio_caps());
        pipeline
            .add_many([&mixer, &mixer_caps])
            .map_err(|e| AppError::Pipeline(e.to_string()))?;
        gst::Element::link_many([&mixer, &mixer_caps, &limiter])
            .map_err(|e| AppError::Pipeline(format!("cannot link audio mixer: {e}")))?;
        Some(mixer)
    } else {
        None
    };

    let mut names = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let name = format!("{AUDIO_SOURCE_PREFIX}-{index}");
        let source = make_source(&input.source, &name)?;

        let convert = make("audioconvert", &format!("audio-convert-{index}"))?;
        let resample = make("audioresample", &format!("audio-resample-{index}"))?;
        let caps = make("capsfilter", &format!("audio-caps-{index}"))?;
        caps.set_property("caps", raw_audio_caps());
        let volume = make("volume", &format!("audio-volume-{index}"))?;
        volume.set_property("volume", input.volume.clamp(0.0, 4.0));

        let mut chain = vec![source, convert, resample, caps, volume];
        if input.meter {
            let level = make("level", LEVEL_NAME)?;
            level.set_property("interval", 100 * gst::ClockTime::MSECOND.nseconds());
            level.set_property("post-messages", true);
            chain.push(level);
        }
        let refs: Vec<&gst::Element> = chain.iter().collect();
        add_and_link(pipeline, &refs)?;

        let downstream = mixer.as_ref().unwrap_or(&limiter);
        if let Some(last) = chain.last() {
            last.link(downstream)
                .map_err(|e| AppError::Pipeline(format!("cannot link audio input {index}: {e}")))?;
        }
        names.push(name);
    }

    tracing::info!(inputs = inputs.len(), encoder = %encoder, "audio branch built");
    Ok(names)
}

fn raw_audio_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("rate", SAMPLE_RATE)
        .field("channels", CHANNELS)
        .build()
}

/// Extracts a 0.0–1.0 loudness value from a `level` element message.
pub fn parse_level(structure: &gst::StructureRef) -> Option<f64> {
    if structure.name() != "level" {
        return None;
    }
    let rms = structure.get::<gst::glib::ValueArray>("rms").ok()?;
    let loudest_db = rms
        .iter()
        .filter_map(|v| v.get::<f64>().ok())
        .fold(f64::NEG_INFINITY, f64::max);
    Some(db_to_meter(loudest_db))
}

/// Maps −60 dB…0 dB to 0.0…1.0 (anything quieter is silence).
pub fn db_to_meter(db: f64) -> f64 {
    if db.is_finite() {
        ((db + 60.0) / 60.0).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn make_source(source: &AudioSourceElement, name: &str) -> Result<gst::Element> {
    match source {
        AudioSourceElement::Pulse { device } => {
            let src = make("pulsesrc", name)?;
            if let Some(device) = device {
                src.set_property("device", device);
            }
            src.set_property("client-name", branding::APP_NAME);
            // The pipeline runs on the system clock; the source slaves to it
            // so audio and video never drift apart.
            src.set_property("provide-clock", false);
            Ok(src)
        }
        AudioSourceElement::Element(element) => {
            element.set_property("name", name);
            Ok(element.clone())
        }
    }
}

fn make(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|_| AppError::MissingElement(factory.to_owned()))
}

fn add_and_link(pipeline: &gst::Pipeline, chain: &[&gst::Element]) -> Result<()> {
    pipeline
        .add_many(chain.iter().copied())
        .map_err(|e| AppError::Pipeline(e.to_string()))?;
    gst::Element::link_many(chain.iter().copied()).map_err(|e| AppError::Pipeline(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_fdk() {
        assert_eq!(
            AacEncoderKind::choose(|_| true).expect("some"),
            AacEncoderKind::Fdk
        );
    }

    #[test]
    fn falls_back_to_libav() {
        let k = AacEncoderKind::choose(|n| n == "avenc_aac").expect("some");
        assert_eq!(k, AacEncoderKind::Libav);
    }

    #[test]
    fn no_aac_encoder_is_an_error() {
        assert!(matches!(
            AacEncoderKind::choose(|_| false),
            Err(AppError::NoAudioEncoder(_))
        ));
    }

    #[test]
    fn meter_mapping() {
        assert_eq!(db_to_meter(0.0), 1.0);
        assert_eq!(db_to_meter(-30.0), 0.5);
        assert_eq!(db_to_meter(-90.0), 0.0);
        assert_eq!(db_to_meter(f64::NEG_INFINITY), 0.0);
        assert_eq!(db_to_meter(6.0), 1.0);
    }
}
