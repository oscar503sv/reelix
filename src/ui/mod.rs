//! GTK4/libadwaita front end.

mod audio_panel;
mod main_window;

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::Controller;
use crate::branding;
use crate::config::Settings;
use main_window::{MainWindow, WindowObserver};

#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchOptions {
    /// Open the source picker immediately (`--record`).
    pub record_on_start: bool,
}

/// Runs the GUI until the last window closes.
pub fn run(options: LaunchOptions) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(branding::APP_ID)
        .build();

    app.connect_activate(move |app| activate(app, options));
    // GTK must not parse our CLI flags; clap already did.
    app.run_with_args::<&str>(&[])
}

fn activate(app: &adw::Application, options: LaunchOptions) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let settings = Settings::load_or_default();
    let controller = Controller::new(settings);
    let window = MainWindow::new(app, Rc::clone(&controller));
    controller.set_observer(Some(Rc::new(WindowObserver(Rc::clone(&window)))));
    install_actions(app, &window, &controller);
    {
        let controller = Rc::clone(&controller);
        window.window().connect_close_request(move |_| {
            // Discard any in-progress recording and break the reference cycle.
            controller.cancel();
            controller.set_observer(None);
            glib::Propagation::Proceed
        });
    }
    window.present();

    if options.record_on_start {
        controller.start(Some(window.window().clone().upcast()));
    }
}

fn install_actions(app: &adw::Application, window: &Rc<MainWindow>, controller: &Rc<Controller>) {
    let toggle = gio::SimpleAction::new("toggle-recording", None);
    {
        let controller = Rc::clone(controller);
        let gtk_window: gtk::Window = window.window().clone().upcast();
        toggle.connect_activate(move |_, _| controller.toggle(Some(gtk_window.clone())));
    }
    app.add_action(&toggle);
    app.set_accels_for_action("app.toggle-recording", &["<Control><Shift>r"]);

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

    let about = gio::SimpleAction::new("about", None);
    {
        let gtk_window: gtk::Window = window.window().clone().upcast();
        about.connect_activate(move |_, _| {
            adw::AboutDialog::builder()
                .application_name(branding::APP_NAME)
                .application_icon(branding::APP_ID)
                .version(branding::VERSION)
                .comments("Grabador de pantalla nativo para GNOME y Wayland")
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
