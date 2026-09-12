//! Client-drawn decorations do not need an additional Windows non-client frame.
use super::*;

pub(super) const MESSAGE: u32 = 0x040d;
pub(super) const CUSTOM_SHAPE: u32 = 0x0411;
#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowRgn(window: Handle, region: Handle, redraw: i32) -> i32;
    fn GetDpiForWindow(window: Handle) -> u32;
    fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
}
static SHAPES: Mutex<Option<HashMap<usize, (i32, i32, i32)>>> = Mutex::new(None);
static CLIENT_SHAPES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
// DWM rounds the composed surface with antialiasing and follows live resizing.
// Region clipping is only the fallback for systems without this attribute.
static DWM_SHAPES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
type ShapeHandler = unsafe extern "system" fn(usize);
static SHAPE_HANDLER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn set_shape_handler(handler: ShapeHandler) {
    SHAPE_HANDLER.store(handler as usize, std::sync::atomic::Ordering::Release);
}
fn radius_key() -> *const u16 {
    static KEY: OnceLock<Vec<u16>> = OnceLock::new();
    KEY.get_or_init(|| wide("KinakazeCornerRadius")).as_ptr()
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetPropW(window: Handle, name: *const u16) -> Handle;
    fn SetPropW(window: Handle, name: *const u16, value: Handle) -> i32;
}
pub fn rounded_radius(window: usize) -> i32 {
    unsafe { GetPropW(window as _, radius_key()) as usize as i32 }
}
fn notify_shape(window: usize, radius: i32) {
    unsafe {
        SetPropW(window as _, radius_key(), radius as usize as _);
    }
    let callback = SHAPE_HANDLER.load(std::sync::atomic::Ordering::Acquire);
    if callback != 0 {
        let callback: ShapeHandler = unsafe { core::mem::transmute(callback) };
        unsafe {
            callback(window);
        }
    }
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        window: Handle,
        attribute: u32,
        value: *const c_void,
        size: u32,
    ) -> i32;
}
pub fn custom_shape(window: usize, custom: bool) {
    unsafe {
        SendMessageW(window as _, CUSTOM_SHAPE, usize::from(custom), 0);
    }
}
pub(super) fn apply_custom_shape(window: usize, custom: bool) {
    let mut shapes = CLIENT_SHAPES.lock().unwrap();
    shapes.retain(|&value| value != window);
    if custom {
        shapes.push(window);
    }
    drop(shapes);
    DWM_SHAPES.lock().unwrap().retain(|&value| value != window);
    SHAPES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .remove(&window);
    // The X server sends the authoritative ShapeNotify after committing the
    // requested region. Do not emit a transient default-shape notification.
    if custom {
        unsafe {
            SetPropW(window as _, radius_key(), core::ptr::null_mut());
        }
    } else {
        refresh_shape(window);
    }
}
pub fn has_client_frame(window: usize) -> bool {
    (unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) } & WS_CHILD as isize) == 0
        && originals()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&window)
        && !is_override_redirect(window)
        && !input_only::is_input_only(window)
}
pub fn refresh_shape(window: usize) {
    if CLIENT_SHAPES.lock().unwrap().contains(&window) {
        return;
    }
    let native = window as Handle;
    let rounded = has_client_frame(window)
        && unsafe { GetWindowLongPtrW(native, GWL_STYLE) } & WS_CHILD as isize == 0
        && unsafe { GetWindowLongPtrW(native, MAXIMIZED_OFFSET) } == 0
        && !fullscreen_windows().lock().unwrap().contains_key(&window);
    let Some((width, height)) = client_size(window) else {
        return;
    };
    let radius = if rounded && width > 32 && height > 32 {
        (unsafe { GetDpiForWindow(native) }.max(96) as i32 * 8 / 96)
            .min(width / 2)
            .min(height / 2)
    } else {
        0
    };
    let mut guard = SHAPES.lock().unwrap();
    let shapes = guard.get_or_insert_with(HashMap::new);
    if shapes.get(&window) == Some(&(width, height, radius)) {
        return;
    }
    let unchanged_preference = DWM_SHAPES.lock().unwrap().contains(&window)
        && shapes
            .get(&window)
            .is_some_and(|shape| (shape.2 != 0) == (radius != 0));
    if radius == 0 && !shapes.contains_key(&window) {
        return;
    }
    shapes.insert(window, (width, height, radius));
    // SetWindowRgn may synchronously cause another position/size notification.
    drop(guard);
    if !unchanged_preference {
        apply_shape(native, width, height, radius);
    }
    notify_shape(window, radius);
}
fn apply_shape(window: Handle, width: i32, height: i32, radius: i32) {
    let preference: u32 = if radius == 0 { 1 } else { 2 };
    if unsafe { DwmSetWindowAttribute(window, 33, (&raw const preference).cast(), 4) } >= 0 {
        let mut windows = DWM_SHAPES.lock().unwrap();
        let first = !windows.contains(&(window as usize));
        if first {
            windows.push(window as usize);
        }
        drop(windows);
        if first {
            unsafe {
                SetWindowRgn(window, core::ptr::null_mut(), 0);
            }
        }
        return;
    }
    let region = if radius == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2) }
    };
    if radius != 0 && region.is_null() {
        return;
    }
    if unsafe { SetWindowRgn(window, region, 1) } == 0 && !region.is_null() {
        unsafe {
            DeleteObject(region);
        }
    }
}
pub(super) fn resize_hit(window: usize, x: i32, y: i32) -> Option<isize> {
    if !has_client_frame(window)
        || unsafe { GetWindowLongPtrW(window as Handle, MAXIMIZED_OFFSET) } != 0
        || fullscreen_windows().lock().unwrap().contains_key(&window)
    {
        return None;
    }
    let mut rect = Rect::default();
    unsafe {
        GetWindowRect(window as Handle, &raw mut rect);
    }
    let dpi = unsafe { GetDpiForWindow(window as Handle) }.max(96);
    let padding = unsafe { GetSystemMetricsForDpi(92, dpi) }; // SM_CXPADDEDBORDER
    let edge_x = (unsafe { GetSystemMetricsForDpi(32, dpi) } + padding).max(1);
    let edge_y = (unsafe { GetSystemMetricsForDpi(33, dpi) } + padding).max(1);
    let corner = dpi as i32 * 24 / 96;
    let (horizontal, vertical) = interaction::resize_axes(window);
    let left = horizontal && x < rect.left + edge_x;
    let right = horizontal && x >= rect.right - edge_x;
    let top = vertical && y < rect.top + edge_y;
    let bottom = vertical && y >= rect.bottom - edge_y;
    // Extend corners along the narrow edge strips, without consuming header
    // buttons or application content in a large square at each corner.
    if (left && vertical && y < rect.top + corner) || (top && horizontal && x < rect.left + corner)
    {
        Some(13)
    } else if (right && vertical && y < rect.top + corner)
        || (top && horizontal && x >= rect.right - corner)
    {
        Some(14)
    } else if (left && vertical && y >= rect.bottom - corner)
        || (bottom && horizontal && x < rect.left + corner)
    {
        Some(16)
    } else if (right && vertical && y >= rect.bottom - corner)
        || (bottom && horizontal && x >= rect.right - corner)
    {
        Some(17)
    } else {
        match (left, right, top, bottom) {
            (true, _, _, _) => Some(10),
            (_, true, _, _) => Some(11),
            (_, _, true, _) => Some(12),
            (_, _, _, true) => Some(15),
            _ => None,
        }
    }
}

