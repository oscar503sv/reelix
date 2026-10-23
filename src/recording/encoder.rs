//! H.264 encoder selection.
//!
//! Encoders are probed at runtime from the GStreamer registry, so installing
//! a hardware driver (e.g. RPM Fusion's `intel-media-driver`, which exposes
//! `vah264lpenc`) is picked up without rebuilding.

use std::fmt;

use gst::prelude::*;

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
    pub fn set_bitrate(self, element: &gst::Element, bitrate_bps: u32) {
        match self {
            // These take kbit/s.
            Self::VaH264Lp | Self::VaH264 | Self::X264 => {
                element.set_property("bitrate", (bitrate_bps / 1000).max(500));
            }
            Self::OpenH264 => element.set_property("bitrate", bitrate_bps),
        }
        tracing::debug!(encoder = %self, bitrate_bps, "encoder bitrate set");
    }

    /// Creates and configures the encoder element for screen content.
    pub fn build(self, bitrate_bps: u32, fps: u32) -> Result<gst::Element> {
        let element = gst::ElementFactory::make(self.factory_name())
            .name("encoder")
            .build()
            .map_err(|_| AppError::MissingElement(self.factory_name().into()))?;
        self.set_bitrate(&element, bitrate_bps);

        match self {
            Self::VaH264Lp | Self::VaH264 => {
                element.set_property_from_str("rate-control", "vbr");
                // One keyframe every 2 s keeps seeking snappy.
                element.set_property("key-int-max", fps * 2);
            }
            Self::X264 => {
                element.set_property_from_str("speed-preset", "veryfast");
                element.set_property_from_str("tune", "zerolatency");
                element.set_property("key-int-max", fps * 2);
            }
            Self::OpenH264 => {
                element.set_property_from_str("rate-control", "bitrate");
                element.set_property_from_str("usage-type", "screen");
                element.set_property_from_str("complexity", "low");
                // 0 = let openh264 pick based on the number of cores.
                element.set_property("multi-thread", 0u32);
                element.set_property("gop-size", fps * 2);
            }
        }
        Ok(element)
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
    fn bitrate_is_clamped() {
        assert_eq!(target_bitrate(1920, 1080, 30, 0.08), 4_976_640);
        assert_eq!(target_bitrate(10, 10, 5, 0.04), 1_000_000);
        assert_eq!(target_bitrate(7680, 4320, 60, 1.0), 50_000_000);
    }
}
