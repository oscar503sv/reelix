//! Camera overlay: a webcam bubble composited onto the screen.
//!
//! ```text
//! screen ─→ videoconvert ─→ queue ─────────────────────────────────────┐ sink_0
//!                                                                       ├→ compositor → …encoder
//! camera ─→ raw 30 fps → crop (1:1 or 4:3) → [flip] → scale → convert ─┘ sink_1
//!                                    (circle only: AYUV + alpha mask probe)
//! ```
//!
//! Shape decides the compositor format, measured on 1080p30 with openh264:
//! - rectangle: I420 throughout, +8 % CPU over no camera;
//! - circle: needs per-pixel alpha, so the compositor works in AYUV
//!   (+28 % CPU). The compositor refuses alpha inputs when it outputs I420.

use gst::prelude::*;

use crate::error::{AppError, Result};

pub const CAMERA_SOURCE_NAME: &str = "camera-source";
const COMPOSITOR_NAME: &str = "compositor";

/// Raw formats accepted from cameras, in order of preference.
const CAMERA_FORMATS: [&str; 9] = [
    "YUY2", "NV12", "UYVY", "I420", "YV12", "BGRx", "RGBx", "BGRA", "RGBA",
];

/// Size of the camera frames fed to the compositor, which scales them to the
/// bubble size. 480 px tall is enough for the largest bubble on 1080p.
const CAMERA_FRAME_HEIGHT: u32 = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rectangle,
    Circle,
}

impl Shape {
    /// Width / height of the bubble.
    fn aspect(self) -> (u32, u32) {
        match self {
            Self::Rectangle => (4, 3),
            Self::Circle => (1, 1),
        }
    }

    fn compositor_format(self) -> &'static str {
        match self {
            Self::Rectangle => "I420",
            Self::Circle => "AYUV",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraLayout {
    pub corner: Corner,
    /// Bubble height as a fraction of the output height.
    pub height_fraction: f64,
    pub shape: Shape,
    /// Flip horizontally, like a mirror.
    pub mirror: bool,
}

/// Where the bubble goes on an output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BubbleRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CameraLayout {
    /// Margin from the frame edges, as a fraction of the output height.
    const MARGIN_FRACTION: f64 = 0.03;

    /// Computes the bubble rectangle for an output of `width`×`height`.
    /// Dimensions are even (required by 4:2:0) and the bubble always fits.
    pub fn bubble_rect(&self, width: u32, height: u32) -> BubbleRect {
        let even = |v: f64| ((v.round().max(2.0)) as u32) & !1;
        let (aw, ah) = self.shape.aspect();
        let fraction = self.height_fraction.clamp(0.05, 0.9);
        let margin = even(f64::from(height) * Self::MARGIN_FRACTION);

        let mut bubble_h = even(f64::from(height) * fraction);
        let mut bubble_w = even(f64::from(bubble_h) * f64::from(aw) / f64::from(ah));
        // Very narrow outputs (e.g. a tall window): shrink to fit.
        let max_w = width.saturating_sub(2 * margin).max(2);
        if bubble_w > max_w {
            bubble_w = max_w & !1;
            bubble_h = even(f64::from(bubble_w) * f64::from(ah) / f64::from(aw));
        }

        let right = width.saturating_sub(bubble_w + margin);
        let bottom = height.saturating_sub(bubble_h + margin);
        let (x, y) = match self.corner {
            Corner::TopLeft => (margin, margin),
            Corner::TopRight => (right, margin),
            Corner::BottomLeft => (margin, bottom),
            Corner::BottomRight => (right, bottom),
        };
        BubbleRect {
            x,
            y,
            width: bubble_w,
            height: bubble_h,
        }
    }

    /// Size of the frames produced by the camera branch.
    fn frame_size(&self) -> (u32, u32) {
        let (aw, ah) = self.shape.aspect();
        (CAMERA_FRAME_HEIGHT * aw / ah, CAMERA_FRAME_HEIGHT)
    }
}

/// Alpha values (0–255) of an anti-aliased circle inscribed in a
/// `size`×`size` square, row-major.
pub fn circle_mask(size: u32) -> Vec<u8> {
    let n = size as usize;
    let radius = f64::from(size) / 2.0;
    let mut mask = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            // Distance from the pixel centre to the circle centre.
            let dx = x as f64 + 0.5 - radius;
            let dy = y as f64 + 0.5 - radius;
            let distance = (dx * dx + dy * dy).sqrt();
            // 1 px wide smooth edge.
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            mask.push((coverage * 255.0).round() as u8);
        }
    }
    mask
}

