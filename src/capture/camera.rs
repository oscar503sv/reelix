//! Webcam access through the XDG Camera portal.
//!
//! The portal hands out a PipeWire remote that only exposes cameras; it works
//! the same on the host and inside Flatpak. Device enumeration uses GStreamer's
//! PipeWire device provider and identifies cameras by `node.name`, which
//! `pipewiresrc target-object` accepts and which stays stable across reboots
//! (unlike the numeric object serial).

use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use ashpd::desktop::ResponseError;
use ashpd::desktop::camera::Camera;
use gst::prelude::*;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDevice {
    /// PipeWire `node.name`.
    pub id: String,
    pub label: String,
}

/// Granted camera access. Keep it alive while recording.
#[derive(Debug)]
pub struct CameraAccess {
    fd: OwnedFd,
}

impl CameraAccess {
    /// Asks the portal for camera access. GNOME asks the user the first time
    /// for sandboxed apps; host apps are granted directly.
    pub async fn request() -> Result<Self> {
        let camera = Camera::new().await.map_err(camera_err)?;
        if !camera.is_present().await.map_err(camera_err)? {
            return Err(AppError::NoCamera);
        }
        camera
            .request_access(Default::default())
            .await
            .map_err(camera_err)?
            .response()
            .map_err(camera_err)?;
        let fd = camera
            .open_pipe_wire_remote(Default::default())
            .await
            .map_err(camera_err)?;
        tracing::info!("camera access granted");
        Ok(Self { fd })
    }

    pub fn pipewire_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// Lists cameras visible to PipeWire.
pub fn list_cameras() -> Vec<VideoDevice> {
    if crate::recording::init().is_err() {
        return Vec::new();
    }
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    if let Err(e) = monitor.start() {
        tracing::warn!(error = %e, "cannot start video device monitor");
        return Vec::new();
    }
    let devices = monitor
        .devices()
        .into_iter()
        .filter_map(|device| {
            let id = device.properties()?.get::<String>("node.name").ok()?;
            Some(VideoDevice {
                id,
                label: device.display_name().to_string(),
            })
        })
        .collect();
    monitor.stop();
    devices
}

fn camera_err(err: ashpd::Error) -> AppError {
    match err {
        ashpd::Error::Response(ResponseError::Cancelled) => AppError::CameraDenied,
        other => {
            tracing::error!(error = %other, "camera portal request failed");
            AppError::Portal(other.to_string())
        }
    }
}
