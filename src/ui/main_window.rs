//! Main window: record button, live timer and audio meter, audio options,
//! recent recordings.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use super::audio_panel::AudioPanel;
use super::camera_bubble::CameraBubble;
use super::camera_panel::CameraPanel;
use super::library::{LibraryActions, LibraryDialog, recording_row};
use crate::app::{Controller, ControllerObserver, RecorderState};
use crate::branding;
use crate::error::AppError;
use crate::storage::format_duration;

const RECENT_LIMIT: usize = 5;

pub struct MainWindow {
    window: adw::ApplicationWindow,
    controller: Rc<Controller>,
    toasts: adw::ToastOverlay,
    status_icon: gtk::Image,
    status_title: gtk::Label,
    status_description: gtk::Label,
    record_button: gtk::Button,
    stop_button: gtk::Button,
    pause_button: gtk::Button,
    timer_label: gtk::Label,
    level_bar: gtk::LevelBar,
    audio_panel: AudioPanel,
    camera_panel: CameraPanel,
    bubble: Rc<CameraBubble>,
    bubble_hint_shown: Cell<bool>,
    recent_group: adw::PreferencesGroup,
    recent_rows: RefCell<Vec<adw::ActionRow>>,
    library_dialog: Rc<LibraryDialog>,
    timer_source: RefCell<Option<glib::SourceId>>,
    inhibit_cookie: RefCell<Option<u32>>,
}

/// Hero icon sizes: the app icon when idle, symbolic status icons otherwise.
const IDLE_ICON_SIZE: i32 = 96;
const STATUS_ICON_SIZE: i32 = 64;
const IDLE_DESCRIPTION: &str = "Graba tu pantalla o una ventana. Todo se guarda localmente.";

impl MainWindow {
    pub fn new(app: &adw::Application, controller: Rc<Controller>) -> Rc<Self> {
        let this = Rc::new_cyclic(|weak: &Weak<Self>| Self::build(app, controller, weak.clone()));
        this.connect_signals();
        this.refresh_recent();
        this
    }

    fn build(app: &adw::Application, controller: Rc<Controller>, weak: Weak<Self>) -> Self {
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
        let pause_button = gtk::Button::builder()
            .label("Pausar")
            .tooltip_text("Pausar o reanudar (Ctrl+Mayús+P)")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
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

        // Header: a plain box rather than AdwStatusPage, which scrolls on its
        // own and gets squeezed when other content shares the window.
        let status_icon = gtk::Image::builder()
            .icon_name(branding::APP_ID)
            .pixel_size(IDLE_ICON_SIZE)
            .build();
        let status_title = gtk::Label::builder()
            .label(branding::APP_NAME)
            .css_classes(["title-1"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let status_description = gtk::Label::builder()
            .label(IDLE_DESCRIPTION)
            .css_classes(["dim-label"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let hero = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .build();
        for widget in [
            status_icon.upcast_ref::<gtk::Widget>(),
            status_title.upcast_ref(),
            status_description.upcast_ref(),
            timer_label.upcast_ref(),
            level_bar.upcast_ref(),
            record_button.upcast_ref(),
            pause_button.upcast_ref(),
            stop_button.upcast_ref(),
        ] {
            hero.append(widget);
        }

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
        let see_all = gtk::Button::builder()
            .label("Ver todas")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .action_name("app.library")
            .build();
        let header_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_buttons.append(&see_all);
        header_buttons.append(&open_folder);
        recent_group.set_header_suffix(Some(&header_buttons));

        let audio_panel = AudioPanel::new(&controller);
        let camera_panel = {
            let weak = weak.clone();
            CameraPanel::new(&controller, move || {
                if let Some(this) = weak.upgrade() {
                    this.sync_bubble();
                }
            })
        };
        let bubble = {
            let stop_controller = Rc::clone(&controller);
            let pause_controller = Rc::clone(&controller);
            let camera_switch = camera_panel.enabled_row().clone();
            CameraBubble::new(
                app.upcast_ref(),
                move || stop_controller.stop(),
                move || pause_controller.toggle_pause(),
                // Closing the bubble window turns the camera off.
                move || camera_switch.set_active(false),
            )
        };

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_bottom(24)
            .build();
        column.append(&hero);
        column.append(audio_panel.widget());
        column.append(camera_panel.widget());
        column.append(&recent_group);
        let clamp = adw::Clamp::builder()
            .maximum_size(560)
            .margin_start(12)
            .margin_end(12)
            .child(&column)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scrolled));

        let menu = gio::Menu::new();
        menu.append(Some("Mis grabaciones"), Some("app.library"));
        menu.append(Some("Preferencias"), Some("app.preferences"));
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

        Self {
            window,
            controller,
            toasts,
            status_icon,
            status_title,
            status_description,
            record_button,
            stop_button,
            pause_button,
            timer_label,
            level_bar,
            audio_panel,
            camera_panel,
            bubble,
            bubble_hint_shown: Cell::new(false),
            recent_group,
            recent_rows: RefCell::new(Vec::new()),
            library_dialog: LibraryDialog::new(),
            timer_source: RefCell::new(None),
            inhibit_cookie: RefCell::new(None),
        }
    }

    /// Shows, restyles or hides the camera bubble to match the settings.
    pub fn sync_bubble(self: &Rc<Self>) {
        let camera = self.controller.settings().camera;
        if !camera.enabled {
            self.bubble.hide();
            self.controller.set_bubble_active(false);
            return;
        }
        let this = Rc::clone(self);
        let show = self.bubble.show(&camera);
        glib::spawn_future_local(async move {
            match show.await {
                Ok(()) => {
                    this.controller.set_bubble_active(this.bubble.is_shown());
                    if this.bubble.is_shown() && !this.bubble_hint_shown.replace(true) {
                        this.toasts.add_toast(
                            adw::Toast::builder()
                                .title("Arrastra la burbuja donde quieras. Con Alt+Espacio sobre ella: «Siempre encima» y «Siempre en el área de trabajo visible»")
                                .timeout(10)
                                .build(),
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "camera bubble unavailable; camera will be composited");
                    this.bubble.hide();
                    this.controller.set_bubble_active(false);
                    let (title, _) = e.user_message();
                    this.toasts.add_toast(adw::Toast::new(&format!(
                        "{title}: sin burbuja en pantalla"
                    )));
                }
            }
        });
    }

    /// Releases the camera and destroys the bubble window (it would
    /// otherwise keep the application alive).
    pub fn close_bubble(&self) {
        self.bubble.hide();
        self.bubble.destroy();
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
        let weak = Rc::downgrade(self);
        self.pause_button.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.controller.toggle_pause();
            }
        });
    }

