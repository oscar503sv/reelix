//! Renders the app's screenshots for AppStream/Flathub and the README
//! (`reelix --screenshots DIR`, hidden). GTK draws each view straight into
//! a PNG at 2× scale, so the result does not depend on the desktop, the
//! shell or the user's theme. `packaging/screenshots.sh` prepares a clean
//! config with sample recordings and runs this.

use std::path::{Path, PathBuf};
use std::time::Duration;

use adw::prelude::*;
use gtk::{glib, graphene};

/// Output scale: HiDPI-sharp screenshots.
const SCALE: f32 = 2.0;
/// Main window size for the screenshots (logical pixels).
const MAIN_SIZE: (i32, i32) = (540, 1240);
/// Time for layout, fonts and dialog animations to settle.
const SETTLE: Duration = Duration::from_millis(1200);

pub async fn capture_all(app: adw::Application, window: adw::ApplicationWindow, dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!(error = %e, "cannot create screenshot directory");
        app.quit();
        return;
    }
    let style = adw::StyleManager::default();
    style.set_color_scheme(adw::ColorScheme::ForceLight);
    // Tall enough to show the recent recordings.
    window.set_default_size(MAIN_SIZE.0, MAIN_SIZE.1);
    glib::timeout_future(SETTLE).await;
    save(&window, &dir.join("main.png"));

    // Dialogs float over a wider window instead of becoming bottom sheets.
    window.set_default_size(920, 720);
    glib::timeout_future(SETTLE).await;
    for (action, name) in [
        ("library", "library.png"),
        ("preferences", "preferences.png"),
    ] {
        app.activate_action(action, None);
        glib::timeout_future(SETTLE).await;
        save(&window, &dir.join(name));
        if let Some(dialog) = window.visible_dialog() {
            dialog.close();
        }
        glib::timeout_future(SETTLE).await;
    }

    window.set_default_size(MAIN_SIZE.0, MAIN_SIZE.1);
    style.set_color_scheme(adw::ColorScheme::ForceDark);
    glib::timeout_future(SETTLE).await;
    save(&window, &dir.join("main-dark.png"));
    app.quit();
}

fn save(widget: &impl IsA<gtk::Widget>, path: &Path) {
    match render(widget.as_ref(), path) {
        Ok(()) => tracing::info!(path = %path.display(), "screenshot saved"),
        Err(e) => tracing::error!(error = %e, path = %path.display(), "screenshot failed"),
    }
}

fn render(widget: &gtk::Widget, path: &Path) -> Result<(), String> {
    let renderer = widget
        .native()
        .and_then(|n| n.renderer())
        .ok_or("widget is not realized")?;
    let (w, h) = (widget.width() as f32, widget.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(SCALE, SCALE);
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let node = snapshot.to_node().ok_or("nothing was drawn")?;
    let viewport = graphene::Rect::new(0.0, 0.0, w * SCALE, h * SCALE);
    renderer
        .render_texture(&node, Some(&viewport))
        .save_to_png(path)
        .map_err(|e| e.to_string())
}
