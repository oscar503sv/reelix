//! The camera bubble: a small floating window with the live camera, like
//! Loom's. When the whole screen is recorded the bubble is captured as part
//! of it, so what you see is what gets recorded and no compositing is
//! needed.
//!
//! Wayland lets the user, not the app, keep a window above others and place
//! it: the bubble can be dragged anywhere, and "Always on Top" is in its
//! window menu (Alt+Space).
//!
//! ```text
//! portal Camera → pipewiresrc → queue → gtk4paintablesink → BubbleVideo
//! ```
//! Frames are not touched on the CPU: they reach GTK in the camera's own
//! format and `BubbleVideo` crops, mirrors, clips and borders them on the
//! GPU, redrawing only when a new frame arrives (see `bubble_video`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

// Only the GStreamer traits needed here: both toolkits define `set_state`.
use gst::prelude::ElementExt;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::capture::camera::CameraAccess;
use crate::config::{CameraSettings, CameraShape, CameraSize};
use crate::error::{AppError, Result};
use crate::recording::camera::{PreviewOutput, build_preview_chain, va_rgb_dmabuf_formats};
use crate::storage::format_duration;
use crate::ui::bubble_video::BubbleVideo;

/// Transparent bubble window; see `bubble_video` for the border.
pub const CSS: &str = r#"
window.reelix-bubble,
window.reelix-bubble.background {
    background: none;
    background-color: transparent;
    box-shadow: none;
}
/* The border is drawn by BubbleVideo; the radius here only clips the
   overlaid controls. */
.reelix-bubble-frame {
    background: none;
}
.reelix-bubble-frame.circle {
    border-radius: 9999px;
}
.reelix-bubble-frame.rectangle {
    border-radius: 18px;
}
.reelix-bubble-controls {
    margin-bottom: 14px;
    padding: 4px 8px;
    border-radius: 9999px;
}
.reelix-countdown {
    font-size: 64px;
    font-weight: 800;
    color: white;
    text-shadow: 0 0 12px rgba(0, 0, 0, 0.8);
}
.reelix-rec-dot {
    color: #ff5f5f;
}
"#;

pub struct CameraBubble {
    window: gtk::Window,
    frame: gtk::Overlay,
    video: BubbleVideo,
    controls: gtk::Revealer,
    timer: gtk::Label,
    pause: gtk::Button,
    countdown: gtk::Label,
    pipeline: RefCell<Option<(gst::Pipeline, gst::bus::BusWatchGuard)>>,
    access: RefCell<Option<CameraAccess>>,
    recording: Cell<bool>,
    /// Set after the GPU preview failed once: use the CPU path from then on.
    cpu_only: Cell<bool>,
    /// Bumped on every show/hide so a slow portal answer cannot resurrect a
    /// bubble that was hidden meanwhile.
    generation: Cell<u64>,
}

impl CameraBubble {
    /// `on_stop` / `on_pause` are called by the bubble's buttons; `on_closed`
    /// when the user closes the bubble window itself.
    pub fn new(
        app: &gtk::Application,
        on_stop: impl Fn() + 'static,
        on_pause: impl Fn() + 'static,
        on_closed: impl Fn() + 'static,
    ) -> Rc<Self> {
        let video = BubbleVideo::default();
        video.set_hexpand(true);
        video.set_vexpand(true);

        let timer = gtk::Label::builder()
            .label("0:00")
            .css_classes(["numeric"])
            .build();
        let stop = gtk::Button::builder()
            .icon_name("media-playback-stop-symbolic")
            .tooltip_text("Detener la grabación")
            .css_classes(["circular", "destructive-action"])
            .build();
        stop.connect_clicked(move |_| on_stop());
        let pause = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text("Pausar")
            .css_classes(["circular"])
            .build();
        pause.connect_clicked(move |_| on_pause());
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["osd", "reelix-bubble-controls"])
            .build();
        bar.append(
            &gtk::Label::builder()
                .label("●")
                .css_classes(["reelix-rec-dot"])
                .build(),
        );
        bar.append(&timer);
        bar.append(&pause);
        bar.append(&stop);
        let controls = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .child(&bar)
            .build();

