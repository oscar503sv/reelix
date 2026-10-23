//! Audio options shown on the main window: microphone on/off and device,
//! system audio. Changes are saved immediately and apply to the next
//! recording.

use std::rc::Rc;

use adw::prelude::*;

use crate::app::Controller;
use crate::capture::audio::{AudioDevice, list_microphones};

pub struct AudioPanel {
    group: adw::PreferencesGroup,
}

impl AudioPanel {
    pub fn new(controller: &Rc<Controller>) -> Self {
        let settings = controller.settings().audio;
        let devices = list_microphones();

        let microphone = adw::SwitchRow::builder()
            .title("Micrófono")
            .active(settings.microphone)
            .build();
        microphone.add_prefix(&gtk::Image::from_icon_name(
            "audio-input-microphone-symbolic",
        ));

        let labels: Vec<String> = std::iter::once("Predeterminado del sistema".to_owned())
            .chain(devices.iter().map(|d| d.label.clone()))
            .collect();
        let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
        let device_row = adw::ComboRow::builder()
            .title("Dispositivo")
            .model(&model)
            .selected(selected_index(
                &devices,
                settings.microphone_device.as_deref(),
            ))
            .sensitive(settings.microphone)
            .build();

        let system_audio = adw::SwitchRow::builder()
            .title("Audio del sistema")
            .subtitle("Lo que suena en tus altavoces o auriculares")
            .active(settings.system_audio)
            .build();
        system_audio.add_prefix(&gtk::Image::from_icon_name("audio-speakers-symbolic"));

        let group = adw::PreferencesGroup::builder()
            .title("Audio")
            .margin_bottom(24)
            .build();
        group.add(&microphone);
        group.add(&device_row);
        group.add(&system_audio);

        {
            let controller = Rc::clone(controller);
            let device_row = device_row.clone();
            microphone.connect_active_notify(move |row| {
                let enabled = row.is_active();
                device_row.set_sensitive(enabled);
                controller.update_settings(|s| s.audio.microphone = enabled);
            });
        }
        {
            let controller = Rc::clone(controller);
            device_row.connect_selected_notify(move |row| {
                // Index 0 is "system default"; the rest map onto `devices`.
                let device = (row.selected() as usize)
                    .checked_sub(1)
                    .and_then(|i| devices.get(i))
                    .map(|d| d.id.clone());
                tracing::info!(?device, "microphone selected");
                controller.update_settings(|s| s.audio.microphone_device = device);
            });
        }
        {
            let controller = Rc::clone(controller);
            system_audio.connect_active_notify(move |row| {
                let enabled = row.is_active();
                controller.update_settings(|s| s.audio.system_audio = enabled);
            });
        }

        Self { group }
    }

    pub fn widget(&self) -> &adw::PreferencesGroup {
        &self.group
    }

    /// Audio options cannot change mid-recording.
    pub fn set_sensitive(&self, sensitive: bool) {
        self.group.set_sensitive(sensitive);
    }
}

fn selected_index(devices: &[AudioDevice], wanted: Option<&str>) -> u32 {
    wanted
        .and_then(|id| devices.iter().position(|d| d.id == id))
        .and_then(|i| u32::try_from(i + 1).ok())
        .unwrap_or(0)
}
