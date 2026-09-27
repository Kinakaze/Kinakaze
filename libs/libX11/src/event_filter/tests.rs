use super::*;
use crate::XAnyEvent;

static SERIAL: Mutex<()> = Mutex::new(());

struct Call {
    count: usize,
    result: Bool,
    window: Window,
    remove: bool,
}

unsafe extern "sysv64" fn callback(
    dpy: *mut Display,
    window: Window,
    _: *mut XEvent,
    data: *mut c_void,
) -> Bool {
    let call = unsafe { &mut *data.cast::<Call>() };
    call.count += 1;
    call.window = window;
    if call.remove {
        unsafe {
            _XUnregisterFilter(dpy, window, Some(callback), data);
        }
    }
    call.result
}

fn event(display: *mut Display, window: Window, kind: i32) -> XEvent {
    XEvent {
        xany: XAnyEvent {
            r#type: kind,
            serial: 0,
            send_event: 0,
            display,
            window,
        },
    }
}

#[test]
fn filters_match_display_window_and_inclusive_type_range() {
    let _serial = SERIAL.lock().unwrap();
    let mut dpy: Display = unsafe { std::mem::zeroed() };
    let mut other: Display = unsafe { std::mem::zeroed() };
    let mut call = Call {
        count: 0,
        result: 1,
        window: 0,
        remove: false,
    };
    let data = (&raw mut call).cast();
    unsafe {
        _XRegisterFilterByType(&raw mut dpy, 17, 32, 34, Some(callback), data);
        assert_eq!(filter(&mut event(&raw mut other, 17, 33), 0), 0);
        assert_eq!(filter(&mut event(&raw mut dpy, 18, 33), 0), 0);
        for kind in [31, 35] {
            assert_eq!(filter(&mut event(&raw mut dpy, 17, kind), 0), 0);
        }
        for kind in [32, 33, 34] {
            assert_eq!(filter(&mut event(&raw mut dpy, 17, kind), 0), 1);
        }
        assert_eq!(call.count, 3);
        assert_eq!(filter(&mut event(&raw mut dpy, 18, 33), 17), 1);
        assert_eq!(call.window, 17);
    }
    close(&raw mut dpy);
}

#[test]
fn newest_matching_filter_returns_false_without_calling_older_one() {
    let _serial = SERIAL.lock().unwrap();
    let mut dpy: Display = unsafe { std::mem::zeroed() };
    let mut old = Call {
        count: 0,
        result: 1,
        window: 0,
        remove: false,
    };
    let mut new = Call {
        count: 0,
        result: 0,
        window: 0,
        remove: false,
    };
    unsafe {
        _XRegisterFilterByType(
            &raw mut dpy,
            17,
            33,
            33,
            Some(callback),
            (&raw mut old).cast(),
        );
        _XRegisterFilterByType(
            &raw mut dpy,
            17,
            33,
            33,
            Some(callback),
            (&raw mut new).cast(),
        );
        assert_eq!(filter(&mut event(&raw mut dpy, 17, 33), 0), 0);
        assert_eq!((old.count, new.count), (0, 1));
        _XUnregisterFilter(&raw mut dpy, 17, Some(callback), (&raw mut new).cast());
        assert_eq!(filter(&mut event(&raw mut dpy, 17, 33), 0), 1);
    }
    close(&raw mut dpy);
}

#[test]
fn callback_can_remove_itself_and_all_exact_duplicates() {
    let _serial = SERIAL.lock().unwrap();
    let mut dpy: Display = unsafe { std::mem::zeroed() };
    let mut call = Call {
        count: 0,
        result: 1,
        window: 0,
        remove: true,
    };
    unsafe {
        for _ in 0..2 {
            _XRegisterFilterByType(
                &raw mut dpy,
                17,
                33,
                33,
                Some(callback),
                (&raw mut call).cast(),
            );
        }
        assert_eq!(filter(&mut event(&raw mut dpy, 17, 33), 0), 1);
        assert_eq!(filter(&mut event(&raw mut dpy, 17, 33), 0), 0);
        assert_eq!(call.count, 1);
    }
    close(&raw mut dpy);
}

#[test]
fn close_discards_only_that_displays_filters() {
    let _serial = SERIAL.lock().unwrap();
    let mut dpy: Display = unsafe { std::mem::zeroed() };
    let mut other: Display = unsafe { std::mem::zeroed() };
    let mut call = Call {
        count: 0,
        result: 1,
        window: 0,
        remove: false,
    };
    unsafe {
        for display in [&raw mut dpy, &raw mut other] {
            _XRegisterFilterByType(display, 17, 33, 33, Some(callback), (&raw mut call).cast());
        }
        close(&raw mut dpy);
        assert_eq!(filter(&mut event(&raw mut dpy, 17, 33), 0), 0);
        assert_eq!(filter(&mut event(&raw mut other, 17, 33), 0), 1);
        assert_eq!(filter(std::ptr::null_mut(), 0), 0);
    }
    close(&raw mut other);
}
