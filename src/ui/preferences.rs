//! Preferences dialog: recording, video, audio volume, storage, shortcuts.
//! Microphone/camera toggles live on the main window for quick access.
//! Every change is saved immediately and applies to the next recording.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::shortcuts::ShortcutsManager;
use crate::app::Controller;
use crate::config::{EncoderPreference, Quality, Settings};
use crate::recording::EncoderKind;

const FPS_CHOICES: [u32; 4] = [15, 24, 30, 60];
const QUALITIES: [(Quality, &str); 3] = [
    (Quality::Low, "Baja (archivos pequeños)"),
    (Quality::Medium, "Media"),
    (Quality::High, "Alta"),
];
const ENCODERS: [(EncoderPreference, &str); 3] = [
    (EncoderPreference::Auto, "Automático"),
    (EncoderPreference::Hardware, "Hardware (GPU)"),
    (EncoderPreference::Software, "Software (CPU)"),
];
const SHORTCUTS: [(&str, &str); 5] = [
    ("<Control><Shift>r", "Empezar o detener la grabación"),
    ("<Control><Shift>p", "Pausar o reanudar"),
    ("Escape", "Cancelar (descarta la grabación)"),
    ("<Control>comma", "Preferencias"),
    ("<Control>q", "Salir"),
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
        .title("Preferencias")
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
        .title("Grabación")
        .icon_name("media-record-symbolic")
        .build();

    let general = adw::PreferencesGroup::builder().title("General").build();
    let countdown = adw::SpinRow::builder()
        .title("Cuenta atrás")
        .subtitle("Segundos antes de empezar a grabar (0 = sin cuenta atrás)")
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
        .title("Mostrar el puntero")
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

    let video = adw::PreferencesGroup::builder().title("Vídeo").build();
    let fps_labels: Vec<String> = FPS_CHOICES.iter().map(|f| format!("{f} fps")).collect();
    let fps = combo(
        "Fotogramas por segundo",
        &fps_labels,
        FPS_CHOICES
            .iter()
            .position(|f| *f == settings.video.fps)
            .unwrap_or(2),
    );
    fps.set_subtitle("Máximo: con la pantalla quieta se graban menos");
    {
        let controller = Rc::clone(controller);
        fps.connect_selected_notify(move |row| {
            if let Some(value) = FPS_CHOICES.get(row.selected() as usize).copied() {
                controller.update_settings(|s| s.video.fps = value);
            }
        });
    }
    let quality = combo(
        "Calidad",
        &labels(&QUALITIES),
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
    let encoder = combo(
        "Codificador",
        &labels(&ENCODERS),
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
    video.add(&encoder);

    page.add(&general);
    page.add(&video);
    page
}

/// "En uso: openh264enc (software)" for the given preference.
fn encoder_in_use(preference: EncoderPreference) -> String {
    match EncoderKind::detect(preference) {
        Ok(kind) => format!(
            "En uso: {kind} ({})",
            if kind.is_hardware() { "GPU" } else { "CPU" }
        ),
        Err(_) => "No hay ningún codificador H.264 disponible".to_owned(),
    }
}

fn audio_page(controller: &Rc<Controller>, settings: &Settings) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Audio")
        .icon_name("audio-input-microphone-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Micrófono")
        .description("El dispositivo se elige en la ventana principal.")
        .build();
    let volume = adw::SpinRow::builder()
        .title("Volumen de entrada")
        .subtitle("Porcentaje aplicado a la grabación (100 % = sin cambios)")
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
        .title("Almacenamiento")
        .icon_name("folder-videos-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Grabaciones")
        .description("Solo se guardan en tu equipo. Nunca se envían a Internet.")
        .build();
    let folder = adw::ActionRow::builder()
        .title("Carpeta")
        .subtitle(settings.recording_dir().display().to_string())
        .subtitle_selectable(true)
        .build();
    let choose = gtk::Button::builder()
        .label("Cambiar…")
        .valign(gtk::Align::Center)
        .build();
    let reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text("Volver a la carpeta predeterminada")
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
                .title("Carpeta de grabaciones")
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
        .title("Atajos")
        .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
        .build();
    page.add(&global_shortcuts_group(controller, manager));
    let group = adw::PreferencesGroup::builder()
        .title("Con la ventana de Reelix enfocada")
        .description("Durante la grabación usa los botones de la burbuja de cámara.")
        .build();
    for (accel, title) in SHORTCUTS {
        let row = adw::ActionRow::builder().title(title).build();
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
        .title("Atajos globales")
        .description(
            "Funcionan aunque Reelix no esté enfocada (por ejemplo, grabando con la \
             ventana minimizada). GNOME te pedirá confirmarlos o cambiarlos.",
        )
        .build();
    let switch = adw::SwitchRow::builder()
        .title("Activar atajos globales")
        .subtitle(if manager.is_active() {
            "Activos"
        } else {
            "Ctrl+Mayús+R y Ctrl+Mayús+P en todo el sistema"
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
            row.set_subtitle("Desactivados");
            return;
        }
        row.set_sensitive(false);
        row.set_subtitle("Esperando confirmación de GNOME…");
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
                    row.set_subtitle(&format!("Activos: {}", assigned.join(", ")));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "global shortcuts not enabled");
                    reverting.set(true);
                    row.set_active(false);
                    reverting.set(false);
                    row.set_subtitle("No se activaron (cancelado o no disponible)");
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

fn labels<T>(choices: &[(T, &str)]) -> Vec<String> {
    choices.iter().map(|(_, l)| (*l).to_owned()).collect()
}

fn index_of<T: PartialEq>(choices: &[(T, &str)], value: T) -> usize {
    choices.iter().position(|(v, _)| *v == value).unwrap_or(0)
}
