//! "My recordings": the full library dialog and the recording rows (with
//! their actions menu) shared with the main window's recent list.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::storage::{Library, Recording, format_duration, format_size, naming, relative_day};

/// Everything a recording row needs to act on a recording.
pub struct LibraryActions {
    pub window: gtk::Window,
    pub library: Library,
    /// Called after a recording was renamed or removed.
    pub on_changed: Rc<dyn Fn()>,
    /// Shows a short message (toast).
    pub notify: Rc<dyn Fn(&str)>,
}

/// A row: title, "Hoy · 2:31 · 12,4 MB", play on activate, actions menu.
pub fn recording_row(recording: &Recording, actions: &Rc<LibraryActions>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&recording.title()))
        .subtitle(subtitle(recording))
        .activatable(true)
        .build();

    let menu = gio::Menu::new();
    let main = gio::Menu::new();
    main.append(Some("Reproducir"), Some("rec.play"));
    main.append(Some("Renombrar…"), Some("rec.rename"));
    main.append(Some("Mostrar en la carpeta"), Some("rec.folder"));
    main.append(Some("Copiar archivo"), Some("rec.copy"));
    main.append(Some("Detalles"), Some("rec.details"));
    menu.append_section(None, &main);
    let danger = gio::Menu::new();
    danger.append(Some("Mover a la papelera"), Some("rec.trash"));
    menu.append_section(None, &danger);
    row.add_suffix(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text("Acciones")
            .css_classes(["flat"])
            .build(),
    );

    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, f: fn(&Recording, &Rc<LibraryActions>)| {
        let action = gio::SimpleAction::new(name, None);
        let recording = recording.clone();
        let actions = Rc::clone(actions);
        action.connect_activate(move |_, _| f(&recording, &actions));
        group.add_action(&action);
    };
    add("play", play);
    add("rename", rename);
    add("folder", show_in_folder);
    add("copy", copy_file);
    add("details", details);
    add("trash", trash);
    row.insert_action_group("rec", Some(&group));

    let recording = recording.clone();
    let actions = Rc::clone(actions);
    row.connect_activated(move |_| play(&recording, &actions));
    row
}

fn subtitle(recording: &Recording) -> String {
    let today = chrono::Local::now().date_naive();
    let mut parts = Vec::new();
    if let Some(created) = recording.created_at() {
        parts.push(format!(
            "{} {}",
            relative_day(created.date_naive(), today),
            created.format("%H:%M")
        ));
    }
    if let Some(meta) = &recording.metadata {
        parts.push(format_duration(meta.duration));
    }
    parts.push(format_size(recording.size_bytes));
    parts.join(" · ")
}

fn file_of(recording: &Recording) -> gio::File {
    gio::File::for_path(&recording.video_path)
}

fn play(recording: &Recording, actions: &Rc<LibraryActions>) {
    super::main_window::open_file(&actions.window, &file_of(recording));
}

fn show_in_folder(recording: &Recording, actions: &Rc<LibraryActions>) {
    let notify = Rc::clone(&actions.notify);
    gtk::FileLauncher::new(Some(&file_of(recording))).open_containing_folder(
        Some(&actions.window),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(e) = result {
                tracing::warn!(error = %e, "cannot open containing folder");
                notify("No se pudo abrir la carpeta");
            }
        },
    );
}

/// Puts the file on the clipboard, ready to paste in Files or a chat app.
fn copy_file(recording: &Recording, actions: &Rc<LibraryActions>) {
    let files = gdk::FileList::from_array(&[file_of(recording)]);
    let provider = gdk::ContentProvider::for_value(&files.to_value());
    let clipboard = WidgetExt::display(&actions.window).clipboard();
    match clipboard.set_content(Some(&provider)) {
        Ok(()) => (actions.notify)("Archivo copiado: pégalo donde quieras"),
        Err(e) => {
            tracing::warn!(error = %e, "cannot copy to clipboard");
            (actions.notify)("No se pudo copiar el archivo");
        }
    }
}

fn rename(recording: &Recording, actions: &Rc<LibraryActions>) {
    let entry = gtk::Entry::builder()
        .text(recording.title())
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("Renombrar grabación")
        .extra_child(&entry)
        .default_response("rename")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancelar");
    dialog.add_response("rename", "Renombrar");
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    {
        let dialog = dialog.clone();
        entry.connect_changed(move |entry| {
            dialog.set_response_enabled("rename", !entry.text().trim().is_empty());
        });
    }
    let recording = recording.clone();
    let actions_for_response = Rc::clone(actions);
    dialog.connect_response(Some("rename"), move |_, _| {
        let title = entry.text();
        match actions_for_response.library.rename(&recording, &title) {
            Ok(_) => (actions_for_response.on_changed)(),
            Err(e) => {
                tracing::error!(error = %e, "rename failed");
                (actions_for_response.notify)(&e.user_message().0);
            }
        }
    });
    dialog.present(Some(&actions.window));
}

