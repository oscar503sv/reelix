//! The bubble's video surface. It draws the camera frames exactly as
//! `gtk4paintablesink` delivers them (native size and format) and lets the
//! GPU do the rest in one pass:
//!
//! - colour conversion (YUY2/NV12 textures are converted by GTK's renderer);
//! - crop to the bubble ("cover" fit) and scaling;
//! - mirror (a transform);
//! - round shape (a rounded clip, not an alpha mask written on the CPU);
//! - the chameleon border, whose colour is computed at draw time.
//!
//! It redraws only when the camera delivers a frame: no CSS animation keeps
//! the frame clock (and gnome-shell) busy at the monitor's refresh rate.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Border width in logical pixels.
const BORDER: f32 = 4.0;
/// Seconds for a full trip around the palette.
const CYCLE_SECONDS: f64 = 12.0;
/// GNOME palette the border drifts through (the first colour closes the loop).
const PALETTE: [(u8, u8, u8); 6] = [
    (0x35, 0x84, 0xe4),
    (0x21, 0x90, 0xa4),
    (0x3a, 0x94, 0x4a),
    (0xc8, 0x88, 0x00),
    (0xed, 0x5b, 0x00),
    (0xc0, 0x61, 0xcb),
];

/// Border colour (sRGB, 0–1) `seconds` into the animation.
pub fn chameleon_color(seconds: f64) -> (f32, f32, f32) {
    let phase = (seconds / CYCLE_SECONDS).rem_euclid(1.0) * PALETTE.len() as f64;
    let index = (phase.floor() as usize).min(PALETTE.len() - 1);
    let t = (phase - index as f64) as f32;
    let (a, b) = (PALETTE[index], PALETTE[(index + 1) % PALETTE.len()]);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) / 255.0;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Rectangle (x, y, width, height) that covers `width`×`height` with an
