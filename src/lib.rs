//! Reelix: native screen recorder for GNOME/Wayland.
//!
//! The crate is split in two layers:
//! - toolkit-independent core (`app::state`, `config`, `storage`,
//!   `recording::encoder`) that is unit-tested without a display;
//! - integration layers (`capture`, `recording::pipeline`, `ui`) that talk to
//!   the XDG portals, PipeWire, GStreamer and GTK.

pub mod app;
pub mod branding;
pub mod capture;
pub mod config;
pub mod error;
pub mod i18n;
pub mod recording;
pub mod storage;
pub mod ui;