fn details(recording: &Recording, actions: &Rc<LibraryActions>) {
    let yes_no = |b: bool| if b { "Sí" } else { "No" };
    let mut lines = Vec::new();
    if let Some(created) = recording.created_at() {
        lines.push(format!("Fecha: {}", created.format("%d/%m/%Y %H:%M:%S")));
    }
    if let Some(meta) = &recording.metadata {
        lines.push(format!("Duración: {}", format_duration(meta.duration)));
        if let Some(resolution) = &meta.resolution {
            lines.push(format!("Resolución: {resolution}"));
        }
        lines.push(format!("FPS máx.: {}", meta.fps));
        lines.push(format!("Audio: {}", yes_no(meta.audio)));
        lines.push(format!("Cámara compuesta: {}", yes_no(meta.camera)));
        if let Some(encoder) = &meta.encoder {
            lines.push(format!("Codificador: {encoder}"));
        }
    } else {
        lines.push("Sin metadatos".to_owned());
    }
    lines.push(format!("Tamaño: {}", format_size(recording.size_bytes)));
    lines.push(format!("Archivo: {}", recording.video_path.display()));

    let dialog = adw::AlertDialog::new(Some(&recording.title()), Some(&lines.join("\n")));
    dialog.add_response("close", "Cerrar");
    dialog.present(Some(&actions.window));
}

/// Moves the video and its metadata to the trash (recoverable), after
/// confirmation.
fn trash(recording: &Recording, actions: &Rc<LibraryActions>) {
    let dialog = adw::AlertDialog::builder()
        .heading("¿Mover a la papelera?")
        .body(format!(
            "«{}» se moverá a la papelera. Podrás recuperarla desde allí.",
            recording.title()
        ))
        .default_response("cancel")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", "Cancelar");
    dialog.add_response("trash", "Mover a la papelera");
    dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
    let recording = recording.clone();
    let actions_for_response = Rc::clone(actions);
    dialog.connect_response(Some("trash"), move |_, _| {
        let video = file_of(&recording);
        if let Err(e) = video.trash(gio::Cancellable::NONE) {
            tracing::error!(error = %e, "cannot trash recording");
            (actions_for_response.notify)("No se pudo mover a la papelera");
            return;
        }
        let meta = gio::File::for_path(naming::metadata_path_for(&recording.video_path));
        if meta.query_exists(gio::Cancellable::NONE)
            && let Err(e) = meta.trash(gio::Cancellable::NONE)
        {
            tracing::warn!(error = %e, "cannot trash metadata");
        }
        (actions_for_response.notify)("Grabación movida a la papelera");
        (actions_for_response.on_changed)();
    });
    dialog.present(Some(&actions.window));
}

/// The "My recordings" dialog listing the whole library.
pub struct LibraryDialog {
    dialog: adw::Dialog,
    list: adw::PreferencesGroup,
    empty: adw::StatusPage,
    rows: std::cell::RefCell<Vec<adw::ActionRow>>,
}

impl LibraryDialog {
    pub fn new() -> Rc<Self> {
        let list = adw::PreferencesGroup::new();
        let empty = adw::StatusPage::builder()
            .icon_name("folder-videos-symbolic")
            .title("Aún no hay grabaciones")
            .description("Tus grabaciones aparecerán aquí.")
            .build();
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&empty);
        column.append(&list);
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(24)
            .child(&column)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scrolled));
        let dialog = adw::Dialog::builder()
            .title("Mis grabaciones")
            .content_width(560)
            .content_height(620)
            .child(&toolbar)
            .build();
        Rc::new(Self {
            dialog,
            list,
            empty,
            rows: std::cell::RefCell::new(Vec::new()),
        })
    }

    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
    }

    /// Rebuilds the list from disk.
    pub fn refresh(&self, actions: &Rc<LibraryActions>) {
        for row in self.rows.borrow_mut().drain(..) {
            self.list.remove(&row);
        }
        let recordings = actions.library.list().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to list recordings");
            Vec::new()
        });
        self.empty.set_visible(recordings.is_empty());
        self.list.set_visible(!recordings.is_empty());
        let mut rows = self.rows.borrow_mut();
        for recording in &recordings {
            let row = recording_row(recording, actions);
            self.list.add(&row);
            rows.push(row);
        }
    }
}
