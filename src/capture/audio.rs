//! Audio capture devices and the mapping from settings to audio inputs.
//!
//! Capture goes through the PulseAudio protocol served by `pipewire-pulse`.
//! That is the most mature path in GStreamer (clock slaving, latency
//! reporting) and is also how Flatpak apps get microphone access
//! (`--socket=pulseaudio`); PipeWire has no microphone portal yet.
//!
//! System audio is the *monitor* of the default output: on Linux there is no
//! Windows-style loopback API, PipeWire simply exposes every sink's output
//! as a capturable source.

use gst::prelude::*;

use crate::config::AudioSettings;
use crate::recording::audio::{AudioInput, AudioSourceElement};

/// PulseAudio's alias for the monitor of the default sink.
pub const DEFAULT_MONITOR: &str = "@DEFAULT_MONITOR@";

/// System audio gain when mixed with the microphone, so speech stays
/// intelligible over music or video (≈ −4.4 dB).
pub const SYSTEM_AUDIO_UNDER_VOICE: f64 = 0.6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// PipeWire `node.name`; also the PulseAudio source name.
    pub id: String,
    /// Human readable description.
    pub label: String,
}

/// Lists microphones (audio sources that are not monitors of an output).
pub fn list_microphones() -> Vec<AudioDevice> {
    if crate::recording::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Source"), None);
    if let Err(e) = monitor.start() {
        tracing::warn!(error = %e, "cannot start audio device monitor");
        return Vec::new();
    }
    let devices = monitor
        .devices()
        .into_iter()
        .filter_map(|device| {
            let props = device.properties()?;
            let id = props.get::<String>("node.name").ok()?;
            let is_monitor = props
                .get::<String>("device.class")
                .is_ok_and(|c| c == "monitor")
                || id.ends_with(".monitor");
            (!is_monitor).then(|| AudioDevice {
                id,
                label: device.display_name().to_string(),
            })
        })
        .collect();
    monitor.stop();
    devices
}

/// Translates settings into pipeline inputs. Pure, so it is unit-tested.
pub fn plan_inputs(settings: &AudioSettings) -> Vec<AudioInput> {
    let mut inputs = Vec::new();
    if settings.microphone {
        inputs.push(AudioInput {
            source: AudioSourceElement::Pulse {
                device: settings.microphone_device.clone(),
            },
            volume: settings.microphone_volume,
            meter: true,
        });
    }
    if settings.system_audio {
        inputs.push(AudioInput {
            source: AudioSourceElement::Pulse {
                device: Some(DEFAULT_MONITOR.to_owned()),
            },
            volume: if settings.microphone {
                SYSTEM_AUDIO_UNDER_VOICE
            } else {
                1.0
            },
            meter: false,
        });
    }
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(input: &AudioInput) -> Option<String> {
        match &input.source {
            AudioSourceElement::Pulse { device } => device.clone(),
            AudioSourceElement::Element(_) => panic!("unexpected element source"),
        }
    }

    #[test]
    fn nothing_enabled_means_no_audio() {
        let s = AudioSettings {
            microphone: false,
            system_audio: false,
            ..AudioSettings::default()
        };
        assert!(plan_inputs(&s).is_empty());
    }

    #[test]
    fn default_microphone() {
        let inputs = plan_inputs(&AudioSettings::default());
        assert_eq!(inputs.len(), 1);
        assert_eq!(device(&inputs[0]), None);
        assert!(inputs[0].meter);
    }

    #[test]
    fn microphone_and_system_audio() {
        let s = AudioSettings {
            microphone: true,
            microphone_device: Some("alsa_input.usb".into()),
            microphone_volume: 1.5,
            system_audio: true,
        };
        let inputs = plan_inputs(&s);
        assert_eq!(inputs.len(), 2);
        assert_eq!(device(&inputs[0]).as_deref(), Some("alsa_input.usb"));
        assert_eq!(inputs[0].volume, 1.5);
        assert_eq!(device(&inputs[1]).as_deref(), Some(DEFAULT_MONITOR));
        assert_eq!(inputs[1].volume, SYSTEM_AUDIO_UNDER_VOICE);
        assert!(!inputs[1].meter);
    }

    #[test]
    fn system_audio_alone_keeps_full_volume() {
        let s = AudioSettings {
            microphone: false,
            system_audio: true,
            ..AudioSettings::default()
        };
        let inputs = plan_inputs(&s);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].volume, 1.0);
    }
}