/// Writes `mask` into the alpha channel of a packed AYUV frame (A is the
/// first byte of each pixel). Returns false if the sizes do not match.
pub fn apply_alpha_mask(ayuv: &mut [u8], mask: &[u8]) -> bool {
    if ayuv.len() != mask.len() * 4 {
        return false;
    }
    for (pixel, &alpha) in ayuv.chunks_exact_mut(4).zip(mask) {
        pixel[0] = alpha;
    }
    true
}

/// Where camera frames come from.
#[derive(Debug, Clone)]
pub enum CameraSource {
    /// A camera exposed by the Camera portal's PipeWire remote. `target` is
    /// the node name of a specific camera (`None` = PipeWire's choice).
    PipeWire {
        fd: std::os::fd::RawFd,
        target: Option<String>,
    },
    /// Any live element producing raw video (tests, future sources).
    Element(gst::Element),
}

#[derive(Debug, Clone)]
pub struct CameraInput {
    pub source: CameraSource,
    pub layout: CameraLayout,
}

/// The composited video head: screen and camera in, one stream out.
pub struct CameraComposition {
    /// Feed the screen into this element.
    pub screen_input: gst::Element,
    /// Take the composited stream from this element.
    pub output: gst::Element,
    /// Compositor pad of the camera, positioned once the output size is known.
    pub camera_pad: gst::Pad,
}

/// Builds compositor + camera branch inside `pipeline`. The caller links its
/// screen source to `screen_input` and `output` to the rest of the chain.
pub fn build(pipeline: &gst::Pipeline, input: &CameraInput, fps: u32) -> Result<CameraComposition> {
    let layout = input.layout;
    let format = layout.shape.compositor_format();

    let compositor = make("compositor", COMPOSITOR_NAME)?;
    compositor.set_property_from_str("background", "black");
    let output_caps = make("capsfilter", "compositor-caps")?;
    output_caps.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", format)
            .field(
                "framerate",
                gst::Fraction::new(i32::try_from(fps).unwrap_or(30), 1),
            )
            .build(),
    );

    // Screen branch: convert once, before compositing.
    let screen_convert = make("videoconvert", "screen-convert")?;
    screen_convert.set_property_from_str("chroma-mode", "none");
    screen_convert.set_property_from_str("dither", "none");
    screen_convert.set_property("n-threads", conversion_threads());
    let screen_queue = leaky_queue("screen-queue")?;

    // Camera branch.
    let camera = make_source(&input.source)?;
    isolate_negotiation(&camera)?;
    let camera_raw = make("capsfilter", "camera-raw-caps")?;
    // Raw formats avoid MJPEG decoding.
    camera_raw.set_property("caps", gst::Caps::new_empty_simple("video/x-raw"));
    let (aw, ah) = layout.shape.aspect();
    let crop = make("aspectratiocrop", "camera-crop")?;
    crop.set_property(
        "aspect-ratio",
        gst::Fraction::new(
            i32::try_from(aw).unwrap_or(1),
            i32::try_from(ah).unwrap_or(1),
        ),
    );
    let flip = make("videoflip", "camera-flip")?;
    flip.set_property_from_str(
        "method",
        if layout.mirror {
            "horizontal-flip"
        } else {
            "none"
        },
    );
    let camera_scale = make("videoscale", "camera-scale")?;
    let camera_convert = make("videoconvert", "camera-convert")?;
    let (frame_w, frame_h) = layout.frame_size();
    let camera_caps = make("capsfilter", "camera-caps")?;
    camera_caps.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", format)
            .field("width", i32::try_from(frame_w).unwrap_or(640))
            .field("height", i32::try_from(frame_h).unwrap_or(480))
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .build(),
    );
    let camera_queue = leaky_queue("camera-queue")?;

    let all = [
        &compositor,
        &output_caps,
        &screen_convert,
        &screen_queue,
        &camera,
        &camera_raw,
        &crop,
        &flip,
        &camera_scale,
        &camera_convert,
        &camera_caps,
        &camera_queue,
    ];
    pipeline
        .add_many(all)
        .map_err(|e| AppError::Pipeline(e.to_string()))?;
    let link = |chain: &[&gst::Element]| {
        gst::Element::link_many(chain.iter().copied())
            .map_err(|e| AppError::Pipeline(format!("camera composition: {e}")))
    };
    link(&[&compositor, &output_caps])?;
    link(&[&screen_convert, &screen_queue])?;
    link(&[
        &camera,
        &camera_raw,
        &crop,
        &flip,
        &camera_scale,
        &camera_convert,
        &camera_caps,
        &camera_queue,
    ])?;

    let screen_pad = request_sink(&compositor, 0)?;
    link_pads(&screen_queue, &screen_pad)?;
    let camera_pad = request_sink(&compositor, 1)?;
    link_pads(&camera_queue, &camera_pad)?;

    if layout.shape == Shape::Circle {
        install_circle_mask(&camera_caps, frame_w)?;
    }

    tracing::info!(?layout, format, "camera composition built");
    Ok(CameraComposition {
        screen_input: screen_convert,
        output: output_caps,
        camera_pad,
    })
}

