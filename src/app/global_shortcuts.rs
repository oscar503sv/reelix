//! System-wide shortcuts through the XDG GlobalShortcuts portal.
//!
//! On Wayland an app cannot grab keys globally; it proposes shortcuts and the
//! compositor (GNOME asks the user to confirm or change them) delivers
//! `Activated` signals. Host (non-Flatpak) apps must first register their app
//! id with the `org.freedesktop.host.portal.Registry` portal so the shortcuts
//! are stored under it.

use futures_util::StreamExt;

use ashpd::WindowIdentifier;
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::{ResponseError, Session};

use crate::branding;
use crate::error::{AppError, Result};
use crate::i18n::gettext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    ToggleRecording,
    TogglePause,
}

impl ShortcutAction {
    pub const ALL: [ShortcutAction; 2] = [Self::ToggleRecording, Self::TogglePause];

    pub fn id(self) -> &'static str {
        match self {
            Self::ToggleRecording => "toggle-recording",
            Self::TogglePause => "toggle-pause",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    fn description(self) -> String {
        match self {
            Self::ToggleRecording => gettext("Start or stop recording"),
            Self::TogglePause => gettext("Pause or resume recording"),
        }
    }

    /// Trigger in the XDG shortcuts format.
    fn preferred_trigger(self) -> &'static str {
        match self {
            Self::ToggleRecording => "CTRL+SHIFT+r",
            Self::TogglePause => "CTRL+SHIFT+p",
        }
    }

    /// The matching in-app GTK action, whose accelerator is disabled while
    /// the global shortcut is active (to avoid double triggers).
    pub fn app_action(self) -> &'static str {
        match self {
            Self::ToggleRecording => "app.toggle-recording",
            Self::TogglePause => "app.toggle-pause",
        }
    }
}

/// A bound shortcuts session; dropping it unbinds the shortcuts.
pub struct GlobalShortcutsSession {
    proxy: GlobalShortcuts,
    session: Session<GlobalShortcuts>,
    /// Triggers as finally assigned by the user/compositor, for display.
    pub triggers: Vec<(ShortcutAction, String)>,
}

impl GlobalShortcutsSession {
    /// Registers the app with the portal, binds the shortcuts (GNOME shows a
    /// confirmation dialog) and returns the session.
    pub async fn bind(parent: Option<&WindowIdentifier>) -> Result<Self> {
        let app_id = branding::APP_ID
            .parse()
            .map_err(|e| AppError::Portal(format!("invalid app id: {e}")))?;
        // Flatpak apps are identified by the sandbox; only host (RPM/cargo)
        // installs need to tell the portal who they are.
        if !std::path::Path::new("/.flatpak-info").exists()
            && let Err(e) = ashpd::register_host_app(app_id).await
        {
            // Older portals lack the Registry; binding may still work.
            tracing::warn!(error = %e, "could not register host app id");
        }

        let proxy = GlobalShortcuts::new().await.map_err(portal_err)?;
        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(portal_err)?;
        let shortcuts: Vec<NewShortcut> = ShortcutAction::ALL
            .iter()
            .map(|a| {
                NewShortcut::new(a.id(), a.description()).preferred_trigger(a.preferred_trigger())
            })
            .collect();
        let bound = proxy
            .bind_shortcuts(&session, &shortcuts, parent, Default::default())
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;
        let triggers = bound
            .shortcuts()
            .iter()
            .filter_map(|s| {
                Some((
                    ShortcutAction::from_id(s.id())?,
                    s.trigger_description().to_owned(),
                ))
            })
            .collect::<Vec<_>>();
        tracing::info!(?triggers, "global shortcuts bound");
        Ok(Self {
            proxy,
            session,
            triggers,
        })
    }

    /// Calls `on_action` for every activation until the session ends.
    pub async fn listen(&self, on_action: impl Fn(ShortcutAction)) {
        let mut activated = match self.proxy.receive_activated().await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!(error = %e, "cannot listen to global shortcuts");
                return;
            }
        };
        while let Some(event) = activated.next().await {
            match ShortcutAction::from_id(event.shortcut_id()) {
                Some(action) => {
                    tracing::debug!(?action, "global shortcut activated");
                    on_action(action);
                }
                None => tracing::debug!(id = event.shortcut_id(), "unknown shortcut"),
            }
        }
    }

    pub async fn close(&self) {
        if let Err(e) = self.session.close().await {
            tracing::debug!(error = %e, "closing global shortcuts session failed");
        }
    }
}

fn portal_err(err: ashpd::Error) -> AppError {
    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => {
            AppError::Portal("global shortcuts were not accepted".into())
        }
        other => AppError::Portal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip() {
        for action in ShortcutAction::ALL {
            assert_eq!(ShortcutAction::from_id(action.id()), Some(action));
        }
        assert_eq!(ShortcutAction::from_id("nope"), None);
    }

    #[test]
    fn triggers_use_xdg_format() {
        for action in ShortcutAction::ALL {
            let trigger = action.preferred_trigger();
            assert!(trigger.starts_with("CTRL+SHIFT+"), "{trigger}");
        }
    }
}
