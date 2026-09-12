//! X11 2D Graphics, GC management, Pixmaps, Colors, and Query functions.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_void};
use std::os::raw::{c_char, c_int, c_short, c_uchar, c_uint, c_ushort};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleDC,
    CreateDIBSection, CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, Ellipse, FillRect,
    GdiFlush, GetDC, GetStockObject, HDC, LineTo, MoveToEx, Pie, Polyline, Rectangle, ReleaseDC,
    SRCCOPY, ScreenToClient, SelectObject, SetBkColor, SetDCBrushColor, SetDCPenColor,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    CompositingModeSourceCopy, CompositingModeSourceOver, FillModeAlternate, FillModeWinding,
    FlushIntentionSync, GdipCreateFromHDC, GdipCreateSolidFill, GdipDeleteBrush,
    GdipDeleteGraphics, GdipFillPolygon, GdipFlush, GdipSetCompositingMode, GdipSetSmoothingMode,
    GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics, GpSolidFill, PointF,
    SmoothingModeAntiAlias8x8,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClientRect, GetCursorPos};

use crate::{Colormap, Display, Visual, Window, XRectangle};

pub(crate) mod background;
pub mod buffered;
pub(crate) mod clipping;
mod points;

static NEXT_DRAWABLE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0x3000);
pub fn allocate_drawable_id() -> usize {
    NEXT_DRAWABLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
pub(crate) fn reserve_drawable_ids(next: usize) {
    NEXT_DRAWABLE.fetch_max(next, std::sync::atomic::Ordering::Relaxed);
}

type c_long = i64;
type c_ulong = u64;
type Bool = c_int;
type Status = c_int;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XGCValues {
    pub function: c_int,
    pub plane_mask: c_ulong,
    pub foreground: c_ulong,
    pub background: c_ulong,
    pub line_width: c_int,
    pub line_style: c_int,
    pub cap_style: c_int,
    pub join_style: c_int,
    pub fill_style: c_int,
    pub fill_rule: c_int,
    pub arc_mode: c_int,
    pub tile: usize,
    pub stipple: usize,
    pub ts_x_origin: c_int,
    pub ts_y_origin: c_int,
    pub font: usize,
    pub subwindow_mode: c_int,
    pub graphics_exposures: Bool,
    pub clip_x_origin: c_int,
    pub clip_y_origin: c_int,
    pub clip_mask: usize,
    pub dash_offset: c_int,
    pub dashes: c_char,
}

#[derive(Clone, Debug, Default)]
pub struct GCRec {
    pub values: XGCValues,
    pub(crate) clip_rectangles: Option<Vec<[i32; 4]>>,
}

pub type GC = *mut GCRec;

fn window_backgrounds() -> &'static Mutex<HashMap<Window, background::Background>> {
    static BACKGROUNDS: OnceLock<Mutex<HashMap<Window, background::Background>>> = OnceLock::new();
    BACKGROUNDS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct BackBuffer {
    owner: Arc<SurfaceOwner>,
    dc: usize,
    bitmap: usize,
    previous_bitmap: usize,
    bits: usize,
    graphics: usize,
    width: i32,
    height: i32,
    depth: u32,
    dirty: Option<RECT>,
    resize_background: bool,
    presentable: bool,
    imported: Option<(u32, u64)>,
}
mod inferiors;

// Composite pixmap names retain this surface even after its window is resized
// or destroyed. GDI ownership is released only with the final reference.
struct SurfaceOwner {
    dc: usize,
    bitmap: usize,
    previous_bitmap: usize,
    shared: Option<crate::shared::Mapping>,
}
impl Drop for SurfaceOwner {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc as HDC, self.previous_bitmap as _);
            DeleteObject(self.bitmap as _);
            DeleteDC(self.dc as HDC);
        }
    }
}

fn back_buffers() -> &'static Mutex<HashMap<Window, BackBuffer>> {
    static BUFFERS: OnceLock<Mutex<HashMap<Window, BackBuffer>>> = OnceLock::new();
    BUFFERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pixmap_buffers() -> &'static Mutex<HashMap<usize, BackBuffer>> {
    static PIXMAPS: OnceLock<Mutex<HashMap<usize, BackBuffer>>> = OnceLock::new();
    PIXMAPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn pixmap_has_visible_pixels(pixmap: usize) -> bool {
    let Ok(pixmaps) = pixmap_buffers().lock() else {
        return true;
    };
    let Some(buffer) = pixmaps.get(&pixmap) else {
        return true;
    };
    synchronize_buffer(buffer);
    let length = (buffer.width as usize).saturating_mul(buffer.height as usize);
    if buffer.bits == 0 || length == 0 {
        return false;
    }
    let pixels = unsafe { core::slice::from_raw_parts(buffer.bits as *const u32, length) };
    pixels.iter().any(|pixel| *pixel != 0)
}

#[derive(Default)]
struct PresentSchedule {
    first_dirty: Option<Instant>,
    last_dirty: Option<Instant>,
}

fn presenter() -> &'static Arc<(Mutex<PresentSchedule>, Condvar)> {
    static PRESENTER: OnceLock<Arc<(Mutex<PresentSchedule>, Condvar)>> = OnceLock::new();
    PRESENTER.get_or_init(|| {
        let state = Arc::new((Mutex::new(PresentSchedule::default()), Condvar::new()));
        let worker = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("kinakaze-x11-present".to_owned())
            .spawn(move || {
                loop {
                    let (lock, ready) = &*worker;
                    let mut schedule = lock.lock().unwrap_or_else(|error| error.into_inner());
                    while schedule.first_dirty.is_none() {
                        schedule = ready
                            .wait(schedule)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                    loop {
                        let now = Instant::now();
                        let first_deadline = schedule
                            .first_dirty
                            .unwrap_or(now)
                            .checked_add(Duration::from_millis(16))
                            .unwrap_or(now);
                        let idle_deadline = schedule
                            .last_dirty
                            .unwrap_or(now)
                            .checked_add(Duration::from_millis(2))
                            .unwrap_or(now);
                        let deadline = first_deadline.min(idle_deadline);
                        if now >= deadline {
                            break;
                        }
                        let waited = ready
                            .wait_timeout(schedule, deadline.saturating_duration_since(now))
                            .unwrap_or_else(|error| error.into_inner());
                        schedule = waited.0;
                    }
                    schedule.first_dirty = None;
                    schedule.last_dirty = None;
                    drop(schedule);
                    flush_all();
                }
            });
        state
    })
}

/// Native X11 calls execute synchronously inside the guest, so there is no remote
/// server that will eventually present a buffered request after the function
/// returns. Coalesce adjacent requests into one short frame while still bounding
/// latency for clients that render and then block without calling XFlush.
fn request_present() {
    let (lock, ready) = &**presenter();
    let now = Instant::now();
    let mut schedule = lock.lock().unwrap_or_else(|error| error.into_inner());
    schedule.first_dirty.get_or_insert(now);
    schedule.last_dirty = Some(now);
    ready.notify_one();
}

fn create_dib_buffer(reference_dc: HDC, width: i32, height: i32, depth: u32) -> Option<BackBuffer> {
    create_dib_surface(reference_dc, width, height, depth, None)
}
fn create_dib_surface(
    reference_dc: HDC,
    width: i32,
    height: i32,
    depth: u32,
    shared: Option<crate::shared::Mapping>,
) -> Option<BackBuffer> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let dc = unsafe { CreateCompatibleDC(reference_dc) };
    if dc.is_null() {
        return None;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height makes the DIB top-down, matching X11 coordinates.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: width.saturating_mul(height).saturating_mul(4) as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = core::ptr::null_mut::<c_void>();
    let bitmap = unsafe {
        CreateDIBSection(
            reference_dc,
            &info,
            DIB_RGB_COLORS,
            &raw mut bits,
            shared
                .as_ref()
                .map_or(core::ptr::null_mut(), |s| s.handle as _),
            0,
        )
    };
    if bitmap.is_null() || bits.is_null() {
        if !bitmap.is_null() {
            unsafe { DeleteObject(bitmap) };
        }
        unsafe { DeleteDC(dc) };
        return None;
    }
    let previous_bitmap = unsafe { SelectObject(dc, bitmap) };
    Some(BackBuffer {
        owner: Arc::new(SurfaceOwner {
            dc: dc as usize,
            bitmap: bitmap as usize,
            previous_bitmap: previous_bitmap as usize,
            shared,
        }),
        dc: dc as usize,
        bitmap: bitmap as usize,
        previous_bitmap: previous_bitmap as usize,
        bits: bits as usize,
        graphics: 0,
        width,
        height,
        depth,
        dirty: None,
        resize_background: false,
        presentable: true,
        imported: None,
    })
}

fn union_rect(first: RECT, second: RECT) -> RECT {
    RECT {
        left: first.left.min(second.left),
        top: first.top.min(second.top),
        right: first.right.max(second.right),
        bottom: first.bottom.max(second.bottom),
    }
}

fn mark_dirty(window: Window, rect: RECT) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let mut changed = false;
    if let Ok(mut buffers) = back_buffers().lock()
        && let Some(buffer) = buffers.get_mut(&window)
    {
        let clipped = RECT {
            left: rect.left.clamp(0, buffer.width),
            top: rect.top.clamp(0, buffer.height),
            right: rect.right.clamp(0, buffer.width),
            bottom: rect.bottom.clamp(0, buffer.height),
        };
        if clipped.right > clipped.left && clipped.bottom > clipped.top {
            buffer.dirty = Some(match buffer.dirty {
                Some(current) => union_rect(current, clipped),
                None => clipped,
            });
            changed = true;
        }
    }
    if changed {
        request_present();
    }
    notify_damage(window, rect);
}