/// Places the bubble once the output size is known.
pub fn position_bubble(pad: &gst::Pad, layout: &CameraLayout, width: u32, height: u32) {
    let rect = layout.bubble_rect(width, height);
    let to_i32 = |v: u32| i32::try_from(v).unwrap_or(0);
    pad.set_property("xpos", to_i32(rect.x));
    pad.set_property("ypos", to_i32(rect.y));
    pad.set_property("width", to_i32(rect.width));
    pad.set_property("height", to_i32(rect.height));
    tracing::info!(?rect, "camera bubble positioned");
}

fn install_circle_mask(camera_caps: &gst::Element, size: u32) -> Result<()> {
    let mask = circle_mask(size);
    let pad = camera_caps
        .static_pad("src")
        .ok_or_else(|| AppError::Pipeline("camera capsfilter has no src pad".into()))?;
    let warned = std::sync::atomic::AtomicBool::new(false);
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let buffer = buffer.make_mut();
        let applied = buffer
            .map_writable()
            .is_ok_and(|mut map| apply_alpha_mask(map.as_mut_slice(), &mask));
        if !applied && !warned.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!("camera frame layout unexpected; circle mask skipped");
        }
        gst::PadProbeReturn::Ok
    })
    .ok_or_else(|| AppError::Pipeline("failed to install circle mask".into()))?;
    Ok(())
}

/// `pipewiresrc` turns the caps its peer accepts into a PipeWire format
/// request, and that fails as soon as downstream constraints pile up (e.g. a
/// frame rate from the compositor plus the size/format lists of the crop and
/// scale elements: "error during negotiation"). Every element after the
/// source adapts to whatever it produces, so answer its caps query with the
/// common raw formats only (no size or rate) and let the camera pick its own
/// native mode. Webcams typically offer YUY2 or NV12.
fn isolate_negotiation(source: &gst::Element) -> Result<()> {
    let pad = source
        .static_pad("src")
        .ok_or_else(|| AppError::Pipeline("camera source has no src pad".into()))?;
    pad.add_probe(gst::PadProbeType::QUERY_DOWNSTREAM, |_, info| {
        let Some(gst::PadProbeData::Query(query)) = info.data.as_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let gst::QueryViewMut::Caps(caps_query) = query.view_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let raw = gst::Caps::builder("video/x-raw")
            .field("format", gst::List::new(CAMERA_FORMATS))
            .build();
        let result = match caps_query.filter() {
            Some(filter) => filter.intersect(&raw),
            None => raw,
        };
        caps_query.set_result(&result);
        gst::PadProbeReturn::Handled
    })
    .ok_or_else(|| AppError::Pipeline("failed to install camera caps probe".into()))?;
    Ok(())
}

fn make_source(source: &CameraSource) -> Result<gst::Element> {
    match source {
        CameraSource::PipeWire { fd, target } => {
            let src = make("pipewiresrc", CAMERA_SOURCE_NAME)?;
            src.set_property("fd", *fd);
            if let Some(target) = target {
                src.set_property("target-object", target);
            }
            Ok(src)
        }
        CameraSource::Element(element) => {
            element.set_property("name", CAMERA_SOURCE_NAME);
            Ok(element.clone())
        }
    }
}