/// image of the given aspect ratio, centred; the overflow gets clipped.
pub fn cover_rect(width: f32, height: f32, aspect: f64) -> (f32, f32, f32, f32) {
    if width <= 0.0 || height <= 0.0 || aspect <= 0.0 {
        return (0.0, 0.0, width.max(0.0), height.max(0.0));
    }
    let aspect = aspect as f32;
    if width / height > aspect {
        let h = width / aspect;
        (0.0, (height - h) / 2.0, width, h)
    } else {
        let w = height * aspect;
        ((width - w) / 2.0, 0.0, w, height)
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BubbleVideo {
        pub paintable: RefCell<Option<(gdk::Paintable, Vec<glib::SignalHandlerId>)>>,
        pub mirror: Cell<bool>,
        /// Outer corner radius; clamped to half the shortest side (circle).
        pub radius: Cell<f32>,
        pub started_at: Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BubbleVideo {
        const NAME: &'static str = "ReelixBubbleVideo";
        type Type = super::BubbleVideo;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for BubbleVideo {
        fn constructed(&self) {
            self.parent_constructed();
            self.started_at.set(glib::monotonic_time());
        }

        fn dispose(&self) {
            self.obj().set_paintable(None);
        }
    }

    impl WidgetImpl for BubbleVideo {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            if w <= 2.0 * BORDER || h <= 2.0 * BORDER {
                return;
            }
            let radius = self.radius.get().clamp(0.0, w.min(h) / 2.0);

            if let Some((paintable, _)) = self.paintable.borrow().as_ref() {
                let inner_bounds =
                    graphene::Rect::new(BORDER, BORDER, w - 2.0 * BORDER, h - 2.0 * BORDER);
                let inner = gsk::RoundedRect::from_rect(inner_bounds, (radius - BORDER).max(0.0));
                let (x, y, cw, ch) = cover_rect(
                    inner_bounds.width(),
                    inner_bounds.height(),
                    paintable.intrinsic_aspect_ratio(),
                );
                snapshot.push_rounded_clip(&inner);
                snapshot.save();
                if self.mirror.get() {
                    snapshot.translate(&graphene::Point::new(w, 0.0));
                    snapshot.scale(-1.0, 1.0);
                }
                snapshot.translate(&graphene::Point::new(BORDER + x, BORDER + y));
                paintable.snapshot(snapshot, f64::from(cw), f64::from(ch));
                snapshot.restore();
                snapshot.pop();
            }

            let elapsed = (glib::monotonic_time() - self.started_at.get()) as f64 / 1e6;
            let (r, g, b) = chameleon_color(elapsed);
            let color = gdk::RGBA::new(r, g, b, 1.0);
            let outer = gsk::RoundedRect::from_rect(graphene::Rect::new(0.0, 0.0, w, h), radius);
            snapshot.append_border(&outer, &[BORDER; 4], &[color; 4]);
        }
    }
}

glib::wrapper! {
    pub struct BubbleVideo(ObjectSubclass<imp::BubbleVideo>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for BubbleVideo {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl BubbleVideo {
    /// Shows `paintable` (or nothing); redraws whenever it changes.
    pub fn set_paintable(&self, paintable: Option<&gdk::Paintable>) {
        let imp = self.imp();
        if let Some((old, handlers)) = imp.paintable.borrow_mut().take() {
            for id in handlers {
                old.disconnect(id);
            }
        }
        if let Some(paintable) = paintable {
            let weak = self.downgrade();
            let contents = paintable.connect_invalidate_contents(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.queue_draw();
                }
            });
            let weak = self.downgrade();
            let size = paintable.connect_invalidate_size(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.queue_draw();
                }
            });
            *imp.paintable.borrow_mut() = Some((paintable.clone(), vec![contents, size]));
        }
        self.queue_draw();
    }

    pub fn set_mirror(&self, mirror: bool) {
        self.imp().mirror.set(mirror);
        self.queue_draw();
    }

    /// Outer corner radius in logical pixels; `f32::INFINITY` for a circle.
    pub fn set_radius(&self, radius: f32) {
        self.imp().radius.set(radius);
        self.queue_draw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_side() {
        // 4:3 camera into a square: full height, sides cut.
        let (x, y, w, h) = cover_rect(300.0, 300.0, 4.0 / 3.0);
        assert_eq!((y, h), (0.0, 300.0));
        assert!((w - 400.0).abs() < 1e-3 && (x + 50.0).abs() < 1e-3);
        // 16:9 camera into 4:3: full height.
        let (_, y, _, h) = cover_rect(400.0, 300.0, 16.0 / 9.0);
        assert_eq!((y, h), (0.0, 300.0));
        // Portrait image into landscape: full width, top/bottom cut.
        let (x, y, w, h) = cover_rect(400.0, 300.0, 0.5);
        assert_eq!((x, w), (0.0, 400.0));
        assert!((h - 800.0).abs() < 1e-3 && (y + 250.0).abs() < 1e-3);
    }

    #[test]
    fn cover_handles_unknown_aspect() {
        assert_eq!(cover_rect(100.0, 50.0, 0.0), (0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn chameleon_cycles_through_the_palette() {
        let near = |a: (f32, f32, f32), b: (u8, u8, u8), tolerance: f32| {
            let b = (
                f32::from(b.0) / 255.0,
                f32::from(b.1) / 255.0,
                f32::from(b.2) / 255.0,
            );
            (a.0 - b.0).abs() < tolerance
                && (a.1 - b.1).abs() < tolerance
                && (a.2 - b.2).abs() < tolerance
        };
        let close = |a, b| near(a, b, 1e-3);
        assert!(close(chameleon_color(0.0), PALETTE[0]));
        assert!(close(chameleon_color(CYCLE_SECONDS), PALETTE[0]));
        assert!(close(
            chameleon_color(CYCLE_SECONDS / 6.0 * 3.0),
            PALETTE[3]
        ));
        // Last segment blends back into the first colour.
        assert!(near(
            chameleon_color(CYCLE_SECONDS * 0.999),
            PALETTE[0],
            0.01
        ));
        // Negative times wrap too.
        assert!(close(chameleon_color(-CYCLE_SECONDS), PALETTE[0]));
    }
}