        let frame = gtk::Overlay::builder()
            .child(&video)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["reelix-bubble-frame"])
            .build();
        frame.add_overlay(&controls);
        let countdown = gtk::Label::builder()
            .css_classes(["reelix-countdown"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        frame.add_overlay(&countdown);

        let window = gtk::Window::builder()
            .application(app)
            .title("Reelix — cámara")
            .decorated(false)
            .resizable(false)
            .css_classes(["reelix-bubble"])
            .child(&frame)
            .build();
        // GTK gives every window an opaque themed background; without
        // removing it the round bubble sits on a dark square.
        window.remove_css_class("background");

        let this = Rc::new(Self {
            window,
            frame,
            video,
            controls,
            timer,
            pause,
            countdown,
            pipeline: RefCell::new(None),
            access: RefCell::new(None),
            recording: Cell::new(false),
            cpu_only: Cell::new(false),
            generation: Cell::new(0),
        });
        this.connect_drag();
        this.connect_hover();

        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                this.hide();
            }
            on_closed();
            glib::Propagation::Stop
        });
        this
    }

    pub fn is_shown(&self) -> bool {
        self.window.is_visible()
    }

    /// Opens the camera and shows the bubble (or restyles it if shown).
    pub fn show(
        self: &Rc<Self>,
        settings: &CameraSettings,
    ) -> impl std::future::Future<Output = Result<()>> + use<> {
        self.apply_style(settings);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let this = Rc::clone(self);
        let settings = settings.clone();
        async move {
            this.stop_pipeline();
            // Take the borrow out before awaiting the portal.
            let existing = this.access.borrow_mut().take();
            let access = match existing {
                Some(access) => access,
                None => CameraAccess::request().await?,
            };
            if this.generation.get() != generation {
                return Ok(());
            }
            let output = preview_output(this.cpu_only.get());
            tracing::info!(?output, "camera preview path");
            let pipeline = build_preview(&access, &settings, &this.video, &output)?;
            let on_error = {
                let weak = Rc::downgrade(&this);
                let gpu = matches!(output, PreviewOutput::Dmabuf(_));
                move || {
                    let Some(this) = weak.upgrade() else { return };
                    if !gpu || this.cpu_only.replace(true) {
                        return;
                    }
                    tracing::warn!("GPU camera preview failed; retrying on the CPU");
                    let settings = settings.clone();
                    // Not from inside the bus watch being torn down.
                    glib::idle_add_local_once(move || {
                        if !this.is_shown() {
                            return;
                        }
                        glib::spawn_future_local(async move {
                            if let Err(e) = this.show(&settings).await {
                                tracing::error!(error = %e, "camera preview fallback failed");
                            }
                        });
                    });
                }
            };
            let watch = watch_errors(&pipeline, on_error)?;
            ElementExt::set_state(&pipeline, gst::State::Playing)
                .map_err(|e| AppError::Pipeline(format!("camera preview: {e}")))?;
            *this.pipeline.borrow_mut() = Some((pipeline, watch));
            *this.access.borrow_mut() = Some(access);
            this.window.present();
            tracing::info!("camera bubble shown");
            Ok(())
        }
    }

    /// Hides the bubble and releases the camera.
    pub fn hide(&self) {
        self.generation.set(self.generation.get() + 1);
        self.stop_pipeline();
        self.access.borrow_mut().take();
        self.window.set_visible(false);
    }

    pub fn destroy(&self) {
        self.window.destroy();
    }

    /// Shows the REC indicator and stop button (on hover) while recording.
    pub fn set_recording(&self, recording: bool) {
        self.recording.set(recording);
        if !recording {
            self.controls.set_reveal_child(false);
            self.timer.set_label("0:00");
        }
    }

    /// Big number over the camera before capture starts; `None` hides it.
    pub fn set_countdown(&self, remaining: Option<u32>) {
        match remaining {
            Some(n) => {
                self.countdown.set_label(&n.to_string());
                self.countdown.set_visible(true);
            }
            None => self.countdown.set_visible(false),
        }
    }

    /// Pause button shows "resume" while paused; controls stay visible so
    /// the paused state is obvious.
    pub fn set_paused(&self, paused: bool) {
        let (icon, tip) = if paused {
            ("media-playback-start-symbolic", "Reanudar")
        } else {
            ("media-playback-pause-symbolic", "Pausar")
        };
        self.pause.set_icon_name(icon);
        self.pause.set_tooltip_text(Some(tip));
        if paused {
            self.controls.set_reveal_child(true);
        }
    }

    pub fn set_elapsed(&self, elapsed: Duration) {
        self.timer
            .set_label(&format_duration(elapsed.as_secs_f64().floor()));
    }

    fn apply_style(&self, settings: &CameraSettings) {
        let (width, height) = bubble_size(settings);
        self.window.set_default_size(width, height);
        self.frame.set_size_request(width, height);
        let (add, remove) = match settings.shape {
            CameraShape::Circle => ("circle", "rectangle"),
            CameraShape::Rectangle => ("rectangle", "circle"),
        };
        self.frame.remove_css_class(remove);
        self.frame.add_css_class(add);
        self.video.set_radius(match settings.shape {
            CameraShape::Circle => f32::INFINITY,
            CameraShape::Rectangle => RECTANGLE_RADIUS as f32,
        });
        self.video.set_mirror(settings.mirror);
    }

    fn stop_pipeline(&self) {
        if let Some((pipeline, _watch)) = self.pipeline.borrow_mut().take() {
            let _ = ElementExt::set_state(&pipeline, gst::State::Null);
        }
    }

    /// Dragging anywhere on the camera moves the window (Wayland only lets
    /// the compositor move windows, on the user's request).
    fn connect_drag(&self) {
        let gesture = gtk::GestureClick::new();
        let window = self.window.clone();
        gesture.connect_pressed(move |gesture, _, x, y| {
            let Some(toplevel) = window.surface().and_downcast::<gdk::Toplevel>() else {
                return;
            };
            let Some(device) = gesture.current_event_device() else {
                return;
            };
            let button = i32::try_from(gesture.current_button()).unwrap_or(1);
            toplevel.begin_move(&device, button, x, y, gesture.current_event_time());
        });
        self.video.add_controller(gesture);
    }

    fn connect_hover(self: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.controls.set_reveal_child(this.recording.get());
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.controls.set_reveal_child(false);
            }
        });
        self.frame.add_controller(motion);
    }
}

