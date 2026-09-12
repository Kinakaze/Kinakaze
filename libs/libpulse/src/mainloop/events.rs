use super::*;

pub(super) fn api() -> pa_mainloop_api {
    pa_mainloop_api {
        userdata: core::ptr::null_mut(),
        io_new,
        io_enable,
        io_free: free,
        io_set_destroy: set_destroy,
        time_new,
        time_restart,
        time_free: free,
        time_set_destroy: set_destroy,
        defer_new,
        defer_enable,
        defer_free: free,
        defer_set_destroy: set_destroy,
        quit,
    }
}

unsafe fn add(api: *mut pa_mainloop_api, kind: Kind, userdata: *mut c_void) -> *mut c_void {
    let owner = unsafe { (*api).userdata.cast::<pa_mainloop>() };
    let loop_ = unsafe { &*owner };
    let mut state = lock(&loop_.state);
    if state.closing {
        return core::ptr::null_mut();
    }
    let Some(id) = state.next_id.checked_add(1) else {
        return core::ptr::null_mut();
    };
    state.next_id = id;
    let mut event = Box::new(Event {
        owner: owner as usize,
        id,
        userdata: userdata as usize,
        destroy: None,
        kind,
    });
    let pointer = (&raw mut *event).cast();
    state.events.insert(id, event);
    drop(state);
    loop_.wake();
    pointer
}

unsafe fn change(pointer: *mut c_void, update: impl FnOnce(&mut Event)) {
    if pointer.is_null() {
        return;
    }
    let (owner, id) = unsafe {
        let event = &*pointer.cast::<Event>();
        (event.owner, event.id)
    };
    let loop_ = unsafe { &*(owner as *const pa_mainloop) };
    if let Some(event) = lock(&loop_.state).events.get_mut(&id) {
        update(event);
    }
    loop_.wake();
}

unsafe extern "sysv64" fn io_new(
    api: *mut pa_mainloop_api,
    fd: c_int,
    flags: c_int,
    callback: IoCallback,
    userdata: *mut c_void,
) -> *mut c_void {
    unsafe {
        add(
            api,
            Kind::Io {
                fd,
                flags,
                callback,
            },
            userdata,
        )
    }
}
unsafe extern "sysv64" fn io_enable(event: *mut c_void, enabled: c_int) {
    unsafe {
        change(event, |event| {
            if let Kind::Io { flags, .. } = &mut event.kind {
                *flags = enabled;
            }
        });
    }
}
unsafe extern "sysv64" fn time_new(
    api: *mut pa_mainloop_api,
    tv: *const Timeval,
    callback: TimeCallback,
    userdata: *mut c_void,
) -> *mut c_void {
    unsafe {
        add(
            api,
            Kind::Time {
                deadline: tv.as_ref().copied(),
                callback,
            },
            userdata,
        )
    }
}
unsafe extern "sysv64" fn time_restart(event: *mut c_void, tv: *const Timeval) {
    let value = unsafe { tv.as_ref().copied() };
    unsafe {
        change(event, |event| {
            if let Kind::Time { deadline, .. } = &mut event.kind {
                *deadline = value;
            }
        });
    }
}
unsafe extern "sysv64" fn defer_new(
    api: *mut pa_mainloop_api,
    callback: DeferCallback,
    userdata: *mut c_void,
) -> *mut c_void {
    unsafe {
        add(
            api,
            Kind::Defer {
                enabled: true,
                callback,
            },
            userdata,
        )
    }
}
unsafe extern "sysv64" fn defer_enable(event: *mut c_void, value: c_int) {
    unsafe {
        change(event, |event| {
            if let Kind::Defer { enabled, .. } = &mut event.kind {
                *enabled = value != 0;
            }
        });
    }
}
unsafe extern "sysv64" fn set_destroy(event: *mut c_void, callback: Option<DestroyCallback>) {
    unsafe {
        change(event, |event| event.destroy = callback);
    }
}
unsafe extern "sysv64" fn free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    let (owner, id) = unsafe {
        let event = &*pointer.cast::<Event>();
        (event.owner, event.id)
    };
    let loop_ = unsafe { &*(owner as *const pa_mainloop) };
    let event = lock(&loop_.state).events.remove(&id);
    loop_.wake();
    if let Some(event) = event {
        unsafe {
            destroy(event);
        }
    }
}
pub(super) unsafe fn destroy(mut event: Box<Event>) {
    if let Some(callback) = event.destroy {
        let api = unsafe { pa_mainloop_get_api(event.owner as *mut pa_mainloop) };
        unsafe {
            callback(api, (&raw mut *event).cast(), event.userdata as *mut c_void);
        }
    }
}
unsafe extern "sysv64" fn quit(api: *mut pa_mainloop_api, retval: c_int) {
    unsafe {
        pa_mainloop_quit((*api).userdata.cast(), retval);
    }
}

struct Once {
    callback: OnceCallback,
    userdata: usize,
    _lease: lifecycle::Lease,
}
unsafe extern "sysv64" fn once_callback(
    api: *mut pa_mainloop_api,
    event: *mut c_void,
    userdata: *mut c_void,
) {
    let once = unsafe { &*userdata.cast::<Once>() };
    let (callback, user) = (once.callback, once.userdata);
    unsafe {
        ((*api).defer_enable)(event, 0);
        callback(api, user as *mut c_void);
        ((*api).defer_free)(event);
    }
}
unsafe extern "sysv64" fn once_destroy(
    _api: *mut pa_mainloop_api,
    _event: *mut c_void,
    userdata: *mut c_void,
) {
    unsafe {
        drop(Box::from_raw(userdata.cast::<Once>()));
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_api_once")]
pub unsafe extern "sysv64" fn pa_mainloop_api_once(
    api: *mut pa_mainloop_api,
    callback: OnceCallback,
    userdata: *mut c_void,
) {
    let once = Box::into_raw(Box::new(Once {
        callback,
        userdata: userdata as usize,
        _lease: lifecycle::Lease::new(),
    }));
    let event = unsafe { ((*api).defer_new)(api, once_callback, once.cast()) };
    if event.is_null() {
        unsafe {
            drop(Box::from_raw(once));
        }
    } else {
        unsafe {
            ((*api).defer_set_destroy)(event, Some(once_destroy));
        }
    }
}