/// Restore retained pixels after native exposure without synthesizing drawing
/// damage or a resize. Clients without ExposureMask still retain their contents.
pub fn expose(window: Window, x: i32, y: i32, width: i32, height: i32) {
    let mut changed = false;
    if let Ok(mut buffers) = back_buffers().lock()
        && let Some(buffer) = buffers.get_mut(&window)
    {
        let rect = RECT {
            left: x.clamp(0, buffer.width),
            top: y.clamp(0, buffer.height),
            right: x.saturating_add(width).clamp(0, buffer.width),
            bottom: y.saturating_add(height).clamp(0, buffer.height),
        };
        if rect.right > rect.left && rect.bottom > rect.top {
            buffer.dirty = Some(buffer.dirty.map_or(rect, |dirty| union_rect(dirty, rect)));
            changed = true;
        }
    }
    if changed {
        request_present();
    }
}

fn notify_damage(drawable: usize, rect: RECT) {
    if let Some((w, h, _)) = drawable_dimensions(drawable) {
        let x = rect.left.clamp(0, w);
        let y = rect.top.clamp(0, h);
        let width = (rect.right.clamp(0, w) - x).max(0);
        let height = (rect.bottom.clamp(0, h) - y).max(0);
        crate::damage::changed(
            drawable,
            crate::XRectangle {
                x: x as i16,
                y: y as i16,
                width: width as u16,
                height: height as u16,
            },
            w.min(65535) as u16,
            h.min(65535) as u16,
        );
    }
}

unsafe fn delete_back_buffer(buffer: BackBuffer) {
    unsafe {
        if buffer.graphics != 0 {
            GdipDeleteGraphics(buffer.graphics as *mut GpGraphics);
        }
    }
}

/// Starts or resets an off-screen X11 frame for a real HWND. The frame remains
/// private until the client returns to its event wait or explicitly flushes.
fn prepare_back_buffer(window: Window, clear: bool) -> bool {
    let hwnd = window as HWND;
    if hwnd.is_null() {
        return false;
    }
    let mut rect: RECT = unsafe { core::mem::zeroed() };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect) } == 0 {
        return false;
    }
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    if kinakaze_libdisplay::window::logical_for_native(window).is_none()
        && crate::shared::window_owner(window).is_some_and(|owner| owner != crate::shared::pid())
    {
        let Some(info) = crate::shared::get(&format!("surface/{window}")) else {
            return false;
        };
        if info.len() != 24 {
            return false;
        }
        let word = |offset| u32::from_le_bytes(info[offset..offset + 4].try_into().unwrap());
        let (width, height, depth, pid) = (word(0) as i32, word(4) as i32, word(8), word(12));
        let serial = u64::from_le_bytes(info[16..24].try_into().unwrap());
        let mut buffers = back_buffers().lock().unwrap_or_else(|e| e.into_inner());
        if buffers
            .get(&window)
            .is_some_and(|buffer| buffer.imported == Some((pid, serial)))
        {
            return true;
        }
        if width <= 0 || height <= 0 {
            return false;
        }
        let Some(mapping) = crate::shared::Mapping::new(
            &format!("surface-{pid}-{serial}"),
            width as usize * height as usize * 4,
        ) else {
            return false;
        };
        let Some(mut buffer) =
            create_dib_surface(core::ptr::null_mut(), width, height, depth, Some(mapping))
        else {
            return false;
        };
        buffer.imported = Some((pid, serial));
        buffer.presentable = false;
        if let Some(previous) = buffers.insert(window, buffer) {
            unsafe {
                delete_back_buffer(previous);
            }
        }
        return true;
    }
    let Ok(mut buffers) = back_buffers().lock() else {
        return false;
    };
    let needs_new = buffers
        .get(&window)
        .is_none_or(|buffer| buffer.width != width || buffer.height != height);
    let resize_background = buffers
        .get(&window)
        .is_some_and(|buffer| buffer.resize_background);
    let clear = clear || (needs_new && resize_background);
    if needs_new {
        let window_dc = unsafe { GetDC(hwnd) };
        if window_dc.is_null() {
            return false;
        }
        static SURFACE_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let serial = SURFACE_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let shared = crate::shared::Mapping::new(
            &format!("surface-{}-{serial}", crate::shared::pid()),
            width as usize * height as usize * 4,
        );
        if shared.is_none() {
            unsafe {
                ReleaseDC(hwnd, window_dc);
            }
            return false;
        }
        let Some(mut buffer) = create_dib_surface(window_dc, width, height, 24, shared) else {
            unsafe { ReleaseDC(hwnd, window_dc) };
            return false;
        };
        buffer.resize_background = resize_background;
        if let Some(old) = buffers.remove(&window) {
            // A resized GPU readback is still a Composite source, not a GDI
            // presentation target. Losing this flag lets the periodic X flush
            // overwrite the WGL front buffer before the next GPU frame arrives.
            buffer.presentable = old.presentable;
            if !clear {
                // Preserve the CPU backing pixels directly. Reading the window
                // DC on every drag step synchronizes with the desktop GPU and
                // can also copy occlusion artifacts into the next frame.
                synchronize_buffer(&old);
                let copy_width = old.width.min(width) as usize;
                let copy_height = old.height.min(height) as usize;
                for y in 0..copy_height {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            (old.bits as *const u32).add(y * old.width as usize),
                            (buffer.bits as *mut u32).add(y * width as usize),
                            copy_width,
                        );
                    }
                }
            }
            unsafe {
                delete_back_buffer(old);
            }
        } else if !clear {
            unsafe {
                BitBlt(
                    buffer.dc as HDC,
                    0,
                    0,
                    width,
                    height,
                    window_dc,
                    0,
                    0,
                    SRCCOPY,
                )
            };
        }
        unsafe { ReleaseDC(hwnd, window_dc) };
        // A new surface must be committed even when it was initialized from the
        // old client DC; otherwise newly exposed resize pixels remain undefined.
        buffer.dirty = Some(RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        });
        buffers.insert(window, buffer);
        {
            let mut info = Vec::with_capacity(24);
            for value in [width as u32, height as u32, 24, crate::shared::pid()] {
                info.extend_from_slice(&value.to_le_bytes());
            }
            info.extend_from_slice(&serial.to_le_bytes());
            let _ = crate::shared::set(format!("surface/{window}"), info);
        }
    }
    let Some(buffer) = buffers.get_mut(&window) else {
        return false;
    };
    if clear {
        if buffer.graphics != 0 {
            unsafe { GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync) };
        }
        let rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(
                buffer.bits as *mut u32,
                width as usize * height as usize,
            )
        };
        background::paint(window, pixels, width, height, rect);
        buffer.dirty = Some(rect);
    }
    true
}

/// Commits complete core-X11 frames in one blit, avoiding observable clear/draw
/// intermediate states during resize.
pub(crate) fn flush_all() {
    let Ok(mut buffers) = back_buffers().lock() else {
        return;
    };
    let mut children = Vec::new();
    for (window, buffer) in buffers.iter_mut() {
        let Some(dirty) = buffer.dirty else {
            continue;
        };
        // Redirected CPU clients still own visible native windows. Their DIB
        // is shared with Mutter, but must also reach the native DWM surface.
        // GPU publication/imported surfaces are not GDI presentation targets.
        if !buffer.presentable {
            synchronize_buffer(buffer);
            buffer.dirty = None;
            continue;
        }
        let hwnd = *window as HWND;
        buffer.presentable = true;
        let destination = unsafe { GetDC(hwnd) };
        if !destination.is_null() {
            unsafe {
                if buffer.graphics != 0 {
                    GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync);
                }
                let presented = BitBlt(
                    destination,
                    dirty.left,
                    dirty.top,
                    dirty.right - dirty.left,
                    dirty.bottom - dirty.top,
                    buffer.dc as HDC,
                    dirty.left,
                    dirty.top,
                    SRCCOPY,
                );
                // The presenter sleeps after this batch. Do not leave its last
                // BitBlt buffered in GDI until another resize/draw wakes it.
                GdiFlush();
                if presented != 0 {
                    kinakaze_libdisplay::ui::presentation::frame_ready(*window);
                    if unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, -16)
                    } as u32
                        & 0x4000_0000
                        != 0
                    {
                        children.push((*window, dirty));
                    }
                }
                if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
                    let sample_x = dirty.left.clamp(0, buffer.width.saturating_sub(1));
                    let sample_y = dirty.top.clamp(0, buffer.height.saturating_sub(1));
                    let source_pixel = windows_sys::Win32::Graphics::Gdi::GetPixel(
                        buffer.dc as HDC,
                        sample_x,
                        sample_y,
                    );
                    let destination_pixel = windows_sys::Win32::Graphics::Gdi::GetPixel(
                        destination,
                        sample_x,
                        sample_y,
                    );
                    crate::diagnostic!(
                        "[libX11] present window={window:#x} dirty=({},{})-({},{}) ok={presented} pixel={source_pixel:#010x}->{destination_pixel:#010x}",
                        dirty.left,
                        dirty.top,
                        dirty.right,
                        dirty.bottom
                    );
                }
                ReleaseDC(hwnd, destination);
            }
            buffer.dirty = None;
        }
    }
    drop(buffers);
    for (window, dirty) in children {
        inferiors::publish(window, dirty);
    }
}

