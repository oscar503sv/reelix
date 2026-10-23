//! Main window: record button, live timer and audio meter, audio options,
//! recent recordings.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use super::audio_panel::AudioPanel;
use super::camera_panel::CameraPanel;
use crate::app::{Controller, ControllerObserver, RecorderState};
use crate::branding;
use crate::error::AppError;
use crate::storage::{Recording, format_duration};

const RECENT_LIMIT: usize = 5;

pub struct MainWindow {
    window: adw::ApplicationWindow,
    controller: Rc<Controller>,
    toasts: adw::ToastOverlay,
    status: adw::StatusPage,
    record_button: gtk::Button,
    stop_button: gtk::Button,
    timer_label: gtk::Label,
    level_bar: gtk::LevelBar,
    audio_panel: AudioPanel,
    camera_panel: CameraPanel,
    recent_group: adw::PreferencesGroup,
    recent_rows: RefCell<Vec<adw::ActionRow>>,
    timer_source: RefCell<Option<glib::SourceId>>,
    inhibit_cookie: RefCell<Option<u32>>,
}

impl MainWindow {
    pub fn new(app: &adw::Application, controller: Rc<Controller>) -> Rc<Self> {
        let record_button = gtk::Button::builder()
            .label("Grabar pantalla")
            .tooltip_text("Grabar pantalla (Ctrl+Mayús+R)")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let stop_button = gtk::Button::builder()
            .label("Detener")
            .tooltip_text("Detener (Ctrl+Mayús+R)")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "destructive-action"])
            .visible(false)
            .build();
        let timer_label = gtk::Label::builder()
            .css_classes(["title-1", "numeric"])
            .visible(false)
            .build();

        let level_bar = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .width_request(200)
            .halign(gtk::Align::Center)
            .tooltip_text("Nivel del micrófono")
            .visible(false)
            .build();
        // Plain meter: no "low/high/full" color offsets.
        for name in ["low", "high", "full"] {
            level_bar.remove_offset_value(Some(name));
        }

        let buttons = gtk::Box::new(gtk::Orientation::Vertical, 12);
        buttons.append(&timer_label);
        buttons.append(&level_bar);
        buttons.append(&record_button);
        buttons.append(&stop_button);

        let status = adw::StatusPage::builder()
            .icon_name("media-record-symbolic")
            .title(branding::APP_NAME)
            .description("Graba tu pantalla o una ventana. Todo se guarda localmente.")
            .child(&buttons)
            .build();

        let recent_group = adw::PreferencesGroup::builder()
            .title("Grabaciones recientes")
            .build();
        let open_folder = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Abrir carpeta de grabaciones")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .action_name("app.open-folder")
            .build();
        recent_group.set_header_suffix(Some(&open_folder));

        let audio_panel = AudioPanel::new(&controller);
        let camera_panel = CameraPanel::new(&controller);
        let groups = gtk::Box::new(gtk::Orientation::Vertical, 0);
        groups.append(audio_panel.widget());
        groups.append(camera_panel.widget());
        groups.append(&recent_group);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&status);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(24)
            .child(&groups)
            .build();
        content.append(&clamp);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scrolled));

        let menu = gio::Menu::new();
        menu.append(
            Some("Abrir carpeta de grabaciones"),
            Some("app.open-folder"),
        );
        menu.append(
            Some(&format!("Acerca de {}", branding::APP_NAME)),
            Some("app.about"),
        );
        let header = adw::HeaderBar::new();
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(true)
                .tooltip_text("Menú principal")
                .build(),
        );

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(branding::APP_NAME)
            .default_width(480)
            .default_height(720)
            .width_request(360)
            .height_request(400)
            .content(&toolbar)
            .build();

        let this = Rc::new(Self {
            window,
            controller,
            toasts,
            status,
            record_button,
            stop_button,
            timer_label,
            level_bar,
            audio_panel,
            camera_panel,
            recent_group,
            recent_rows: RefCell::new(Vec::new()),
            timer_source: RefCell::new(None),
            inhibit_cookie: RefCell::new(None),
        });
        this.connect_signals();
        this.refresh_recent();
        this
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.record_button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.controller.start(Some(this.window.clone().upcast()));
            }
        });
        let weak = Rc::downgrade(self);
        self.stop_button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.controller.stop();
            }
        });
    }

    pub fn refresh_recent(&self) {
        for row in self.recent_rows.borrow_mut().drain(..) {
            self.recent_group.remove(&row);
        }
        let recordings = match self.controller.library().list() {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(error = %e, "failed to list recordings");
                Vec::new()
            }
        };
        self.recent_group.set_visible(!recordings.is_empty());
        let mut rows = self.recent_rows.borrow_mut();
        for recording in recordings.into_iter().take(RECENT_LIMIT) {
            let row = recording_row(&recording);
            self.recent_group.add(&row);
            rows.push(row);
        }
    }

    fn start_timer(self: &Rc<Self>) {
        if self.timer_source.borrow().is_some() {
            return;
        }
        self.timer_label.set_label("0:00");
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(Duration::from_millis(500), move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if let Some(elapsed) = this.controller.elapsed() {
                this.timer_label
                    .set_label(&format_duration(elapsed.as_secs_f64().floor()));
            }
            glib::ControlFlow::Continue
        });
        *self.timer_source.borrow_mut() = Some(id);
    }

    fn stop_timer(&self) {
        if let Some(id) = self.timer_source.borrow_mut().take() {
            id.remove();
        }
    }

    /// Prevents the session from going idle or suspending mid-recording.
    fn set_inhibited(&self, inhibit: bool) {
        let Some(app) = self.window.application() else {
            return;
        };
        let mut cookie = self.inhibit_cookie.borrow_mut();
        match (inhibit, cookie.take()) {
            (true, None) => {
                let id = app.inhibit(
                    Some(&self.window),
                    gtk::ApplicationInhibitFlags::IDLE | gtk::ApplicationInhibitFlags::SUSPEND,
                    Some("Grabando la pantalla"),
                );
                *cookie = (id != 0).then_some(id);
            }
            (true, Some(id)) => *cookie = Some(id),
            (false, Some(id)) => app.uninhibit(id),
            (false, None) => {}
        }
    }

    fn render(self: &Rc<Self>, state: &RecorderState) {
        let capturing = state.is_capturing();
        self.record_button.set_visible(
            !capturing && *state != RecorderState::Stopping && *state != RecorderState::Processing,
        );
        self.record_button.set_sensitive(!state.is_busy());
        self.stop_button.set_visible(capturing);
        self.timer_label.set_visible(capturing);
        if !capturing {
            self.level_bar.set_visible(false);
            self.level_bar.set_value(0.0);
        }
        self.audio_panel.set_sensitive(!state.is_busy());
        self.camera_panel.set_sensitive(!state.is_busy());
        self.set_inhibited(capturing);

        let (icon, title, description) = match state {
            RecorderState::Idle
            | RecorderState::Completed { .. }
            | RecorderState::Failed { .. } => (
                "media-record-symbolic",
                branding::APP_NAME.to_owned(),
                "Graba tu pantalla o una ventana. Todo se guarda localmente.".to_owned(),
            ),
            RecorderState::SelectingSource => (
                "video-display-symbolic",
                "Elige qué grabar".to_owned(),
                "Selecciona una pantalla o ventana en el diálogo del sistema.".to_owned(),
            ),
            RecorderState::Preparing => (
                "content-loading-symbolic",
                "Preparando…".to_owned(),
                String::new(),
            ),
            RecorderState::Recording => (
                "media-record-symbolic",
                "Grabando".to_owned(),
                "Pulsa Ctrl+Mayús+R o «Dejar de compartir» en la barra superior para terminar."
                    .to_owned(),
            ),
            RecorderState::Paused => (
                "media-playback-pause-symbolic",
                "En pausa".to_owned(),
                String::new(),
            ),
            RecorderState::Stopping | RecorderState::Processing => (
                "document-save-symbolic",
                "Guardando…".to_owned(),
                "Finalizando el archivo de vídeo.".to_owned(),
            ),
        };
        self.status.set_icon_name(Some(icon));
        self.status.set_title(&title);
        self.status.set_description(
            Some(&description)
                .filter(|d| !d.is_empty())
                .map(String::as_str),
        );

        match state {
            RecorderState::Recording => self.start_timer(),
            _ if !capturing => self.stop_timer(),
            _ => {}
        }

        if let RecorderState::Completed { path } = state {
            self.refresh_recent();
            self.window.present();
            let toast = adw::Toast::builder()
                .title("Grabación guardada")
                .button_label("Reproducir")
                .timeout(8)
                .build();
            let file = gio::File::for_path(path);
            let window = self.window.clone();
            toast.connect_button_clicked(move |_| open_file(&window, &file));
            self.toasts.add_toast(toast);
            self.controller.reset();
        }
    }
}

