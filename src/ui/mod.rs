//! GTK4/libadwaita front end.

mod audio_panel;
mod bubble_video;
mod camera_bubble;
mod camera_panel;
mod library;
mod main_window;
mod preferences;
mod screenshots;
mod shortcuts;

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::Controller;
use crate::branding;
use crate::config::Settings;
use crate::i18n::gettext;
use main_window::{MainWindow, WindowObserver};
use shortcuts::ShortcutsManager;

#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// Open the source picker immediately (`--record`).
    pub record_on_start: bool,
    /// Render the screenshots into this directory and quit
    /// (`--screenshots`, hidden; see `packaging/screenshots.sh`).
    pub screenshots: Option<std::path::PathBuf>,
}

/// Runs the GUI until the last window closes.
pub fn run(options: LaunchOptions) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(branding::APP_ID)
        .build();

    app.connect_activate(move |app| activate(app, options.clone()));
    // GTK must not parse our CLI flags; clap already did.
    app.run_with_args::<&str>(&[])
}

fn activate(app: &adw::Application, options: LaunchOptions) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    load_css();
    let settings = Settings::load_or_default();
    let controller = Controller::new(settings);
    let window = MainWindow::new(app, Rc::clone(&controller));
    controller.set_observer(Some(Rc::new(WindowObserver(Rc::clone(&window)))));
    let shortcuts = ShortcutsManager::new(app, window.window().upcast_ref(), &controller);
    install_actions(app, &window, &controller, &shortcuts);
    {
        let controller = Rc::clone(&controller);
        let main = Rc::clone(&window);
        let shortcuts_on_close = Rc::clone(&shortcuts);
        window.window().connect_close_request(move |_| {
            // Discard any in-progress recording and break the reference cycle.
            controller.cancel();
            main.close_bubble();
            shortcuts_on_close.disable();
            controller.set_observer(None);
            glib::Propagation::Proceed
        });
    }
    window.present();
    window.sync_bubble();

    if controller.settings().shortcuts.global {
        let shortcuts = Rc::clone(&shortcuts);
        glib::spawn_future_local(async move {
            if let Err(e) = shortcuts.enable().await {
                tracing::warn!(error = %e, "global shortcuts unavailable at startup");
            }
        });
    }

    if let Some(dir) = options.screenshots {
        let app = app.clone();
        let gtk_window = window.window().clone();
        glib::spawn_future_local(screenshots::capture_all(app, gtk_window, dir));
        return;
    }

    if options.record_on_start {
        controller.start(Some(window.window().clone().upcast()));
    }
}

fn install_actions(
    app: &adw::Application,
    window: &Rc<MainWindow>,
    controller: &Rc<Controller>,
    shortcuts: &Rc<ShortcutsManager>,
) {
    let toggle = gio::SimpleAction::new("toggle-recording", None);
    {
        let controller = Rc::clone(controller);
        let gtk_window: gtk::Window = window.window().clone().upcast();
        toggle.connect_activate(move |_, _| controller.toggle(Some(gtk_window.clone())));
    }
    app.add_action(&toggle);

    let pause = gio::SimpleAction::new("toggle-pause", None);
    {
        let controller = Rc::clone(controller);
        pause.connect_activate(move |_, _| controller.toggle_pause());
    }
    app.add_action(&pause);
    shortcuts::restore_app_accels(app);

    let cancel = gio::SimpleAction::new("cancel", None);
    {
        let controller = Rc::clone(controller);
        cancel.connect_activate(move |_, _| controller.cancel());
    }
    app.add_action(&cancel);
    app.set_accels_for_action("app.cancel", &["Escape"]);

    let open_folder = gio::SimpleAction::new("open-folder", None);
    {
        let controller = Rc::clone(controller);
        let gtk_window: gtk::Window = window.window().clone().upcast();
        open_folder.connect_activate(move |_, _| {
            let library = controller.library();
            if let Err(e) = library.ensure_dir() {
                tracing::warn!(error = %e, "cannot create recordings directory");
                return;
            }
            let folder = gio::File::for_path(library.dir());
            main_window::open_file(&gtk_window, &folder);
        });
    }
    app.add_action(&open_folder);

    let library = gio::SimpleAction::new("library", None);
    {
        let window = Rc::clone(window);
        library.connect_activate(move |_, _| window.show_library());
    }
    app.add_action(&library);

    let prefs = gio::SimpleAction::new("preferences", None);
    {
        let window = Rc::clone(window);
        let controller = Rc::clone(controller);
        let shortcuts = Rc::clone(shortcuts);
        prefs.connect_activate(move |_, _| {
            let on_storage_changed: Rc<dyn Fn()> = {
                let window = Rc::clone(&window);
                Rc::new(move || window.refresh_recent())
            };
            preferences::show(window.window(), &controller, &shortcuts, on_storage_changed);
        });
    }
    app.add_action(&prefs);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);

    let about = gio::SimpleAction::new("about", None);
    {
        let gtk_window: gtk::Window = window.window().clone().upcast();
        about.connect_activate(move |_, _| {
            adw::AboutDialog::builder()
                .application_name(branding::APP_NAME)
                .application_icon(branding::APP_ID)
                .version(branding::VERSION)
                .comments(gettext("Native screen recorder for GNOME and Wayland"))
                // Translators: replace with your name (and e-mail), one
                // translator per line. It appears in the About dialog.
                .translator_credits(gettext("translator-credits"))
                .license_type(gtk::License::MitX11)
                .website("https://github.com/oscar503sv/reelix")
                .build()
                .present(Some(&gtk_window));
        });
    }
    app.add_action(&about);

    let quit = gio::SimpleAction::new("quit", None);
    {
        let app = app.clone();
        let controller = Rc::clone(controller);
        quit.connect_activate(move |_, _| {
            controller.cancel();
            app.quit();
        });
    }
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

fn load_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    // Running from a source checkout (`cargo run`): the app icon is not
    // installed yet, so look for it in the repository's data/icons.
    let source_icons = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons");
    if source_icons.is_dir() {
        gtk::IconTheme::for_display(&display).add_search_path(source_icons);
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(camera_bubble::CSS);
    // Above USER priority on purpose: themes installed in
    // ~/.config/gtk-4.0/gtk.css (e.g. "window { background-color: … }")
    // otherwise paint the bubble's transparent corners. The CSS only targets
    // the bubble, so the user's theme still styles everything else.
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );
}
