//! Camera options shown on the main window. Changes are saved immediately
//! and apply to the next recording.

use std::rc::Rc;

use adw::prelude::*;

use crate::app::Controller;
use crate::capture::camera::list_cameras;
use crate::config::{CameraShape, CameraSize, Settings};

const SIZES: [(CameraSize, &str); 3] = [
    (CameraSize::Small, "Pequeña"),
    (CameraSize::Medium, "Mediana"),
    (CameraSize::Large, "Grande"),
];
const SHAPES: [(CameraShape, &str); 2] = [
    (CameraShape::Rectangle, "Rectángulo"),
    (CameraShape::Circle, "Círculo (usa más CPU)"),
];

pub struct CameraPanel {
    group: adw::PreferencesGroup,
    enabled: adw::SwitchRow,
}

impl CameraPanel {
    /// `on_change` runs after any camera setting changes (the bubble follows).
    pub fn new(controller: &Rc<Controller>, on_change: impl Fn() + 'static) -> Self {
        let on_change: Rc<dyn Fn()> = Rc::new(on_change);
        let settings = controller.settings().camera;
        let cameras = list_cameras();

        let enabled = adw::SwitchRow::builder()
            .title("Cámara")
            .subtitle("Superpone tu cámara sobre la grabación")
            .active(settings.enabled)
            .build();
        enabled.add_prefix(&gtk::Image::from_icon_name("camera-web-symbolic"));

        let device_labels: Vec<String> = std::iter::once("Predeterminada".to_owned())
            .chain(cameras.iter().map(|c| c.label.clone()))
            .collect();
        let device_index = settings
            .device
            .as_deref()
            .and_then(|id| cameras.iter().position(|c| c.id == id))
            .map_or(0, |i| i + 1);
        let device = combo("Dispositivo", &device_labels, device_index);

        let size = combo("Tamaño", &labels(&SIZES), index_of(&SIZES, settings.size));
        let shape = combo("Forma", &labels(&SHAPES), index_of(&SHAPES, settings.shape));
        let mirror = adw::SwitchRow::builder()
            .title("Efecto espejo")
            .active(settings.mirror)
            .build();

        let options = [
            device.clone().upcast::<gtk::Widget>(),
            size.clone().upcast(),
            shape.clone().upcast(),
            mirror.clone().upcast(),
        ];
        let group = adw::PreferencesGroup::builder()
            .title("Cámara")
            .margin_bottom(24)
            .build();
        group.add(&enabled);
        for row in &options {
            row.set_sensitive(settings.enabled);
            group.add(row);
        }

        {
            let controller = Rc::clone(controller);
            let on_change = Rc::clone(&on_change);
            enabled.connect_active_notify(move |row| {
                let on = row.is_active();
                for option in &options {
                    option.set_sensitive(on);
                }
                controller.update_settings(|s| s.camera.enabled = on);
                on_change();
            });
        }
        {
            let controller = Rc::clone(controller);
            let on_change = Rc::clone(&on_change);
            device.connect_selected_notify(move |row| {
                // Index 0 is "default"; the rest map onto `cameras`.
                let id = (row.selected() as usize)
                    .checked_sub(1)
                    .and_then(|i| cameras.get(i))
                    .map(|c| c.id.clone());
                controller.update_settings(|s| s.camera.device = id);
                on_change();
            });
        }
        bind_choice(controller, &on_change, &size, &SIZES, |s, v| {
            s.camera.size = v
        });
        bind_choice(controller, &on_change, &shape, &SHAPES, |s, v| {
            s.camera.shape = v
        });
        {
            let controller = Rc::clone(controller);
            let on_change = Rc::clone(&on_change);
            mirror.connect_active_notify(move |row| {
                let on = row.is_active();
                controller.update_settings(|s| s.camera.mirror = on);
                on_change();
            });
        }

        Self { group, enabled }
    }

    pub fn enabled_row(&self) -> &adw::SwitchRow {
        &self.enabled
    }

    pub fn widget(&self) -> &adw::PreferencesGroup {
        &self.group
    }

    /// Camera options cannot change mid-recording.
    pub fn set_sensitive(&self, sensitive: bool) {
        self.group.set_sensitive(sensitive);
    }
}

fn combo(title: &str, labels: &[String], selected: usize) -> adw::ComboRow {
    let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    adw::ComboRow::builder()
        .title(title)
        .model(&model)
        .selected(u32::try_from(selected).unwrap_or(0))
        .build()
}

fn labels<T>(choices: &[(T, &str)]) -> Vec<String> {
    choices.iter().map(|(_, l)| (*l).to_owned()).collect()
}

fn index_of<T: PartialEq>(choices: &[(T, &str)], value: T) -> usize {
    choices.iter().position(|(v, _)| *v == value).unwrap_or(0)
}

fn bind_choice<T: Copy + 'static>(
    controller: &Rc<Controller>,
    on_change: &Rc<dyn Fn()>,
    row: &adw::ComboRow,
    choices: &'static [(T, &'static str)],
    apply: fn(&mut Settings, T),
) {
    let controller = Rc::clone(controller);
    let on_change = Rc::clone(on_change);
    row.connect_selected_notify(move |row| {
        if let Some((value, _)) = choices.get(row.selected() as usize) {
            let value = *value;
            controller.update_settings(|s| apply(s, value));
            on_change();
        }
    });
}
