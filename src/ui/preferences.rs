//! Preferences dialog: recording, video, audio volume, storage, shortcuts.
//! Microphone/camera toggles live on the main window for quick access.
//! Every change is saved immediately and applies to the next recording.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::shortcuts::ShortcutsManager;
use crate::app::Controller;
use crate::config::{EncoderPreference, Quality, Resolution, Settings};
use crate::i18n::{N_, NC_, fill, gettext, pgettext};
use crate::recording::EncoderKind;

const FPS_CHOICES: [u32; 4] = [15, 24, 30, 60];
// Must match the literal contexts below (xgettext needs literals).
const QUALITY_CONTEXT: &str = "video quality";
const RESOLUTION_CONTEXT: &str = "recording resolution";
const ENCODER_CONTEXT: &str = "video encoder";
const QUALITIES: [(Quality, &str); 3] = [
    (Quality::Low, NC_("video quality", "Low (smaller files)")),
    (Quality::Medium, NC_("video quality", "Medium")),
    (Quality::High, NC_("video quality", "High")),
];
const RESOLUTIONS: [(Resolution, &str); 3] = [
    (
        Resolution::Original,
        NC_("recording resolution", "Original"),
    ),
    (
        Resolution::P1080,
        NC_("recording resolution", "Up to 1080p"),
    ),
    (
        Resolution::P720,
        NC_("recording resolution", "Up to 720p (fewer resources)"),
    ),
];
const ENCODERS: [(EncoderPreference, &str); 3] = [
    (EncoderPreference::Auto, NC_("video encoder", "Automatic")),
    (
        EncoderPreference::Hardware,
        NC_("video encoder", "Hardware (GPU)"),
    ),
    (
        EncoderPreference::Software,
        NC_("video encoder", "Software (CPU)"),
    ),
];
const SHORTCUTS: [(&str, &str); 5] = [
    ("<Control><Shift>r", N_("Start or stop recording")),
    ("<Control><Shift>p", N_("Pause or resume")),
    ("Escape", N_("Cancel (discards the recording)")),
    ("<Control>comma", N_("Preferences")),
    ("<Control>q", N_("Quit")),
];

/// Opens the preferences dialog. `on_storage_changed` runs when the
/// recordings directory changes (the recent list must be reloaded).
pub fn show(
    parent: &impl IsA<gtk::Widget>,
    controller: &Rc<Controller>,
    shortcuts: &Rc<ShortcutsManager>,
    on_storage_changed: Rc<dyn Fn()>,
) {
    let settings = controller.settings();
    let dialog = adw::PreferencesDialog::builder()
        .title(gettext("Preferences"))
        .search_enabled(false)
        .build();
    dialog.add(&recording_page(controller, &settings));
    dialog.add(&audio_page(controller, &settings));
    dialog.add(&storage_page(controller, &settings, on_storage_changed));
    dialog.add(&shortcuts_page(controller, shortcuts));
    dialog.present(Some(parent));
}