/// Corner radius of the rectangle bubble, in logical pixels (matches CSS).
const RECTANGLE_RADIUS: i32 = 18;

/// Bubble size in logical pixels, border included.
fn bubble_size(settings: &CameraSettings) -> (i32, i32) {
    let height = match settings.size {
        CameraSize::Small => 160,
        CameraSize::Medium => 220,
        CameraSize::Large => 300,
    };
    let width = match settings.shape {
        CameraShape::Circle => height,
        CameraShape::Rectangle => height * 4 / 3,
    };
    (width, height)
}

/// Logs preview errors (calling `on_error`); a broken preview must never
/// affect recording.
fn watch_errors(
    pipeline: &gst::Pipeline,
    on_error: impl Fn() + 'static,
) -> Result<gst::bus::BusWatchGuard> {
    let bus = ElementExt::bus(pipeline)
        .ok_or_else(|| AppError::Pipeline("camera preview has no bus".into()))?;
    bus.add_watch_local(move |_, msg| {
        match msg.view() {
            gst::MessageView::Error(err) => {
                tracing::error!(
                    error = %err.error(),
                    details = ?err.debug(),
                    "camera preview error"
                );
                on_error();
            }
            gst::MessageView::Warning(w) => {
                tracing::warn!(warning = %w.error(), "camera preview warning")
            }
            _ => {}
        }
        glib::ControlFlow::Continue
    })
    .map_err(|e| AppError::Pipeline(format!("camera preview bus: {e}")))
}

fn build_preview(
    access: &CameraAccess,
    settings: &CameraSettings,
    video: &BubbleVideo,
    output: &PreviewOutput,
) -> Result<gst::Pipeline> {
    crate::recording::init()?;
    let source = gst::ElementFactory::make("pipewiresrc")
        .build()
        .map_err(|_| AppError::MissingElement("pipewiresrc".into()))?;
    source.set_property("fd", access.pipewire_fd());
    if let Some(device) = &settings.device {
        source.set_property("target-object", device);
    }
    let sink = gst::ElementFactory::make("gtk4paintablesink")
        .build()
        .map_err(|_| AppError::MissingElement("gtk4paintablesink".into()))?;
    let paintable = sink.property::<gdk::Paintable>("paintable");
    video.set_paintable(Some(&paintable));
    let pipeline = gst::Pipeline::with_name("reelix-camera-preview");
    build_preview_chain(&pipeline, source, sink, output)?;
    Ok(pipeline)
}

/// GPU conversion into a DMA-BUF when VA-API can produce an RGB format GTK
/// imports; otherwise (or after a failure) BGRx on the CPU.
fn preview_output(cpu_only: bool) -> PreviewOutput {
    if !cpu_only && let Some(display) = gdk::Display::default() {
        let supported = display.dmabuf_formats();
        if let Some((name, ..)) = va_rgb_dmabuf_formats()
            .into_iter()
            .find(|&(_, fourcc, modifier)| supported.contains(fourcc, modifier))
        {
            return PreviewOutput::Dmabuf(name);
        }
    }
    PreviewOutput::SystemBgrx
}
