//! Native stacking and pointer capture run on the HWND's owning UI thread.
use super::*;

pub(super) const RAISE: u32 = 0x040c;
pub(super) const CAPTURE: u32 = 0x040e;
pub(super) const MOVE_RESIZE: u32 = 0x040f;
pub(super) const RELEASE_SOURCE: u32 = 0x0412;
pub(super) const GRAB_CROSSING: u32 = 0x0413;
type ResizeHandler = unsafe extern "system" fn(usize, i32, i32, i32) -> bool;
static RESIZE_HANDLER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn set_resize_handler(handler: ResizeHandler) {
    RESIZE_HANDLER.store(handler as usize, std::sync::atomic::Ordering::Release);
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetCapture(window: Handle) -> Handle;
    fn GetCapture() -> Handle;
    fn ReleaseCapture() -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
}
/// Size limits for an interactive resize, in client pixels; zero means none.
#[derive(Clone, Copy, Default)]
pub struct SizeLimits {
    pub min_width: i32,
    pub min_height: i32,
    pub max_width: i32,
    pub max_height: i32,
}
fn size_limits() -> &'static Mutex<HashMap<usize, SizeLimits>> {
    static LIMITS: OnceLock<Mutex<HashMap<usize, SizeLimits>>> = OnceLock::new();
    LIMITS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn set_size_limits(window: usize, limits: SizeLimits) {
    size_limits().lock().unwrap().insert(window, limits);
}
pub(super) fn resize_axes(window: usize) -> (bool, bool) {
    let limits = size_limits()
        .lock()
        .unwrap()
        .get(&window)
        .copied()
        .unwrap_or_default();
    (
        limits.max_width <= 0 || limits.min_width != limits.max_width,
        limits.max_height <= 0 || limits.min_height != limits.max_height,
    )
}
pub(super) fn native_resize(window: usize, x: i32, y: i32, hit: usize) {
    let direction = match hit {
        10 => 7,
        11 => 3,
        12 => 1,
        13 => 0,
        14 => 2,
        15 => 5,
        16 => 6,
        17 => 4,
        _ => return,
    };
    let handler = RESIZE_HANDLER.load(std::sync::atomic::Ordering::Acquire);
    if handler != 0 {
        let handler: ResizeHandler = unsafe { core::mem::transmute(handler) };
        // Acquire on the foreground thread before the WM asynchronously starts
        // its X grab. This also keeps motion/release arriving outside the desktop.
        button(window, true, 1);
        if unsafe { handler(window, x, y, direction) } {
            return;
        }
        button(window, false, 0);
    }
    let limits = size_limits()
        .lock()
        .unwrap()
        .get(&window)
        .copied()
        .unwrap_or_default();
    move_resize(window, x, y, direction, limits);
}
#[derive(Clone, Copy)]
struct Drag {
    window: usize,
    x: i32,
    y: i32,
    bounds: Rect,
    /// Non-client extent of `bounds` beyond the client area.
    frame: (i32, i32),
    limits: SizeLimits,
    direction: i32,
}
#[derive(Default)]
struct Capture {
    implicit: usize,
    explicit: usize,
    drag: Option<Drag>,
}
static CAPTURE_STATE: Mutex<Capture> = Mutex::new(Capture {
    implicit: 0,
    explicit: 0,
    drag: None,
});