/// WM_PAINT must restore the cached frame before EndPaint commits the native
/// update region. Never block the UI thread behind a renderer waiting on it.
pub(crate) unsafe extern "system" fn paint(
    window: usize,
    dc: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> bool {
    let Ok(buffers) = back_buffers().try_lock() else {
        return false;
    };
    let Some(buffer) = buffers.get(&window).filter(|b| b.presentable) else {
        return false;
    };
    let width = width.min(buffer.width.saturating_sub(x));
    let height = height.min(buffer.height.saturating_sub(y));
    if dc == 0 || width <= 0 || height <= 0 {
        return false;
    }
    unsafe {
        let ok = BitBlt(
            dc as HDC,
            x,
            y,
            width,
            height,
            buffer.dc as HDC,
            x,
            y,
            SRCCOPY,
        ) != 0;
        GdiFlush();
        ok
    }
}

pub(crate) fn set_window_background(window: Window, pixel: c_ulong) {
    if let Ok(mut backgrounds) = window_backgrounds().lock() {
        backgrounds.insert(window, background::Background::Pixel(pixel as u32));
    }
}

pub(crate) fn forget_window(window: Window) {
    crate::composite::forget(window);
    buffered::forget(window);
    if let Ok(mut backgrounds) = window_backgrounds().lock() {
        backgrounds.remove(&window);
    }
    if let Ok(mut buffers) = back_buffers().lock()
        && let Some(buffer) = buffers.remove(&window)
    {
        unsafe { delete_back_buffer(buffer) };
    }
}

#[derive(Clone)]
pub(crate) struct DrawableSnapshot {
    pub width: i32,
    pub height: i32,
    pub depth: u32,
    pub pixels: Vec<u32>,
}

fn synchronize_buffer(buffer: &BackBuffer) {
    if buffer.graphics != 0 {
        unsafe { GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync) };
    }
    unsafe { GdiFlush() };
}

fn fill_pixels(pixels: &mut [u32], width: i32, height: i32, rect: RECT, pixel: u32) {
    let left = rect.left.clamp(0, width) as usize;
    let right = rect.right.clamp(0, width) as usize;
    if left >= right {
        return;
    }
    for row in rect.top.clamp(0, height)..rect.bottom.clamp(0, height) {
        let start = row as usize * width as usize;
        pixels[start + left..start + right].fill(pixel);
    }
}

#[cfg(test)]
#[test]
fn clear_clips_to_the_locked_surface_after_a_resize() {
    let mut pixels = [7; 12];
    fill_pixels(
        &mut pixels,
        4,
        3,
        RECT {
            left: 1,
            top: 1,
            right: 80,
            bottom: 60,
        },
        9,
    );
    assert_eq!(pixels, [7, 7, 7, 7, 7, 9, 9, 9, 7, 9, 9, 9]);
    fill_pixels(
        &mut pixels,
        4,
        3,
        RECT {
            left: 8,
            top: -20,
            right: 90,
            bottom: 90,
        },
        0,
    );
    assert_eq!(pixels, [7, 7, 7, 7, 7, 9, 9, 9, 7, 9, 9, 9]);
    fill_pixels(
        &mut pixels,
        4,
        3,
        RECT {
            left: -20,
            top: -20,
            right: 1,
            bottom: 1,
        },
        2,
    );
    assert_eq!(pixels[0], 2);
    assert_eq!(pixels[1], 7);
}

fn snapshot_buffer(buffer: &BackBuffer) -> DrawableSnapshot {
    synchronize_buffer(buffer);
    let length = (buffer.width as usize).saturating_mul(buffer.height as usize);
    let pixels = unsafe { core::slice::from_raw_parts(buffer.bits as *const u32, length) }.to_vec();
    DrawableSnapshot {
        width: buffer.width,
        height: buffer.height,
        depth: buffer.depth,
        pixels,
    }
}

