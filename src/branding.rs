//! Every user-visible name of the application lives here.
//!
//! Renaming the project should only require changing these constants plus
//! the files under `data/` and the package name in `Cargo.toml`.
//!
//! WARNING: `APP_ID` is what GNOME uses to remember portal permissions
//! (screen cast, camera, microphone). Changing it makes GNOME treat the app
//! as a different one.

/// Reverse-DNS application id used by GTK, D-Bus, portals and the .desktop
/// file. Flathub requires the `io.github.<user>` prefix to match the owner.
pub const APP_ID: &str = "io.github.oscar503sv.Reelix";

/// Human readable name shown in the UI.
pub const APP_NAME: &str = "Reelix";

/// Directory name for recordings: `~/Videos/<DIR_NAME>`.
pub const DIR_NAME: &str = "Reelix";

/// Qualifier/organization/application triple for `directories::ProjectDirs`
/// (on Linux the config dir becomes `~/.config/reelix`).
pub const PROJECT_DIRS: (&str, &str, &str) = ("io.github", "oscar503sv", "Reelix");

/// Prefix of every recording file name: `<prefix>-2026-10-22-210500.mp4`.
pub const RECORDING_PREFIX: &str = "recording";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