fn originals() -> &'static Mutex<HashMap<usize, (isize, isize)>> {
    static STYLES: OnceLock<Mutex<HashMap<usize, (isize, isize)>>> = OnceLock::new();
    STYLES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn set(window: usize, decorated: bool) -> bool {
    window != 0
        && unsafe { SendMessageW(window as Handle, MESSAGE, usize::from(decorated), 0) != 0 }
}

pub(super) unsafe fn apply(window: Handle, decorated: bool) -> bool {
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    let extended = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    if style & WS_CHILD as isize != 0 {
        return true;
    }
    const EDGES: isize = 0x1 | 0x100 | 0x200 | 0x20000;
    let (next, next_extended) = {
        let mut saved = originals().lock().unwrap_or_else(|e| e.into_inner());
        if decorated {
            let Some(original) = saved.remove(&(window as usize)) else {
                return true;
            };
            (
                (style & !(WS_OVERLAPPEDWINDOW as isize))
                    | (original.0 & WS_OVERLAPPEDWINDOW as isize),
                (extended & !EDGES) | (original.1 & EDGES),
            )
        } else {
            saved.entry(window as usize).or_insert((style, extended));
            (style & !(WS_OVERLAPPEDWINDOW as isize), extended & !EDGES)
        }
    };
    // Override-redirect/fullscreen still suppress the frame; update the style
    // those transitions will restore when they end.
    if let Some(restore) = override_redirect_windows()
        .lock()
        .unwrap()
        .get_mut(&(window as usize))
    {
        restore.style = (restore.style & !(WS_OVERLAPPEDWINDOW as isize))
            | (next & WS_OVERLAPPEDWINDOW as isize);
        return true;
    }
    if let Some(restore) = fullscreen_windows()
        .lock()
        .unwrap()
        .get_mut(&(window as usize))
    {
        restore.style = (restore.style & !(WS_OVERLAPPEDWINDOW as isize))
            | (next & WS_OVERLAPPEDWINDOW as isize);
        return true;
    }
    // Windows 11 otherwise draws its own one-pixel outline around GTK's CSD.
    let border: u32 = if decorated { 0xffff_ffff } else { 0xffff_fffe };
    unsafe { DwmSetWindowAttribute(window, 34, (&raw const border).cast(), 4) };
    refresh_shape(window as usize);
    if next == style && extended == next_extended {
        return true;
    }
    let mut client = Rect::default();
    let mut origin = Point { x: 0, y: 0 };
    if unsafe { GetClientRect(window, &raw mut client) } == 0
        || unsafe { ClientToScreen(window, &raw mut origin) } == 0
    {
        return false;
    }
    let mut frame = client;
    if unsafe { AdjustWindowRectEx(&raw mut frame, next as u32, 0, next_extended as u32) } == 0 {
        return false;
    }
    unsafe {
        SetWindowLongPtrW(window, GWL_STYLE, next);
        SetWindowLongPtrW(window, GWL_EXSTYLE, next_extended);
        SetWindowPos(
            window,
            core::ptr::null_mut(),
            origin.x + frame.left,
            origin.y + frame.top,
            frame.right - frame.left,
            frame.bottom - frame.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | 0x0400,
        ) != 0
    }
}