fn recording_page(controller: &Rc<Controller>, settings: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Recording"))
        .icon_name("media-record-symbolic")
        .build();

    let general = adw::PreferencesGroup::builder()
        .title(gettext("General"))
        .build();
    let countdown = adw::SpinRow::builder()
        .title(gettext("Countdown"))
        .subtitle(gettext(
            "Seconds before recording starts (0 = no countdown)",
        ))
        .adjustment(&gtk::Adjustment::new(
            f64::from(settings.recording.countdown_seconds),
            0.0,
            f64::from(Settings::MAX_COUNTDOWN),
            1.0,
            1.0,
            0.0,
        ))
        .build();
    {
        let controller = Rc::clone(controller);
        countdown.connect_value_notify(move |row| {
            let seconds = row.value().round().max(0.0) as u32;
            controller.update_settings(|s| s.recording.countdown_seconds = seconds);
        });
    }
    let cursor = adw::SwitchRow::builder()
        .title(gettext("Show Pointer"))
        .active(settings.video.show_cursor)
        .build();
    {
        let controller = Rc::clone(controller);
        cursor.connect_active_notify(move |row| {
            let on = row.is_active();
            controller.update_settings(|s| s.video.show_cursor = on);
        });
    }
    general.add(&countdown);
    general.add(&cursor);

    let video = adw::PreferencesGroup::builder()
        .title(gettext("Video"))
        .build();
    let fps_labels: Vec<String> = FPS_CHOICES
        .iter()
        .map(|f| fill(&gettext("{fps} fps"), &[("fps", f)]))
        .collect();
    let fps = combo(
        &gettext("Frames per Second"),
        &fps_labels,
        FPS_CHOICES
            .iter()
            .position(|f| *f == settings.video.fps)
            .unwrap_or(2),
    );
    fps.set_subtitle(&gettext(
        "Maximum: fewer are recorded while the screen is still",
    ));
    {
        let controller = Rc::clone(controller);
        fps.connect_selected_notify(move |row| {
            if let Some(value) = FPS_CHOICES.get(row.selected() as usize).copied() {
                controller.update_settings(|s| s.video.fps = value);
            }
        });
    }
    let quality = combo(
        &gettext("Quality"),
        &labels(QUALITY_CONTEXT, &QUALITIES),
        index_of(&QUALITIES, settings.video.quality),
    );
    {
        let controller = Rc::clone(controller);
        quality.connect_selected_notify(move |row| {
            if let Some((value, _)) = QUALITIES.get(row.selected() as usize) {
                let value = *value;
                controller.update_settings(|s| s.video.quality = value);
            }
        });
    }
    let resolution = combo(
        &gettext("Resolution"),
        &labels(RESOLUTION_CONTEXT, &RESOLUTIONS),
        index_of(&RESOLUTIONS, settings.video.resolution),
    );
    resolution.set_subtitle(&gettext("Lower resolution: less CPU and smaller files"));
    {
        let controller = Rc::clone(controller);
        resolution.connect_selected_notify(move |row| {
            if let Some((value, _)) = RESOLUTIONS.get(row.selected() as usize) {
                let value = *value;
                controller.update_settings(|s| s.video.resolution = value);
            }
        });
    }
    let encoder = combo(
        &gettext("Encoder"),
        &labels(ENCODER_CONTEXT, &ENCODERS),
        index_of(&ENCODERS, settings.video.encoder),
    );
    encoder.set_subtitle(&encoder_in_use(settings.video.encoder));
    {
        let controller = Rc::clone(controller);
        encoder.connect_selected_notify(move |row| {
            if let Some((value, _)) = ENCODERS.get(row.selected() as usize) {
                let value = *value;
                controller.update_settings(|s| s.video.encoder = value);
                row.set_subtitle(&encoder_in_use(value));
            }
        });
    }
    video.add(&fps);
    video.add(&quality);
    video.add(&resolution);
    video.add(&encoder);

    page.add(&general);
    page.add(&video);
    page
}

/// "In use: openh264enc (CPU)" for the given preference.
fn encoder_in_use(preference: EncoderPreference) -> String {
    match EncoderKind::detect(preference) {
        Ok(kind) => fill(
            // Translators: {encoder} is a GStreamer element name, {device}
            // is "GPU" or "CPU".
            &gettext("In use: {encoder} ({device})"),
            &[
                ("encoder", &kind),
                ("device", &if kind.is_hardware() { "GPU" } else { "CPU" }),
            ],
        ),
        Err(_) => gettext("No H.264 encoder available"),
    }
}

fn audio_page(controller: &Rc<Controller>, settings: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Audio"))
        .icon_name("audio-input-microphone-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Microphone"))
        .description(gettext("The device is chosen in the main window."))
        .build();
    let volume = adw::SpinRow::builder()
        .title(gettext("Input Volume"))
        .subtitle(gettext(
            "Percentage applied to the recording (100% = unchanged)",
        ))
        .digits(0)
        .adjustment(&gtk::Adjustment::new(
            (settings.audio.microphone_volume * 100.0).round(),
            0.0,
            Settings::MAX_VOLUME * 100.0,
            10.0,
            50.0,
            0.0,
        ))
        .build();
    {
        let controller = Rc::clone(controller);
        volume.connect_value_notify(move |row| {
            let gain = row.value() / 100.0;
            controller.update_settings(|s| s.audio.microphone_volume = gain);
        });
    }
    group.add(&volume);
    page.add(&group);
    page
}

