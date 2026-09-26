//! Composite redirects native window backing stores and retains named surfaces.
use crate::xkb::server::{finish, put32, reply, u32_at};
use crate::{Display, Window, errors::ProtocolError};
use std::sync::Mutex;
pub const OPCODE: u8 = 133;
static OVERLAY: Mutex<(Window, usize)> = Mutex::new((0, 0));
fn valid(window: Window) -> bool {
    crate::shared::valid(window)
}
fn mode(window: Window) -> Option<u8> {
    let direct = crate::shared::get(&format!("r/{window}"));
    if let Some(value) = direct
        && value.get(5) == Some(&0)
    {
        return value.get(4).copied();
    }
    let parent = crate::shared::parent(window);
    let value = crate::shared::get(&format!("r/{parent}"))?;
    return (value.get(5) == Some(&1)).then(|| value[4]);
}
pub fn manual(window: Window) -> bool {
    mode(window) == Some(1)
}
pub fn forget(window: Window) {
    crate::shared::remove(&format!("r/{window}"));
}
fn error(minor: u8, code: u8, resource: usize) -> ProtocolError {
    ProtocolError {
        code,
        request: OPCODE,
        minor,
        resource,
    }
}
pub(crate) fn dispatch(
    display: *mut Display,
    minor: u8,
    req: &[u8],
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let needed = if matches!(minor, 7 | 8) { 8 } else { 12 };
    if req.len() < needed {
        return Err(error(minor, 16, req.len()));
    }
    if minor == 0 {
        let mut r = reply(32);
        r[1] = 0;
        put32(&mut r, 8, 0);
        put32(&mut r, 12, 4);
        return Ok(Some(finish(r)));
    }
    let w = u32_at(req, 4) as usize;
    match minor {
        1..=4 => {
            if !valid(w) {
                return Err(error(minor, 3, w));
            }
            if req[8] > 1 {
                return Err(error(minor, 2, req[8] as usize));
            }
            {
                crate::shared::transaction(|tx| {
                    let key = format!("r/{w}");
                    let old = tx.get(&key);
                    if let Some(old) = old {
                        let owner = u32::from_le_bytes(old[..4].try_into().unwrap());
                        if owner != crate::shared::pid() && kinakaze_runtime::job::alive(owner) {
                            return Err(10);
                        }
                    }
                    if minor <= 2 {
                        let mut value = crate::shared::pid().to_le_bytes().to_vec();
                        value.extend_from_slice(&[req[8], u8::from(minor == 2)]);
                        tx.set(key, value);
                    } else {
                        if !old.is_some_and(|v| v[4] == req[8] && v[5] == u8::from(minor == 4)) {
                            return Err(2);
                        }
                        tx.remove(&key);
                    }
                    Ok(())
                })
                .map_err(|code| error(minor, code, w))?;
                return Ok(None);
            }
        }
        5 => {
            let window = u32_at(req, 8) as usize;
            let r = crate::xfixes::window_region(display, window).ok_or(error(minor, 3, window))?;
            if !crate::xfixes::create(w as u32, r) {
                return Err(error(minor, 14, w));
            }
        }
        6 => {
            if !valid(w) {
                return Err(error(minor, 3, w));
            }
            if mode(w).is_none() {
                return Err(error(minor, 8, w));
            }
            let id = u32_at(req, 8) as usize;
            crate::graphics::name_window_pixmap(w, id).map_err(|code| error(minor, code, id))?;
        }
        7 => {
            if !valid(w) {
                return Err(error(minor, 3, w));
            }
            let mut overlay = OVERLAY.lock().unwrap_or_else(|e| e.into_inner());
            if overlay.0 == 0 {
                let width = unsafe { crate::GLOBAL_SCREEN.width };
                let height = unsafe { crate::GLOBAL_SCREEN.height };
                // The Composite overlay is a screen-sized override-redirect
                // window. A host caption would offset its drawable origin from
                // the root coordinates used by the compositor's XI2 input.
                let mut attributes: crate::XSetWindowAttributes = unsafe { core::mem::zeroed() };
                attributes.override_redirect = 1;
                let win = unsafe {
                    crate::XCreateWindow(
                        display,
                        1,
                        0,
                        0,
                        width as u32,
                        height as u32,
                        0,
                        24,
                        1,
                        core::ptr::null_mut(),
                        1 << 9, // CWOverrideRedirect
                        &raw mut attributes,
                    )
                };
                if win == 0 {
                    return Err(error(minor, 11, w));
                }
                kinakaze_libdisplay::ui::desktop::mark(win);
                unsafe {
                    // Creating a decorated host window can clamp its initial
                    // client size to Windows' tracking bounds. Restore the exact
                    // X screen rectangle after removing the frame, before map.
                    // The overlay is above this X display's scene, not above
                    // unrelated Windows applications. Native override-redirect
                    // popups normally use TOPMOST; this desktop surface must
                    // remain switchable alongside the user's host windows.
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        HWND_NOTOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SetWindowPos,
                    };
                    SetWindowPos(
                        win as _,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                    crate::XStoreName(display, win, c"Kinakaze GNOME Shell".as_ptr());
                    crate::XMapWindow(display, win);
                }
                overlay.0 = win;
            }
            overlay.1 += 1;
            let mut r = reply(32);
            r[1] = 0;
            put32(&mut r, 8, overlay.0 as u32);
            return Ok(Some(finish(r)));
        }
        8 => {
            if !valid(w) {
                return Err(error(minor, 3, w));
            }
            let mut overlay = OVERLAY.lock().unwrap_or_else(|e| e.into_inner());
            if overlay.1 > 0 {
                overlay.1 -= 1;
                if overlay.1 == 0 {
                    let win = core::mem::take(&mut overlay.0);
                    drop(overlay);
                    unsafe {
                        crate::XDestroyWindow(display, win);
                    }
                }
            }
        }
        _ => return Err(error(minor, 1, minor as usize)),
    }
    Ok(None)
}
