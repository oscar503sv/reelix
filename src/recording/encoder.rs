//! H.264 encoder selection.
//!
//! Encoders are probed at runtime from the GStreamer registry, so installing
//! a hardware driver (e.g. RPM Fusion's `intel-media-driver`, which exposes
//! `vah264lpenc`) is picked up without rebuilding.

use std::fmt;

use super::props::{try_set, try_set_enum};
use crate::config::EncoderPreference;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    /// VA-API low-power (fixed function) encoder: lowest CPU/power on Intel.
    VaH264Lp,
    /// VA-API encoder using the GPU's shader/VME path.
    VaH264,
    X264,
    OpenH264,
}

impl EncoderKind {
    /// Preference order: fastest/cheapest first.
    pub const ALL: [EncoderKind; 4] = [Self::VaH264Lp, Self::VaH264, Self::X264, Self::OpenH264];

    pub fn factory_name(self) -> &'static str {
        match self {
            Self::VaH264Lp => "vah264lpenc",
            Self::VaH264 => "vah264enc",
            Self::X264 => "x264enc",
            Self::OpenH264 => "openh264enc",
        }
    }

    pub fn is_hardware(self) -> bool {
        matches!(self, Self::VaH264Lp | Self::VaH264)
    }

    /// Raw format the encoder consumes most efficiently. Hardware encoders
    /// take NV12; the software ones want planar I420.
    pub fn preferred_format(self) -> &'static str {
        if self.is_hardware() { "NV12" } else { "I420" }
    }

    /// Picks the first encoder allowed by `pref` for which `is_available`
    /// returns true. Kept pure so it can be tested without GStreamer.
    pub fn choose(
        pref: EncoderPreference,
        is_available: impl Fn(&str) -> bool,
    ) -> Result<EncoderKind> {
        let allowed = |k: &EncoderKind| match pref {
            EncoderPreference::Auto => true,
            EncoderPreference::Hardware => k.is_hardware(),
            EncoderPreference::Software => !k.is_hardware(),
        };
        let candidates: Vec<EncoderKind> = Self::ALL.into_iter().filter(allowed).collect();
        let chosen = candidates
            .iter()
            .copied()
            .find(|k| is_available(k.factory_name()));

        match (chosen, pref) {
            (Some(kind), _) => Ok(kind),
            // An explicit hardware request falls back to software rather than
            // making recording impossible.
            (None, EncoderPreference::Hardware) => {
                tracing::warn!("no hardware H.264 encoder available, falling back to software");
                Self::choose(EncoderPreference::Software, is_available)
            }
            (None, _) => Err(AppError::NoEncoder(
                candidates
                    .iter()
                    .map(|k| k.factory_name())
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
        }
    }

    /// Probes the GStreamer registry.
    pub fn detect(pref: EncoderPreference) -> Result<EncoderKind> {
        super::init()?;
        Self::choose(pref, |name| gst::ElementFactory::find(name).is_some())
    }

    /// Sets the target bitrate, converting to the unit each encoder expects.
    /// Ignored by encoders running in constant-quality mode.
    pub fn set_bitrate(self, element: &gst::Element, bitrate_bps: u32) {
        let applied = match self {
            // These take kbit/s.
            Self::VaH264Lp | Self::VaH264 | Self::X264 => {
                try_set(element, "bitrate", (bitrate_bps / 1000).max(500))
            }
            Self::OpenH264 => try_set(element, "bitrate", bitrate_bps),
        };
        if applied {
            tracing::debug!(encoder = %self, bitrate_bps, "encoder bitrate set");
        }
    }

    /// Creates and configures the encoder element for screen content.
    /// Every setting is optional: unsupported ones are logged and skipped.
    pub fn build(self, bitrate_bps: u32, fps: u32, bits_per_pixel: f64) -> Result<gst::Element> {
        let element = gst::ElementFactory::make(self.factory_name())
            .name("encoder")
            .build()
            .map_err(|_| AppError::MissingElement(self.factory_name().into()))?;
        // One keyframe every 2 s keeps seeking snappy.
        let gop = fps * 2;

        match self {
            Self::VaH264Lp | Self::VaH264 => {
                // Intel's low-power encoder only offers constant QP on many
                // GPUs; use VBR where available, else QP from the quality.
                if try_set_enum(&element, "rate-control", "vbr") {
                    self.set_bitrate(&element, bitrate_bps);
                } else if try_set_enum(&element, "rate-control", "cqp") {
                    let qp = qp_for(bits_per_pixel);
                    try_set(&element, "qpi", qp);
                    try_set(&element, "qpp", qp);
                    tracing::info!(encoder = %self, qp, "constant-QP rate control");
                }
                try_set(&element, "key-int-max", gop);
                // No reordering: lower latency, and stats match frames by PTS.
                try_set(&element, "b-frames", 0u32);
            }
            Self::X264 => {
                self.set_bitrate(&element, bitrate_bps);
                try_set_enum(&element, "speed-preset", "veryfast");
                try_set_enum(&element, "tune", "zerolatency");
                try_set(&element, "key-int-max", gop);
            }
            Self::OpenH264 => {
                self.set_bitrate(&element, bitrate_bps);
                try_set_enum(&element, "rate-control", "bitrate");
                try_set_enum(&element, "usage-type", "screen");
                try_set_enum(&element, "complexity", "low");
                // 0 = let openh264 pick based on the number of cores.
                try_set(&element, "multi-thread", 0u32);
                try_set(&element, "gop-size", gop);
            }
        }
        Ok(element)
    }
}

/// H.264 quantizer for constant-QP mode from the quality's bits per pixel:
/// Low (0.04) → 30, Medium (0.08) → 26, High (0.15) → 22.
pub fn qp_for(bits_per_pixel: f64) -> u32 {
    match bits_per_pixel {
        b if b < 0.06 => 30,
        b if b < 0.12 => 26,
        _ => 22,
    }
}

impl fmt::Display for EncoderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.factory_name())
    }
}