    /// Row actions bound to the current library directory.
    pub fn library_actions(self: &Rc<Self>) -> Rc<LibraryActions> {
        let on_changed = {
            let weak = Rc::downgrade(self);
            Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_recent();
                    this.library_dialog.refresh(&this.library_actions());
                }
            })
        };
        let notify = {
            let toasts = self.toasts.clone();
            Rc::new(move |message: &str| toasts.add_toast(adw::Toast::new(message)))
        };
        Rc::new(LibraryActions {
            window: self.window.clone().upcast(),
            library: self.controller.library(),
            on_changed,
            notify,
        })
    }

    pub fn show_library(self: &Rc<Self>) {
        self.library_dialog.refresh(&self.library_actions());
        self.library_dialog.present(&self.window);
    }

    pub fn refresh_recent(self: &Rc<Self>) {
        for row in self.recent_rows.borrow_mut().drain(..) {
            self.recent_group.remove(&row);
        }
        let actions = self.library_actions();
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
            let row = recording_row(&recording, &actions);
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
                this.bubble.set_elapsed(elapsed);
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
        self.pause_button.set_visible(capturing);
        let paused = *state == RecorderState::Paused;
        self.pause_button
            .set_label(if paused { "Reanudar" } else { "Pausar" });
        self.bubble.set_paused(paused);
        self.timer_label.set_visible(capturing);
        if !capturing {
            self.level_bar.set_visible(false);
            self.level_bar.set_value(0.0);
        }
        self.audio_panel.set_sensitive(!state.is_busy());
        self.camera_panel.set_sensitive(!state.is_busy());
        self.bubble.set_recording(capturing);
        // Keep the main window out of a full-screen recording; the bubble
        // carries the stop button meanwhile.
        if *state != RecorderState::Preparing {
            self.bubble.set_countdown(None);
        }
        // With the bubble on screen (which carries the controls), minimize
        // while the countdown runs so the window is gone when capture starts.
        if *state == RecorderState::Preparing && self.bubble.is_shown() {
            // Wait for the portal dialog (modal to this window) to finish
            // closing: GNOME re-focuses the parent and would undo it.
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                if let Some(this) = weak.upgrade()
                    && (this.controller.state().is_capturing()
                        || this.controller.state() == RecorderState::Preparing)
                {
                    tracing::debug!("minimizing main window during recording");
                    this.window.minimize();
                }
            });
        }
        self.set_inhibited(capturing);

        let (icon, title, description) = match state {
            RecorderState::Idle
            | RecorderState::Completed { .. }
            | RecorderState::Failed { .. } => (
                branding::APP_ID,
                branding::APP_NAME.to_owned(),
                IDLE_DESCRIPTION.to_owned(),
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
        // Idle shows the app icon in colour; other states a dimmed symbolic
        // status icon.
        let idle = icon == branding::APP_ID;
        self.status_icon.set_icon_name(Some(icon));
        self.status_icon.set_pixel_size(if idle {
            IDLE_ICON_SIZE
        } else {
            STATUS_ICON_SIZE
        });
        if idle {
            self.status_icon.remove_css_class("dim-label");
        } else {
            self.status_icon.add_css_class("dim-label");
        }
        self.status_title.set_label(&title);
        self.status_description.set_label(&description);
        self.status_description.set_visible(!description.is_empty());

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

    fn countdown(&self, remaining: u32) {
        let this = &self.0;
        if remaining == 0 {
            this.bubble.set_countdown(None);
            this.status_title.set_label("Grabando…");
            return;
        }
        this.status_title.set_label(&remaining.to_string());
        this.status_description
            .set_label("La grabación empieza enseguida. Esc para cancelar.");
        this.status_description.set_visible(true);
        this.bubble.set_countdown(Some(remaining));
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

/// Opens a file with the default application (respects Flatpak portals).
pub fn open_file(window: &impl IsA<gtk::Window>, file: &gio::File) {
    gtk::FileLauncher::new(Some(file)).launch(Some(window), gio::Cancellable::NONE, |res| {
        if let Err(e) = res {
            tracing::warn!(error = %e, "failed to open file");
        }
    });
}
