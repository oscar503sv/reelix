//! XDG Desktop Portal ScreenCast integration.
//!
//! The portal shows GNOME's own screen/window picker, then hands us a
//! PipeWire remote fd and node id. We never touch the compositor directly,
//! which is what makes this work on Wayland and inside Flatpak.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use ashpd::WindowIdentifier;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use futures_util::StreamExt;

use crate::error::{AppError, Result};

/// What the user is allowed to pick in the portal dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    #[default]
    MonitorOrWindow,
    Monitor,
    Window,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureRequest {
    pub kind: SourceKind,
    pub show_cursor: bool,
}

/// A live screen-cast session. Dropping it closes the PipeWire fd; call
/// [`ScreenCastSession::close`] to also end the portal session (which removes
/// GNOME's "screen is being shared" indicator).
pub struct ScreenCastSession {
    // The session object is only valid while its proxy is alive.
    _proxy: Screencast,
    session: Session<Screencast>,
    fd: OwnedFd,
    node_id: u32,
    size: Option<(i32, i32)>,
}

impl std::fmt::Debug for ScreenCastSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenCastSession")
            .field("node_id", &self.node_id)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ScreenCastSession {
    /// Opens the portal picker and starts a screen cast.
    pub async fn request(
        request: CaptureRequest,
        parent: Option<&WindowIdentifier>,
    ) -> Result<Self> {
        let proxy = Screencast::new().await.map_err(portal_err)?;

        let available = proxy.available_source_types().await.map_err(portal_err)?;
        let wanted = match request.kind {
            SourceKind::MonitorOrWindow => SourceType::Monitor | SourceType::Window,
            SourceKind::Monitor => SourceType::Monitor.into(),
            SourceKind::Window => SourceType::Window.into(),
        };
        let sources = wanted & available;
        if sources.is_empty() {
            return Err(AppError::Portal(format!(
                "portal does not support the requested sources (available: {available:?})"
            )));
        }

        let cursor = if request.show_cursor {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        };

        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(portal_err)?;

        proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(cursor)
                    .set_sources(sources)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;

        let streams = proxy
            .start(&session, parent, Default::default())
            .await
            .map_err(portal_err)?
            .response()
            .map_err(portal_err)?;

        let stream = streams.streams().first().ok_or(AppError::NoStream)?;
        let node_id = stream.pipe_wire_node_id();
        let size = stream.size();

        let fd = proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .map_err(portal_err)?;

        tracing::info!(node_id, ?size, source_type = ?stream.source_type(), "screen cast started");

        Ok(Self {
            _proxy: proxy,
            session,
            fd,
            node_id,
            size,
        })
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    pub fn pipewire_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Stream size reported by the portal (may be logical pixels).
    pub fn size(&self) -> Option<(i32, i32)> {
        self.size
    }

    /// Resolves when the portal session is closed by the compositor, e.g.
    /// when the user clicks "Stop" on GNOME's screen-sharing indicator.
    pub async fn closed(&self) {
        match self.session.receive_closed().await {
            Ok(mut stream) => {
                stream.next().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot watch portal session; ignoring");
                std::future::pending::<()>().await;
            }
        }
    }

    pub async fn close(&self) {
        if let Err(e) = self.session.close().await {
            tracing::debug!(error = %e, "portal session close failed (probably already closed)");
        }
    }
}

fn portal_err(err: ashpd::Error) -> AppError {
    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => AppError::SelectionCancelled,
        other => {
            tracing::error!(error = %other, "screen cast portal request failed");
            AppError::Portal(other.to_string())
        }
    }
}
