//! "My recordings": the full library dialog and the recording rows (with
//! their actions menu) shared with the main window's recent list.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::{self, N_, fill, gettext};
use crate::storage::{Library, Recording, RelativeDay, format_duration, naming, relative_day};

/// Everything a recording row needs to act on a recording.
pub struct LibraryActions {
    pub window: gtk::Window,
    pub library: Library,
    /// Called after a recording was renamed or removed.
    pub on_changed: Rc<dyn Fn()>,
    /// Shows a short message (toast).
    pub notify: Rc<dyn Fn(&str)>,
}

/// A row: title, "Today 14:05 · 2:31 · 12.4 MB", play on activate, actions menu.
pub fn recording_row(recording: &Recording, actions: &Rc<LibraryActions>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&recording.title()))
        .subtitle(subtitle(recording))
        .activatable(true)
        .build();

    let menu = gio::Menu::new();
    let main = gio::Menu::new();
    main.append(Some(&gettext("Play")), Some("rec.play"));
    main.append(Some(&gettext("Rename…")), Some("rec.rename"));
    main.append(Some(&gettext("Show in Folder")), Some("rec.folder"));
    main.append(Some(&gettext("Copy File")), Some("rec.copy"));
    main.append(Some(&gettext("Details")), Some("rec.details"));
    menu.append_section(None, &main);
    let danger = gio::Menu::new();
    danger.append(Some(&gettext("Move to Trash")), Some("rec.trash"));
    menu.append_section(None, &danger);
    row.add_suffix(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .tooltip_text(gettext("Actions"))
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
        parts.push(day_label(
            &created,
            relative_day(created.date_naive(), today),
        ));
    }
    if let Some(meta) = &recording.metadata {
        parts.push(format_duration(meta.duration));
    }
    parts.push(glib::format_size(recording.size_bytes).into());
    parts.join(" · ")
}

/// "Today 14:05", "Oct 20 14:05", "Dec 31, 2025 14:05": month names come
/// from the system locale (GLib), the patterns from the translation.
fn day_label(created: &chrono::DateTime<chrono::Local>, day: RelativeDay) -> String {
    let time = created.format("%H:%M");
    let pattern = match day {
        // Translators: recording made today; {time} is e.g. 14:05.
        RelativeDay::Today => return fill(&gettext("Today {time}"), &[("time", &time)]),
        // Translators: recording made yesterday; {time} is e.g. 14:05.
        RelativeDay::Yesterday => return fill(&gettext("Yesterday {time}"), &[("time", &time)]),
        // Translators: GLib date format for a recording made this year
        // (e.g. "Oct 20 14:05"); see g_date_time_format().
        RelativeDay::ThisYear => N_("%b %-d %H:%M"),
        // Translators: GLib date format for an older recording
        // (e.g. "Dec 31, 2025 14:05"); see g_date_time_format().
        RelativeDay::OtherYear => N_("%b %-d, %Y %H:%M"),
    };
    glib::DateTime::from_unix_local(created.timestamp())
        .and_then(|dt| dt.format(&gettext(pattern)))
        .map(String::from)
        .unwrap_or_else(|_| created.format("%Y-%m-%d %H:%M").to_string())
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
                notify(&gettext("Could not open the folder"));
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
        Ok(()) => (actions.notify)(&gettext("File copied: paste it anywhere")),
        Err(e) => {
            tracing::warn!(error = %e, "cannot copy to clipboard");
            (actions.notify)(&gettext("Could not copy the file"));
        }
    }
}

fn rename(recording: &Recording, actions: &Rc<LibraryActions>) {
    let entry = gtk::Entry::builder()
        .text(recording.title())
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Rename Recording"))
        .extra_child(&entry)
        .default_response("rename")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", &gettext("Cancel"));
    dialog.add_response("rename", &gettext("Rename"));
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
    let yes_no = |b: bool| if b { gettext("Yes") } else { gettext("No") };
    let line =
        |template: String, value: &dyn std::fmt::Display| fill(&template, &[("value", value)]);
    let mut lines = Vec::new();
    if let Some(created) = recording.created_at() {
        lines.push(line(
            gettext("Date: {value}"),
            &i18n::date_time_seconds(&created),
        ));
    }
    if let Some(meta) = &recording.metadata {
        lines.push(line(
            gettext("Duration: {value}"),
            &format_duration(meta.duration),
        ));
        if let Some(resolution) = &meta.resolution {
            lines.push(line(gettext("Resolution: {value}"), resolution));
        }
        lines.push(line(gettext("Max. FPS: {value}"), &meta.fps));
        lines.push(line(gettext("Audio: {value}"), &yes_no(meta.audio)));
        lines.push(line(
            gettext("Camera overlay: {value}"),
            &yes_no(meta.camera),
        ));
        if let Some(encoder) = &meta.encoder {
            lines.push(line(gettext("Encoder: {value}"), encoder));
        }
    } else {
        lines.push(gettext("No metadata"));
    }
    lines.push(line(
        gettext("Size: {value}"),
        &glib::format_size(recording.size_bytes),
    ));
    lines.push(line(
        gettext("File: {value}"),
        &recording.video_path.display(),
    ));

    let dialog = adw::AlertDialog::new(Some(&recording.title()), Some(&lines.join("\n")));
    dialog.add_response("close", &gettext("Close"));
    dialog.present(Some(&actions.window));
}

/// Moves the video and its metadata to the trash (recoverable), after
/// confirmation.
fn trash(recording: &Recording, actions: &Rc<LibraryActions>) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Move to Trash?"))
        .body(fill(
            &gettext("“{title}” will be moved to the trash. You can restore it from there."),
            &[("title", &recording.title())],
        ))
        .default_response("cancel")
        .close_response("cancel")
        .build();
    dialog.add_response("cancel", &gettext("Cancel"));
    dialog.add_response("trash", &gettext("Move to Trash"));
    dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
    let recording = recording.clone();
    let actions_for_response = Rc::clone(actions);
    dialog.connect_response(Some("trash"), move |_, _| {
        let video = file_of(&recording);
        if let Err(e) = video.trash(gio::Cancellable::NONE) {
            tracing::error!(error = %e, "cannot trash recording");
            (actions_for_response.notify)(&gettext("Could not move to the trash"));
            return;
        }
        let meta = gio::File::for_path(naming::metadata_path_for(&recording.video_path));
        if meta.query_exists(gio::Cancellable::NONE)
            && let Err(e) = meta.trash(gio::Cancellable::NONE)
        {
            tracing::warn!(error = %e, "cannot trash metadata");
        }
        (actions_for_response.notify)(&gettext("Recording moved to the trash"));
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
            .title(gettext("No Recordings Yet"))
            .description(gettext("Your recordings will appear here."))
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
            .title(gettext("My Recordings"))
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