pub fn raise(window: usize, activate: bool) -> bool {
    window > 1 && unsafe { SendMessageW(window as Handle, RAISE, usize::from(activate), 0) != 0 }
}
pub fn capture(window: usize, enabled: bool) -> bool {
    let Some(controller) = controller() else {
        return false;
    };
    unsafe { SendMessageW(controller, CAPTURE, window, isize::from(enabled)) != 0 }
}
/// Starts a `_NET_WM_MOVERESIZE` style drag from the pointer's root position.
/// Directions 0–7 resize from that edge or corner, 8 moves, 11 cancels; the
/// keyboard variants (9, 10) have no native counterpart and are rejected.
pub fn move_resize(window: usize, x: i32, y: i32, direction: i32, limits: SizeLimits) -> bool {
    if window <= 1 || !(0..=8).contains(&direction) && direction != 11 {
        return false;
    }
    // The request travels as a pointer, which only the owning process can read.
    let mut process = 0;
    if unsafe { GetWindowThreadProcessId(window as Handle, &raw mut process) } == 0
        || process != unsafe { GetCurrentProcessId() }
    {
        return false;
    }
    let request = Drag {
        window,
        x,
        y,
        bounds: Rect::default(),
        frame: (0, 0),
        limits,
        direction,
    };
    unsafe {
        SendMessageW(
            window as Handle,
            MOVE_RESIZE,
            0,
            &raw const request as isize,
        ) != 0
    }
}
pub(super) unsafe fn raise_native(window: Handle, activate: bool) -> bool {
    if activate {
        taskbar::refresh(window);
    }
    if activate && unsafe { IsIconic(window) } != 0 {
        unsafe {
            ShowWindow(window, 9);
        }
    }
    let raised = unsafe {
        SetWindowPos(
            window,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    } != 0;
    if activate {
        unsafe {
            SetForegroundWindow(window);
            SetFocus(window);
        }
    }
    raised
}
pub(super) unsafe fn capture_native(window: usize, enabled: bool) -> bool {
    if enabled {
        let mut bounds = Rect::default();
        if window <= 1 || unsafe { GetClientRect(window as Handle, &raw mut bounds) } == 0 {
            return false;
        }
        {
            let mut state = CAPTURE_STATE.lock().unwrap();
            state.explicit = window;
            state.implicit = 0;
        }
        if pointer_grab::has_foreign_source(window) {
            return true;
        }
        unsafe {
            SetCapture(window as Handle);
        }
        unsafe { GetCapture() == window as Handle }
    } else {
        let release = {
            let mut state = CAPTURE_STATE.lock().unwrap();
            if window != 0 && state.explicit != window {
                return true;
            }
            let previous = state.explicit;
            state.explicit = 0;
            previous
        };
        if release != 0 && unsafe { GetCapture() as usize } == release {
            unsafe {
                ReleaseCapture();
            }
        }
        true
    }
}
pub(super) unsafe fn begin_drag(parameter: isize) -> bool {
    let mut request = unsafe { *(parameter as *const Drag) };
    if request.direction == 11 {
        forget(request.window);
        return true;
    }
    let mut client = Rect::default();
    if unsafe { GetWindowRect(request.window as Handle, &raw mut request.bounds) } == 0
        || unsafe { GetClientRect(request.window as Handle, &raw mut client) } == 0
    {
        return false;
    }
    request.frame = (
        (request.bounds.right - request.bounds.left) - client.right,
        (request.bounds.bottom - request.bounds.top) - client.bottom,
    );
    {
        let mut state = CAPTURE_STATE.lock().unwrap();
        state.drag = Some(request);
        state.implicit = 0;
    }
    unsafe {
        SetCapture(request.window as Handle);
    }
    true
}
/// Clamps one axis of a resize so the client size respects the hint limits.
fn clamp(size: i32, frame: i32, min: i32, max: i32) -> i32 {
    let mut size = size.max(frame + min.max(1));
    if max > 0 {
        size = size.min(frame + max.max(1));
    }
    size
}
pub(super) fn motion(window: usize, x: i32, y: i32) -> bool {
    let drag = CAPTURE_STATE.lock().unwrap().drag;
    let Some(drag) = drag.filter(|drag| drag.window == window) else {
        return false;
    };
    let dx = x.saturating_sub(drag.x);
    let dy = y.saturating_sub(drag.y);
    let mut bounds = drag.bounds;
    if drag.direction == 8 {
        bounds.left += dx;
        bounds.right += dx;
        bounds.top += dy;
        bounds.bottom += dy;
    } else {
        let limits = drag.limits;
        if matches!(drag.direction, 0 | 6 | 7) {
            let width = clamp(
                bounds.right - bounds.left - dx,
                drag.frame.0,
                limits.min_width,
                limits.max_width,
            );
            bounds.left = bounds.right - width;
        }
        if matches!(drag.direction, 2 | 3 | 4) {
            let width = clamp(
                bounds.right - bounds.left + dx,
                drag.frame.0,
                limits.min_width,
                limits.max_width,
            );
            bounds.right = bounds.left + width;
        }
        if matches!(drag.direction, 0 | 1 | 2) {
            let height = clamp(
                bounds.bottom - bounds.top - dy,
                drag.frame.1,
                limits.min_height,
                limits.max_height,
            );
            bounds.top = bounds.bottom - height;
        }
        if matches!(drag.direction, 4 | 5 | 6) {
            let height = clamp(
                bounds.bottom - bounds.top + dy,
                drag.frame.1,
                limits.min_height,
                limits.max_height,
            );
            bounds.bottom = bounds.top + height;
        }
    }
    unsafe {
        SetWindowPos(
            window as Handle,
            core::ptr::null_mut(),
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    true
}
pub(super) fn button(window: usize, pressed: bool, native_buttons: usize) {
    let (acquire, release) = {
        let mut state = CAPTURE_STATE.lock().unwrap();
        if pressed && state.explicit == 0 && state.drag.is_none() {
            state.implicit = window;
            (true, false)
        } else if !pressed && native_buttons & 0x73 == 0 {
            let release =
                state.implicit == window || state.drag.is_some_and(|d| d.window == window);
            state.implicit = 0;
            state.drag = None;
            (false, release && state.explicit == 0)
        } else {
            (false, false)
        }
    };
    if acquire {
        unsafe {
            SetCapture(window as Handle);
        }
        if unsafe { GetCapture() as usize } == window {
            pointer_grab::source_changed(window, true);
        }
    }
    if release && unsafe { GetCapture() as usize } == window {
        unsafe {
            ReleaseCapture();
        }
    }
    if !pressed && native_buttons & 0x73 == 0 {
        pointer_grab::source_changed(window, false);
    }
}
pub(super) fn forget(window: usize) {
    pointer_grab::source_changed(window, false);
    {
        let mut state = CAPTURE_STATE.lock().unwrap();
        if state.implicit == window {
            state.implicit = 0;
        }
        if state.explicit == window {
            state.explicit = 0;
        }
        if state.drag.is_some_and(|d| d.window == window) {
            state.drag = None;
        }
    }
    if unsafe { GetCapture() as usize } == window {
        unsafe {
            ReleaseCapture();
        }
    }
}