fn storage_page(
    controller: &Rc<Controller>,
    settings: &Settings,
    on_changed: Rc<dyn Fn()>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Storage"))
        .icon_name("folder-videos-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Recordings"))
        .description(gettext(
            "They are only saved on your computer. Never sent to the Internet.",
        ))
        .build();
    let folder = adw::ActionRow::builder()
        .title(gettext("Folder"))
        .subtitle(settings.recording_dir().display().to_string())
        .subtitle_selectable(true)
        .build();
    let choose = gtk::Button::builder()
        .label(gettext("Change…"))
        .valign(gtk::Align::Center)
        .build();
    let reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text(gettext("Back to the default folder"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .sensitive(settings.storage.recording_dir.is_some())
        .build();
    folder.add_suffix(&reset);
    folder.add_suffix(&choose);

    let apply = {
        let controller = Rc::clone(controller);
        let folder = folder.clone();
        let reset = reset.clone();
        Rc::new(move |dir: Option<std::path::PathBuf>| {
            reset.set_sensitive(dir.is_some());
            controller.update_settings(|s| s.storage.recording_dir = dir);
            folder.set_subtitle(&controller.settings().recording_dir().display().to_string());
            on_changed();
        })
    };
    {
        let apply = Rc::clone(&apply);
        reset.connect_clicked(move |_| apply(None));
    }
    {
        let controller = Rc::clone(controller);
        choose.connect_clicked(move |button| {
            let dialog = gtk::FileDialog::builder()
                .title(gettext("Recordings Folder"))
                .initial_folder(&gio::File::for_path(controller.settings().recording_dir()))
                .build();
            let parent = button.root().and_downcast::<gtk::Window>();
            let apply = Rc::clone(&apply);
            dialog.select_folder(
                parent.as_ref(),
                gio::Cancellable::NONE,
                move |result| match result {
                    Ok(file) => {
                        if let Some(path) = file.path() {
                            apply(Some(path));
                        }
                    }
                    Err(e) if e.matches(gtk::DialogError::Dismissed) => {}
                    Err(e) => tracing::warn!(error = %e, "folder selection failed"),
                },
            );
        });
    }
    group.add(&folder);
    page.add(&group);
    page
}

fn shortcuts_page(
    controller: &Rc<Controller>,
    manager: &Rc<ShortcutsManager>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title(gettext("Shortcuts"))
        .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
        .build();
    page.add(&global_shortcuts_group(controller, manager));
    let group = adw::PreferencesGroup::builder()
        .title(gettext("With the Reelix Window Focused"))
        .description(gettext("While recording, use the camera bubble buttons."))
        .build();
    for (accel, title) in SHORTCUTS {
        let row = adw::ActionRow::builder().title(gettext(title)).build();
        let label = adw::ShortcutLabel::new(accel);
        label.set_valign(gtk::Align::Center);
        row.add_suffix(&label);
        group.add(&row);
    }
    page.add(&group);
    page
}

fn global_shortcuts_group(
    controller: &Rc<Controller>,
    manager: &Rc<ShortcutsManager>,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(gettext("Global Shortcuts"))
        .description(gettext(
            "They work even when Reelix is not focused (for example, recording with the \
             window minimized). GNOME will ask you to confirm or change them.",
        ))
        .build();
    let switch = adw::SwitchRow::builder()
        .title(gettext("Enable Global Shortcuts"))
        .subtitle(if manager.is_active() {
            gettext("Active")
        } else {
            gettext("Ctrl+Shift+R and Ctrl+Shift+P system-wide")
        })
        .active(manager.is_active())
        .build();
    // Set while the switch is flipped back after a failure, so that change
    // does not re-enter the handler.
    let reverting = Rc::new(std::cell::Cell::new(false));
    let controller = Rc::clone(controller);
    let manager = Rc::clone(manager);
    switch.connect_active_notify(move |row| {
        if reverting.get() {
            return;
        }
        if !row.is_active() {
            manager.disable();
            controller.update_settings(|s| s.shortcuts.global = false);
            row.set_subtitle(&gettext("Disabled"));
            return;
        }
        row.set_sensitive(false);
        row.set_subtitle(&gettext("Waiting for GNOME to confirm…"));
        let (row, manager, controller, reverting) = (
            row.clone(),
            Rc::clone(&manager),
            Rc::clone(&controller),
            Rc::clone(&reverting),
        );
        gtk::glib::spawn_future_local(async move {
            match manager.enable().await {
                Ok(triggers) => {
                    controller.update_settings(|s| s.shortcuts.global = true);
                    let assigned: Vec<String> = triggers.into_iter().map(|(_, t)| t).collect();
                    row.set_subtitle(&fill(
                        &gettext("Active: {shortcuts}"),
                        &[("shortcuts", &assigned.join(", "))],
                    ));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "global shortcuts not enabled");
                    reverting.set(true);
                    row.set_active(false);
                    reverting.set(false);
                    row.set_subtitle(&gettext("Not enabled (cancelled or unavailable)"));
                }
            }
            row.set_sensitive(true);
        });
    });
    group.add(&switch);
    group
}

fn combo(title: &str, labels: &[String], selected: usize) -> adw::ComboRow {
    let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    adw::ComboRow::builder()
        .title(title)
        .model(&model)
        .selected(u32::try_from(selected).unwrap_or(0))
        .build()
}

fn labels<T>(context: &str, choices: &[(T, &str)]) -> Vec<String> {
    choices.iter().map(|(_, l)| pgettext(context, *l)).collect()
}

fn index_of<T: PartialEq>(choices: &[(T, &str)], value: T) -> usize {
    choices.iter().position(|(v, _)| *v == value).unwrap_or(0)
}
