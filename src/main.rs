use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use reelix::branding;
use reelix::config::Settings;
use reelix::storage::{Library, format_duration};
use reelix::ui::{self, LaunchOptions};

/// Native screen recorder for GNOME/Wayland.
#[derive(Debug, Parser)]
#[command(name = env!("CARGO_PKG_NAME"), version, about)]
struct Cli {
    /// Open the screen picker and start recording right away.
    #[arg(long)]
    record: bool,

    /// List saved recordings and exit.
    #[arg(long, conflicts_with = "record")]
    list: bool,

    /// Render screenshots into DIR and quit (for AppStream/README).
    #[arg(long, value_name = "DIR", hide = true)]
    screenshots: Option<std::path::PathBuf>,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,zbus=warn,zbus::proxy=error")),
        )
        .init();

    if is_root() {
        eprintln!("{} no debe ejecutarse como root.", branding::APP_NAME);
        return ExitCode::FAILURE;
    }

    let cli = Cli::parse();
    if cli.list {
        return list_recordings();
    }

    tracing::info!(
        version = branding::VERSION,
        "starting {}",
        branding::APP_NAME
    );
    let code = ui::run(LaunchOptions {
        record_on_start: cli.record,
        screenshots: cli.screenshots,
    });
    ExitCode::from(code.get())
}

fn list_recordings() -> ExitCode {
    let library = Library::new(Settings::load_or_default().recording_dir());
    match library.list() {
        Ok(recordings) if recordings.is_empty() => {
            println!("No hay grabaciones en {}", library.dir().display());
            ExitCode::SUCCESS
        }
        Ok(recordings) => {
            for r in recordings {
                let duration = r
                    .metadata
                    .as_ref()
                    .map(|m| format_duration(m.duration))
                    .unwrap_or_else(|| "--:--".into());
                let date = r
                    .created_at()
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!("{:<36} {:>8}  {}  {}", r.stem(), duration, date, r.title());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}: {}", e.user_message().0, e);
            ExitCode::FAILURE
        }
    }
}

fn is_root() -> bool {
    // Reading the effective uid without libc/unsafe: /proc/self/status
    // contains "Uid:\t<real>\t<effective>\t...".
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|uids| uids.split_whitespace().nth(1).map(|e| e == "0"))
        })
        .unwrap_or(false)
}
