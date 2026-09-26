//! X Render 0.11 implementation over Kinakaze's native drawable surfaces.
//!
//! Render pictures are kept as server-side resources, just as they are on Xorg.
//! Window and pixmap pictures share the 32-bit top-down DIB surfaces provided by
//! `graphics`; solid colours, gradients and glyph masks are sampled in premultiplied
//! linear arithmetic and committed to a drawable as one dirty rectangle.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::os::raw::{c_int, c_short, c_ushort};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::GdiPlus::PointF;

use crate::graphics::{DrawableSnapshot, drawable_snapshot, mutate_drawable};
use crate::region::Region;
use crate::{Atom, Bool, Cursor, Display, Drawable, Pixmap, Status, Visual, XRectangle};

type c_ulong = u64;
pub type Picture = usize;
pub type GlyphSet = usize;
pub type Glyph = u32;
pub type XFixed = i32;

const FIXED_ONE: f32 = 65_536.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XRenderPictFormat {
    pub id: usize,
    pub type_: c_int,
    pub depth: c_int,
    pub direct: [c_short; 8],
    pub colormap: usize,
}

static mut STANDARD_FORMAT_ARGB32: XRenderPictFormat = XRenderPictFormat {
    id: 1,
    type_: 1,
    depth: 32,
    direct: [16, 0xff, 8, 0xff, 0, 0xff, 24, 0xff],
    colormap: 0,
};
static mut STANDARD_FORMAT_RGB24: XRenderPictFormat = XRenderPictFormat {
    id: 2,
    type_: 1,
    depth: 24,
    direct: [16, 0xff, 8, 0xff, 0, 0xff, 0, 0],
    colormap: 0,
};
static mut STANDARD_FORMAT_A8: XRenderPictFormat = XRenderPictFormat {
    id: 3,
    type_: 1,
    depth: 8,
    direct: [0, 0, 0, 0, 0, 0, 0, 0xff],
    colormap: 0,
};
static mut STANDARD_FORMAT_A4: XRenderPictFormat = XRenderPictFormat {
    id: 4,
    type_: 1,
    depth: 4,
    direct: [0, 0, 0, 0, 0, 0, 0, 0x0f],
    colormap: 0,
};
static mut STANDARD_FORMAT_A1: XRenderPictFormat = XRenderPictFormat {
    id: 5,
    type_: 1,
    depth: 1,
    direct: [0, 0, 0, 0, 0, 0, 0, 1],
    colormap: 0,
};

fn format_by_id(id: usize) -> Option<XRenderPictFormat> {
    unsafe {
        match id {
            1 => Some(STANDARD_FORMAT_ARGB32),
            2 => Some(STANDARD_FORMAT_RGB24),
            3 => Some(STANDARD_FORMAT_A8),
            4 => Some(STANDARD_FORMAT_A4),
            5 => Some(STANDARD_FORMAT_A1),
            _ => None,
        }
    }
}

