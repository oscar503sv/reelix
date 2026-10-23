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
//! portal Camera → pipewiresrc → crop (1:1 | 4:3) → [videoflip] → scale
//!   → BGRA + alpha mask (circle | rounded rect) → queue → gtk4paintablesink → gtk::Picture
//! ```
//! GTK does not clip the video to the frame's rounded border, so the shape
//! is cut into the frames themselves with the same alpha mask the recording
//! compositor uses. Frames are 2× the bubble's logical size to stay sharp.

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
use crate::recording::camera::{PreviewSpec, build_preview_chain, circle_mask, rounded_rect_mask};
use crate::storage::format_duration;

/// Chameleon border: the colour drifts through the GNOME palette.
pub const CSS: &str = r#"
window.reelix-bubble,
window.reelix-bubble.background {
    background: none;
    background-color: transparent;
    box-shadow: none;
}
.reelix-bubble-frame {
    background: none;
    border: 4px solid #3584e4;
    animation: reelix-chameleon 12s linear infinite;
}
.reelix-bubble-frame.circle {
    border-radius: 9999px;
}
.reelix-bubble-frame.rectangle {
    border-radius: 18px;
}
.reelix-bubble-frame picture {
    background: none;
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
@keyframes reelix-chameleon {
    0%   { border-color: #3584e4; }
    17%  { border-color: #2190a4; }
    33%  { border-color: #3a944a; }
    50%  { border-color: #c88800; }
    67%  { border-color: #ed5b00; }
    83%  { border-color: #c061cb; }
    100% { border-color: #3584e4; }
}
"#;

pub struct CameraBubble {
    window: gtk::Window,
    frame: gtk::Overlay,
    picture: gtk::Picture,
    controls: gtk::Revealer,
    timer: gtk::Label,
    pause: gtk::Button,
    countdown: gtk::Label,
    pipeline: RefCell<Option<(gst::Pipeline, gst::bus::BusWatchGuard)>>,
    access: RefCell<Option<CameraAccess>>,
    recording: Cell<bool>,
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
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();

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
            .child(&picture)
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
            picture,
            controls,
            timer,
            pause,
            countdown,
            pipeline: RefCell::new(None),
            access: RefCell::new(None),
            recording: Cell::new(false),
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
            let pipeline = build_preview(&access, &settings, &this.picture)?;
            let watch = watch_errors(&pipeline)?;
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
        self.picture.add_controller(gesture);
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
/// CSS border width in logical pixels.
const BORDER: i32 = 4;

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

/// Logs preview errors; a broken preview must never affect recording.
fn watch_errors(pipeline: &gst::Pipeline) -> Result<gst::bus::BusWatchGuard> {
    let bus = ElementExt::bus(pipeline)
        .ok_or_else(|| AppError::Pipeline("camera preview has no bus".into()))?;
    bus.add_watch_local(|_, msg| {
        match msg.view() {
            gst::MessageView::Error(err) => tracing::error!(
                error = %err.error(),
                details = ?err.debug(),
                "camera preview error"
            ),
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
    picture: &gtk::Picture,
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
    picture.set_paintable(Some(&paintable));

    // Video area inside the border, at 2× for HiDPI sharpness.
    let (width, height) = bubble_size(settings);
    let video_w = u32::try_from((width - 2 * BORDER) * 2).unwrap_or(400) & !1;
    let video_h = u32::try_from((height - 2 * BORDER) * 2).unwrap_or(400) & !1;
    let (aspect, mask) = match settings.shape {
        CameraShape::Circle => ((1, 1), circle_mask(video_w)),
        CameraShape::Rectangle => (
            (4, 3),
            // Inner radius = outer radius minus the border, doubled.
            rounded_rect_mask(video_w, video_h, f64::from((RECTANGLE_RADIUS - BORDER) * 2)),
        ),
    };
    let spec = PreviewSpec {
        aspect,
        width: video_w,
        height: video_h,
        mirror: settings.mirror,
        mask,
    };
    let pipeline = gst::Pipeline::with_name("reelix-camera-preview");
    build_preview_chain(&pipeline, source, sink, spec)?;
    Ok(pipeline)
}