/// Forwards controller callbacks to the window.
///
/// This is the owner of the `MainWindow` struct: GTK keeps the widgets
/// alive, but our Rust-side state must be kept alive by someone. The
/// controller → observer → window → controller cycle is broken in
/// `ui::activate` when the window is closed.
pub struct WindowObserver(pub Rc<MainWindow>);

impl ControllerObserver for WindowObserver {
    fn state_changed(&self, state: &RecorderState) {
        self.0.render(state);
    }

    fn error(&self, error: &AppError) {
        let this = &self.0;
        let (title, body) = error.user_message();
        if matches!(error, AppError::SelectionCancelled) {
            this.toasts.add_toast(adw::Toast::new(&title));
            return;
        }
        let dialog = adw::AlertDialog::new(Some(&title), Some(&body));
        dialog.add_response("ok", "Aceptar");
        this.window.present();
        dialog.present(Some(&this.window));
    }

    fn notice(&self, message: &str) {
        self.0
            .toasts
            .add_toast(adw::Toast::builder().title(message).timeout(6).build());
    }

    fn audio_level(&self, level: f64) {
        let bar = &self.0.level_bar;
        if !bar.is_visible() && self.0.controller.state().is_capturing() {
            bar.set_visible(true);
        }
        bar.set_value(level);
    }
}

fn recording_row(recording: &Recording) -> adw::ActionRow {
    let subtitle = recording
        .created_at()
        .map(|d| d.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_default();
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&recording.title()))
        .subtitle(subtitle)
        .activatable(true)
        .build();
    if let Some(meta) = &recording.metadata {
        row.add_suffix(
            &gtk::Label::builder()
                .label(format_duration(meta.duration))
                .css_classes(["dim-label", "numeric"])
                .build(),
        );
    }
    row.add_suffix(&gtk::Image::from_icon_name("media-playback-start-symbolic"));
    let file = gio::File::for_path(&recording.video_path);
    row.connect_activated(move |row| {
        if let Some(window) = row.root().and_downcast::<gtk::Window>() {
            open_file(&window, &file);
        }
    });
    row
}

/// Opens a file with the default application (respects Flatpak portals).
pub fn open_file(window: &impl IsA<gtk::Window>, file: &gio::File) {
    gtk::FileLauncher::new(Some(file)).launch(Some(window), gio::Cancellable::NONE, |res| {
        if let Err(e) = res {
            tracing::warn!(error = %e, "failed to open file");
        }
    });
}
