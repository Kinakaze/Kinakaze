//! Reparent on the owning UI thread and keep the client extent unchanged.
use super::*;
pub(super) const MESSAGE: u32 = WM_SET_VISIBLE + 1;
struct Request {
    window: Handle,
    parent: Handle,
    x: i32,
    y: i32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetParent(window: Handle, parent: Handle) -> Handle;
    fn GetParent(window: Handle) -> Handle;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetLastError(error: u32);
}

pub(super) fn apply(window: usize, parent: usize, x: i32, y: i32) -> bool {
    let Some(controller) = controller() else {
        return false;
    };
    let request = Request {
        window: window as Handle,
        parent: parent as Handle,
        x,
        y,
    };
    unsafe { SendMessageW(controller, MESSAGE, 0, &raw const request as isize) != 0 }
}
pub(super) unsafe fn dispatch(parameter: isize) -> isize {
    let request = unsafe { &*(parameter as *const Request) };
    let mut client = Rect::default();
    if unsafe { GetClientRect(request.window, &raw mut client) } == 0 {
        return 0;
    }
    let old_style = unsafe { GetWindowLongPtrW(request.window, GWL_STYLE) };
    let old_parent = unsafe { GetParent(request.window) };
    const WS_POPUP: isize = 0x8000_0000;
    let style = if request.parent.is_null() {
        (old_style & !(WS_CHILD as isize | WS_POPUP)) | WS_OVERLAPPEDWINDOW as isize
    } else {
        (old_style & !(WS_OVERLAPPEDWINDOW as isize | WS_POPUP)) | WS_CHILD as isize
    };
    let extended = unsafe { GetWindowLongPtrW(request.window, GWL_EXSTYLE) };
    let mut frame = client;
    if unsafe { AdjustWindowRectEx(&raw mut frame, style as u32, 0, extended as u32) } == 0 {
        return 0;
    }
    let (Some(x), Some(y)) = (
        request.x.checked_add(frame.left),
        request.y.checked_add(frame.top),
    ) else {
        return 0;
    };
    // Reparent preserves the drawable extent even below the decorated window's
    // default minimum tracking size. WM_SIZE is still delivered after resizing.
    const SWP_NOSENDCHANGING: u32 = 0x0400;
    unsafe {
        SetLastError(0);
    }
    if unsafe { SetWindowLongPtrW(request.window, GWL_STYLE, style) } == 0
        && unsafe { GetLastError() } != 0
    {
        return 0;
    }
    unsafe {
        SetLastError(0);
    }
    if unsafe { SetParent(request.window, request.parent) }.is_null()
        && unsafe { GetLastError() } != 0
    {
        unsafe {
            SetWindowLongPtrW(request.window, GWL_STYLE, old_style);
        }
        return 0;
    }
    if unsafe {
        SetWindowPos(
            request.window,
            core::ptr::null_mut(),
            x,
            y,
            frame.right - frame.left,
            frame.bottom - frame.top,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
        )
    } == 0
    {
        unsafe {
            SetParent(request.window, old_parent);
            SetWindowLongPtrW(request.window, GWL_STYLE, old_style);
        }
        return 0;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moves_between_native_parents_without_resizing_client() {
        let a = create(180, 120, "reparent-a").unwrap();
        let b = create(190, 130, "reparent-b").unwrap();
        let child = create(64, 48, "reparent-child").unwrap();
        // Native decorated window creation may apply the desktop's minimum
        // tracking width. Reparent must preserve the actual client extent.
        let original = client_size(child).unwrap();
        assert!(apply(child, a, 7, 9));
        assert_eq!(unsafe { GetParent(child as Handle) } as usize, a);
        assert_eq!(client_size(child), Some(original));
        assert!(apply(child, b, 11, 13));
        assert_eq!(unsafe { GetParent(child as Handle) } as usize, b);
        assert_eq!(client_size(child), Some(original));
        assert!(apply(child, 0, 20, 30));
        assert!(unsafe { GetParent(child as Handle) }.is_null());
        assert_eq!(client_size(child), Some(original));
        assert!(destroy(child) && destroy(b) && destroy(a));
    }
}