fn request_sink(compositor: &gst::Element, zorder: u32) -> Result<gst::Pad> {
    let pad = compositor
        .request_pad_simple("sink_%u")
        .ok_or_else(|| AppError::Pipeline("compositor refused a sink pad".into()))?;
    pad.set_property("zorder", zorder);
    Ok(pad)
}

fn link_pads(upstream: &gst::Element, sink: &gst::Pad) -> Result<()> {
    let src = upstream
        .static_pad("src")
        .ok_or_else(|| AppError::Pipeline("element has no src pad".into()))?;
    src.link(sink)
        .map(|_| ())
        .map_err(|e| AppError::Pipeline(format!("cannot link to compositor: {e:?}")))
}

fn leaky_queue(name: &str) -> Result<gst::Element> {
    let queue = make("queue", name)?;
    queue.set_property("max-size-buffers", 2u32);
    queue.set_property("max-size-bytes", 0u32);
    queue.set_property("max-size-time", 0u64);
    queue.set_property_from_str("leaky", "downstream");
    Ok(queue)
}

fn conversion_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(1).min(4))
        .unwrap_or(1)
}

fn make(factory: &str, name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .map_err(|_| AppError::MissingElement(factory.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(corner: Corner, shape: Shape) -> CameraLayout {
        CameraLayout {
            corner,
            height_fraction: 0.25,
            shape,
            mirror: false,
        }
    }

    #[test]
    fn bottom_right_circle_on_1080p() {
        let r = layout(Corner::BottomRight, Shape::Circle).bubble_rect(1920, 1080);
        // 25 % of 1080 = 270; margin 3 % = 32.4 → 32.
        assert_eq!(
            r,
            BubbleRect {
                x: 1920 - 270 - 32,
                y: 1080 - 270 - 32,
                width: 270,
                height: 270
            }
        );
    }

    #[test]
    fn rectangle_is_four_by_three() {
        let r = layout(Corner::TopLeft, Shape::Rectangle).bubble_rect(1920, 1080);
        assert_eq!((r.x, r.y), (32, 32));
        assert_eq!((r.width, r.height), (360, 270));
    }

    #[test]
    fn dimensions_are_even() {
        for (w, h) in [(1366, 768), (1001, 777), (2560, 1440)] {
            for shape in [Shape::Circle, Shape::Rectangle] {
                let r = layout(Corner::BottomRight, shape).bubble_rect(w, h);
                assert_eq!(r.width % 2, 0, "{w}x{h} {shape:?}");
                assert_eq!(r.height % 2, 0, "{w}x{h} {shape:?}");
            }
        }
    }

    #[test]
    fn bubble_always_fits_narrow_outputs() {
        let r = layout(Corner::BottomRight, Shape::Rectangle).bubble_rect(300, 1000);
        assert!(r.x + r.width <= 300);
        assert!(r.y + r.height <= 1000);
    }

    #[test]
    fn circle_mask_is_opaque_in_centre_and_clear_in_corners() {
        let size = 100;
        let mask = circle_mask(size);
        let at = |x: usize, y: usize| mask[y * size as usize + x];
        assert_eq!(at(50, 50), 255);
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(99, 99), 0);
        // The edge is anti-aliased: some pixels are partially transparent.
        assert!(mask.iter().any(|&a| a > 0 && a < 255));
        // Covered area ≈ π r² (within 1 %).
        let area: f64 = mask.iter().map(|&a| f64::from(a) / 255.0).sum();
        let expected = std::f64::consts::PI * 50.0 * 50.0;
        assert!((area - expected).abs() / expected < 0.01, "area {area}");
    }

    #[test]
    fn alpha_mask_writes_first_byte_only() {
        let mut frame = vec![9u8; 2 * 4];
        assert!(apply_alpha_mask(&mut frame, &[0, 200]));
        assert_eq!(frame, [0, 9, 9, 9, 200, 9, 9, 9]);
        assert!(!apply_alpha_mask(&mut frame, &[0]));
    }
}