/// Target bitrate for the given resolution, frame rate and bits-per-pixel.
pub fn target_bitrate(width: u32, height: u32, fps: u32, bits_per_pixel: f64) -> u32 {
    let bps = f64::from(width) * f64::from(height) * f64::from(fps) * bits_per_pixel;
    bps.clamp(1_000_000.0, 50_000_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_hardware() {
        let k = EncoderKind::choose(EncoderPreference::Auto, |_| true).expect("some");
        assert_eq!(k, EncoderKind::VaH264Lp);
    }

    #[test]
    fn auto_falls_back_to_openh264_on_stock_fedora() {
        let k = EncoderKind::choose(EncoderPreference::Auto, |n| n == "openh264enc").expect("some");
        assert_eq!(k, EncoderKind::OpenH264);
    }

    #[test]
    fn software_skips_hardware() {
        let k = EncoderKind::choose(EncoderPreference::Software, |_| true).expect("some");
        assert_eq!(k, EncoderKind::X264);
    }

    #[test]
    fn hardware_request_falls_back_to_software() {
        let k = EncoderKind::choose(EncoderPreference::Hardware, |n| n == "openh264enc")
            .expect("fallback");
        assert_eq!(k, EncoderKind::OpenH264);
    }

    #[test]
    fn nothing_available_is_an_error() {
        assert!(matches!(
            EncoderKind::choose(EncoderPreference::Auto, |_| false),
            Err(AppError::NoEncoder(_))
        ));
    }

    #[test]
    fn qp_follows_quality() {
        use crate::config::Quality;
        assert_eq!(qp_for(Quality::Low.bits_per_pixel()), 30);
        assert_eq!(qp_for(Quality::Medium.bits_per_pixel()), 26);
        assert_eq!(qp_for(Quality::High.bits_per_pixel()), 22);
    }

    #[test]
    fn bitrate_is_clamped() {
        assert_eq!(target_bitrate(1920, 1080, 30, 0.08), 4_976_640);
        assert_eq!(target_bitrate(10, 10, 5, 0.04), 1_000_000);
        assert_eq!(target_bitrate(7680, 4320, 60, 1.0), 50_000_000);
    }
}
