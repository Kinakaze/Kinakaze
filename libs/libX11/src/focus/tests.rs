use super::*;
use crate::{XCloseDisplay, XCreateSimpleWindow, XDestroyWindow, XOpenDisplay, XPending};
use kinakaze_libdisplay::event::{EVENT_FOCUS, Event, publish};

#[test]
fn native_focus_updates_queries_and_key_routing_across_connections() {
    unsafe {
        let a = XOpenDisplay(core::ptr::null());
        let b = XOpenDisplay(core::ptr::null());
        let first = XCreateSimpleWindow(a, 1, 0, 0, 160, 120, 0, 0, 0);
        let second = XCreateSimpleWindow(b, 1, 0, 0, 160, 120, 0, 0, 0);
        let previous = *FOCUS.lock().unwrap();
        let previous_shared = crate::shared::get("focus");
        *FOCUS.lock().unwrap() = Focus {
            mode: 1,
            window: 0,
            revert: 0,
            time: 0,
            grab: None,
        };
        crate::shared::remove("focus");

        let changed = |window, gained, time| {
            publish(Event {
                kind: EVENT_FOCUS,
                window: kinakaze_libdisplay::window::logical_for_native(window).unwrap(),
                state: u32::from(gained),
                time_ms: time,
                ..Default::default()
            });
            XPending(a);
        };
        let query = |expected, expected_revert| {
            for display in [a, b] {
                let mut target = usize::MAX;
                let mut revert = -1;
                XGetInputFocus(display, &raw mut target, &raw mut revert);
                assert_eq!((target, revert), (expected, expected_revert));
            }
        };

        changed(first, true, 100);
        query(first, 0);
        assert_eq!(key_target(first), Some(first));

        // The new process can drain its gain before the old one drains its loss.
        changed(second, true, 102);
        changed(first, false, 101);
        changed(first, false, 102);
        query(second, 0);
        assert_eq!(key_target(first), Some(second));
        changed(second, false, 103);
        query(0, 0);
        assert_eq!(key_target(second), None);
        changed(second, true, 103);
        query(second, 0);

        // Match the record written by an explicit XSetInputFocus request.
        let mut record = (second as u64).to_le_bytes().to_vec();
        record.extend_from_slice(&2i32.to_le_bytes());
        record.extend_from_slice(&110u32.to_le_bytes());
        crate::shared::set("focus".into(), record).unwrap();
        changed(first, true, 109);
        query(second, 2);
        changed(second, true, 110);
        query(second, 2);

        XDestroyWindow(a, first);
        XDestroyWindow(b, second);
        XCloseDisplay(a);
        XCloseDisplay(b);
        *FOCUS.lock().unwrap() = previous;
        match previous_shared {
            Some(record) => crate::shared::set("focus".into(), record).unwrap(),
            None => crate::shared::remove("focus"),
        }
    }
}