pub(crate) fn drawable_snapshot(drawable: usize) -> Option<DrawableSnapshot> {
    if let Ok(result) = buffered::access(drawable, |buffer| snapshot_buffer(buffer)) {
        return result;
    }
    if let Ok(pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get(&drawable)
    {
        return Some(snapshot_buffer(buffer));
    }
    if !prepare_back_buffer(drawable, false) {
        return None;
    }
    back_buffers()
        .lock()
        .ok()
        .and_then(|buffers| buffers.get(&drawable).map(snapshot_buffer))
}

/// Borrow the synchronized surface while its owner holds the drawing lock.
/// Image readback can copy directly into guest storage without a full-frame Vec.
pub fn read_drawable<T>(
    drawable: usize,
    read: impl FnOnce(&[u32], i32, i32, u32) -> T,
) -> Option<T> {
    let apply = |buffer: &BackBuffer| {
        synchronize_buffer(buffer);
        let length = buffer.width as usize * buffer.height as usize;
        let pixels = unsafe { core::slice::from_raw_parts(buffer.bits as *const u32, length) };
        read(pixels, buffer.width, buffer.height, buffer.depth)
    };
    let apply = match buffered::access(drawable, |buffer| apply(buffer)) {
        Ok(result) => return result,
        Err(apply) => apply,
    };
    if let Ok(mut pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get_mut(&drawable)
    {
        return Some(apply(buffer));
    }
    if !prepare_back_buffer(drawable, false) {
        return None;
    }
    back_buffers()
        .lock()
        .ok()
        .and_then(|mut buffers| buffers.get_mut(&drawable).map(apply))
}

pub fn mutate_drawable<F>(drawable: usize, dirty: RECT, mutate: F) -> bool
where
    F: FnOnce(&mut [u32], i32, i32, u32),
{
    let mutate = match buffered::access(drawable, |buffer| {
        synchronize_buffer(buffer);
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(
                buffer.bits as *mut u32,
                buffer.width as usize * buffer.height as usize,
            )
        };
        mutate(pixels, buffer.width, buffer.height, buffer.depth);
    }) {
        Ok(result) => {
            let ok = result.is_some();
            if ok {
                notify_damage(drawable, dirty);
            }
            return ok;
        }
        Err(apply) => apply,
    };
    if let Ok(mut pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get_mut(&drawable)
    {
        mutate(buffer);
        drop(pixmaps);
        notify_damage(drawable, dirty);
        return true;
    }
    if !prepare_back_buffer(drawable, false) {
        return false;
    }
    let Ok(mut buffers) = back_buffers().lock() else {
        return false;
    };
    let Some(buffer) = buffers.get_mut(&drawable) else {
        return false;
    };
    mutate(buffer);
    let clipped = RECT {
        left: dirty.left.clamp(0, buffer.width),
        top: dirty.top.clamp(0, buffer.height),
        right: dirty.right.clamp(0, buffer.width),
        bottom: dirty.bottom.clamp(0, buffer.height),
    };
    if clipped.right > clipped.left && clipped.bottom > clipped.top {
        buffer.dirty = Some(match buffer.dirty {
            Some(current) => union_rect(current, clipped),
            None => clipped,
        });
    }
    drop(buffers);
    notify_damage(drawable, dirty);
    request_present();
    true
}

/// Publish the bottom-up GPU readback for Composite without presenting it a
/// second time through GDI. The native WGL swapchain owns its visible frame.
pub fn publish_gl_frame(drawable: usize, width: i32, height: i32, pixels: &[u32]) -> bool {
    if width <= 0
        || height <= 0
        || pixels.len() != width as usize * height as usize
        || !prepare_back_buffer(drawable, false)
    {
        return false;
    }
    let mut buffers = back_buffers().lock().unwrap_or_else(|e| e.into_inner());
    let Some(buffer) = buffers.get_mut(&drawable) else {
        return false;
    };
    if (buffer.width, buffer.height) != (width, height) {
        return false;
    }
    synchronize_buffer(buffer);
    let target = unsafe { core::slice::from_raw_parts_mut(buffer.bits as *mut u32, pixels.len()) };
    for (output, input) in target
        .chunks_exact_mut(width as usize)
        .zip(pixels.rchunks_exact(width as usize))
    {
        output.copy_from_slice(input);
    }
    buffer.presentable = false;
    buffer.dirty = None;
    drop(buffers);
    notify_damage(
        drawable,
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
    );
    inferiors::publish(
        drawable,
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
    );
    true
}

pub fn drawable_dimensions(drawable: usize) -> Option<(i32, i32, u32)> {
    if let Ok(result) = buffered::access(drawable, |buffer| {
        (buffer.width, buffer.height, buffer.depth)
    }) {
        return result;
    }
    let dimensions = |buffer: &BackBuffer| (buffer.width, buffer.height, buffer.depth);
    if let Ok(pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get(&drawable)
    {
        return Some(dimensions(buffer));
    }
    if !prepare_back_buffer(drawable, false) {
        return None;
    }
    back_buffers()
        .lock()
        .ok()
        .and_then(|buffers| buffers.get(&drawable).map(dimensions))
}

fn create_pixmap_buffer(width: u32, height: u32, depth: u32) -> Option<BackBuffer> {
    if width == 0
        || height == 0
        || width > i32::MAX as u32
        || height > i32::MAX as u32
        || !matches!(depth, 1 | 4 | 8 | 24 | 32)
    {
        return None;
    }
    let screen = unsafe { GetDC(core::ptr::null_mut()) };
    if screen.is_null() {
        return None;
    }
    let buffer = create_dib_buffer(screen, width as i32, height as i32, depth);
    unsafe { ReleaseDC(core::ptr::null_mut(), screen) };
    buffer
}

/// Create another XID for the current window backing store. Pixel storage is
/// shared; resize/destruction keeps the named old surface alive.
pub fn retain_pixmap(source: usize) -> Option<usize> {
    let mut pixmaps = pixmap_buffers().lock().unwrap_or_else(|e| e.into_inner());
    let front = pixmaps.get(&source)?;
    let alias = BackBuffer {
        owner: Arc::clone(&front.owner),
        dc: front.dc,
        bitmap: front.bitmap,
        previous_bitmap: front.previous_bitmap,
        bits: front.bits,
        graphics: 0,
        width: front.width,
        height: front.height,
        depth: front.depth,
        dirty: None,
        resize_background: false,
        presentable: false,
        imported: front.imported,
    };
    let id = allocate_drawable_id();
    pixmaps.insert(id, alias);
    Some(id)
}
pub(crate) fn name_window_pixmap(window: Window, pixmap: usize) -> Result<(), u8> {
    if pixmap == 0 {
        return Err(14);
    }
    if !prepare_back_buffer(window, false) {
        return Err(3);
    }
    let buffers = back_buffers().lock().unwrap_or_else(|e| e.into_inner());
    let front = buffers.get(&window).ok_or(3u8)?;
    synchronize_buffer(front);
    let mut pixmaps = pixmap_buffers().lock().unwrap_or_else(|e| e.into_inner());
    if pixmaps.contains_key(&pixmap) {
        return Err(14);
    }
    pixmaps.insert(
        pixmap,
        BackBuffer {
            owner: Arc::clone(&front.owner),
            dc: front.dc,
            bitmap: front.bitmap,
            previous_bitmap: front.previous_bitmap,
            bits: front.bits,
            graphics: 0,
            width: front.width,
            height: front.height,
            depth: front.depth,
            dirty: None,
            resize_background: false,
            presentable: false,
            imported: front.imported,
        },
    );
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XArc {
    pub x: c_short,
    pub y: c_short,
    pub width: c_ushort,
    pub height: c_ushort,
    pub angle1: c_short, // in 1/64ths of a degree
    pub angle2: c_short,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XSegment {
    pub x1: c_short,
    pub y1: c_short,
    pub x2: c_short,
    pub y2: c_short,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XPoint {
    pub x: c_short,
    pub y: c_short,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XColor {
    pub pixel: c_ulong,
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub flags: c_char,
    pub pad: c_char,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateGC")]
pub unsafe extern "sysv64" fn XCreateGC(
    _dpy: *mut Display,
    _d: usize,
    valuemask: c_ulong,
    values: *const XGCValues,
) -> GC {
    let mut gc = Box::new(GCRec {
        clip_rectangles: None,
        values: XGCValues {
            function: 3,
            plane_mask: u64::MAX,
            background: 1,
            graphics_exposures: 1,
            cap_style: 1,
            fill_rule: 0,
            arc_mode: 1,
            dashes: 4,
            ..Default::default()
        },
    });
    if !values.is_null() {
        unsafe {
            apply_gc(&mut gc.values, valuemask, &*values);
            if valuemask & (1 << 19) != 0 {
                XSetClipMask(_dpy, &mut *gc, (*values).clip_mask);
            }
        }
    }
    Box::into_raw(gc)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeGC")]
pub unsafe extern "sysv64" fn XFreeGC(_dpy: *mut Display, gc: GC) -> c_int {
    if !gc.is_null() {
        unsafe { drop(Box::from_raw(gc)) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCopyGC")]
pub unsafe extern "sysv64" fn XCopyGC(
    dpy: *mut Display,
    source: GC,
    mask: c_ulong,
    dest: GC,
) -> c_int {
    if source.is_null() || dest.is_null() {
        return unsafe { crate::errors::report(dpy, 13, 57, 0, 0) };
    }
    if source != dest {
        unsafe {
            apply_gc(&mut (*dest).values, mask, &(*source).values);
            if mask & (1 << 19) != 0 {
                (*dest).clip_rectangles = (*source).clip_rectangles.clone();
            }
        }
    }
    1
}

/// Native GCs update their values immediately. Flush outstanding native drawing
/// before a protocol extension observes the GC, matching Xlib's ordering point.
#[unsafe(export_name = "kinakaze_engine_libX11__XFlushGCCache")]
pub unsafe extern "sysv64" fn _XFlushGCCache(_display: *mut Display, _gc: GC) {
    flush_all();
}

#[unsafe(export_name = "kinakaze_engine_libX11_XChangeGC")]
pub unsafe extern "sysv64" fn XChangeGC(
    _dpy: *mut Display,
    gc: GC,
    valuemask: c_ulong,
    values: *const XGCValues,
) -> c_int {
    if !gc.is_null() && !values.is_null() {
        unsafe {
            apply_gc(&mut (*gc).values, valuemask, &*values);
            if valuemask & (1 << 19) != 0 {
                XSetClipMask(_dpy, gc, (*values).clip_mask);
            }
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetGCValues")]
pub unsafe extern "sysv64" fn XGetGCValues(
    _dpy: *mut Display,
    gc: GC,
    _valuemask: c_ulong,
    values_return: *mut XGCValues,
) -> Status {
    if !gc.is_null() && !values_return.is_null() {
        unsafe { *values_return = (*gc).values };
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetForeground")]
pub unsafe extern "sysv64" fn XSetForeground(
    _dpy: *mut Display,
    gc: GC,
    foreground: c_ulong,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.foreground = foreground };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetBackground")]
pub unsafe extern "sysv64" fn XSetBackground(
    _dpy: *mut Display,
    gc: GC,
    background: c_ulong,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.background = background };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetFunction")]
pub unsafe extern "sysv64" fn XSetFunction(_dpy: *mut Display, gc: GC, function: c_int) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.function = function };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetPlaneMask")]
pub unsafe extern "sysv64" fn XSetPlaneMask(
    _dpy: *mut Display,
    gc: GC,
    plane_mask: c_ulong,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.plane_mask = plane_mask };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetLineAttributes")]
pub unsafe extern "sysv64" fn XSetLineAttributes(
    _dpy: *mut Display,
    gc: GC,
    line_width: c_uint,
    line_style: c_int,
    cap_style: c_int,
    join_style: c_int,
) -> c_int {
    if !gc.is_null() {
        unsafe {
            (*gc).values.line_width = line_width as c_int;
            (*gc).values.line_style = line_style;
            (*gc).values.cap_style = cap_style;
            (*gc).values.join_style = join_style;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetDashes")]
pub unsafe extern "sysv64" fn XSetDashes(
    _dpy: *mut Display,
    _gc: GC,
    _dash_offset: c_int,
    _dash_list: *const c_char,
    _n: c_int,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetClipMask")]
pub unsafe extern "sysv64" fn XSetClipMask(_dpy: *mut Display, gc: GC, pixmap: usize) -> c_int {
    if !gc.is_null() {
        let rectangles = if pixmap == 0 {
            None
        } else {
            let Some(rects) = clipping::bitmap_rectangles(pixmap) else {
                return unsafe { crate::errors::report(_dpy, 8, 56, 0, pixmap) };
            };
            Some(rects)
        };
        unsafe {
            (*gc).values.clip_mask = pixmap;
            (*gc).clip_rectangles = rectangles;
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetClipOrigin")]
pub unsafe extern "sysv64" fn XSetClipOrigin(
    _dpy: *mut Display,
    gc: GC,
    clip_x_origin: c_int,
    clip_y_origin: c_int,
) -> c_int {
    if !gc.is_null() {
        unsafe {
            (*gc).values.clip_x_origin = clip_x_origin;
            (*gc).values.clip_y_origin = clip_y_origin;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetClipRectangles")]
pub unsafe extern "sysv64" fn XSetClipRectangles(
    dpy: *mut Display,
    gc: GC,
    clip_x_origin: c_int,
    clip_y_origin: c_int,
    rectangles: *mut XRectangle,
    n: c_int,
    ordering: c_int,
) -> c_int {
    if gc.is_null() {
        return unsafe { crate::errors::report(dpy, 13, 59, 0, 0) };
    }
    if n < 0 || !(0..=3).contains(&ordering) || (n != 0 && rectangles.is_null()) {
        return unsafe { crate::errors::report(dpy, 2, 59, 0, ordering as usize) };
    }
    let rects = if n == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(rectangles, n as usize) }
    };
    unsafe {
        (*gc).clip_rectangles = Some(
            rects
                .iter()
                .map(|r| {
                    [
                        r.x as i32,
                        r.y as i32,
                        r.x as i32 + r.width as i32,
                        r.y as i32 + r.height as i32,
                    ]
                })
                .collect(),
        );
        (*gc).values.clip_mask = 0;
        (*gc).values.clip_x_origin = clip_x_origin;
        (*gc).values.clip_y_origin = clip_y_origin;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetArcMode")]
pub unsafe extern "sysv64" fn XSetArcMode(_dpy: *mut Display, gc: GC, arc_mode: c_int) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.arc_mode = arc_mode };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetFillStyle")]
pub unsafe extern "sysv64" fn XSetFillStyle(
    _dpy: *mut Display,
    gc: GC,
    fill_style: c_int,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.fill_style = fill_style };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetFillRule")]
pub unsafe extern "sysv64" fn XSetFillRule(_dpy: *mut Display, gc: GC, fill_rule: c_int) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.fill_rule = fill_rule };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetTile")]
pub unsafe extern "sysv64" fn XSetTile(_dpy: *mut Display, gc: GC, tile: usize) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.tile = tile };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetStipple")]
pub unsafe extern "sysv64" fn XSetStipple(_dpy: *mut Display, gc: GC, stipple: usize) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.stipple = stipple };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetTSOrigin")]
pub unsafe extern "sysv64" fn XSetTSOrigin(
    _dpy: *mut Display,
    gc: GC,
    ts_x_origin: c_int,
    ts_y_origin: c_int,
) -> c_int {
    if !gc.is_null() {
        unsafe {
            (*gc).values.ts_x_origin = ts_x_origin;
            (*gc).values.ts_y_origin = ts_y_origin;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetFont")]
pub unsafe extern "sysv64" fn XSetFont(_dpy: *mut Display, gc: GC, font: usize) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.font = font };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetSubwindowMode")]
pub unsafe extern "sysv64" fn XSetSubwindowMode(
    _dpy: *mut Display,
    gc: GC,
    subwindow_mode: c_int,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.subwindow_mode = subwindow_mode };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetGraphicsExposures")]
pub unsafe extern "sysv64" fn XSetGraphicsExposures(
    _dpy: *mut Display,
    gc: GC,
    graphics_exposures: Bool,
) -> c_int {
    if !gc.is_null() {
        unsafe { (*gc).values.graphics_exposures = graphics_exposures };
    }
    1
}

// ---------------------------------------------------------------------------
// 2D Drawing primitives
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawString")]
pub unsafe extern "sysv64" fn XDrawString(
    _display: *mut Display,
    drawable: usize,
    gc: GC,
    x: c_int,
    y: c_int,
    text: *const c_char,
    length: c_int,
) -> c_int {
    if length < 0 || (text.is_null() && length != 0) || gc.is_null() {
        return 0;
    }
    if length == 0 {
        return 1;
    }
    // XDrawString addresses 8-bit glyphs, not a UTF-8 string.
    let glyphs: Vec<u16> =
        unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length as usize) }
            .iter()
            .map(|&byte| u16::from(byte))
            .collect();
    unsafe { draw_utf16(drawable, gc, x, y, &glyphs) }
}

pub(crate) unsafe fn draw_utf16(drawable: usize, gc: GC, x: i32, y: i32, glyphs: &[u16]) -> i32 {
    use windows_sys::Win32::Graphics::Gdi::{
        RestoreDC, SaveDC, SetBkMode, SetTextAlign, SetTextColor, TA_BASELINE, TRANSPARENT,
        TextOutW,
    };
    if gc.is_null() || glyphs.len() > i32::MAX as usize {
        return 0;
    }
    if glyphs.is_empty() {
        return 1;
    }
    let Some((window, dc, clip_state)) = (unsafe { clipping::get(drawable, gc) }) else {
        return 0;
    };
    let saved = unsafe { SaveDC(dc) };
    if saved == 0 {
        unsafe { clipping::release(window, dc, clip_state) };
        return 0;
    }
    let result = unsafe {
        SetTextAlign(dc, TA_BASELINE);
        SetTextColor(dc, color_from_pixel((*gc).values.foreground));
        SetBkMode(dc, TRANSPARENT as i32);
        let result = TextOutW(dc, x, y, glyphs.as_ptr(), glyphs.len() as i32);
        RestoreDC(dc, saved);
        clipping::release(window, dc, clip_state);
        result
    };
    if result != 0
        && let Some((width, height, _)) = drawable_dimensions(drawable)
    {
        mark_dirty(
            drawable,
            RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
        );
    }
    i32::from(result != 0)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawString16")]
pub unsafe extern "sysv64" fn XDrawString16(
    _d: *mut Display,
    drawable: usize,
    gc: GC,
    x: i32,
    y: i32,
    text: *const u8,
    count: i32,
) -> i32 {
    if count < 0 || count > 0 && text.is_null() {
        return 0;
    }
    let glyphs: Vec<u16> = (0..count as usize)
        .map(|i| unsafe { u16::from_be_bytes([*text.add(i * 2), *text.add(i * 2 + 1)]) })
        .collect();
    unsafe { draw_utf16(drawable, gc, x, y, &glyphs) }
}

unsafe fn get_win_hdc(d: usize) -> Option<(HWND, HDC)> {
    if let Ok(result) = buffered::access(d, |buffer| {
        synchronize_buffer(buffer);
        (core::ptr::null_mut(), buffer.dc as HDC)
    }) {
        return result;
    }
    if let Ok(pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get(&d)
    {
        if buffer.graphics != 0 {
            unsafe { GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync) };
        }
        return Some((core::ptr::null_mut(), buffer.dc as HDC));
    }
    // Every core drawing operation must land in retained storage, including
    // the first one on an unmapped/occluded window. Reading the screen after
    // that first draw loses its pixels and breaks both exposure and Composite.
    if !prepare_back_buffer(d, false) {
        return None;
    }
    if let Ok(buffers) = back_buffers().lock()
        && let Some(buffer) = buffers.get(&d)
    {
        if buffer.graphics != 0 {
            unsafe { GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync) };
        }
        // A null HWND marks a borrowed memory DC; it must not be ReleaseDC'd.
        return Some((core::ptr::null_mut(), buffer.dc as HDC));
    }
    None
}

unsafe fn release_win_hdc(hwnd: HWND, hdc: HDC) {
    if !hwnd.is_null() {
        unsafe { ReleaseDC(hwnd, hdc) };
    }
}

unsafe fn color_from_pixel(pixel: c_ulong) -> u32 {
    let r = ((pixel >> 16) & 0xff) as u32;
    let g = ((pixel >> 8) & 0xff) as u32;
    let b = (pixel & 0xff) as u32;
    r | (g << 8) | (b << 16)
}

fn extent_rect(x: i32, y: i32, width: u32, height: u32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x.saturating_add(width.min(i32::MAX as u32) as i32),
        bottom: y.saturating_add(height.min(i32::MAX as u32) as i32),
    }
}

fn point_bounds<I>(points: I, padding: i32) -> Option<RECT>
where
    I: IntoIterator<Item = (i32, i32)>,
{
    let mut points = points.into_iter();
    let (first_x, first_y) = points.next()?;
    let (mut left, mut top, mut right, mut bottom) = (first_x, first_y, first_x, first_y);
    for (x, y) in points {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    Some(RECT {
        left: left.saturating_sub(padding),
        top: top.saturating_sub(padding),
        right: right.saturating_add(padding + 1),
        bottom: bottom.saturating_add(padding + 1),
    })
}

struct GdiPlusRuntime {
    _token: usize,
}

fn gdiplus_runtime() -> Option<&'static GdiPlusRuntime> {
    static RUNTIME: OnceLock<Option<GdiPlusRuntime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: 0,
                SuppressExternalCodecs: 1,
            };
            let mut token = 0usize;
            (unsafe { GdiplusStartup(&raw mut token, &raw const input, core::ptr::null_mut()) }
                == 0)
                .then_some(GdiPlusRuntime { _token: token })
        })
        .as_ref()
}

pub(crate) fn render_available() -> bool {
    gdiplus_runtime().is_some()
}

fn buffer_graphics(buffer: &mut BackBuffer) -> Option<*mut GpGraphics> {
    if buffer.graphics == 0 {
        let mut graphics = core::ptr::null_mut::<GpGraphics>();
        if unsafe { GdipCreateFromHDC(buffer.dc as HDC, &raw mut graphics) } != 0 {
            return None;
        }
        if unsafe { GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias8x8) } != 0 {
            unsafe { GdipDeleteGraphics(graphics) };
            return None;
        }
        buffer.graphics = graphics as usize;
    }
    Some(buffer.graphics as *mut GpGraphics)
}

/// Implements the solid-source polygon operation used by libXrender's
/// `XRenderCompositeDoublePoly`. GDI+ performs the antialias coverage and
/// Source/Over compositing directly into the same frame as core X11 drawing.
pub(crate) fn composite_solid_polygon(
    drawable: Window,
    argb: u32,
    points: &[PointF],
    op: c_int,
    winding: c_int,
) -> bool {
    if points.len() < 3 || gdiplus_runtime().is_none() {
        return false;
    }
    // PictOpDst leaves the destination unchanged. The GDI+ backend directly
    // represents the two useful solid-source operations; unsupported operators
    // are rejected instead of silently producing different Porter-Duff results.
    if op == 2 {
        return true;
    }
    let compositing_mode = match op {
        0 | 1 => CompositingModeSourceCopy, // Clear/Src
        3 => CompositingModeSourceOver,
        _ => return false,
    };
    let draw = |buffer: &mut BackBuffer| {
        let Some(graphics) = buffer_graphics(buffer) else {
            return false;
        };
        let color = if op == 0 { 0 } else { argb };
        let mut brush = core::ptr::null_mut::<GpSolidFill>();
        let ok = unsafe {
            GdipSetCompositingMode(graphics, compositing_mode) == 0
                && GdipCreateSolidFill(color, &raw mut brush) == 0
                && GdipFillPolygon(
                    graphics,
                    brush.cast::<GpBrush>(),
                    points.as_ptr(),
                    points.len() as c_int,
                    if winding != 0 {
                        FillModeWinding
                    } else {
                        FillModeAlternate
                    },
                ) == 0
        };
        if !brush.is_null() {
            unsafe { GdipDeleteBrush(brush.cast::<GpBrush>()) };
        }
        ok
    };
    let draw = match buffered::access(drawable, draw) {
        Ok(result) => return result.unwrap_or(false),
        Err(draw) => draw,
    };
    if let Ok(mut pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.get_mut(&drawable)
    {
        return draw(buffer);
    }
    if !prepare_back_buffer(drawable, false) {
        return false;
    }
    let ok = back_buffers()
        .lock()
        .ok()
        .and_then(|mut buffers| buffers.get_mut(&drawable).map(draw))
        .unwrap_or(false);
    if ok
        && let Some(rect) = point_bounds(
            points
                .iter()
                .map(|point| (point.X.floor() as i32, point.Y.floor() as i32)),
            2,
        )
    {
        mark_dirty(drawable, rect);
    }
    ok
}

#[cfg(test)]
pub(crate) fn back_buffer_pixel(window: Window, x: i32, y: i32) -> Option<u32> {
    use windows_sys::Win32::Graphics::Gdi::GetPixel;

    let buffers = back_buffers().lock().ok()?;
    let buffer = buffers.get(&window)?;
    if buffer.graphics != 0 {
        unsafe { GdipFlush(buffer.graphics as *mut GpGraphics, FlushIntentionSync) };
    }
    let pixel = unsafe { GetPixel(buffer.dc as HDC, x, y) };
    (pixel != u32::MAX).then_some(pixel)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawLine")]
pub unsafe extern "sysv64" fn XDrawLine(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    x1: c_int,
    y1: c_int,
    x2: c_int,
    y2: c_int,
) -> c_int {
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19)); // DC_PEN
            MoveToEx(hdc, x1, y1, core::ptr::null_mut());
            LineTo(hdc, x2, y2);
            clipping::release(hwnd, hdc, clip_state);
        }
        if let Some(rect) = point_bounds([(x1, y1), (x2, y2)], 1) {
            mark_dirty(d, rect);
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawLines")]
pub unsafe extern "sysv64" fn XDrawLines(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    points: *const XPoint,
    npoints: c_int,
    _mode: c_int,
) -> c_int {
    if points.is_null() || npoints < 2 {
        return 1;
    }
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            let mut pts = Vec::with_capacity(npoints as usize);
            for i in 0..npoints as usize {
                let p = *points.add(i);
                pts.push(windows_sys::Win32::Foundation::POINT {
                    x: p.x as i32,
                    y: p.y as i32,
                });
            }
            Polyline(hdc, pts.as_ptr(), npoints);
            clipping::release(hwnd, hdc, clip_state);
            if let Some(rect) = point_bounds(pts.iter().map(|point| (point.x, point.y)), 1) {
                mark_dirty(d, rect);
            }
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawSegments")]
pub unsafe extern "sysv64" fn XDrawSegments(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    segments: *const XSegment,
    nsegments: c_int,
) -> c_int {
    if segments.is_null() || nsegments <= 0 {
        return 1;
    }
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            let mut dirty = None;
            for i in 0..nsegments as usize {
                let s = *segments.add(i);
                MoveToEx(hdc, s.x1 as i32, s.y1 as i32, core::ptr::null_mut());
                LineTo(hdc, s.x2 as i32, s.y2 as i32);
                if let Some(bounds) =
                    point_bounds([(s.x1 as i32, s.y1 as i32), (s.x2 as i32, s.y2 as i32)], 1)
                {
                    dirty = Some(match dirty {
                        Some(current) => union_rect(current, bounds),
                        None => bounds,
                    });
                }
            }
            clipping::release(hwnd, hdc, clip_state);
            if let Some(dirty) = dirty {
                mark_dirty(d, dirty);
            }
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawRectangle")]
pub unsafe extern "sysv64" fn XDrawRectangle(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            SelectObject(hdc, GetStockObject(5)); // NULL_BRUSH
            Rectangle(hdc, x, y, x + width as i32, y + height as i32);
            clipping::release(hwnd, hdc, clip_state);
        }
        let mut dirty = extent_rect(x, y, width, height);
        dirty.right = dirty.right.saturating_add(1);
        dirty.bottom = dirty.bottom.saturating_add(1);
        mark_dirty(d, dirty);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawRectangles")]
pub unsafe extern "sysv64" fn XDrawRectangles(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    rectangles: *const XRectangle,
    nrectangles: c_int,
) -> c_int {
    if rectangles.is_null() || nrectangles <= 0 {
        return 1;
    }
    for i in 0..nrectangles as usize {
        let r = unsafe { *rectangles.add(i) };
        unsafe {
            XDrawRectangle(
                _dpy,
                d,
                gc,
                r.x as c_int,
                r.y as c_int,
                r.width as c_uint,
                r.height as c_uint,
            )
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawArc")]
pub unsafe extern "sysv64" fn XDrawArc(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    _angle1: c_int,
    _angle2: c_int,
) -> c_int {
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!("[libX11] XDrawArc drawable={d:#x} at {x},{y} {width}x{height}");
    }
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            SelectObject(hdc, GetStockObject(5)); // NULL_BRUSH
            Ellipse(hdc, x, y, x + width as i32, y + height as i32);
            clipping::release(hwnd, hdc, clip_state);
        }
        mark_dirty(
            d,
            extent_rect(x, y, width.saturating_add(1), height.saturating_add(1)),
        );
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDrawArcs")]
pub unsafe extern "sysv64" fn XDrawArcs(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    arcs: *const XArc,
    narcs: c_int,
) -> c_int {
    if arcs.is_null() || narcs <= 0 {
        return 1;
    }
    for i in 0..narcs as usize {
        let a = unsafe { *arcs.add(i) };
        unsafe {
            XDrawArc(
                _dpy,
                d,
                gc,
                a.x as c_int,
                a.y as c_int,
                a.width as c_uint,
                a.height as c_uint,
                a.angle1 as c_int,
                a.angle2 as c_int,
            )
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFillRectangle")]
pub unsafe extern "sysv64" fn XFillRectangle(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            let brush = CreateSolidBrush(color_from_pixel(fg));
            let r = RECT {
                left: x,
                top: y,
                right: x + width as i32,
                bottom: y + height as i32,
            };
            FillRect(hdc, &r, brush);
            DeleteObject(brush);
            clipping::release(hwnd, hdc, clip_state);
        }
        mark_dirty(d, extent_rect(x, y, width, height));
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFillRectangles")]
pub unsafe extern "sysv64" fn XFillRectangles(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    rectangles: *const XRectangle,
    nrectangles: c_int,
) -> c_int {
    if rectangles.is_null() || nrectangles <= 0 {
        return 1;
    }
    for i in 0..nrectangles as usize {
        let r = unsafe { *rectangles.add(i) };
        unsafe {
            XFillRectangle(
                _dpy,
                d,
                gc,
                r.x as c_int,
                r.y as c_int,
                r.width as c_uint,
                r.height as c_uint,
            )
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFillArc")]
pub unsafe extern "sysv64" fn XFillArc(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    _angle1: c_int,
    _angle2: c_int,
) -> c_int {
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!("[libX11] XFillArc drawable={d:#x} at {x},{y} {width}x{height}");
    }
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            let brush = CreateSolidBrush(color_from_pixel(fg));
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            let old_brush = SelectObject(hdc, brush);
            Ellipse(hdc, x, y, x + width as i32, y + height as i32);
            SelectObject(hdc, old_brush);
            DeleteObject(brush);
            clipping::release(hwnd, hdc, clip_state);
        }
        mark_dirty(
            d,
            extent_rect(x, y, width.saturating_add(1), height.saturating_add(1)),
        );
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFillArcs")]
pub unsafe extern "sysv64" fn XFillArcs(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    arcs: *const XArc,
    narcs: c_int,
) -> c_int {
    if arcs.is_null() || narcs <= 0 {
        return 1;
    }
    for i in 0..narcs as usize {
        let a = unsafe { *arcs.add(i) };
        unsafe {
            XFillArc(
                _dpy,
                d,
                gc,
                a.x as c_int,
                a.y as c_int,
                a.width as c_uint,
                a.height as c_uint,
                a.angle1 as c_int,
                a.angle2 as c_int,
            )
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFillPolygon")]
pub unsafe extern "sysv64" fn XFillPolygon(
    _dpy: *mut Display,
    d: usize,
    gc: GC,
    points: *const XPoint,
    npoints: c_int,
    _shape: c_int,
    _mode: c_int,
) -> c_int {
    if points.is_null() || npoints < 3 {
        return 1;
    }
    if let Some((hwnd, hdc, clip_state)) = unsafe { clipping::get(d, gc) } {
        let fg = if !gc.is_null() {
            unsafe { (*gc).values.foreground }
        } else {
            0
        };
        unsafe {
            let brush = CreateSolidBrush(color_from_pixel(fg));
            SetDCPenColor(hdc, color_from_pixel(fg));
            SelectObject(hdc, GetStockObject(19));
            let old_brush = SelectObject(hdc, brush);
            let mut pts = Vec::with_capacity(npoints as usize);
            for i in 0..npoints as usize {
                let p = *points.add(i);
                pts.push(windows_sys::Win32::Foundation::POINT {
                    x: p.x as i32,
                    y: p.y as i32,
                });
            }
            windows_sys::Win32::Graphics::Gdi::Polygon(hdc, pts.as_ptr(), npoints);
            SelectObject(hdc, old_brush);
            DeleteObject(brush);
            clipping::release(hwnd, hdc, clip_state);
            if let Some(rect) = point_bounds(pts.iter().map(|point| (point.x, point.y)), 1) {
                mark_dirty(d, rect);
            }
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XClearWindow")]
pub unsafe extern "sysv64" fn XClearWindow(dpy: *mut Display, w: Window) -> c_int {
    unsafe { XClearArea(dpy, w, 0, 0, 0, 0, 0) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XClearArea")]
pub unsafe extern "sysv64" fn XClearArea(
    dpy: *mut Display,
    w: Window,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    exposures: Bool,
) -> c_int {
    let mut bounds: RECT = unsafe { core::mem::zeroed() };
    if unsafe { GetClientRect(w as HWND, &mut bounds) } == 0 {
        unsafe {
            crate::errors::report(dpy, 3, 61, 0, w);
        }
        return 0;
    }
    if !prepare_back_buffer(w, false) {
        return 0;
    }
    let r = RECT {
        left: x.max(0),
        top: y.max(0),
        right: if width == 0 {
            bounds.right
        } else {
            x.saturating_add(width.min(i32::MAX as u32) as i32)
                .min(bounds.right)
        },
        bottom: if height == 0 {
            bounds.bottom
        } else {
            y.saturating_add(height.min(i32::MAX as u32) as i32)
                .min(bounds.bottom)
        },
    };
    if r.right <= r.left || r.bottom <= r.top {
        return 1;
    }
    if !mutate_drawable(w, r, |pixels, width, height, _| {
        // WM_SIZE can arrive between the initial bounds query and surface access.
        background::paint(w, pixels, width, height, r);
    }) {
        return 0;
    }
    buffered::clear_area(w, r);
    if exposures != 0 {
        let mut event: crate::XEvent = unsafe { core::mem::zeroed() };
        event.xexpose = crate::XExposeEvent {
            r#type: crate::Expose,
            serial: 0,
            send_event: 0,
            display: dpy,
            window: w,
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            count: 0,
        };
        crate::queue_extension_event(dpy, event);
    }
    1
}

// ---------------------------------------------------------------------------
// Pixmaps and Bitmaps
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XCreatePixmap")]
pub unsafe extern "sysv64" fn XCreatePixmap(
    _dpy: *mut Display,
    _d: usize,
    width: c_uint,
    height: c_uint,
    depth: c_uint,
) -> usize {
    let Some(buffer) = create_pixmap_buffer(width, height, depth) else {
        return 0;
    };
    let pixmap = allocate_drawable_id();
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] pid={} XCreatePixmap {pixmap:#x} {width}x{height} depth={depth}",
            std::process::id()
        );
    }
    if let Ok(mut pixmaps) = pixmap_buffers().lock() {
        pixmaps.insert(pixmap, buffer);
        pixmap
    } else {
        unsafe { delete_back_buffer(buffer) };
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreePixmap")]
pub unsafe extern "sysv64" fn XFreePixmap(_dpy: *mut Display, pixmap: usize) -> c_int {
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] pid={} XFreePixmap {pixmap:#x}",
            std::process::id()
        );
    }
    if let Ok(mut pixmaps) = pixmap_buffers().lock()
        && let Some(buffer) = pixmaps.remove(&pixmap)
    {
        unsafe { delete_back_buffer(buffer) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateBitmapFromData")]
pub unsafe extern "sysv64" fn XCreateBitmapFromData(
    _dpy: *mut Display,
    _d: usize,
    data: *const c_char,
    width: c_uint,
    height: c_uint,
) -> usize {
    if data.is_null() {
        return 0;
    }
    let pixmap = unsafe { XCreatePixmap(_dpy, _d, width, height, 1) };
    if pixmap == 0 {
        return 0;
    }
    let stride = width.div_ceil(8) as usize;
    let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), stride * height as usize) };
    let _ = mutate_drawable(
        pixmap,
        RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        },
        |pixels, surface_width, _, _| {
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let set = bytes[y * stride + x / 8] & (1 << (x & 7)) != 0;
                    pixels[y * surface_width as usize + x] = if set { u32::MAX } else { 0 };
                }
            }
        },
    );
    pixmap
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreatePixmapFromBitmapData")]
pub unsafe extern "sysv64" fn XCreatePixmapFromBitmapData(
    _dpy: *mut Display,
    _d: usize,
    data: *mut c_char,
    width: c_uint,
    height: c_uint,
    fg: c_ulong,
    bg: c_ulong,
    depth: c_uint,
) -> usize {
    if data.is_null() {
        return 0;
    }
    let pixmap = unsafe { XCreatePixmap(_dpy, _d, width, height, depth) };
    if pixmap == 0 {
        return 0;
    }
    let stride = width.div_ceil(8) as usize;
    let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), stride * height as usize) };
    let to_raw = |pixel: c_ulong| {
        let red = (pixel & 0xff) as u32;
        let green = ((pixel >> 8) & 0xff) as u32;
        let blue = ((pixel >> 16) & 0xff) as u32;
        0xff00_0000 | (red << 16) | (green << 8) | blue
    };
    let (fg, bg) = (to_raw(fg), to_raw(bg));
    let _ = mutate_drawable(
        pixmap,
        RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        },
        |pixels, surface_width, _, _| {
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let set = bytes[y * stride + x / 8] & (1 << (x & 7)) != 0;
                    pixels[y * surface_width as usize + x] = if set { fg } else { bg };
                }
            }
        },
    );
    pixmap
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCopyArea")]
pub unsafe extern "sysv64" fn XCopyArea(
    _dpy: *mut Display,
    src: usize,
    dest: usize,
    _gc: GC,
    src_x: c_int,
    src_y: c_int,
    width: c_uint,
    height: c_uint,
    dest_x: c_int,
    dest_y: c_int,
) -> c_int {
    if pixmap_buffers()
        .lock()
        .ok()
        .is_none_or(|p| !p.contains_key(&src))
    {
        let _ = prepare_back_buffer(src, false);
    }
    if pixmap_buffers()
        .lock()
        .ok()
        .is_none_or(|p| !p.contains_key(&dest))
    {
        let _ = prepare_back_buffer(dest, false);
    }
    if let (Some((src_hwnd, src_dc)), Some((dest_hwnd, dest_dc, clip_state))) =
        (unsafe { get_win_hdc(src) }, unsafe {
            clipping::get(dest, _gc)
        })
    {
        unsafe {
            BitBlt(
                dest_dc,
                dest_x,
                dest_y,
                width as i32,
                height as i32,
                src_dc,
                src_x,
                src_y,
                SRCCOPY,
            );
            release_win_hdc(src_hwnd, src_dc);
            clipping::release(dest_hwnd, dest_dc, clip_state);
        }
        mark_dirty(dest, extent_rect(dest_x, dest_y, width, height));
    }
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!("[libX11] XCopyArea src={src:#x} dest={dest:#x}");
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCopyPlane")]
pub unsafe extern "sysv64" fn XCopyPlane(
    _dpy: *mut Display,
    src: usize,
    dest: usize,
    gc: GC,
    src_x: c_int,
    src_y: c_int,
    width: c_uint,
    height: c_uint,
    dest_x: c_int,
    dest_y: c_int,
    _plane: c_ulong,
) -> c_int {
    let Some(source) = drawable_snapshot(src) else {
        return 0;
    };
    let (foreground, background) = if gc.is_null() {
        (0xffff_ffff, 0xff00_0000)
    } else {
        let values = unsafe { &(*gc).values };
        let convert = |pixel: c_ulong| {
            0xff00_0000
                | (((pixel & 0xff) as u32) << 16)
                | ((((pixel >> 8) & 0xff) as u32) << 8)
                | (((pixel >> 16) & 0xff) as u32)
        };
        (convert(values.foreground), convert(values.background))
    };
    let dirty = extent_rect(dest_x, dest_y, width, height);
    let _ = mutate_drawable(dest, dirty, |pixels, dest_width, dest_height, _| {
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let (sx, sy, dx, dy) = (src_x + x, src_y + y, dest_x + x, dest_y + y);
                if sx < 0
                    || sy < 0
                    || sx >= source.width
                    || sy >= source.height
                    || dx < 0
                    || dy < 0
                    || dx >= dest_width
                    || dy >= dest_height
                    || !unsafe { clipping::contains(gc, dx, dy) }
                {
                    continue;
                }
                let source_pixel = source.pixels[sy as usize * source.width as usize + sx as usize];
                pixels[dy as usize * dest_width as usize + dx as usize] = if source_pixel != 0 {
                    foreground
                } else {
                    background
                };
            }
        }
    });
    1
}

// ---------------------------------------------------------------------------
// Pointer and Window Queries
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryPointer")]
pub unsafe extern "sysv64" fn XQueryPointer(
    dpy: *mut Display,
    w: Window,
    root_return: *mut Window,
    child_return: *mut Window,
    root_x_return: *mut c_int,
    root_y_return: *mut c_int,
    win_x_return: *mut c_int,
    win_y_return: *mut c_int,
    mask_return: *mut c_uint,
) -> Bool {
    unsafe { crate::issue_request(dpy) };
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } == 0 {
        return 0;
    }

    if !root_return.is_null() {
        unsafe { *root_return = 1 };
    }
    if !child_return.is_null() {
        unsafe { *child_return = 0 };
    }
    if !root_x_return.is_null() {
        unsafe { *root_x_return = pt.x };
    }
    if !root_y_return.is_null() {
        unsafe { *root_y_return = pt.y };
    }

    let hwnd = w as HWND;
    if !hwnd.is_null() && w != 1 {
        let mut client_pt = pt;
        if unsafe { ScreenToClient(hwnd, &mut client_pt) } == 0 {
            return 0;
        }
        if !win_x_return.is_null() {
            unsafe { *win_x_return = client_pt.x };
        }
        if !win_y_return.is_null() {
            unsafe { *win_y_return = client_pt.y };
        }
        if !child_return.is_null() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                CWP_SKIPINVISIBLE, ChildWindowFromPointEx,
            };
            let child = unsafe { ChildWindowFromPointEx(hwnd, client_pt, CWP_SKIPINVISIBLE) };
            if child != hwnd && !child.is_null() {
                unsafe { *child_return = child as Window };
            }
        }
    } else {
        if !win_x_return.is_null() {
            unsafe { *win_x_return = pt.x };
        }
        if !win_y_return.is_null() {
            unsafe { *win_y_return = pt.y };
        }
        if !child_return.is_null() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GWL_STYLE, GetClassNameW, GetParent, GetWindowLongW, WS_CHILD, WindowFromPoint,
            };
            // GDK re-queries the root after releasing a grab. Returning None
            // here clears its pointer window and drops subsequent hover and
            // scroll events until another button press establishes a grab.
            let mut child = unsafe { WindowFromPoint(pt) };
            while !child.is_null()
                && unsafe { GetWindowLongW(child, GWL_STYLE) } as u32 & WS_CHILD != 0
            {
                child = unsafe { GetParent(child) };
            }
            let mut name = [0u16; 64];
            let len = unsafe { GetClassNameW(child, name.as_mut_ptr(), name.len() as i32) };
            if len > 0
                && name[..len as usize]
                    .iter()
                    .copied()
                    .eq("kinakaze.display.window".encode_utf16())
            {
                unsafe { *child_return = child as Window };
            }
        }
    }

    if !mask_return.is_null() {
        unsafe { *mask_return = crate::keyboard::pointer_mask() };
    }
    1
}

// ---------------------------------------------------------------------------
// Server Grabs & Sync
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XAllowEvents")]
pub unsafe extern "sysv64" fn XAllowEvents(dpy: *mut Display, mode: c_int, _time: usize) -> c_int {
    if !(0..=7).contains(&mode) {
        return unsafe { crate::errors::report(dpy, 2, 35, 0, mode as usize) };
    }
    // Native input is dispatched asynchronously; there is no frozen device
    // queue. XAllowEvents has no effect when no device is frozen by this client.
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGrabServer")]
pub unsafe extern "sysv64" fn XGrabServer(_dpy: *mut Display) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUngrabServer")]
pub unsafe extern "sysv64" fn XUngrabServer(_dpy: *mut Display) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGrabPointer")]
pub unsafe extern "sysv64" fn XGrabPointer(
    _dpy: *mut Display,
    grab_window: Window,
    _owner_events: Bool,
    _event_mask: c_uint,
    _pointer_mode: c_int,
    _keyboard_mode: c_int,
    _confine_to: Window,
    _cursor: usize,
    _time: usize,
) -> c_int {
    unsafe { crate::issue_request(_dpy) };
    let hwnd = grab_window as HWND;
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XGrabPointer {grab_window:#x} confine={_confine_to:#x} owner_events={_owner_events}"
        );
    }
    if hwnd.is_null() || grab_window == 1 {
        return 3; // GrabNotViewable
    }

    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) } == 0 {
        return 3;
    }
    if _confine_to == 0 {
        return if kinakaze_libdisplay::ui::interaction::capture(grab_window, true) {
            kinakaze_libdisplay::ui::pointer_grab::changed(grab_window, true);
            0
        } else {
            1
        };
    }
    let confine = _confine_to as HWND;
    let mut client: RECT = unsafe { core::mem::zeroed() };
    if unsafe { GetClientRect(confine, &mut client) } == 0 {
        return 3;
    }
    let mut top_left = POINT {
        x: client.left,
        y: client.top,
    };
    let mut bottom_right = POINT {
        x: client.right,
        y: client.bottom,
    };
    if unsafe { ClientToScreen(confine, &mut top_left) } == 0
        || unsafe { ClientToScreen(confine, &mut bottom_right) } == 0
    {
        return 3;
    }
    let bounds = RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    };
    if unsafe { ClipCursor(&bounds) } == 0 {
        return 1; // AlreadyGrabbed is the closest X11 failure mode.
    }
    if !kinakaze_libdisplay::ui::interaction::capture(grab_window, true) {
        unsafe {
            ClipCursor(core::ptr::null());
        }
        return 1;
    }
    kinakaze_libdisplay::ui::pointer_grab::changed(grab_window, true);
    0 // GrabSuccess
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUngrabPointer")]
pub unsafe extern "sysv64" fn XUngrabPointer(_dpy: *mut Display, _time: usize) -> c_int {
    unsafe { crate::issue_request(_dpy) };
    kinakaze_libdisplay::ui::pointer_grab::release_owned();
    kinakaze_libdisplay::ui::interaction::capture(0, false);
    unsafe { ClipCursor(core::ptr::null()) };
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGrabButton")]
pub unsafe extern "sysv64" fn XGrabButton(
    _dpy: *mut Display,
    _button: c_uint,
    _modifiers: c_uint,
    _grab_window: Window,
    _owner_events: Bool,
    _event_mask: c_uint,
    _pointer_mode: c_int,
    _keyboard_mode: c_int,
    _confine_to: Window,
    _cursor: usize,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUngrabButton")]
pub unsafe extern "sysv64" fn XUngrabButton(
    _dpy: *mut Display,
    _button: c_uint,
    _modifiers: c_uint,
    _grab_window: Window,
) -> c_int {
    1
}

pub use crate::focus::{XGrabKeyboard, XUngrabKeyboard};

#[unsafe(export_name = "kinakaze_engine_libX11_XGrabKey")]
pub unsafe extern "sysv64" fn XGrabKey(
    _dpy: *mut Display,
    _keycode: c_int,
    _modifiers: c_uint,
    _grab_window: Window,
    _owner_events: Bool,
    _pointer_mode: c_int,
    _keyboard_mode: c_int,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUngrabKey")]
pub unsafe extern "sysv64" fn XUngrabKey(
    _dpy: *mut Display,
    _keycode: c_int,
    _modifiers: c_uint,
    _grab_window: Window,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XBell")]
pub unsafe extern "sysv64" fn XBell(dpy: *mut Display, percent: c_int) -> c_int {
    if !(-100..=100).contains(&percent) {
        return unsafe { crate::errors::report(dpy, 2, 104, 0, percent as usize) };
    }
    crate::keyboard::bell(percent) as i32
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMaxRequestSize")]
pub unsafe extern "sysv64" fn XMaxRequestSize(_dpy: *mut Display) -> c_long {
    65535
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetCloseDownMode")]
pub unsafe extern "sysv64" fn XSetCloseDownMode(_dpy: *mut Display, _close_mode: c_int) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XKillClient")]
pub unsafe extern "sysv64" fn XKillClient(_dpy: *mut Display, _resource: usize) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateFontCursor")]
pub unsafe extern "sysv64" fn XCreateFontCursor(_dpy: *mut Display, shape: c_uint) -> usize {
    use kinakaze_libdisplay::ui::CursorShape;

    // Values from X11/cursorfont.h.  Xlib font cursors predate themed cursors,
    // but applications still use them as a portable fallback.
    let shape = match shape {
        34 => CursorShape::Crosshair,         // XC_crosshair
        52 => CursorShape::ResizeAll,         // XC_fleur
        60 => CursorShape::Hand,              // XC_hand2
        68 => CursorShape::Arrow,             // XC_left_ptr
        108 => CursorShape::ResizeHorizontal, // XC_sb_h_double_arrow
        116 => CursorShape::ResizeVertical,   // XC_sb_v_double_arrow
        152 => CursorShape::IBeam,            // XC_xterm
        _ => CursorShape::Arrow,
    };
    crate::xcursor::register_cursor_shape(true, shape)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreatePixmapCursor")]
pub unsafe extern "sysv64" fn XCreatePixmapCursor(
    _dpy: *mut Display,
    _source: usize,
    mask: usize,
    _foreground_color: *mut XColor,
    _background_color: *mut XColor,
    _x: c_uint,
    _y: c_uint,
) -> usize {
    // With no mask X11 treats the whole source rectangle as visible. Otherwise
    // an all-zero mask is the conventional transparent cursor.
    crate::xcursor::register_cursor(mask == 0 || pixmap_has_visible_pixels(mask))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateGlyphCursor")]
pub unsafe extern "sysv64" fn XCreateGlyphCursor(
    _dpy: *mut Display,
    _source_font: usize,
    _mask_font: usize,
    _source_char: c_uint,
    _mask_char: c_uint,
    _foreground_color: *const XColor,
    _background_color: *const XColor,
) -> usize {
    crate::xcursor::register_cursor(true)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRecolorCursor")]
pub unsafe extern "sysv64" fn XRecolorCursor(
    _dpy: *mut Display,
    _cursor: usize,
    _foreground_color: *mut XColor,
    _background_color: *mut XColor,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeCursor")]
pub unsafe extern "sysv64" fn XFreeCursor(_dpy: *mut Display, cursor: usize) -> c_int {
    let _ = crate::xcursor::free_cursor(cursor);
    let _ = crate::render::free_cursor(cursor);
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAddConnectionWatch")]
pub unsafe extern "sysv64" fn XAddConnectionWatch(
    _dpy: *mut Display,
    _watch_proc: *mut c_void,
    _client_data: *mut c_char,
) -> Status {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XProcessInternalConnection")]
pub unsafe extern "sysv64" fn XProcessInternalConnection(_dpy: *mut Display, _fd: c_int) {}

fn apply_gc(to: &mut XGCValues, mask: u64, from: &XGCValues) {
    if mask & (1 << 0) != 0 {
        to.function = from.function;
    }
    if mask & (1 << 1) != 0 {
        to.plane_mask = from.plane_mask;
    }
    if mask & (1 << 2) != 0 {
        to.foreground = from.foreground;
    }
    if mask & (1 << 3) != 0 {
        to.background = from.background;
    }
    if mask & (1 << 4) != 0 {
        to.line_width = from.line_width;
    }
    if mask & (1 << 5) != 0 {
        to.line_style = from.line_style;
    }
    if mask & (1 << 6) != 0 {
        to.cap_style = from.cap_style;
    }
    if mask & (1 << 7) != 0 {
        to.join_style = from.join_style;
    }
    if mask & (1 << 8) != 0 {
        to.fill_style = from.fill_style;
    }
    if mask & (1 << 9) != 0 {
        to.fill_rule = from.fill_rule;
    }
    if mask & (1 << 10) != 0 {
        to.tile = from.tile;
    }
    if mask & (1 << 11) != 0 {
        to.stipple = from.stipple;
    }
    if mask & (1 << 12) != 0 {
        to.ts_x_origin = from.ts_x_origin;
    }
    if mask & (1 << 13) != 0 {
        to.ts_y_origin = from.ts_y_origin;
    }
    if mask & (1 << 14) != 0 {
        to.font = from.font;
    }
    if mask & (1 << 15) != 0 {
        to.subwindow_mode = from.subwindow_mode;
    }
    if mask & (1 << 16) != 0 {
        to.graphics_exposures = from.graphics_exposures;
    }
    if mask & (1 << 17) != 0 {
        to.clip_x_origin = from.clip_x_origin;
    }
    if mask & (1 << 18) != 0 {
        to.clip_y_origin = from.clip_y_origin;
    }
    if mask & (1 << 19) != 0 {
        to.clip_mask = from.clip_mask;
    }
    if mask & (1 << 20) != 0 {
        to.dash_offset = from.dash_offset;
    }
    if mask & (1 << 21) != 0 {
        to.dashes = from.dashes;
    }
    if mask & (1 << 22) != 0 {
        to.arc_mode = from.arc_mode;
    }
}
