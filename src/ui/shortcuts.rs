//! Turns the global shortcuts session on and off and keeps the in-app
//! accelerators in sync (disabled while the global ones are active, so a
//! key press with the window focused does not trigger twice).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::app::Controller;
use crate::app::global_shortcuts::{GlobalShortcutsSession, ShortcutAction};
use crate::error::Result;

pub struct ShortcutsManager {
    app: adw::Application,
    window: gtk::Window,
    controller: Rc<Controller>,
    active: RefCell<Option<(Rc<GlobalShortcutsSession>, glib::JoinHandle<()>)>>,
}

impl ShortcutsManager {
    pub fn new(
        app: &adw::Application,
        window: &gtk::Window,
        controller: &Rc<Controller>,
    ) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            window: window.clone(),
            controller: Rc::clone(controller),
            active: RefCell::new(None),
        })
    }

    pub fn is_active(&self) -> bool {
        self.active.borrow().is_some()
    }

    /// Binds the shortcuts (GNOME asks the user) and starts listening.
    /// Returns the triggers as assigned, e.g. "Ctrl+Shift+R".
    pub async fn enable(self: &Rc<Self>) -> Result<Vec<(ShortcutAction, String)>> {
        if let Some((session, _)) = self.active.borrow().as_ref() {
            return Ok(session.triggers.clone());
        }
        let identifier = ashpd::WindowIdentifier::from_native(&self.window).await;
        let session = Rc::new(GlobalShortcutsSession::bind(identifier.as_ref()).await?);
        let triggers = session.triggers.clone();

        let listener = {
            let session = Rc::clone(&session);
            let controller = Rc::clone(&self.controller);
            let window = self.window.clone();
            glib::spawn_future_local(async move {
                session
                    .listen(|action| match action {
                        ShortcutAction::ToggleRecording => controller.toggle(Some(window.clone())),
                        ShortcutAction::TogglePause => controller.toggle_pause(),
                    })
                    .await;
            })
        };
        for action in ShortcutAction::ALL {
            self.app.set_accels_for_action(action.app_action(), &[]);
        }
        *self.active.borrow_mut() = Some((session, listener));
        Ok(triggers)
    }

    /// Unbinds the shortcuts and restores the in-app accelerators.
    pub fn disable(&self) {
        let Some((session, listener)) = self.active.borrow_mut().take() else {
            return;
        };
        listener.abort();
        glib::spawn_future_local(async move { session.close().await });
        restore_app_accels(&self.app);
        tracing::info!("global shortcuts disabled");
    }
}

/// In-app accelerators for the actions that can also be global.
pub fn restore_app_accels(app: &adw::Application) {
    app.set_accels_for_action("app.toggle-recording", &["<Control><Shift>r"]);
    app.set_accels_for_action("app.toggle-pause", &["<Control><Shift>p"]);
}
