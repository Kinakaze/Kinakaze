//! ICCCM transients are owned top-level windows, not Win32 child windows.
use super::*;

pub(super) const MESSAGE: u32 = 0x0410;
const OWNER: i32 = -8;
static PENDING: Mutex<Vec<usize>> = Mutex::new(Vec::new());

pub fn set(window: usize, owner: usize) -> bool {
    window > 1 && unsafe { SendMessageW(window as Handle, MESSAGE, owner, 0) != 0 }
}

pub(super) unsafe fn apply(window: Handle, owner: usize) -> bool {
    if unsafe { GetWindowLongPtrW(window, GWL_STYLE) } & WS_CHILD as isize != 0 {
        return false;
    }
    let owner = if owner <= 1 { 0 } else { owner };
    if owner == window as usize {
        return false;
    }
    // Do not create an ownership cycle, which Windows also rejects.
    let mut ancestor = owner;
    for _ in 0..128 {
        if ancestor == 0 {
            break;
        }
        if ancestor == window as usize {
            return false;
        }
        ancestor = unsafe { GetWindowLongPtrW(ancestor as Handle, OWNER) } as usize;
    }
    unsafe {
        SetWindowLongPtrW(window, OWNER, owner as isize);
    }
    let mut pending = PENDING.lock().unwrap();
    pending.retain(|&value| value != window as usize);
    if owner != 0 {
        pending.push(window as usize);
    }
    true
}

pub(super) fn position_on_show(window: usize) {
    let mut pending = PENDING.lock().unwrap();
    let Some(index) = pending.iter().position(|&value| value == window) else {
        return;
    };
    pending.swap_remove(index);
    drop(pending);
    // Menus, tooltips and popovers are also owned windows. Their toolkits
    // position them at a pointer/anchor; centering changes the drawable origin
    // behind the toolkit's back and makes menu hit testing appear displaced.
    if is_override_redirect(window) {
        return;
    }
    let native = window as Handle;
    let owner = unsafe { GetWindowLongPtrW(native, OWNER) } as Handle;
    if owner.is_null() {
        return;
    }
    let (mut bounds, mut parent) = (Rect::default(), Rect::default());
    if unsafe { GetWindowRect(native, &raw mut bounds) } == 0
        || unsafe { GetWindowRect(owner, &raw mut parent) } == 0
    {
        return;
    }
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    if width <= 32 || height <= 32 {
        return;
    }
    let mut x = parent.left + (parent.right - parent.left - width) / 2;
    let mut y = parent.top + (parent.bottom - parent.top - height) / 2;
    let monitor = unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) };
    let mut info = MonitorInfo {
        size: core::mem::size_of::<MonitorInfo>() as u32,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
    };
    if unsafe { GetMonitorInfoW(monitor, &raw mut info) } != 0 {
        x = x.clamp(
            info.work.left,
            (info.work.right - width).max(info.work.left),
        );
        y = y.clamp(
            info.work.top,
            (info.work.bottom - height).max(info.work.top),
        );
    }
    unsafe {
        SetWindowPos(
            native,
            core::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn forget(window: usize) {
    PENDING.lock().unwrap().retain(|&value| value != window);
}