pub(super) fn forget(window: usize) {
    DWM_SHAPES.lock().unwrap().retain(|&value| value != window);
    SHAPES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .remove(&window);
    CLIENT_SHAPES
        .lock()
        .unwrap()
        .retain(|&value| value != window);
    originals()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&window);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_decorations_preserve_client_origin_size_and_stacking_style() {
        let window = create(320, 200, "CSD decorations probe").unwrap();
        let hwnd = window as Handle;
        let original = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let mut before = Point { x: 0, y: 0 };
        unsafe {
            ClientToScreen(hwnd, &raw mut before);
        }
        let size = client_size(window).unwrap();
        assert!(set(window, false));
        assert_eq!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } & WS_OVERLAPPEDWINDOW as isize,
            0
        );
        // WS_EX_WINDOWEDGE follows the frame styles; topmost and the rest
        // must not change.
        const WS_EX_WINDOWEDGE: isize = 0x100;
        assert_eq!(
            unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } & !WS_EX_WINDOWEDGE,
            extended & !WS_EX_WINDOWEDGE
        );
        let mut after = Point { x: 0, y: 0 };
        unsafe {
            ClientToScreen(hwnd, &raw mut after);
        }
        assert_eq!((before.x, before.y), (after.x, after.y));
        assert_eq!(client_size(window).unwrap(), size);
        assert!(set(window, true));
        assert_eq!(unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) }, original);
        assert_eq!(client_size(window).unwrap(), size);
        assert!(destroy(window));
    }
}