fn format_pointer(id: usize) -> *mut XRenderPictFormat {
    match id {
        1 => &raw mut STANDARD_FORMAT_ARGB32,
        2 => &raw mut STANDARD_FORMAT_RGB24,
        3 => &raw mut STANDARD_FORMAT_A8,
        4 => &raw mut STANDARD_FORMAT_A4,
        5 => &raw mut STANDARD_FORMAT_A1,
        _ => core::ptr::null_mut(),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XRenderPictureAttributes {
    pub repeat: c_int,
    pub alpha_map: Picture,
    pub alpha_x_origin: c_int,
    pub alpha_y_origin: c_int,
    pub clip_x_origin: c_int,
    pub clip_y_origin: c_int,
    pub clip_mask: Pixmap,
    pub graphics_exposures: Bool,
    pub subwindow_mode: c_int,
    pub poly_edge: c_int,
    pub poly_mode: c_int,
    pub dither: Atom,
    pub component_alpha: Bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XRenderColor {
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub alpha: c_ushort,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XGlyphInfo {
    pub width: c_ushort,
    pub height: c_ushort,
    pub x: c_short,
    pub y: c_short,
    pub xOff: c_short,
    pub yOff: c_short,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XPointDouble {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XPointFixed {
    pub x: XFixed,
    pub y: XFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XLineFixed {
    pub p1: XPointFixed,
    pub p2: XPointFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XTriangle {
    pub p1: XPointFixed,
    pub p2: XPointFixed,
    pub p3: XPointFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XTrapezoid {
    pub top: XFixed,
    pub bottom: XFixed,
    pub left: XLineFixed,
    pub right: XLineFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct XTransform {
    pub matrix: [[XFixed; 3]; 3],
}

impl Default for XTransform {
    fn default() -> Self {
        Self {
            matrix: [[65_536, 0, 0], [0, 65_536, 0], [0, 0, 65_536]],
        }
    }
}

#[repr(C)]
pub struct XFilters {
    pub nfilter: c_int,
    pub filter: *mut *mut c_char,
    pub nalias: c_int,
    pub alias: *mut c_short,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XIndexValue {
    pub pixel: c_ulong,
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub alpha: c_ushort,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XAnimCursor {
    pub cursor: Cursor,
    pub delay: c_ulong,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XSpanFix {
    pub left: XFixed,
    pub right: XFixed,
    pub y: XFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XTrap {
    pub top: XSpanFix,
    pub bottom: XSpanFix,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XCircle {
    pub x: XFixed,
    pub y: XFixed,
    pub radius: XFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XLinearGradient {
    pub p1: XPointFixed,
    pub p2: XPointFixed,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XRadialGradient {
    pub inner: XCircle,
    pub outer: XCircle,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XConicalGradient {
    pub center: XPointFixed,
    pub angle: XFixed,
}

#[repr(C)]
pub struct XGlyphElt8 {
    pub glyphset: GlyphSet,
    pub chars: *const c_char,
    pub nchars: c_int,
    pub xOff: c_int,
    pub yOff: c_int,
}

#[repr(C)]
pub struct XGlyphElt16 {
    pub glyphset: GlyphSet,
    pub chars: *const c_ushort,
    pub nchars: c_int,
    pub xOff: c_int,
    pub yOff: c_int,
}

#[repr(C)]
pub struct XGlyphElt32 {
    pub glyphset: GlyphSet,
    pub chars: *const u32,
    pub nchars: c_int,
    pub xOff: c_int,
    pub yOff: c_int,
}

#[derive(Clone, Copy, Debug, Default)]
struct Pixel {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Pixel {
    const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    fn clamp(self) -> Self {
        Self {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
            a: self.a.clamp(0.0, 1.0),
        }
    }

    fn scale(self, value: f32) -> Self {
        Self {
            r: self.r * value,
            g: self.g * value,
            b: self.b * value,
            a: self.a * value,
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            r: self.r + other.r,
            g: self.g + other.g,
            b: self.b + other.b,
            a: self.a + other.a,
        }
    }

    fn from_render_color(color: XRenderColor, premultiply: bool) -> Self {
        let alpha = color.alpha as f32 / 65_535.0;
        let mut value = Self {
            r: color.red as f32 / 65_535.0,
            g: color.green as f32 / 65_535.0,
            b: color.blue as f32 / 65_535.0,
            a: alpha,
        };
        if premultiply {
            value.r *= alpha;
            value.g *= alpha;
            value.b *= alpha;
        }
        value.clamp()
    }

    fn from_raw(raw: u32, format: usize) -> Self {
        let byte = |shift: u32| ((raw >> shift) & 0xffu32) as f32 / 255.0;
        match format {
            1 => Self {
                r: byte(16),
                g: byte(8),
                b: byte(0),
                a: byte(24),
            },
            2 => Self {
                r: byte(16),
                g: byte(8),
                b: byte(0),
                a: 1.0,
            },
            3 => Self {
                a: byte(0),
                ..Self::TRANSPARENT
            },
            4 => Self {
                a: ((raw & 0xf) as f32) / 15.0,
                ..Self::TRANSPARENT
            },
            5 => Self {
                a: if raw & 1 != 0 { 1.0 } else { 0.0 },
                ..Self::TRANSPARENT
            },
            _ => Self::TRANSPARENT,
        }
    }

    fn to_raw(self, format: usize) -> u32 {
        let value = self.clamp();
        let byte = |channel: f32| (channel * 255.0 + 0.5) as u32;
        match format {
            1 => {
                (byte(value.a) << 24) | (byte(value.r) << 16) | (byte(value.g) << 8) | byte(value.b)
            }
            2 => 0xff00_0000 | (byte(value.r) << 16) | (byte(value.g) << 8) | byte(value.b),
            3 => byte(value.a),
            4 => (value.a * 15.0 + 0.5) as u32,
            5 => u32::from(value.a >= 0.5),
            _ => 0,
        }
    }
}

#[derive(Clone, Debug)]
enum PictureSource {
    Drawable {
        drawable: Drawable,
    },
    Solid(Pixel),
    Linear {
        gradient: XLinearGradient,
        stops: Vec<f32>,
        colors: Vec<Pixel>,
    },
    Radial {
        gradient: XRadialGradient,
        stops: Vec<f32>,
        colors: Vec<Pixel>,
    },
    Conical {
        gradient: XConicalGradient,
        stops: Vec<f32>,
        colors: Vec<Pixel>,
    },
}

#[derive(Clone)]
struct PictureRec {
    source: PictureSource,
    format: usize,
    attributes: XRenderPictureAttributes,
    transform: XTransform,
    filter: String,
    filter_params: Vec<f32>,
    clip: Option<Vec<RECT>>,
}

impl PictureRec {
    fn new(source: PictureSource, format: usize) -> Self {
        Self {
            source,
            format,
            attributes: XRenderPictureAttributes::default(),
            transform: XTransform::default(),
            filter: "nearest".to_owned(),
            filter_params: Vec::new(),
            clip: None,
        }
    }
}

struct PictureRegistry {
    next: Picture,
    pictures: HashMap<Picture, PictureRec>,
    subpixel_order: c_int,
}

fn pictures() -> &'static Mutex<PictureRegistry> {
    static PICTURES: OnceLock<Mutex<PictureRegistry>> = OnceLock::new();
    PICTURES.get_or_init(|| {
        Mutex::new(PictureRegistry {
            next: 0x6000,
            pictures: HashMap::new(),
            subpixel_order: 0,
        })
    })
}
pub(crate) fn picture_clip(id: usize) -> Option<crate::region::RegionRec> {
    let record = pictures().lock().ok()?.pictures.get(&id)?.clone();
    let rects = if let Some(clip) = record.clip {
        clip.iter()
            .map(|r| XRectangle {
                x: r.left as i16,
                y: r.top as i16,
                width: (r.right - r.left).max(0).min(65535) as u16,
                height: (r.bottom - r.top).max(0).min(65535) as u16,
            })
            .collect()
    } else if let PictureSource::Drawable { drawable } = record.source {
        let (w, h, _) = crate::graphics::drawable_dimensions(drawable)?;
        vec![XRectangle {
            x: 0,
            y: 0,
            width: w.min(65535) as u16,
            height: h.min(65535) as u16,
        }]
    } else {
        return None;
    };
    Some(crate::region::RegionRec {
        rects,
        extents: XRectangle::default(),
    })
}

fn insert_picture(resource: PictureRec) -> Picture {
    let Ok(mut registry) = pictures().lock() else {
        return 0;
    };
    let id = registry.next;
    registry.next = registry.next.saturating_add(1);
    registry.pictures.insert(id, resource);
    id
}

#[derive(Clone, Debug)]
struct GlyphRec {
    info: XGlyphInfo,
    format: usize,
    pixels: Vec<Pixel>,
}

#[derive(Debug)]
struct GlyphSetData {
    format: usize,
    glyphs: HashMap<Glyph, GlyphRec>,
}

struct GlyphRegistry {
    next: GlyphSet,
    sets: HashMap<GlyphSet, Arc<Mutex<GlyphSetData>>>,
}

fn glyph_sets() -> &'static Mutex<GlyphRegistry> {
    static GLYPHS: OnceLock<Mutex<GlyphRegistry>> = OnceLock::new();
    GLYPHS.get_or_init(|| {
        Mutex::new(GlyphRegistry {
            next: 0x7000,
            sets: HashMap::new(),
        })
    })
}

fn fixed(value: XFixed) -> f32 {
    value as f32 / FIXED_ONE
}

pub(crate) fn available() -> bool {
    // The software implementation only needs the real DIB drawable backend.
    true
}

#[derive(Clone)]
struct PreparedPicture {
    record: PictureRec,
    surface: Option<DrawableSnapshot>,
    clip_mask: Option<DrawableSnapshot>,
}

/// Collects the records an operation touches, following alpha maps, without
/// holding the registry lock across pixel snapshots.
fn collect_records(
    id: Picture,
    registry: &PictureRegistry,
    records: &mut Vec<(Picture, PictureRec, bool)>,
    depth: usize,
    sampled: bool,
) -> bool {
    if let Some(entry) = records.iter_mut().find(|(known, ..)| *known == id) {
        entry.2 |= sampled;
        return true;
    }
    if depth > 16 {
        return false;
    }
    let Some(record) = registry.pictures.get(&id).cloned() else {
        return false;
    };
    let alpha_map = record.attributes.alpha_map;
    records.push((id, record, sampled));
    alpha_map == 0 || collect_records(alpha_map, registry, records, depth + 1, true)
}

/// Prepares the pictures an operation reads (`sources`) and the one it
/// writes (`destination`, 0 for none).  Snapshots copy whole surfaces, so
/// only pictures whose pixels are read get one; a destination is written in
/// place through `mutate_drawable`.
fn prepared_pictures(
    sources: &[Picture],
    destination: Picture,
) -> Option<HashMap<Picture, PreparedPicture>> {
    let mut records = Vec::new();
    {
        let registry = pictures().lock().ok()?;
        if destination != 0 && !collect_records(destination, &registry, &mut records, 0, false) {
            return None;
        }
        for id in sources.iter().copied().filter(|id| *id != 0) {
            if !collect_records(id, &registry, &mut records, 0, true) {
                return None;
            }
        }
    }
    let mut prepared = HashMap::new();
    for (id, record, sampled) in records {
        let surface = match record.source {
            PictureSource::Drawable { drawable } if sampled => drawable_snapshot(drawable),
            _ => None,
        };
        if sampled && matches!(record.source, PictureSource::Drawable { .. }) && surface.is_none() {
            return None;
        }
        let clip_mask = (record.attributes.clip_mask != 0)
            .then(|| drawable_snapshot(record.attributes.clip_mask))
            .flatten();
        prepared.insert(
            id,
            PreparedPicture {
                record,
                surface,
                clip_mask,
            },
        );
    }
    Some(prepared)
}

fn transformed(transform: XTransform, x: f32, y: f32) -> Option<(f32, f32)> {
    let m = transform.matrix;
    let value = |row: usize| fixed(m[row][0]) * x + fixed(m[row][1]) * y + fixed(m[row][2]);
    let w = value(2);
    if w.abs() < 1.0e-12 {
        None
    } else {
        Some((value(0) / w, value(1) / w))
    }
}

fn repeated_coordinate(value: i32, size: i32, repeat: c_int) -> Option<i32> {
    if size <= 0 {
        return None;
    }
    match repeat {
        1 => Some(value.rem_euclid(size)),
        2 => Some(value.clamp(0, size - 1)),
        3 => {
            let period = size.saturating_mul(2).max(1);
            let reflected = value.rem_euclid(period);
            Some(if reflected < size {
                reflected
            } else {
                period - reflected - 1
            })
        }
        _ if (0..size).contains(&value) => Some(value),
        _ => None,
    }
}

fn inside_clip(picture: &PreparedPicture, x: f32, y: f32) -> bool {
    let rectangles_allow = picture.record.clip.as_ref().is_none_or(|rectangles| {
        rectangles.iter().any(|rect| {
            x >= rect.left as f32
                && y >= rect.top as f32
                && x < rect.right as f32
                && y < rect.bottom as f32
        })
    });
    if !rectangles_allow {
        return false;
    }
    let Some(mask) = picture.clip_mask.as_ref() else {
        return true;
    };
    let mx = x.floor() as i32 - picture.record.attributes.clip_x_origin;
    let my = y.floor() as i32 - picture.record.attributes.clip_y_origin;
    if mx < 0 || my < 0 || mx >= mask.width || my >= mask.height {
        return false;
    }
    mask.pixels[my as usize * mask.width as usize + mx as usize] != 0
}

fn surface_pixel(picture: &PreparedPicture, surface: &DrawableSnapshot, x: i32, y: i32) -> Pixel {
    let repeat = picture.record.attributes.repeat;
    let Some(x) = repeated_coordinate(x, surface.width, repeat) else {
        return Pixel::TRANSPARENT;
    };
    let Some(y) = repeated_coordinate(y, surface.height, repeat) else {
        return Pixel::TRANSPARENT;
    };
    let raw = surface.pixels[y as usize * surface.width as usize + x as usize];
    Pixel::from_raw(raw, picture.record.format)
}

fn interpolate_stops(stops: &[f32], colors: &[Pixel], mut t: f32, repeat: c_int) -> Pixel {
    if stops.is_empty() || stops.len() != colors.len() {
        return Pixel::TRANSPARENT;
    }
    t = match repeat {
        1 => t - t.floor(),
        2 => t.clamp(stops[0], *stops.last().unwrap_or(&stops[0])),
        3 => {
            let unit = t.rem_euclid(2.0);
            if unit <= 1.0 { unit } else { 2.0 - unit }
        }
        _ if t < stops[0] || t > *stops.last().unwrap_or(&stops[0]) => {
            return Pixel::TRANSPARENT;
        }
        _ => t,
    };
    if t <= stops[0] {
        let mut color = colors[0];
        color.r *= color.a;
        color.g *= color.a;
        color.b *= color.a;
        return color.clamp();
    }
    let upper = stops
        .partition_point(|stop| *stop <= t)
        .min(stops.len() - 1);
    let lower = upper.saturating_sub(1);
    let span = stops[upper] - stops[lower];
    let amount = if span.abs() < f32::EPSILON {
        0.0
    } else {
        ((t - stops[lower]) / span).clamp(0.0, 1.0)
    };
    let lerp = |a: f32, b: f32| a + (b - a) * amount;
    let mut color = Pixel {
        r: lerp(colors[lower].r, colors[upper].r),
        g: lerp(colors[lower].g, colors[upper].g),
        b: lerp(colors[lower].b, colors[upper].b),
        a: lerp(colors[lower].a, colors[upper].a),
    };
    color.r *= color.a;
    color.g *= color.a;
    color.b *= color.a;
    color.clamp()
}

fn gradient_value(source: &PictureSource, x: f32, y: f32) -> Option<f32> {
    match source {
        PictureSource::Linear { gradient, .. } => {
            let (x1, y1) = (fixed(gradient.p1.x), fixed(gradient.p1.y));
            let (dx, dy) = (fixed(gradient.p2.x) - x1, fixed(gradient.p2.y) - y1);
            let length = dx * dx + dy * dy;
            (length > f32::EPSILON).then_some(((x - x1) * dx + (y - y1) * dy) / length)
        }
        PictureSource::Radial { gradient, .. } => {
            let c0 = (fixed(gradient.inner.x), fixed(gradient.inner.y));
            let dc = (
                fixed(gradient.outer.x) - c0.0,
                fixed(gradient.outer.y) - c0.1,
            );
            let r0 = fixed(gradient.inner.radius);
            let dr = fixed(gradient.outer.radius) - r0;
            let px = x - c0.0;
            let py = y - c0.1;
            let a = dc.0 * dc.0 + dc.1 * dc.1 - dr * dr;
            let b = -2.0 * (px * dc.0 + py * dc.1 + r0 * dr);
            let c = px * px + py * py - r0 * r0;
            if a.abs() < 1.0e-8 {
                (b.abs() > 1.0e-8).then_some(-c / b)
            } else {
                let discriminant = b * b - 4.0 * a * c;
                if discriminant < 0.0 {
                    None
                } else {
                    let root = discriminant.sqrt();
                    let first = (-b + root) / (2.0 * a);
                    let second = (-b - root) / (2.0 * a);
                    [first, second]
                        .into_iter()
                        .filter(|value| r0 + *value * dr >= 0.0)
                        .max_by(|a, b| a.total_cmp(b))
                }
            }
        }
        PictureSource::Conical { gradient, .. } => {
            let cx = fixed(gradient.center.x);
            let cy = fixed(gradient.center.y);
            let angle = (y - cy).atan2(x - cx).to_degrees();
            Some((angle - fixed(gradient.angle)).rem_euclid(360.0) / 360.0)
        }
        _ => None,
    }
}

fn sample_picture(
    id: Picture,
    x: f32,
    y: f32,
    prepared: &HashMap<Picture, PreparedPicture>,
    depth: usize,
) -> Pixel {
    if depth > 16 {
        return Pixel::TRANSPARENT;
    }
    let Some(picture) = prepared.get(&id) else {
        return Pixel::TRANSPARENT;
    };
    let Some((x, y)) = transformed(picture.record.transform, x, y) else {
        return Pixel::TRANSPARENT;
    };
    if !inside_clip(picture, x, y) {
        return Pixel::TRANSPARENT;
    }
    let mut result = match &picture.record.source {
        PictureSource::Solid(color) => *color,
        PictureSource::Drawable { .. } => {
            let Some(surface) = picture.surface.as_ref() else {
                return Pixel::TRANSPARENT;
            };
            if picture.record.filter == "convolution" && picture.record.filter_params.len() >= 3 {
                let width = picture.record.filter_params[0].round() as i32;
                let height = picture.record.filter_params[1].round() as i32;
                let coefficient_count = width
                    .checked_mul(height)
                    .and_then(|count| usize::try_from(count).ok());
                if width > 0
                    && height > 0
                    && coefficient_count.is_some_and(|count| {
                        picture.record.filter_params.len() == count.saturating_add(2)
                    })
                {
                    let left = (width - 1) / 2;
                    let top = (height - 1) / 2;
                    let origin_x = x.floor() as i32;
                    let origin_y = y.floor() as i32;
                    let mut result = Pixel::TRANSPARENT;
                    for filter_y in 0..height {
                        for filter_x in 0..width {
                            let index = 2 + (filter_y * width + filter_x) as usize;
                            result = result.add(
                                surface_pixel(
                                    picture,
                                    surface,
                                    origin_x + filter_x - left,
                                    origin_y + filter_y - top,
                                )
                                .scale(picture.record.filter_params[index]),
                            );
                        }
                    }
                    result
                } else {
                    surface_pixel(picture, surface, x.floor() as i32, y.floor() as i32)
                }
            } else if matches!(picture.record.filter.as_str(), "bilinear" | "good" | "best") {
                // Picture pixels are centred on half-integer coordinates.  Offset
                // before interpolation so an identity transform samples a pixel
                // exactly instead of blending it with its right/bottom neighbour.
                let sample_x = x - 0.5;
                let sample_y = y - 0.5;
                let base_x = sample_x.floor();
                let base_y = sample_y.floor();
                let fx = sample_x - base_x;
                let fy = sample_y - base_y;
                let samples = [
                    (0, 0, (1.0 - fx) * (1.0 - fy)),
                    (1, 0, fx * (1.0 - fy)),
                    (0, 1, (1.0 - fx) * fy),
                    (1, 1, fx * fy),
                ];
                samples
                    .into_iter()
                    .fold(Pixel::TRANSPARENT, |sum, (dx, dy, weight)| {
                        sum.add(
                            surface_pixel(picture, surface, base_x as i32 + dx, base_y as i32 + dy)
                                .scale(weight),
                        )
                    })
            } else {
                surface_pixel(picture, surface, x.floor() as i32, y.floor() as i32)
            }
        }
        PictureSource::Linear { stops, colors, .. }
        | PictureSource::Radial { stops, colors, .. }
        | PictureSource::Conical { stops, colors, .. } => {
            gradient_value(&picture.record.source, x, y)
                .map(|value| {
                    interpolate_stops(stops, colors, value, picture.record.attributes.repeat)
                })
                .unwrap_or(Pixel::TRANSPARENT)
        }
    };
    if picture.record.attributes.alpha_map != 0 {
        let alpha = sample_picture(
            picture.record.attributes.alpha_map,
            x - picture.record.attributes.alpha_x_origin as f32,
            y - picture.record.attributes.alpha_y_origin as f32,
            prepared,
            depth + 1,
        )
        .a;
        // Render replaces the drawable's alpha channel with alpha-map; it does
        // not multiply the two alpha values. Re-premultiply the original colour
        // against the replacement alpha so ARGB sources remain well formed.
        if result.a > f32::EPSILON {
            result.r = result.r / result.a * alpha;
            result.g = result.g / result.a * alpha;
            result.b = result.b / result.a * alpha;
        } else {
            result.r = 0.0;
            result.g = 0.0;
            result.b = 0.0;
        }
        result.a = alpha;
    }
    result.clamp()
}

fn coefficient(value: f32, alpha: f32) -> f32 {
    if alpha <= f32::EPSILON {
        0.0
    } else {
        (value / alpha).clamp(0.0, 1.0)
    }
}

fn disjoint_or_conjoint(op: c_int, source: Pixel, destination: Pixel) -> Pixel {
    let conjoint = op >= 0x20;
    let base = op & 0x0f;
    let overlap = if conjoint {
        source.a.min(destination.a)
    } else {
        (source.a + destination.a - 1.0).max(0.0)
    };
    let source_in = coefficient(overlap, source.a);
    let destination_in = coefficient(overlap, destination.a);
    let source_out = coefficient(source.a - overlap, source.a);
    let destination_out = coefficient(destination.a - overlap, destination.a);
    let (fa, fb) = match base {
        0 => (0.0, 0.0),
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        3 => (1.0, destination_out),
        4 => (source_out, 1.0),
        5 => (source_in, 0.0),
        6 => (0.0, destination_in),
        7 => (source_out, 0.0),
        8 => (0.0, destination_out),
        9 => (source_in, destination_out),
        10 => (source_out, destination_in),
        11 => (source_out, destination_out),
        _ => (0.0, 0.0),
    };
    source.scale(fa).add(destination.scale(fb)).clamp()
}

fn unpremultiplied(pixel: Pixel) -> [f32; 3] {
    if pixel.a <= f32::EPSILON {
        [0.0; 3]
    } else {
        [pixel.r / pixel.a, pixel.g / pixel.a, pixel.b / pixel.a]
    }
}

fn luminosity(color: [f32; 3]) -> f32 {
    0.3 * color[0] + 0.59 * color[1] + 0.11 * color[2]
}

fn saturation(color: [f32; 3]) -> f32 {
    color.into_iter().fold(f32::MIN, f32::max) - color.into_iter().fold(f32::MAX, f32::min)
}

fn clip_color(mut color: [f32; 3]) -> [f32; 3] {
    let l = luminosity(color);
    let n = color.into_iter().fold(f32::MAX, f32::min);
    let x = color.into_iter().fold(f32::MIN, f32::max);
    if n < 0.0 && (l - n).abs() > f32::EPSILON {
        for channel in &mut color {
            *channel = l + ((*channel - l) * l) / (l - n);
        }
    }
    if x > 1.0 && (x - l).abs() > f32::EPSILON {
        for channel in &mut color {
            *channel = l + ((*channel - l) * (1.0 - l)) / (x - l);
        }
    }
    color
}

fn set_luminosity(mut color: [f32; 3], value: f32) -> [f32; 3] {
    let delta = value - luminosity(color);
    for channel in &mut color {
        *channel += delta;
    }
    clip_color(color)
}

fn set_saturation(color: [f32; 3], value: f32) -> [f32; 3] {
    let mut indices = [0usize, 1, 2];
    indices.sort_by(|a, b| color[*a].total_cmp(&color[*b]));
    let (min, mid, max) = (indices[0], indices[1], indices[2]);
    let mut result = [0.0; 3];
    if color[max] > color[min] {
        result[mid] = (color[mid] - color[min]) * value / (color[max] - color[min]);
        result[max] = value;
    }
    result[min] = 0.0;
    result
}

fn blend_channel(op: c_int, source: f32, destination: f32) -> f32 {
    match op {
        0x30 => source * destination,
        0x31 => source + destination - source * destination,
        0x32 => {
            if destination <= 0.5 {
                2.0 * source * destination
            } else {
                1.0 - 2.0 * (1.0 - source) * (1.0 - destination)
            }
        }
        0x33 => source.min(destination),
        0x34 => source.max(destination),
        0x35 => {
            if source >= 1.0 {
                1.0
            } else {
                (destination / (1.0 - source)).min(1.0)
            }
        }
        0x36 => {
            if source <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - destination) / source).min(1.0)
            }
        }
        0x37 => {
            if source <= 0.5 {
                2.0 * source * destination
            } else {
                1.0 - 2.0 * (1.0 - source) * (1.0 - destination)
            }
        }
        0x38 => {
            if source <= 0.5 {
                destination - (1.0 - 2.0 * source) * destination * (1.0 - destination)
            } else {
                let d = if destination <= 0.25 {
                    ((16.0 * destination - 12.0) * destination + 4.0) * destination
                } else {
                    destination.sqrt()
                };
                destination + (2.0 * source - 1.0) * (d - destination)
            }
        }
        0x39 => (destination - source).abs(),
        0x3a => destination + source - 2.0 * destination * source,
        _ => source,
    }
}

fn blend(op: c_int, source: Pixel, destination: Pixel) -> Pixel {
    let cs = unpremultiplied(source);
    let cd = unpremultiplied(destination);
    let blended = blended_color(op, cs, cd);
    let shared = source.a * destination.a;
    Pixel {
        r: (1.0 - source.a) * destination.r
            + (1.0 - destination.a) * source.r
            + shared * blended[0],
        g: (1.0 - source.a) * destination.g
            + (1.0 - destination.a) * source.g
            + shared * blended[1],
        b: (1.0 - source.a) * destination.b
            + (1.0 - destination.a) * source.b
            + shared * blended[2],
        a: source.a + destination.a - shared,
    }
    .clamp()
}

fn blended_color(op: c_int, source: [f32; 3], destination: [f32; 3]) -> [f32; 3] {
    match op {
        0x3b => set_luminosity(
            set_saturation(source, saturation(destination)),
            luminosity(destination),
        ),
        0x3c => set_luminosity(
            set_saturation(destination, saturation(source)),
            luminosity(destination),
        ),
        0x3d => set_luminosity(source, luminosity(destination)),
        0x3e => set_luminosity(destination, luminosity(source)),
        _ => [
            blend_channel(op, source[0], destination[0]),
            blend_channel(op, source[1], destination[1]),
            blend_channel(op, source[2], destination[2]),
        ],
    }
}

fn porter_factors(op: c_int, source_alpha: f32, destination_alpha: f32) -> Option<(f32, f32)> {
    if (0x10..=0x1b).contains(&op) || (0x20..=0x2b).contains(&op) {
        let conjoint = op >= 0x20;
        let base = op & 0x0f;
        let overlap = if conjoint {
            source_alpha.min(destination_alpha)
        } else {
            (source_alpha + destination_alpha - 1.0).max(0.0)
        };
        let source_in = coefficient(overlap, source_alpha);
        let destination_in = coefficient(overlap, destination_alpha);
        let source_out = coefficient(source_alpha - overlap, source_alpha);
        let destination_out = coefficient(destination_alpha - overlap, destination_alpha);
        return Some(match base {
            0 => (0.0, 0.0),
            1 => (1.0, 0.0),
            2 => (0.0, 1.0),
            3 => (1.0, destination_out),
            4 => (source_out, 1.0),
            5 => (source_in, 0.0),
            6 => (0.0, destination_in),
            7 => (source_out, 0.0),
            8 => (0.0, destination_out),
            9 => (source_in, destination_out),
            10 => (source_out, destination_in),
            11 => (source_out, destination_out),
            _ => return None,
        });
    }
    Some(match op {
        0 => (0.0, 0.0),
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        3 => (1.0, 1.0 - source_alpha),
        4 => (1.0 - destination_alpha, 1.0),
        5 => (destination_alpha, 0.0),
        6 => (0.0, source_alpha),
        7 => (1.0 - destination_alpha, 0.0),
        8 => (0.0, 1.0 - source_alpha),
        9 => (destination_alpha, 1.0 - source_alpha),
        10 => (1.0 - destination_alpha, source_alpha),
        11 => (1.0 - destination_alpha, 1.0 - source_alpha),
        12 => (1.0, 1.0),
        13 => (
            if source_alpha <= f32::EPSILON {
                1.0
            } else {
                ((1.0 - destination_alpha) / source_alpha).min(1.0)
            },
            1.0,
        ),
        _ => return None,
    })
}

fn composite_component_alpha(op: c_int, source: Pixel, mask: Pixel, destination: Pixel) -> Pixel {
    let mask_values = [mask.r, mask.g, mask.b, mask.a];
    let source_values = [source.r, source.g, source.b, source.a];
    let destination_values = [destination.r, destination.g, destination.b, destination.a];
    let mut output = [0.0; 4];
    if (0x30..=0x3e).contains(&op) {
        let blended = blended_color(op, unpremultiplied(source), unpremultiplied(destination));
        for channel in 0..3 {
            let alpha = source.a * mask_values[channel];
            let color = source_values[channel] * mask_values[channel];
            output[channel] = (1.0 - alpha) * destination_values[channel]
                + (1.0 - destination.a) * color
                + alpha * destination.a * blended[channel];
        }
        let alpha = source.a * mask.a;
        output[3] = alpha + destination.a - alpha * destination.a;
    } else {
        for channel in 0..4 {
            let alpha = source.a * mask_values[channel];
            let color = source_values[channel] * mask_values[channel];
            let Some((source_factor, destination_factor)) =
                porter_factors(op, alpha, destination.a)
            else {
                return destination;
            };
            output[channel] =
                color * source_factor + destination_values[channel] * destination_factor;
        }
    }
    Pixel {
        r: output[0],
        g: output[1],
        b: output[2],
        a: output[3],
    }
    .clamp()
}

fn composite_pixel(op: c_int, source: Pixel, destination: Pixel) -> Pixel {
    if (0x10..=0x1b).contains(&op) || (0x20..=0x2b).contains(&op) {
        return disjoint_or_conjoint(op, source, destination);
    }
    if (0x30..=0x3e).contains(&op) {
        return blend(op, source, destination);
    }
    let (fa, fb) = match op {
        0 => (0.0, 0.0),
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        3 => (1.0, 1.0 - source.a),
        4 => (1.0 - destination.a, 1.0),
        5 => (destination.a, 0.0),
        6 => (0.0, source.a),
        7 => (1.0 - destination.a, 0.0),
        8 => (0.0, 1.0 - source.a),
        9 => (destination.a, 1.0 - source.a),
        10 => (1.0 - destination.a, source.a),
        11 => (1.0 - destination.a, 1.0 - source.a),
        12 => return source.add(destination).clamp(),
        13 => {
            let factor = if source.a <= f32::EPSILON {
                1.0
            } else {
                ((1.0 - destination.a) / source.a).min(1.0)
            };
            return source.scale(factor).add(destination).clamp();
        }
        _ => return destination,
    };
    source.scale(fa).add(destination.scale(fb)).clamp()
}

fn valid_operator(op: c_int) -> bool {
    (0..=13).contains(&op)
        || (0x10..=0x1b).contains(&op)
        || (0x20..=0x2b).contains(&op)
        || (0x30..=0x3e).contains(&op)
}

fn destination_picture(id: Picture) -> Option<(PictureRec, Drawable)> {
    let record = pictures().lock().ok()?.pictures.get(&id)?.clone();
    let PictureSource::Drawable { drawable } = record.source else {
        return None;
    };
    Some((record, drawable))
}

#[allow(clippy::too_many_arguments)]
fn composite_region(
    op: c_int,
    src: Picture,
    mask: Picture,
    dst: Picture,
    src_x: i32,
    src_y: i32,
    mask_x: i32,
    mask_y: i32,
    dst_x: i32,
    dst_y: i32,
    width: u32,
    height: u32,
    coverage: Option<&dyn Fn(f32, f32) -> f32>,
) -> bool {
    if !valid_operator(op) || width == 0 || height == 0 {
        return false;
    }
    let Some((destination_record, drawable)) = destination_picture(dst) else {
        return false;
    };
    let Some(prepared) = prepared_pictures(&[src, mask], dst) else {
        return false;
    };
    let Some(destination_picture) = prepared.get(&dst) else {
        return false;
    };
    // Cairo presents GTK's double buffer as a rectangular Src copy. Avoid
    // float sampling/blending per pixel: row copies use the native memcpy
    // implementation; format conversion loops are auto-vectorizable by LLVM.
    if mask == 0
        && coverage.is_none()
        && destination_picture.clip_mask.is_none()
        && destination_record.attributes.alpha_map == 0
        && let Some(source) = prepared.get(&src)
        && source.record.attributes.alpha_map == 0
        && source.record.clip.is_none()
        && source.clip_mask.is_none()
        && source.record.filter != "convolution"
        && source.record.transform.matrix == XTransform::default().matrix
        && matches!(source.record.format, 1 | 2)
        && matches!(destination_record.format, 1 | 2)
        && (op == 1 || op == 3 && source.record.format == 2)
        && let Some(surface) = source.surface.as_ref()
        && src_x >= 0
        && src_y >= 0
        && i64::from(src_x) + i64::from(width) <= i64::from(surface.width)
        && i64::from(src_y) + i64::from(height) <= i64::from(surface.height)
    {
        let dirty = RECT {
            left: dst_x,
            top: dst_y,
            right: dst_x.saturating_add(width as i32),
            bottom: dst_y.saturating_add(height as i32),
        };
        return mutate_drawable(drawable, dirty, |pixels, w, h, _| {
            let mut paint = |clip: &RECT| {
                let left = dirty.left.max(clip.left).max(0);
                let right = dirty.right.min(clip.right).min(w);
                let top = dirty.top.max(clip.top).max(0);
                let bottom = dirty.bottom.min(clip.bottom).min(h);
                if left >= right || top >= bottom {
                    return;
                }
                for y in top..bottom {
                    let from = (src_y + y - dst_y) as usize * surface.width as usize
                        + (src_x + left - dst_x) as usize;
                    let to = y as usize * w as usize + left as usize;
                    let count = (right - left) as usize;
                    let source_pixels = &surface.pixels[from..from + count];
                    let target_pixels = &mut pixels[to..to + count];
                    if source.record.format == destination_record.format {
                        target_pixels.copy_from_slice(source_pixels);
                    } else if destination_record.format == 2 {
                        for (target, &source) in target_pixels.iter_mut().zip(source_pixels) {
                            *target = source & 0x00ff_ffff;
                        }
                    } else {
                        for (target, &source) in target_pixels.iter_mut().zip(source_pixels) {
                            *target = source | 0xff00_0000;
                        }
                    }
                }
            };
            if let Some(clips) = &destination_picture.record.clip {
                for clip in clips {
                    paint(clip);
                }
            } else {
                paint(&dirty);
            }
        });
    }
    // GTK clears and paints large opaque backgrounds very frequently. These
    // operations are constant stores, not texture samples or alpha blends.
    if mask == 0 && coverage.is_none() && destination_picture.clip_mask.is_none() {
        let solid = prepared.get(&src).and_then(|source| {
            if source.record.attributes.alpha_map != 0
                || source.record.clip.is_some()
                || source.clip_mask.is_some()
                || source.record.transform.matrix != XTransform::default().matrix
            {
                return None;
            }
            match source.record.source {
                PictureSource::Solid(color) => Some(color),
                _ => None,
            }
        });
        if op == 3 && solid.is_some_and(|color| color.a == 0.0) {
            return true;
        }
        let fill = match (op, solid) {
            (0, _) => Some(Pixel::TRANSPARENT),
            (1, color) => color,
            (3, Some(color)) if color.a >= 1.0 => Some(color),
            _ => None,
        };
        if let Some(fill) = fill {
            let dirty = RECT {
                left: dst_x,
                top: dst_y,
                right: dst_x.saturating_add(width.min(i32::MAX as u32) as i32),
                bottom: dst_y.saturating_add(height.min(i32::MAX as u32) as i32),
            };
            let raw = fill.to_raw(destination_record.format);
            return mutate_drawable(drawable, dirty, |pixels, width, height, _| {
                let mut paint = |clip: &RECT| {
                    let left = dirty.left.max(clip.left).max(0);
                    let right = dirty.right.min(clip.right).min(width);
                    let top = dirty.top.max(clip.top).max(0);
                    let bottom = dirty.bottom.min(clip.bottom).min(height);
                    if left >= right {
                        return;
                    }
                    for y in top..bottom {
                        let row = y as usize * width as usize;
                        pixels[row + left as usize..row + right as usize].fill(raw);
                    }
                };
                if let Some(clips) = &destination_picture.record.clip {
                    for clip in clips {
                        paint(clip);
                    }
                } else {
                    paint(&dirty);
                }
            });
        }
    }
    let component_alpha = mask != 0
        && prepared
            .get(&mask)
            .is_some_and(|picture| picture.record.attributes.component_alpha != 0);
    let dirty = RECT {
        left: dst_x,
        top: dst_y,
        right: dst_x.saturating_add(width.min(i32::MAX as u32) as i32),
        bottom: dst_y.saturating_add(height.min(i32::MAX as u32) as i32),
    };
    mutate_drawable(
        drawable,
        dirty,
        |pixels, surface_width, surface_height, _| {
            let x_start = dirty.left.max(0);
            let y_start = dirty.top.max(0);
            let x_end = dirty.right.min(surface_width);
            let y_end = dirty.bottom.min(surface_height);
            for y in y_start..y_end {
                for x in x_start..x_end {
                    let center_x = x as f32 + 0.5;
                    let center_y = y as f32 + 0.5;
                    if !inside_clip(destination_picture, center_x, center_y) {
                        continue;
                    }
                    let relative_x = x - dst_x;
                    let relative_y = y - dst_y;
                    let mut source = sample_picture(
                        src,
                        (src_x + relative_x) as f32 + 0.5,
                        (src_y + relative_y) as f32 + 0.5,
                        &prepared,
                        0,
                    );
                    let mut component_mask = None;
                    if mask != 0 {
                        let mask_pixel = sample_picture(
                            mask,
                            (mask_x + relative_x) as f32 + 0.5,
                            (mask_y + relative_y) as f32 + 0.5,
                            &prepared,
                            0,
                        );
                        if component_alpha {
                            component_mask = Some(mask_pixel);
                        } else {
                            source = source.scale(mask_pixel.a);
                        }
                    }
                    if let Some(coverage) = coverage {
                        let coverage = coverage(center_x, center_y).clamp(0.0, 1.0);
                        if let Some(mask) = component_mask.as_mut() {
                            *mask = mask.scale(coverage);
                        } else {
                            source = source.scale(coverage);
                        }
                    }
                    let index = y as usize * surface_width as usize + x as usize;
                    let destination = Pixel::from_raw(pixels[index], destination_record.format);
                    let result = component_mask.map_or_else(
                        || composite_pixel(op, source, destination),
                        |mask| composite_component_alpha(op, source, mask, destination),
                    );
                    pixels[index] = result.to_raw(destination_record.format);
                }
            }
        },
    )
}

fn apply_attributes(
    record: &mut PictureRec,
    value_mask: c_ulong,
    attributes: *const XRenderPictureAttributes,
) {
    if attributes.is_null() {
        return;
    }
    let values = unsafe { *attributes };
    // Rectangle clips and bitmap clips are two representations of the same
    // protocol attribute. CPClipMask=None must discard either representation.
    // Cairo uses this when moving from one widget to the next.
    if value_mask & (1 << 6) != 0 {
        record.clip = None;
    } else if let Some(rectangles) = record.clip.as_mut() {
        let dx = if value_mask & (1 << 4) != 0 {
            values
                .clip_x_origin
                .saturating_sub(record.attributes.clip_x_origin)
        } else {
            0
        };
        let dy = if value_mask & (1 << 5) != 0 {
            values
                .clip_y_origin
                .saturating_sub(record.attributes.clip_y_origin)
        } else {
            0
        };
        for rectangle in rectangles {
            rectangle.left = rectangle.left.saturating_add(dx);
            rectangle.right = rectangle.right.saturating_add(dx);
            rectangle.top = rectangle.top.saturating_add(dy);
            rectangle.bottom = rectangle.bottom.saturating_add(dy);
        }
    }
    macro_rules! field {
        ($bit:expr, $name:ident) => {
            if value_mask & (1 << $bit) != 0 {
                record.attributes.$name = values.$name;
            }
        };
    }
    field!(0, repeat);
    field!(1, alpha_map);
    field!(2, alpha_x_origin);
    field!(3, alpha_y_origin);
    field!(4, clip_x_origin);
    field!(5, clip_y_origin);
    field!(6, clip_mask);
    field!(7, graphics_exposures);
    field!(8, subwindow_mode);
    field!(9, poly_edge);
    field!(10, poly_mode);
    field!(11, dither);
    field!(12, component_alpha);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQueryExtension")]
pub unsafe extern "sysv64" fn XRenderQueryExtension(
    _display: *mut Display,
    event_base: *mut c_int,
    error_base: *mut c_int,
) -> Bool {
    if !event_base.is_null() {
        unsafe { *event_base = 68 };
    }
    if !error_base.is_null() {
        unsafe { *error_base = 132 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQueryVersion")]
pub unsafe extern "sysv64" fn XRenderQueryVersion(
    _display: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Status {
    if !major.is_null() {
        unsafe { *major = 0 };
    }
    if !minor.is_null() {
        unsafe { *minor = 11 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQueryFormats")]
pub unsafe extern "sysv64" fn XRenderQueryFormats(_display: *mut Display) -> Status {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQuerySubpixelOrder")]
pub unsafe extern "sysv64" fn XRenderQuerySubpixelOrder(
    _display: *mut Display,
    screen: c_int,
) -> c_int {
    if screen != 0 {
        return 0;
    }
    pictures()
        .lock()
        .map(|registry| registry.subpixel_order)
        .unwrap_or(0)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderSetSubpixelOrder")]
pub unsafe extern "sysv64" fn XRenderSetSubpixelOrder(
    _display: *mut Display,
    screen: c_int,
    subpixel: c_int,
) -> Bool {
    if screen != 0 || !(0..=5).contains(&subpixel) {
        return 0;
    }
    if let Ok(mut registry) = pictures().lock() {
        registry.subpixel_order = subpixel;
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFindVisualFormat")]
pub unsafe extern "sysv64" fn XRenderFindVisualFormat(
    _display: *mut Display,
    _visual: *const Visual,
) -> *mut XRenderPictFormat {
    format_pointer(2)
}

fn format_matches(format: XRenderPictFormat, mask: c_ulong, template: XRenderPictFormat) -> bool {
    let direct = format.direct;
    let requested = template.direct;
    (mask & (1 << 0) == 0 || format.id == template.id)
        && (mask & (1 << 1) == 0 || format.type_ == template.type_)
        && (mask & (1 << 2) == 0 || format.depth == template.depth)
        && (mask & (1 << 3) == 0 || direct[0] == requested[0])
        && (mask & (1 << 4) == 0 || direct[1] == requested[1])
        && (mask & (1 << 5) == 0 || direct[2] == requested[2])
        && (mask & (1 << 6) == 0 || direct[3] == requested[3])
        && (mask & (1 << 7) == 0 || direct[4] == requested[4])
        && (mask & (1 << 8) == 0 || direct[5] == requested[5])
        && (mask & (1 << 9) == 0 || direct[6] == requested[6])
        && (mask & (1 << 10) == 0 || direct[7] == requested[7])
        && (mask & (1 << 11) == 0 || format.colormap == template.colormap)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFindFormat")]
pub unsafe extern "sysv64" fn XRenderFindFormat(
    _display: *mut Display,
    mask: c_ulong,
    template: *const XRenderPictFormat,
    count: c_int,
) -> *mut XRenderPictFormat {
    if template.is_null() || count < 0 {
        return core::ptr::null_mut();
    }
    let requested = unsafe { *template };
    [1usize, 2, 3, 4, 5]
        .into_iter()
        .filter(|id| {
            format_by_id(*id).is_some_and(|format| format_matches(format, mask, requested))
        })
        .nth(count as usize)
        .map(format_pointer)
        .unwrap_or(core::ptr::null_mut())
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFindStandardFormat")]
pub unsafe extern "sysv64" fn XRenderFindStandardFormat(
    _display: *mut Display,
    format: c_int,
) -> *mut XRenderPictFormat {
    match format {
        0 => format_pointer(1),
        1 => format_pointer(2),
        2 => format_pointer(3),
        3 => format_pointer(4),
        4 => format_pointer(5),
        _ => core::ptr::null_mut(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQueryPictIndexValues")]
pub unsafe extern "sysv64" fn XRenderQueryPictIndexValues(
    _display: *mut Display,
    _format: *const XRenderPictFormat,
    count: *mut c_int,
) -> *mut XIndexValue {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    // Kinakaze exposes direct formats only.
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreatePicture")]
pub unsafe extern "sysv64" fn XRenderCreatePicture(
    _display: *mut Display,
    drawable: Drawable,
    format: *const XRenderPictFormat,
    value_mask: c_ulong,
    attributes: *const XRenderPictureAttributes,
) -> Picture {
    if drawable == 0 || format.is_null() {
        return 0;
    }
    let format = unsafe { *format };
    let Some(surface) = drawable_snapshot(drawable) else {
        return 0;
    };
    if format_by_id(format.id).is_none()
        || !matches!(
            (surface.depth, format.depth),
            (32, 32) | (24, 24) | (8, 8) | (4, 4) | (1, 1)
        )
    {
        return 0;
    }
    let mut record = PictureRec::new(PictureSource::Drawable { drawable }, format.id);
    apply_attributes(&mut record, value_mask, attributes);
    let picture = insert_picture(record);
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!(
            "[libX11] XRenderCreatePicture picture={picture:#x} drawable={drawable:#x} format={} depth={}",
            format.id,
            format.depth
        );
    }
    picture
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFreePicture")]
pub unsafe extern "sysv64" fn XRenderFreePicture(_display: *mut Display, picture: Picture) {
    if let Ok(mut registry) = pictures().lock() {
        registry.pictures.remove(&picture);
        for record in registry.pictures.values_mut() {
            if record.attributes.alpha_map == picture {
                record.attributes.alpha_map = 0;
            }
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderChangePicture")]
pub unsafe extern "sysv64" fn XRenderChangePicture(
    _display: *mut Display,
    picture: Picture,
    value_mask: c_ulong,
    attributes: *const XRenderPictureAttributes,
) {
    if let Ok(mut registry) = pictures().lock()
        && let Some(record) = registry.pictures.get_mut(&picture)
    {
        apply_attributes(record, value_mask, attributes);
        if record.attributes.alpha_map == picture {
            record.attributes.alpha_map = 0;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderSetPictureClipRectangles")]
pub unsafe extern "sysv64" fn XRenderSetPictureClipRectangles(
    _display: *mut Display,
    picture: Picture,
    x_origin: c_int,
    y_origin: c_int,
    rectangles: *const XRectangle,
    count: c_int,
) {
    if count < 0 || (count > 0 && rectangles.is_null()) {
        return;
    }
    let values = if count == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(rectangles, count as usize) }
            .iter()
            .map(|rectangle| RECT {
                left: x_origin.saturating_add(rectangle.x as i32),
                top: y_origin.saturating_add(rectangle.y as i32),
                right: x_origin
                    .saturating_add(rectangle.x as i32)
                    .saturating_add(rectangle.width as i32),
                bottom: y_origin
                    .saturating_add(rectangle.y as i32)
                    .saturating_add(rectangle.height as i32),
            })
            .collect()
    };
    if let Ok(mut registry) = pictures().lock()
        && let Some(record) = registry.pictures.get_mut(&picture)
    {
        record.clip = Some(values);
        record.attributes.clip_x_origin = x_origin;
        record.attributes.clip_y_origin = y_origin;
        record.attributes.clip_mask = 0;
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderSetPictureClipRegion")]
pub unsafe extern "sysv64" fn XRenderSetPictureClipRegion(
    display: *mut Display,
    picture: Picture,
    region: Region,
) {
    if region.is_null() {
        if let Ok(mut registry) = pictures().lock() {
            if let Some(record) = registry.pictures.get_mut(&picture) {
                record.clip = None;
                record.attributes.clip_mask = 0;
            }
        }
        return;
    }
    let rectangles = unsafe { &(*region).rects };
    unsafe {
        XRenderSetPictureClipRectangles(
            display,
            picture,
            0,
            0,
            rectangles.as_ptr(),
            rectangles.len() as c_int,
        )
    };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderSetPictureTransform")]
pub unsafe extern "sysv64" fn XRenderSetPictureTransform(
    _display: *mut Display,
    picture: Picture,
    transform: *mut XTransform,
) {
    if transform.is_null() {
        return;
    }
    if let Ok(mut registry) = pictures().lock()
        && let Some(record) = registry.pictures.get_mut(&picture)
    {
        record.transform = unsafe { *transform };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderComposite")]
pub unsafe extern "sysv64" fn XRenderComposite(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    mask: Picture,
    dst: Picture,
    src_x: c_int,
    src_y: c_int,
    mask_x: c_int,
    mask_y: c_int,
    dst_x: c_int,
    dst_y: c_int,
    width: u32,
    height: u32,
) {
    let started = std::time::Instant::now();
    let _ = composite_region(
        op, src, mask, dst, src_x, src_y, mask_x, mask_y, dst_x, dst_y, width, height, None,
    );
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XRenderComposite op={op} src={src:#x} mask={mask:#x} dst={dst:#x} {width}x{height} at ({dst_x},{dst_y}) took {:?}",
            started.elapsed()
        );
    }
}

fn copy_gradient_stops(
    stops: *const XFixed,
    colors: *const XRenderColor,
    count: c_int,
) -> Option<(Vec<f32>, Vec<Pixel>)> {
    if count <= 0 || stops.is_null() || colors.is_null() {
        return None;
    }
    let stops = unsafe { core::slice::from_raw_parts(stops, count as usize) }
        .iter()
        .map(|value| fixed(*value))
        .collect::<Vec<_>>();
    if stops.windows(2).any(|pair| pair[0] > pair[1]) {
        return None;
    }
    // Gradient colours are the one Render API input that is not premultiplied.
    let colors = unsafe { core::slice::from_raw_parts(colors, count as usize) }
        .iter()
        .map(|color| Pixel::from_render_color(*color, false))
        .collect();
    Some((stops, colors))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateSolidFill")]
pub unsafe extern "sysv64" fn XRenderCreateSolidFill(
    _display: *mut Display,
    color: *const XRenderColor,
) -> Picture {
    if color.is_null() {
        return 0;
    }
    let value = unsafe { *color };
    let picture = insert_picture(PictureRec::new(
        PictureSource::Solid(Pixel::from_render_color(value, true)),
        1,
    ));
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!(
            "[libX11] XRenderCreateSolidFill picture={picture:#x} rgba=({:#06x},{:#06x},{:#06x},{:#06x})",
            value.red,
            value.green,
            value.blue,
            value.alpha
        );
    }
    picture
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateLinearGradient")]
pub unsafe extern "sysv64" fn XRenderCreateLinearGradient(
    _display: *mut Display,
    gradient: *const XLinearGradient,
    stops: *const XFixed,
    colors: *const XRenderColor,
    count: c_int,
) -> Picture {
    if gradient.is_null() {
        return 0;
    }
    let Some((stops, colors)) = copy_gradient_stops(stops, colors, count) else {
        return 0;
    };
    insert_picture(PictureRec::new(
        PictureSource::Linear {
            gradient: unsafe { *gradient },
            stops,
            colors,
        },
        1,
    ))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateRadialGradient")]
pub unsafe extern "sysv64" fn XRenderCreateRadialGradient(
    _display: *mut Display,
    gradient: *const XRadialGradient,
    stops: *const XFixed,
    colors: *const XRenderColor,
    count: c_int,
) -> Picture {
    if gradient.is_null() {
        return 0;
    }
    let Some((stops, colors)) = copy_gradient_stops(stops, colors, count) else {
        return 0;
    };
    insert_picture(PictureRec::new(
        PictureSource::Radial {
            gradient: unsafe { *gradient },
            stops,
            colors,
        },
        1,
    ))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateConicalGradient")]
pub unsafe extern "sysv64" fn XRenderCreateConicalGradient(
    _display: *mut Display,
    gradient: *const XConicalGradient,
    stops: *const XFixed,
    colors: *const XRenderColor,
    count: c_int,
) -> Picture {
    if gradient.is_null() {
        return 0;
    }
    let Some((stops, colors)) = copy_gradient_stops(stops, colors, count) else {
        return 0;
    };
    insert_picture(PictureRec::new(
        PictureSource::Conical {
            gradient: unsafe { *gradient },
            stops,
            colors,
        },
        1,
    ))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFillRectangle")]
pub unsafe extern "sysv64" fn XRenderFillRectangle(
    display: *mut Display,
    op: c_int,
    dst: Picture,
    color: *const XRenderColor,
    x: c_int,
    y: c_int,
    width: u32,
    height: u32,
) {
    if color.is_null() {
        return;
    }
    let source = unsafe { XRenderCreateSolidFill(display, color) };
    if source != 0 {
        unsafe { XRenderComposite(display, op, source, 0, dst, 0, 0, 0, 0, x, y, width, height) };
        unsafe { XRenderFreePicture(display, source) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFillRectangles")]
pub unsafe extern "sysv64" fn XRenderFillRectangles(
    display: *mut Display,
    op: c_int,
    dst: Picture,
    color: *const XRenderColor,
    rectangles: *const XRectangle,
    count: c_int,
) {
    if color.is_null() || count < 0 || (count > 0 && rectangles.is_null()) {
        return;
    }
    let source = unsafe { XRenderCreateSolidFill(display, color) };
    if source == 0 {
        return;
    }
    for rectangle in unsafe { core::slice::from_raw_parts(rectangles, count as usize) } {
        unsafe {
            XRenderComposite(
                display,
                op,
                source,
                0,
                dst,
                0,
                0,
                0,
                0,
                rectangle.x as i32,
                rectangle.y as i32,
                rectangle.width as u32,
                rectangle.height as u32,
            )
        };
    }
    unsafe { XRenderFreePicture(display, source) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderSetPictureFilter")]
pub unsafe extern "sysv64" fn XRenderSetPictureFilter(
    _display: *mut Display,
    picture: Picture,
    filter: *const c_char,
    params: *mut XFixed,
    count: c_int,
) {
    if filter.is_null() || count < 0 || (count > 0 && params.is_null()) {
        return;
    }
    let Ok(name) = unsafe { CStr::from_ptr(filter) }.to_str() else {
        return;
    };
    let normalized = match name.to_ascii_lowercase().as_str() {
        "nearest" | "fast" => "nearest",
        "bilinear" | "good" | "best" => "bilinear",
        "convolution" => "convolution",
        _ => return,
    };
    let values = if count == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(params, count as usize) }
            .iter()
            .map(|value| fixed(*value))
            .collect()
    };
    if let Ok(mut registry) = pictures().lock()
        && let Some(record) = registry.pictures.get_mut(&picture)
    {
        record.filter = normalized.to_owned();
        record.filter_params = values;
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderQueryFilters")]
pub unsafe extern "sysv64" fn XRenderQueryFilters(
    _display: *mut Display,
    drawable: Drawable,
) -> *mut XFilters {
    if drawable_snapshot(drawable).is_none() {
        return core::ptr::null_mut();
    }
    let names = ["nearest", "bilinear", "convolution", "fast", "good", "best"];
    let mut strings = names
        .iter()
        .map(|name| {
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            Box::into_raw(bytes.into_boxed_slice()) as *mut c_char
        })
        .collect::<Vec<_>>();
    let mut aliases = vec![0i16, 1, -1];
    let result = XFilters {
        nfilter: strings.len() as c_int,
        filter: strings.as_mut_ptr(),
        nalias: aliases.len() as c_int,
        alias: aliases.as_mut_ptr(),
    };
    core::mem::forget(strings);
    core::mem::forget(aliases);
    Box::into_raw(Box::new(result))
}

fn parse_hex_color(value: &str) -> Option<XRenderColor> {
    let digits = value.strip_prefix('#')?;
    let expand = |part: &str| -> Option<u16> {
        let value = u16::from_str_radix(part, 16).ok()?;
        Some(match part.len() {
            1 => value * 0x1111,
            2 => value * 0x0101,
            3 => (value << 4) | (value >> 8),
            4 => value,
            _ => return None,
        })
    };
    if !matches!(digits.len(), 3 | 6 | 9 | 12) {
        return None;
    }
    let width = digits.len() / 3;
    Some(XRenderColor {
        red: expand(&digits[0..width])?,
        green: expand(&digits[width..width * 2])?,
        blue: expand(&digits[width * 2..width * 3])?,
        alpha: u16::MAX,
    })
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderParseColor")]
pub unsafe extern "sysv64" fn XRenderParseColor(
    _display: *mut Display,
    specification: *mut c_char,
    result: *mut XRenderColor,
) -> Status {
    if specification.is_null() || result.is_null() {
        return 0;
    }
    let Ok(value) = unsafe { CStr::from_ptr(specification) }.to_str() else {
        return 0;
    };
    let color = parse_hex_color(value).or_else(|| {
        let (red, green, blue) = match value.to_ascii_lowercase().as_str() {
            "black" => (0, 0, 0),
            "white" => (u16::MAX, u16::MAX, u16::MAX),
            "red" => (u16::MAX, 0, 0),
            "green" => (0, u16::MAX, 0),
            "blue" => (0, 0, u16::MAX),
            "yellow" => (u16::MAX, u16::MAX, 0),
            "magenta" => (u16::MAX, 0, u16::MAX),
            "cyan" => (0, u16::MAX, u16::MAX),
            "transparent" => return Some(XRenderColor::default()),
            _ => return None,
        };
        Some(XRenderColor {
            red,
            green,
            blue,
            alpha: u16::MAX,
        })
    });
    let Some(color) = color else {
        return 0;
    };
    unsafe { *result = color };
    1
}

fn line_x_at_y(line: XLineFixed, y: f32) -> f32 {
    let (x1, y1) = (fixed(line.p1.x), fixed(line.p1.y));
    let (x2, y2) = (fixed(line.p2.x), fixed(line.p2.y));
    if (y2 - y1).abs() < f32::EPSILON {
        x1
    } else {
        x1 + (x2 - x1) * ((y - y1) / (y2 - y1))
    }
}

fn polygon_bounds(polygons: &[Vec<(f32, f32)>]) -> Option<RECT> {
    let mut points = polygons.iter().flatten();
    let &(first_x, first_y) = points.next()?;
    let (mut left, mut top, mut right, mut bottom) = (first_x, first_y, first_x, first_y);
    for &(x, y) in points {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    Some(RECT {
        left: left.floor() as i32,
        top: top.floor() as i32,
        right: right.ceil() as i32,
        bottom: bottom.ceil() as i32,
    })
}

fn point_in_polygon(points: &[(f32, f32)], x: f32, y: f32, winding: bool) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut winding_number = 0i32;
    let mut alternate = false;
    for index in 0..points.len() {
        let (x1, y1) = points[index];
        let (x2, y2) = points[(index + 1) % points.len()];
        let crosses = (y1 > y) != (y2 > y);
        if crosses {
            let intersection = x1 + (y - y1) * (x2 - x1) / (y2 - y1);
            if intersection > x {
                alternate = !alternate;
            }
        }
        let side = (x2 - x1) * (y - y1) - (x - x1) * (y2 - y1);
        if y1 <= y {
            if y2 > y && side > 0.0 {
                winding_number += 1;
            }
        } else if y2 <= y && side < 0.0 {
            winding_number -= 1;
        }
    }
    if winding {
        winding_number != 0
    } else {
        alternate
    }
}

fn polygon_coverage(
    polygons: &[Vec<(f32, f32)>],
    x: f32,
    y: f32,
    winding: bool,
    antialias: bool,
) -> f32 {
    if !antialias {
        return f32::from(
            polygons
                .iter()
                .any(|polygon| point_in_polygon(polygon, x, y, winding)),
        );
    }
    fn clip<FInside, FIntersect>(
        polygon: Vec<(f32, f32)>,
        inside: FInside,
        intersect: FIntersect,
    ) -> Vec<(f32, f32)>
    where
        FInside: Fn((f32, f32)) -> bool,
        FIntersect: Fn((f32, f32), (f32, f32)) -> (f32, f32),
    {
        let Some(mut previous) = polygon.last().copied() else {
            return Vec::new();
        };
        let mut previous_inside = inside(previous);
        let mut output = Vec::with_capacity(polygon.len() + 2);
        for current in polygon {
            let current_inside = inside(current);
            if current_inside != previous_inside {
                output.push(intersect(previous, current));
            }
            if current_inside {
                output.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
        output
    }

    fn clipped_area(polygon: &[(f32, f32)], left: f32, top: f32) -> f32 {
        let right = left + 1.0;
        let bottom = top + 1.0;
        let intersection_x = |a: (f32, f32), b: (f32, f32), edge: f32| {
            let amount = if (b.0 - a.0).abs() <= f32::EPSILON {
                0.0
            } else {
                (edge - a.0) / (b.0 - a.0)
            };
            (edge, a.1 + (b.1 - a.1) * amount)
        };
        let intersection_y = |a: (f32, f32), b: (f32, f32), edge: f32| {
            let amount = if (b.1 - a.1).abs() <= f32::EPSILON {
                0.0
            } else {
                (edge - a.1) / (b.1 - a.1)
            };
            (a.0 + (b.0 - a.0) * amount, edge)
        };
        let polygon = clip(
            polygon.to_vec(),
            |point| point.0 >= left,
            |a, b| intersection_x(a, b, left),
        );
        let polygon = clip(
            polygon,
            |point| point.0 <= right,
            |a, b| intersection_x(a, b, right),
        );
        let polygon = clip(
            polygon,
            |point| point.1 >= top,
            |a, b| intersection_y(a, b, top),
        );
        let polygon = clip(
            polygon,
            |point| point.1 <= bottom,
            |a, b| intersection_y(a, b, bottom),
        );
        if polygon.len() < 3 {
            return 0.0;
        }
        let twice_area = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
            .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
            .sum::<f32>();
        (twice_area.abs() * 0.5).clamp(0.0, 1.0)
    }

    let left = x - 0.5;
    let top = y - 0.5;
    polygons
        .iter()
        .map(|polygon| clipped_area(polygon, left, top))
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

fn accelerated_solid_polygons(
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    polygons: &[Vec<(f32, f32)>],
    winding: bool,
) -> Option<bool> {
    // GDI+ implements this common Render pipeline directly on the same persistent
    // DIB surface. Keep the dispatch capability-based: complex clips, alpha maps,
    // one-bit masks, overlapping geometry and non-solid sources continue through
    // the complete software compositor below.
    if polygons.len() != 1
        || !matches!(op, 0..=3)
        || (!mask_format.is_null() && unsafe { (*mask_format).depth } == 1)
    {
        return None;
    }
    let (color, drawable) = {
        let registry = pictures().lock().ok()?;
        let source = registry.pictures.get(&src)?;
        let destination = registry.pictures.get(&dst)?;
        let PictureSource::Solid(color) = &source.source else {
            return None;
        };
        let PictureSource::Drawable { drawable } = &destination.source else {
            return None;
        };
        if source.attributes.alpha_map != 0
            || source.attributes.clip_mask != 0
            || source.clip.is_some()
            || destination.format != 2
            || destination.attributes.alpha_map != 0
            || destination.attributes.clip_mask != 0
            || destination.clip.is_some()
        {
            return None;
        }
        (*color, *drawable)
    };
    let alpha = (color.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let channel = |value: f32| {
        if color.a <= f32::EPSILON {
            0
        } else {
            ((value / color.a).clamp(0.0, 1.0) * 255.0 + 0.5) as u32
        }
    };
    let argb =
        (alpha << 24) | (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b);
    let points = polygons[0]
        .iter()
        .map(|&(x, y)| PointF { X: x, Y: y })
        .collect::<Vec<_>>();
    Some(crate::graphics::composite_solid_polygon(
        drawable,
        argb,
        &points,
        op,
        c_int::from(winding),
    ))
}

#[allow(clippy::too_many_arguments)]
fn composite_polygons(
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: i32,
    src_y: i32,
    polygons: Vec<Vec<(f32, f32)>>,
    winding: bool,
) -> bool {
    let started = std::time::Instant::now();
    let result =
        composite_polygons_inner(op, src, dst, mask_format, src_x, src_y, polygons, winding);
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XRender polygons to dst={dst:#x} took {:?}",
            started.elapsed()
        );
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn composite_polygons_inner(
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: i32,
    src_y: i32,
    polygons: Vec<Vec<(f32, f32)>>,
    winding: bool,
) -> bool {
    if let Some(result) = accelerated_solid_polygons(op, src, dst, mask_format, &polygons, winding)
    {
        return result;
    }
    let Some(bounds) = polygon_bounds(&polygons) else {
        return false;
    };
    let antialias = mask_format.is_null() || unsafe { (*mask_format).depth } != 1;
    let coverage = |x: f32, y: f32| polygon_coverage(&polygons, x, y, winding, antialias);
    composite_region(
        op,
        src,
        0,
        dst,
        src_x,
        src_y,
        0,
        0,
        bounds.left,
        bounds.top,
        (bounds.right - bounds.left).max(0) as u32,
        (bounds.bottom - bounds.top).max(0) as u32,
        Some(&coverage),
    )
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeTrapezoids")]
pub unsafe extern "sysv64" fn XRenderCompositeTrapezoids(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: c_int,
    src_y: c_int,
    trapezoids: *const XTrapezoid,
    count: c_int,
) {
    if count < 0 || (count > 0 && trapezoids.is_null()) {
        return;
    }
    let polygons = unsafe { core::slice::from_raw_parts(trapezoids, count as usize) }
        .iter()
        .filter_map(|trapezoid| {
            let top = fixed(trapezoid.top);
            let bottom = fixed(trapezoid.bottom);
            (bottom > top).then(|| {
                vec![
                    (line_x_at_y(trapezoid.left, top), top),
                    (line_x_at_y(trapezoid.right, top), top),
                    (line_x_at_y(trapezoid.right, bottom), bottom),
                    (line_x_at_y(trapezoid.left, bottom), bottom),
                ]
            })
        })
        .collect();
    let _ = composite_polygons(op, src, dst, mask_format, src_x, src_y, polygons, false);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeTriangles")]
pub unsafe extern "sysv64" fn XRenderCompositeTriangles(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: c_int,
    src_y: c_int,
    triangles: *const XTriangle,
    count: c_int,
) {
    if count < 0 || (count > 0 && triangles.is_null()) {
        return;
    }
    let point = |value: XPointFixed| (fixed(value.x), fixed(value.y));
    let polygons = unsafe { core::slice::from_raw_parts(triangles, count as usize) }
        .iter()
        .map(|triangle| vec![point(triangle.p1), point(triangle.p2), point(triangle.p3)])
        .collect();
    let _ = composite_polygons(op, src, dst, mask_format, src_x, src_y, polygons, false);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeTriStrip")]
pub unsafe extern "sysv64" fn XRenderCompositeTriStrip(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: c_int,
    src_y: c_int,
    points: *const XPointFixed,
    count: c_int,
) {
    if count < 3 || points.is_null() {
        return;
    }
    let points = unsafe { core::slice::from_raw_parts(points, count as usize) };
    let point = |value: XPointFixed| (fixed(value.x), fixed(value.y));
    let polygons = (2..points.len())
        .map(|index| {
            if index & 1 == 0 {
                vec![
                    point(points[index - 2]),
                    point(points[index - 1]),
                    point(points[index]),
                ]
            } else {
                vec![
                    point(points[index - 1]),
                    point(points[index - 2]),
                    point(points[index]),
                ]
            }
        })
        .collect();
    let _ = composite_polygons(op, src, dst, mask_format, src_x, src_y, polygons, false);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeTriFan")]
pub unsafe extern "sysv64" fn XRenderCompositeTriFan(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: c_int,
    src_y: c_int,
    points: *const XPointFixed,
    count: c_int,
) {
    if count < 3 || points.is_null() {
        return;
    }
    let points = unsafe { core::slice::from_raw_parts(points, count as usize) };
    let point = |value: XPointFixed| (fixed(value.x), fixed(value.y));
    let polygons = (2..points.len())
        .map(|index| {
            vec![
                point(points[0]),
                point(points[index - 1]),
                point(points[index]),
            ]
        })
        .collect();
    let _ = composite_polygons(op, src, dst, mask_format, src_x, src_y, polygons, false);
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeDoublePoly")]
pub unsafe extern "sysv64" fn XRenderCompositeDoublePoly(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    mask_format: *const XRenderPictFormat,
    src_x: c_int,
    src_y: c_int,
    _dst_x: c_int,
    _dst_y: c_int,
    points: *const XPointDouble,
    count: c_int,
    winding: c_int,
) {
    if count < 3 || points.is_null() {
        return;
    }
    // libXrender itself leaves dst_x/dst_y unapplied for this client-side helper.
    let polygon = unsafe { core::slice::from_raw_parts(points, count as usize) }
        .iter()
        .map(|point| (point.x as f32, point.y as f32))
        .collect::<Vec<_>>();
    let bounds = polygon_bounds(core::slice::from_ref(&polygon));
    let ok = composite_polygons(
        op,
        src,
        dst,
        mask_format,
        src_x,
        src_y,
        vec![polygon],
        winding != 0,
    );
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        let (left, top, right, bottom) = bounds
            .map(|value| (value.left, value.top, value.right, value.bottom))
            .unwrap_or_default();
        crate::diagnostic!(
            "[libX11] XRenderCompositeDoublePoly op={op} src={src:#x} dst={dst:#x} points={count} bounds=({left},{top})-({right},{bottom}) ok={ok}"
        );
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderAddTraps")]
pub unsafe extern "sysv64" fn XRenderAddTraps(
    _display: *mut Display,
    picture: Picture,
    x_offset: c_int,
    y_offset: c_int,
    traps: *const XTrap,
    count: c_int,
) {
    if count < 0 || (count > 0 && traps.is_null()) {
        return;
    }
    let Some((record, drawable)) = destination_picture(picture) else {
        return;
    };
    let polygons = unsafe { core::slice::from_raw_parts(traps, count as usize) }
        .iter()
        .filter_map(|trap| {
            let top_y = fixed(trap.top.y) + y_offset as f32;
            let bottom_y = fixed(trap.bottom.y) + y_offset as f32;
            (bottom_y > top_y).then(|| {
                vec![
                    (fixed(trap.top.left) + x_offset as f32, top_y),
                    (fixed(trap.top.right) + x_offset as f32, top_y),
                    (fixed(trap.bottom.right) + x_offset as f32, bottom_y),
                    (fixed(trap.bottom.left) + x_offset as f32, bottom_y),
                ]
            })
        })
        .collect::<Vec<_>>();
    let Some(bounds) = polygon_bounds(&polygons) else {
        return;
    };
    let coverage = |x, y| polygon_coverage(&polygons, x, y, false, record.format != 5);
    let _ = mutate_drawable(drawable, bounds, |pixels, width, height, _| {
        for y in bounds.top.max(0)..bounds.bottom.min(height) {
            for x in bounds.left.max(0)..bounds.right.min(width) {
                let index = y as usize * width as usize + x as usize;
                let mut value = Pixel::from_raw(pixels[index], record.format);
                value.a = (value.a + coverage(x as f32 + 0.5, y as f32 + 0.5)).min(1.0);
                pixels[index] = value.to_raw(record.format);
            }
        }
    });
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateGlyphSet")]
pub unsafe extern "sysv64" fn XRenderCreateGlyphSet(
    _display: *mut Display,
    format: *const XRenderPictFormat,
) -> GlyphSet {
    if format.is_null() {
        return 0;
    }
    let format = unsafe { (*format).id };
    if format_by_id(format).is_none() {
        return 0;
    }
    let Ok(mut registry) = glyph_sets().lock() else {
        return 0;
    };
    let id = registry.next;
    registry.next = registry.next.saturating_add(1);
    registry.sets.insert(
        id,
        Arc::new(Mutex::new(GlyphSetData {
            format,
            glyphs: HashMap::new(),
        })),
    );
    id
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderReferenceGlyphSet")]
pub unsafe extern "sysv64" fn XRenderReferenceGlyphSet(
    _display: *mut Display,
    existing: GlyphSet,
) -> GlyphSet {
    let Ok(mut registry) = glyph_sets().lock() else {
        return 0;
    };
    let Some(data) = registry.sets.get(&existing).cloned() else {
        return 0;
    };
    let id = registry.next;
    registry.next = registry.next.saturating_add(1);
    registry.sets.insert(id, data);
    id
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFreeGlyphSet")]
pub unsafe extern "sysv64" fn XRenderFreeGlyphSet(_display: *mut Display, glyph_set: GlyphSet) {
    if let Ok(mut registry) = glyph_sets().lock() {
        registry.sets.remove(&glyph_set);
    }
}

fn glyph_stride(width: usize, format: usize) -> usize {
    let bits = match format {
        1 => width.saturating_mul(32),
        3 => width.saturating_mul(8),
        4 => width.saturating_mul(4),
        5 => width,
        _ => 0,
    };
    bits.div_ceil(32).saturating_mul(4)
}

fn decode_glyph_image(
    bytes: &[u8],
    width: usize,
    height: usize,
    format: usize,
) -> Option<Vec<Pixel>> {
    let stride = glyph_stride(width, format);
    if stride.saturating_mul(height) > bytes.len() {
        return None;
    }
    let mut pixels = vec![Pixel::TRANSPARENT; width.saturating_mul(height)];
    for y in 0..height {
        let row = &bytes[y * stride..(y + 1) * stride];
        for x in 0..width {
            pixels[y * width + x] = match format {
                1 => {
                    let offset = x * 4;
                    Pixel::from_raw(
                        u32::from_ne_bytes(row[offset..offset + 4].try_into().ok()?),
                        1,
                    )
                }
                3 => Pixel {
                    a: row[x] as f32 / 255.0,
                    ..Pixel::TRANSPARENT
                },
                4 => {
                    let byte = row[x / 2];
                    let value = if x & 1 == 0 { byte >> 4 } else { byte & 0x0f };
                    Pixel {
                        a: value as f32 / 15.0,
                        ..Pixel::TRANSPARENT
                    }
                }
                5 => Pixel {
                    a: f32::from(row[x / 8] & (1 << (x & 7)) != 0),
                    ..Pixel::TRANSPARENT
                },
                _ => return None,
            };
        }
    }
    Some(pixels)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderAddGlyphs")]
pub unsafe extern "sysv64" fn XRenderAddGlyphs(
    _display: *mut Display,
    glyph_set: GlyphSet,
    ids: *const Glyph,
    information: *const XGlyphInfo,
    count: c_int,
    images: *const c_char,
    image_bytes: c_int,
) {
    // Glyphs without pixels (a space, for one) still carry an advance and
    // arrive with no image bytes at all.
    if count < 0
        || image_bytes < 0
        || (count > 0 && (ids.is_null() || information.is_null()))
        || (image_bytes > 0 && images.is_null())
    {
        return;
    }
    let Some(set) = glyph_sets()
        .lock()
        .ok()
        .and_then(|registry| registry.sets.get(&glyph_set).cloned())
    else {
        return;
    };
    let ids = unsafe { core::slice::from_raw_parts(ids, count as usize) };
    let information = unsafe { core::slice::from_raw_parts(information, count as usize) };
    let images = if image_bytes == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(images.cast::<u8>(), image_bytes as usize) }
    };
    let Ok(mut set) = set.lock() else {
        return;
    };
    let format = set.format;
    let mut offset = 0usize;
    for (&id, &info) in ids.iter().zip(information) {
        let size = glyph_stride(info.width as usize, format).saturating_mul(info.height as usize);
        let Some(end) = offset.checked_add(size).filter(|end| *end <= images.len()) else {
            return;
        };
        let Some(pixels) = decode_glyph_image(
            &images[offset..end],
            info.width as usize,
            info.height as usize,
            format,
        ) else {
            return;
        };
        set.glyphs.insert(
            id,
            GlyphRec {
                info,
                format,
                pixels,
            },
        );
        offset = end;
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderFreeGlyphs")]
pub unsafe extern "sysv64" fn XRenderFreeGlyphs(
    _display: *mut Display,
    glyph_set: GlyphSet,
    ids: *const Glyph,
    count: c_int,
) {
    if count < 0 || (count > 0 && ids.is_null()) {
        return;
    }
    let Some(set) = glyph_sets()
        .lock()
        .ok()
        .and_then(|registry| registry.sets.get(&glyph_set).cloned())
    else {
        return;
    };
    if let Ok(mut set) = set.lock() {
        for id in unsafe { core::slice::from_raw_parts(ids, count as usize) } {
            set.glyphs.remove(id);
        }
    }
}

#[derive(Clone)]
struct GlyphPlacement {
    glyph: GlyphRec,
    x: i32,
    y: i32,
}

fn append_glyphs(
    placements: &mut Vec<GlyphPlacement>,
    glyph_set: GlyphSet,
    ids: &[Glyph],
    pen_x: &mut i32,
    pen_y: &mut i32,
) {
    let Some(set) = glyph_sets()
        .lock()
        .ok()
        .and_then(|registry| registry.sets.get(&glyph_set).cloned())
    else {
        return;
    };
    let Ok(set) = set.lock() else {
        return;
    };
    for id in ids {
        let Some(glyph) = set.glyphs.get(id).cloned() else {
            continue;
        };
        placements.push(GlyphPlacement {
            x: pen_x.saturating_sub(glyph.info.x as i32),
            y: pen_y.saturating_sub(glyph.info.y as i32),
            glyph: glyph.clone(),
        });
        *pen_x = pen_x.saturating_add(glyph.info.xOff as i32);
        *pen_y = pen_y.saturating_add(glyph.info.yOff as i32);
    }
}

#[allow(clippy::too_many_arguments)]
fn composite_glyph_placements(
    op: c_int,
    src: Picture,
    dst: Picture,
    x_src: i32,
    y_src: i32,
    origin_x: i32,
    origin_y: i32,
    placements: &[GlyphPlacement],
) -> bool {
    if !valid_operator(op) || placements.is_empty() {
        return false;
    }
    let left = placements.iter().map(|item| item.x).min().unwrap_or(0);
    let top = placements.iter().map(|item| item.y).min().unwrap_or(0);
    let right = placements
        .iter()
        .map(|item| item.x.saturating_add(item.glyph.info.width as i32))
        .max()
        .unwrap_or(left);
    let bottom = placements
        .iter()
        .map(|item| item.y.saturating_add(item.glyph.info.height as i32))
        .max()
        .unwrap_or(top);
    let mask_width = (right - left).max(0) as usize;
    let mask_height = (bottom - top).max(0) as usize;
    if mask_width == 0 || mask_height == 0 {
        return true;
    }
    let mut mask = vec![Pixel::TRANSPARENT; mask_width.saturating_mul(mask_height)];
    for placement in placements {
        for y in 0..placement.glyph.info.height as usize {
            for x in 0..placement.glyph.info.width as usize {
                let dx = placement.x - left + x as i32;
                let dy = placement.y - top + y as i32;
                if dx < 0 || dy < 0 || dx >= mask_width as i32 || dy >= mask_height as i32 {
                    continue;
                }
                let glyph = placement.glyph.pixels[y * placement.glyph.info.width as usize + x];
                let index = dy as usize * mask_width + dx as usize;
                mask[index] = glyph.add(mask[index].scale(1.0 - glyph.a)).clamp();
            }
        }
    }
    let Some((destination_record, drawable)) = destination_picture(dst) else {
        return false;
    };
    let Some(prepared) = prepared_pictures(&[src], dst) else {
        return false;
    };
    let Some(destination_picture) = prepared.get(&dst) else {
        return false;
    };
    let dirty = RECT {
        left,
        top,
        right,
        bottom,
    };
    let component_alpha = placement_component_alpha(placements);
    mutate_drawable(drawable, dirty, |pixels, width, height, _| {
        for y in top.max(0)..bottom.min(height) {
            for x in left.max(0)..right.min(width) {
                if !inside_clip(destination_picture, x as f32 + 0.5, y as f32 + 0.5) {
                    continue;
                }
                let glyph = mask[(y - top) as usize * mask_width + (x - left) as usize];
                if glyph.a <= 0.0 && glyph.r <= 0.0 && glyph.g <= 0.0 && glyph.b <= 0.0 {
                    continue;
                }
                let mut source = sample_picture(
                    src,
                    (x_src + x - origin_x) as f32 + 0.5,
                    (y_src + y - origin_y) as f32 + 0.5,
                    &prepared,
                    0,
                );
                if component_alpha {
                    source.r *= glyph.r;
                    source.g *= glyph.g;
                    source.b *= glyph.b;
                    source.a *= glyph.a;
                } else {
                    source = source.scale(glyph.a);
                }
                let index = y as usize * width as usize + x as usize;
                let destination = Pixel::from_raw(pixels[index], destination_record.format);
                pixels[index] =
                    composite_pixel(op, source, destination).to_raw(destination_record.format);
            }
        }
    })
}

fn placement_component_alpha(placements: &[GlyphPlacement]) -> bool {
    placements
        .iter()
        .any(|placement| placement.glyph.format == 1)
}

#[allow(clippy::too_many_arguments)]
fn composite_string(
    op: c_int,
    src: Picture,
    dst: Picture,
    glyph_set: GlyphSet,
    x_src: i32,
    y_src: i32,
    x_dst: i32,
    y_dst: i32,
    ids: &[Glyph],
) {
    let (mut pen_x, mut pen_y) = (x_dst, y_dst);
    let mut placements = Vec::new();
    append_glyphs(&mut placements, glyph_set, ids, &mut pen_x, &mut pen_y);
    let _ = composite_glyph_placements(op, src, dst, x_src, y_src, x_dst, y_dst, &placements);
}

macro_rules! composite_string_api {
    ($name:ident, $character:ty) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libX11_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _display: *mut Display,
            op: c_int,
            src: Picture,
            dst: Picture,
            _mask_format: *const XRenderPictFormat,
            glyph_set: GlyphSet,
            x_src: c_int,
            y_src: c_int,
            x_dst: c_int,
            y_dst: c_int,
            string: *const $character,
            count: c_int,
        ) {
            if count < 0 || (count > 0 && string.is_null()) {
                return;
            }
            let ids = unsafe { core::slice::from_raw_parts(string, count as usize) }
                .iter()
                .map(|value| *value as Glyph)
                .collect::<Vec<_>>();
            composite_string(op, src, dst, glyph_set, x_src, y_src, x_dst, y_dst, &ids);
        }
    };
}

composite_string_api!(XRenderCompositeString8, u8);
composite_string_api!(XRenderCompositeString16, u16);
composite_string_api!(XRenderCompositeString32, u32);

#[allow(clippy::too_many_arguments)]
fn composite_text_elements<T, F>(
    op: c_int,
    src: Picture,
    dst: Picture,
    x_src: i32,
    y_src: i32,
    x_dst: i32,
    y_dst: i32,
    elements: &[T],
    fields: F,
) where
    F: Fn(&T) -> (GlyphSet, i32, i32, Vec<Glyph>),
{
    // CompositeGlyphs carries no destination origin: the pen starts at (0, 0)
    // and each element's offset is relative to the pen after the previous
    // glyph's advance.  Xlib's xDst/yDst only align xSrc/ySrc with the
    // destination; cairo passes the first element's offset there as well.
    let (mut pen_x, mut pen_y) = (0i32, 0i32);
    let mut placements = Vec::new();
    for element in elements {
        let (glyph_set, x_offset, y_offset, ids) = fields(element);
        pen_x = pen_x.saturating_add(x_offset);
        pen_y = pen_y.saturating_add(y_offset);
        append_glyphs(&mut placements, glyph_set, &ids, &mut pen_x, &mut pen_y);
    }
    let started = std::time::Instant::now();
    let _ = composite_glyph_placements(op, src, dst, x_src, y_src, x_dst, y_dst, &placements);
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XRenderCompositeText {} glyphs to dst={dst:#x} took {:?}",
            placements.len(),
            started.elapsed()
        );
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeText8")]
pub unsafe extern "sysv64" fn XRenderCompositeText8(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    _mask_format: *const XRenderPictFormat,
    x_src: c_int,
    y_src: c_int,
    x_dst: c_int,
    y_dst: c_int,
    elements: *const XGlyphElt8,
    count: c_int,
) {
    if count < 0 || (count > 0 && elements.is_null()) {
        return;
    }
    let elements = unsafe { core::slice::from_raw_parts(elements, count as usize) };
    composite_text_elements(
        op,
        src,
        dst,
        x_src,
        y_src,
        x_dst,
        y_dst,
        elements,
        |element| {
            let ids = if element.nchars <= 0 || element.chars.is_null() {
                Vec::new()
            } else {
                unsafe {
                    core::slice::from_raw_parts(element.chars.cast::<u8>(), element.nchars as usize)
                }
                .iter()
                .map(|value| *value as Glyph)
                .collect()
            };
            (element.glyphset, element.xOff, element.yOff, ids)
        },
    );
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeText16")]
pub unsafe extern "sysv64" fn XRenderCompositeText16(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    _mask_format: *const XRenderPictFormat,
    x_src: c_int,
    y_src: c_int,
    x_dst: c_int,
    y_dst: c_int,
    elements: *const XGlyphElt16,
    count: c_int,
) {
    if count < 0 || (count > 0 && elements.is_null()) {
        return;
    }
    let elements = unsafe { core::slice::from_raw_parts(elements, count as usize) };
    composite_text_elements(
        op,
        src,
        dst,
        x_src,
        y_src,
        x_dst,
        y_dst,
        elements,
        |element| {
            let ids = if element.nchars <= 0 || element.chars.is_null() {
                Vec::new()
            } else {
                unsafe { core::slice::from_raw_parts(element.chars, element.nchars as usize) }
                    .iter()
                    .map(|value| *value as Glyph)
                    .collect()
            };
            (element.glyphset, element.xOff, element.yOff, ids)
        },
    );
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCompositeText32")]
pub unsafe extern "sysv64" fn XRenderCompositeText32(
    _display: *mut Display,
    op: c_int,
    src: Picture,
    dst: Picture,
    _mask_format: *const XRenderPictFormat,
    x_src: c_int,
    y_src: c_int,
    x_dst: c_int,
    y_dst: c_int,
    elements: *const XGlyphElt32,
    count: c_int,
) {
    if count < 0 || (count > 0 && elements.is_null()) {
        return;
    }
    let elements = unsafe { core::slice::from_raw_parts(elements, count as usize) };
    composite_text_elements(
        op,
        src,
        dst,
        x_src,
        y_src,
        x_dst,
        y_dst,
        elements,
        |element| {
            let ids = if element.nchars <= 0 || element.chars.is_null() {
                Vec::new()
            } else {
                unsafe { core::slice::from_raw_parts(element.chars, element.nchars as usize) }
                    .to_vec()
            };
            (element.glyphset, element.xOff, element.yOff, ids)
        },
    );
}

#[derive(Clone)]
enum RenderCursor {
    Picture {
        width: i32,
        height: i32,
        hotspot_x: u32,
        hotspot_y: u32,
        pixels: Vec<u32>,
    },
    Animated(Vec<XAnimCursor>),
}

struct CursorRegistry {
    next: Cursor,
    cursors: HashMap<Cursor, RenderCursor>,
}

fn cursors() -> &'static Mutex<CursorRegistry> {
    static CURSORS: OnceLock<Mutex<CursorRegistry>> = OnceLock::new();
    CURSORS.get_or_init(|| {
        Mutex::new(CursorRegistry {
            next: 0x8000,
            cursors: HashMap::new(),
        })
    })
}

fn insert_cursor(cursor: RenderCursor) -> Cursor {
    let Ok(mut registry) = cursors().lock() else {
        return 0;
    };
    let id = registry.next;
    registry.next = registry.next.saturating_add(1);
    registry.cursors.insert(id, cursor);
    id
}

pub(crate) fn free_cursor(cursor: Cursor) -> bool {
    cursors()
        .lock()
        .ok()
        .and_then(|mut registry| registry.cursors.remove(&cursor))
        .is_some()
}

pub(crate) fn cursor_has_visible_pixels(cursor: Cursor) -> Option<bool> {
    fn visible(registry: &CursorRegistry, cursor: Cursor) -> Option<bool> {
        match registry.cursors.get(&cursor)? {
            RenderCursor::Picture { pixels, .. } => {
                Some(pixels.iter().any(|pixel| pixel >> 24 != 0))
            }
            RenderCursor::Animated(frames) => Some(
                frames
                    .iter()
                    .any(|frame| visible(registry, frame.cursor).unwrap_or(true)),
            ),
        }
    }

    let registry = cursors().lock().ok()?;
    visible(&registry, cursor)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateCursor")]
pub unsafe extern "sysv64" fn XRenderCreateCursor(
    _display: *mut Display,
    source: Picture,
    hotspot_x: u32,
    hotspot_y: u32,
) -> Cursor {
    let Some(prepared) = prepared_pictures(&[source], 0) else {
        return 0;
    };
    let Some(picture) = prepared.get(&source) else {
        return 0;
    };
    let Some(surface) = picture.surface.as_ref() else {
        return 0;
    };
    if hotspot_x >= surface.width as u32 || hotspot_y >= surface.height as u32 {
        return 0;
    }
    let mut pixels = Vec::with_capacity(surface.pixels.len());
    for y in 0..surface.height {
        for x in 0..surface.width {
            pixels.push(
                sample_picture(source, x as f32 + 0.5, y as f32 + 0.5, &prepared, 0).to_raw(1),
            );
        }
    }
    insert_cursor(RenderCursor::Picture {
        width: surface.width,
        height: surface.height,
        hotspot_x,
        hotspot_y,
        pixels,
    })
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRenderCreateAnimCursor")]
pub unsafe extern "sysv64" fn XRenderCreateAnimCursor(
    _display: *mut Display,
    count: c_int,
    frames: *mut XAnimCursor,
) -> Cursor {
    if count <= 0 || frames.is_null() {
        return 0;
    }
    let frames = unsafe { core::slice::from_raw_parts(frames, count as usize) };
    let Ok(registry) = cursors().lock() else {
        return 0;
    };
    if frames
        .iter()
        .any(|frame| frame.delay == 0 || !registry.cursors.contains_key(&frame.cursor))
    {
        return 0;
    }
    drop(registry);
    insert_cursor(RenderCursor::Animated(frames.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{XCreatePixmap, XFreePixmap};

    fn fixed_test(value: f32) -> XFixed {
        (value * FIXED_ONE).round() as XFixed
    }

    unsafe fn make_picture(
        standard_format: c_int,
        depth: u32,
        width: u32,
        height: u32,
    ) -> (Pixmap, Picture) {
        let pixmap = unsafe { XCreatePixmap(core::ptr::null_mut(), 0, width, height, depth) };
        assert_ne!(pixmap, 0);
        let format = unsafe { XRenderFindStandardFormat(core::ptr::null_mut(), standard_format) };
        assert!(!format.is_null());
        let picture = unsafe {
            XRenderCreatePicture(core::ptr::null_mut(), pixmap, format, 0, core::ptr::null())
        };
        assert_ne!(picture, 0);
        (pixmap, picture)
    }

    unsafe fn destroy_picture(pixmap: Pixmap, picture: Picture) {
        unsafe { XRenderFreePicture(core::ptr::null_mut(), picture) };
        unsafe { XFreePixmap(core::ptr::null_mut(), pixmap) };
    }

    fn raw_pixels(drawable: Drawable) -> Vec<u32> {
        drawable_snapshot(drawable)
            .expect("drawable snapshot")
            .pixels
    }

    #[test]
    fn standard_formats_match_render_0_11_layout() {
        unsafe {
            let argb = &*XRenderFindStandardFormat(core::ptr::null_mut(), 0);
            assert_eq!(argb.depth, 32);
            assert_eq!(argb.direct, [16, 0xff, 8, 0xff, 0, 0xff, 24, 0xff]);
            let a8 = &*XRenderFindStandardFormat(core::ptr::null_mut(), 2);
            assert_eq!(a8.depth, 8);
            assert_eq!(a8.direct[6..], [0, 0xff]);
            let mut major = -1;
            let mut minor = -1;
            assert_eq!(
                XRenderQueryVersion(core::ptr::null_mut(), &raw mut major, &raw mut minor),
                1
            );
            assert_eq!((major, minor), (0, 11));
        }
    }

    #[test]
    fn solid_composite_is_premultiplied_and_supports_all_operators() {
        let source = Pixel {
            r: 0.20,
            g: 0.10,
            b: 0.05,
            a: 0.50,
        };
        let destination = Pixel {
            r: 0.10,
            g: 0.20,
            b: 0.30,
            a: 0.75,
        };
        for op in (0..=13)
            .chain(0x10..=0x1b)
            .chain(0x20..=0x2b)
            .chain(0x30..=0x3e)
        {
            let value = composite_pixel(op, source, destination);
            assert!(value.r.is_finite() && value.g.is_finite() && value.b.is_finite());
            assert!(value.a.is_finite());
            assert!((0.0..=1.0).contains(&value.a));
        }
        assert_eq!(composite_pixel(0, source, destination).a, 0.0);
        assert_eq!(composite_pixel(1, source, destination).a, source.a);
        assert_eq!(composite_pixel(2, source, destination).a, destination.a);

        unsafe {
            let (pixmap, picture) = make_picture(0, 32, 1, 1);
            let red = XRenderColor {
                red: u16::MAX,
                alpha: 0x8000,
                ..XRenderColor::default()
            };
            let solid = XRenderCreateSolidFill(core::ptr::null_mut(), &raw const red);
            XRenderComposite(
                core::ptr::null_mut(),
                3,
                solid,
                0,
                picture,
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                1,
            );
            assert_eq!(raw_pixels(pixmap)[0], 0x8080_0000);
            XRenderFreePicture(core::ptr::null_mut(), solid);
            destroy_picture(pixmap, picture);
        }
    }

    #[test]
    fn component_alpha_masks_each_channel_independently() {
        unsafe {
            let (mask_pixmap, mask_picture) = make_picture(0, 32, 1, 1);
            let _ = mutate_drawable(
                mask_pixmap,
                RECT {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                |pixels, _, _, _| pixels[0] = 0xffff_0000,
            );
            let mask_attributes = XRenderPictureAttributes {
                component_alpha: 1,
                ..XRenderPictureAttributes::default()
            };
            XRenderChangePicture(
                core::ptr::null_mut(),
                mask_picture,
                1 << 12,
                &raw const mask_attributes,
            );
            let white = XRenderColor {
                red: u16::MAX,
                green: u16::MAX,
                blue: u16::MAX,
                alpha: u16::MAX,
            };
            let source = XRenderCreateSolidFill(core::ptr::null_mut(), &raw const white);
            let (destination_pixmap, destination) = make_picture(0, 32, 1, 1);
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                source,
                mask_picture,
                destination,
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                1,
            );
            assert_eq!(raw_pixels(destination_pixmap)[0], 0xffff_0000);

            XRenderFreePicture(core::ptr::null_mut(), source);
            destroy_picture(destination_pixmap, destination);
            destroy_picture(mask_pixmap, mask_picture);
        }
    }

    #[test]
    fn transform_repeat_bilinear_and_convolution_sample_pixel_centres() {
        unsafe {
            let (source_pixmap, source_picture) = make_picture(0, 32, 3, 1);
            let _ = mutate_drawable(
                source_pixmap,
                RECT {
                    left: 0,
                    top: 0,
                    right: 3,
                    bottom: 1,
                },
                |pixels, _, _, _| pixels.copy_from_slice(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff]),
            );
            let mut attributes = XRenderPictureAttributes {
                repeat: 2,
                ..XRenderPictureAttributes::default()
            };
            XRenderChangePicture(
                core::ptr::null_mut(),
                source_picture,
                1,
                &raw const attributes,
            );

            let (bilinear_pixmap, bilinear_picture) = make_picture(0, 32, 3, 1);
            XRenderSetPictureFilter(
                core::ptr::null_mut(),
                source_picture,
                c"bilinear".as_ptr(),
                core::ptr::null_mut(),
                0,
            );
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                source_picture,
                0,
                bilinear_picture,
                0,
                0,
                0,
                0,
                0,
                0,
                3,
                1,
            );
            assert_eq!(
                raw_pixels(bilinear_pixmap),
                [0xffff_0000, 0xff00_ff00, 0xff00_00ff]
            );

            let mut convolution = [
                fixed_test(3.0),
                fixed_test(1.0),
                fixed_test(0.25),
                fixed_test(0.5),
                fixed_test(0.25),
            ];
            XRenderSetPictureFilter(
                core::ptr::null_mut(),
                source_picture,
                c"convolution".as_ptr(),
                convolution.as_mut_ptr(),
                convolution.len() as c_int,
            );
            let (convolution_pixmap, convolution_picture) = make_picture(0, 32, 3, 1);
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                source_picture,
                0,
                convolution_picture,
                0,
                0,
                0,
                0,
                0,
                0,
                3,
                1,
            );
            assert_eq!(raw_pixels(convolution_pixmap)[1], 0xff40_8040);

            XRenderSetPictureFilter(
                core::ptr::null_mut(),
                source_picture,
                c"nearest".as_ptr(),
                core::ptr::null_mut(),
                0,
            );
            let mut transform = XTransform::default();
            transform.matrix[0][2] = fixed_test(1.0);
            XRenderSetPictureTransform(core::ptr::null_mut(), source_picture, &raw mut transform);
            let (translated_pixmap, translated_picture) = make_picture(0, 32, 1, 1);
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                source_picture,
                0,
                translated_picture,
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                1,
            );
            assert_eq!(raw_pixels(translated_pixmap)[0], 0xff00_ff00);

            attributes.repeat = 0;
            XRenderChangePicture(
                core::ptr::null_mut(),
                source_picture,
                1,
                &raw const attributes,
            );
            destroy_picture(translated_pixmap, translated_picture);
            destroy_picture(convolution_pixmap, convolution_picture);
            destroy_picture(bilinear_pixmap, bilinear_picture);
            destroy_picture(source_pixmap, source_picture);
        }
    }

    #[test]
    fn gradients_clip_and_alpha_map_compose_through_picture_state() {
        unsafe {
            let (pixmap, destination) = make_picture(0, 32, 3, 1);
            let gradient = XLinearGradient {
                p1: XPointFixed { x: 0, y: 0 },
                p2: XPointFixed {
                    x: fixed_test(2.0),
                    y: 0,
                },
            };
            let stops = [0, fixed_test(1.0)];
            let colors = [
                XRenderColor {
                    red: u16::MAX,
                    alpha: u16::MAX,
                    ..XRenderColor::default()
                },
                XRenderColor {
                    blue: u16::MAX,
                    alpha: u16::MAX,
                    ..XRenderColor::default()
                },
            ];
            let source = XRenderCreateLinearGradient(
                core::ptr::null_mut(),
                &raw const gradient,
                stops.as_ptr(),
                colors.as_ptr(),
                2,
            );
            let clip = XRectangle {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            };
            XRenderSetPictureClipRectangles(
                core::ptr::null_mut(),
                destination,
                0,
                0,
                &raw const clip,
                1,
            );
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                source,
                0,
                destination,
                0,
                0,
                0,
                0,
                0,
                0,
                3,
                1,
            );
            assert_eq!(raw_pixels(pixmap), [0, 0xff40_00bf, 0]);

            let (alpha_pixmap, alpha_picture) = make_picture(2, 8, 1, 1);
            let _ = mutate_drawable(
                alpha_pixmap,
                RECT {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                |pixels, _, _, _| pixels[0] = 0x80,
            );
            let green = XRenderColor {
                green: u16::MAX,
                // Matching half-alpha in the source and alpha-map proves that
                // alpha-map replaces source alpha instead of multiplying it.
                alpha: 0x8000,
                ..XRenderColor::default()
            };
            let solid = XRenderCreateSolidFill(core::ptr::null_mut(), &raw const green);
            let alpha_attributes = XRenderPictureAttributes {
                alpha_map: alpha_picture,
                ..XRenderPictureAttributes::default()
            };
            XRenderChangePicture(
                core::ptr::null_mut(),
                solid,
                1 << 1,
                &raw const alpha_attributes,
            );
            let (alpha_dst_pixmap, alpha_destination) = make_picture(0, 32, 1, 1);
            XRenderComposite(
                core::ptr::null_mut(),
                1,
                solid,
                0,
                alpha_destination,
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                1,
            );
            assert_eq!(raw_pixels(alpha_dst_pixmap)[0], 0x8000_8000);

            XRenderFreePicture(core::ptr::null_mut(), solid);
            XRenderFreePicture(core::ptr::null_mut(), source);
            destroy_picture(alpha_dst_pixmap, alpha_destination);
            destroy_picture(alpha_pixmap, alpha_picture);
            destroy_picture(pixmap, destination);
        }
    }

    /// cairo passes the first element's offset both as xDst/yDst and in the
    /// element; the protocol pen starts at the origin, so the glyph lands once.
    #[test]
    fn glyph_elements_position_from_the_origin_not_from_the_source_alignment() {
        unsafe {
            let (pixmap, destination) = make_picture(0, 32, 8, 4);
            let red = XRenderColor {
                red: u16::MAX,
                alpha: u16::MAX,
                ..XRenderColor::default()
            };
            let source = XRenderCreateSolidFill(core::ptr::null_mut(), &raw const red);
            let glyph_set = XRenderCreateGlyphSet(
                core::ptr::null_mut(),
                XRenderFindStandardFormat(core::ptr::null_mut(), 2),
            );
            let glyph_id = 9;
            let glyph_info = XGlyphInfo {
                width: 1,
                height: 1,
                xOff: 1,
                ..XGlyphInfo::default()
            };
            let glyph_image = [0xffu8, 0, 0, 0];
            XRenderAddGlyphs(
                core::ptr::null_mut(),
                glyph_set,
                &raw const glyph_id,
                &raw const glyph_info,
                1,
                glyph_image.as_ptr().cast(),
                glyph_image.len() as c_int,
            );
            let characters = [glyph_id as u8, glyph_id as u8];
            let elements = [
                XGlyphElt8 {
                    glyphset: glyph_set,
                    chars: characters.as_ptr().cast(),
                    nchars: 1,
                    xOff: 2,
                    yOff: 1,
                },
                // Relative to the pen after the first glyph's advance (3, 1).
                XGlyphElt8 {
                    glyphset: glyph_set,
                    chars: characters.as_ptr().cast(),
                    nchars: 1,
                    xOff: 2,
                    yOff: 1,
                },
            ];
            XRenderCompositeText8(
                core::ptr::null_mut(),
                3,
                source,
                destination,
                core::ptr::null(),
                2,
                1,
                2,
                1,
                elements.as_ptr(),
                2,
            );
            let pixels = raw_pixels(pixmap);
            let lit: Vec<usize> = pixels
                .iter()
                .enumerate()
                .filter(|(_, p)| **p != 0)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(lit, vec![1 * 8 + 2, 2 * 8 + 5]);
            XRenderFreeGlyphSet(core::ptr::null_mut(), glyph_set);
            XRenderFreePicture(core::ptr::null_mut(), source);
            destroy_picture(pixmap, destination);
        }
    }

    #[test]
    fn glyph_geometry_traps_and_cursor_resources_have_real_pixels() {
        unsafe {
            let (pixmap, destination) = make_picture(0, 32, 4, 2);
            let red = XRenderColor {
                red: u16::MAX,
                alpha: u16::MAX,
                ..XRenderColor::default()
            };
            let source = XRenderCreateSolidFill(core::ptr::null_mut(), &raw const red);
            let glyph_set = XRenderCreateGlyphSet(
                core::ptr::null_mut(),
                XRenderFindStandardFormat(core::ptr::null_mut(), 2),
            );
            let glyph_id = 7;
            let glyph_info = XGlyphInfo {
                width: 2,
                height: 1,
                xOff: 2,
                ..XGlyphInfo::default()
            };
            let glyph_image = [0xffu8, 0x80, 0, 0];
            XRenderAddGlyphs(
                core::ptr::null_mut(),
                glyph_set,
                &raw const glyph_id,
                &raw const glyph_info,
                1,
                glyph_image.as_ptr().cast(),
                glyph_image.len() as c_int,
            );
            let character = glyph_id as u8;
            XRenderCompositeString8(
                core::ptr::null_mut(),
                3,
                source,
                destination,
                core::ptr::null(),
                glyph_set,
                0,
                0,
                0,
                0,
                &raw const character,
                1,
            );
            let glyph_pixels = raw_pixels(pixmap);
            assert_eq!(glyph_pixels[0], 0xffff_0000);
            assert_eq!(glyph_pixels[1], 0x8080_0000);

            let triangle = XTriangle {
                p1: XPointFixed {
                    x: fixed_test(2.0),
                    y: 0,
                },
                p2: XPointFixed {
                    x: fixed_test(4.0),
                    y: 0,
                },
                p3: XPointFixed {
                    x: fixed_test(4.0),
                    y: fixed_test(2.0),
                },
            };
            XRenderCompositeTriangles(
                core::ptr::null_mut(),
                3,
                source,
                destination,
                XRenderFindStandardFormat(core::ptr::null_mut(), 4),
                0,
                0,
                &raw const triangle,
                1,
            );
            assert_ne!(raw_pixels(pixmap)[3], 0);

            let (mask_pixmap, mask_picture) = make_picture(2, 8, 1, 1);
            let trap = XTrap {
                top: XSpanFix {
                    left: 0,
                    right: fixed_test(1.0),
                    y: 0,
                },
                bottom: XSpanFix {
                    left: 0,
                    right: fixed_test(1.0),
                    y: fixed_test(1.0),
                },
            };
            XRenderAddTraps(
                core::ptr::null_mut(),
                mask_picture,
                0,
                0,
                &raw const trap,
                1,
            );
            assert_eq!(raw_pixels(mask_pixmap)[0], 0xff);

            let cursor = XRenderCreateCursor(core::ptr::null_mut(), destination, 0, 0);
            assert_ne!(cursor, 0);
            assert!(free_cursor(cursor));
            let referenced = XRenderReferenceGlyphSet(core::ptr::null_mut(), glyph_set);
            assert_ne!(referenced, 0);
            XRenderFreeGlyphSet(core::ptr::null_mut(), glyph_set);
            XRenderFreeGlyphSet(core::ptr::null_mut(), referenced);
            XRenderFreePicture(core::ptr::null_mut(), source);
            destroy_picture(mask_pixmap, mask_picture);
            destroy_picture(pixmap, destination);
        }
    }
}
