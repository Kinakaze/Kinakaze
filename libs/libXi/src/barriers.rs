//! XI 2.3 pointer barrier cookies from the native mouse constraint backend.
use super::*;
#[repr(C)]
pub(super) struct BarrierEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut Display,
    extension: i32,
    evtype: i32,
    time: u64,
    device: i32,
    source: i32,
    event: Window,
    root: Window,
    root_x: f64,
    root_y: f64,
    dx: f64,
    dy: f64,
    dtime: i32,
    flags: i32,
    barrier: u64,
    eventid: u32,
}
pub(super) fn poll(_display: *mut Display, close: bool) {
    if close {
        return;
    }
    for note in kinakaze_libdisplay::ui::barriers::notices() {
        let kind = if note.leave { 26 } else { 25 };
        for display in kinakaze_libX11::connection::displays() {
            let selected = {
                let masks = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
                [0, 1, 2].into_iter().any(|device| {
                    masks
                        .get(&(display as usize, note.window, device))
                        .is_some_and(|v| v & (1 << kind) != 0)
                })
            };
            if !selected {
                continue;
            }
            let data = unsafe { kinakaze_alloc::guest::malloc(size_of::<BarrierEvent>()) }
                .cast::<BarrierEvent>();
            if data.is_null() {
                continue;
            }
            unsafe {
                data.write(BarrierEvent {
                    kind: GenericEvent,
                    serial: 0,
                    send: 0,
                    display,
                    extension: XI_EXTENSION_OPCODE,
                    evtype: kind,
                    time: note.time,
                    device: 2,
                    source: 4,
                    event: note.window,
                    root: 1,
                    root_x: note.x as f64,
                    root_y: note.y as f64,
                    dx: note.dx as f64,
                    dy: note.dy as f64,
                    dtime: note.dtime,
                    flags: note.flags,
                    barrier: note.barrier as u64,
                    eventid: note.event,
                });
            }
            let mut event = unsafe { core::mem::zeroed::<XEvent>() };
            event.xcookie = XGenericEventCookie {
                r#type: GenericEvent,
                serial: 0,
                send_event: 0,
                display,
                extension: XI_EXTENSION_OPCODE,
                evtype: kind,
                cookie: NEXT_COOKIE.fetch_add(1, Ordering::Relaxed),
                data: data.cast(),
            };
            kinakaze_libX11::queue_extension_event(display, event);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn barrier_cookie_matches_linux_lp64() {
        assert_eq!(size_of::<BarrierEvent>(), 128);
        assert_eq!(core::mem::offset_of!(BarrierEvent, root_x), 72);
        assert_eq!(core::mem::offset_of!(BarrierEvent, barrier), 112);
    }
}
