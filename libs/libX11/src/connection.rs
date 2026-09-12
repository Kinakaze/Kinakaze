//! Lifetime of the shared native display connection and its poll descriptor.
//!
//! Toolkits briefly open another display while initializing extensions. Closing
//! that reference must leave the original connection's event source registered.
use libc::fdio;
use std::collections::BTreeMap;
use std::sync::Mutex;

mod lifecycle;

thread_local! {
    static REQUEST_DISPLAY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
pub struct RequestDisplay(usize);
impl Drop for RequestDisplay {
    fn drop(&mut self) {
        REQUEST_DISPLAY.set(self.0);
    }
}
/// XCB requests may be issued on an Xlib-owned queue (Chromium does this).
/// Request routing follows the opaque connection, independently of event ownership.
pub fn scope_xcb(connection: *mut core::ffi::c_void) -> RequestDisplay {
    let display = if CONNECTION
        .lock()
        .unwrap()
        .displays
        .contains_key(&(connection as usize))
    {
        connection as usize
    } else {
        0
    };
    RequestDisplay(REQUEST_DISPLAY.replace(display))
}

struct Connection {
    users: usize,
    fd: i32,
    displays: BTreeMap<usize, Endpoint>,
}
struct Endpoint {
    fd: i32,
    references: usize,
    masks: BTreeMap<usize, u32>,
    xcb_events: bool,
}

static CONNECTION: Mutex<Connection> = Mutex::new(Connection {
    users: 0,
    fd: -1,
    displays: BTreeMap::new(),
});

pub fn xcb_events() -> bool {
    CONNECTION
        .lock()
        .unwrap()
        .displays
        .values()
        .any(|d| d.xcb_events)
}
pub fn try_xcb_events() -> Result<bool, i32> {
    CONNECTION
        .try_lock()
        .map(|s| s.displays.values().any(|d| d.xcb_events))
        .map_err(|_| 11)
}
pub fn xcb_events_for(display: *mut crate::Display) -> bool {
    CONNECTION
        .lock()
        .unwrap()
        .displays
        .get(&(display as usize))
        .is_some_and(|d| d.xcb_events)
}
/// Display used by the native XCB frontend. Other Xlib clients retain their
/// independent queues, masks and notification descriptors.
pub fn xcb_display() -> *mut crate::Display {
    let scoped = REQUEST_DISPLAY.get();
    if scoped != 0 {
        return scoped as *mut crate::Display;
    }
    CONNECTION
        .lock()
        .unwrap()
        .displays
        .iter()
        .find_map(|(&display, d)| d.xcb_events.then_some(display as *mut crate::Display))
        .unwrap_or(crate::shared_display())
}
pub fn try_xcb_display() -> Result<*mut crate::Display, i32> {
    let scoped = REQUEST_DISPLAY.get();
    if scoped != 0 {
        return Ok(scoped as *mut crate::Display);
    }
    let connection = CONNECTION.try_lock().map_err(|_| 11)?;
    Ok(connection
        .displays
        .iter()
        .find_map(|(&display, d)| d.xcb_events.then_some(display as *mut crate::Display))
        .unwrap_or(crate::shared_display()))
}
pub fn event_fd() -> i32 {
    let connection = CONNECTION.lock().unwrap();
    if let Some(endpoint) = connection.displays.get(&REQUEST_DISPLAY.get()) {
        return endpoint.fd;
    }
    connection
        .displays
        .values()
        .find(|d| d.xcb_events)
        .map_or(connection.fd, |d| d.fd)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetEventQueueOwner")]
pub unsafe extern "sysv64" fn XSetEventQueueOwner(display: *mut crate::Display, owner: i32) {
    if display.is_null() || !(0..=1).contains(&owner) {
        return;
    }
    if let Some(endpoint) = CONNECTION
        .lock()
        .unwrap()
        .displays
        .get_mut(&(display as usize))
    {
        endpoint.xcb_events = owner == 1;
    }
    if crate::trace_enabled() {
        crate::diagnostic!("[libX11] XSetEventQueueOwner display={display:p} owner={owner}");
    }
    notify();
}

pub(super) fn open() -> *mut crate::Display {
    let mut connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    let fd = unsafe { fdio::kinakaze_abi_eventfd(0, fdio::EFD_NONBLOCK | fdio::EFD_CLOEXEC) };
    if fd < 0 {
        return core::ptr::null_mut();
    }
    let display = if connection.users == 0 {
        connection.fd = fd;
        unsafe { crate::GLOBAL_DISPLAY.fd = fd };
        unsafe {
            crate::GLOBAL_XCB_PRIVATE.connection = crate::shared_display().cast();
        }
        kinakaze_libdisplay::event::register_notifier(crate::notify_x11_event);
        crate::shared_display()
    } else {
        unsafe {
            // Guest allocation preserves the public Display address across fork.
            let display = kinakaze_alloc::guest::malloc(core::mem::size_of::<crate::Display>())
                .cast::<crate::Display>();
            let screen = kinakaze_alloc::guest::malloc(core::mem::size_of::<crate::Screen>())
                .cast::<crate::Screen>();
            let xcb = kinakaze_alloc::guest::malloc(core::mem::size_of::<crate::X11XCBPrivate>())
                .cast::<crate::X11XCBPrivate>();
            if display.is_null() || screen.is_null() || xcb.is_null() {
                kinakaze_alloc::guest::free(display.cast());
                kinakaze_alloc::guest::free(screen.cast());
                kinakaze_alloc::guest::free(xcb.cast());
                libc::kinakaze_abi_close(fd);
                return core::ptr::null_mut();
            }
            core::ptr::copy_nonoverlapping(crate::shared_display(), display, 1);
            *screen = crate::GLOBAL_SCREEN;
            (*screen).display = display;
            (*display).screens = screen;
            core::ptr::copy_nonoverlapping(&raw const crate::GLOBAL_XCB_PRIVATE, xcb, 1);
            (*xcb).connection = display.cast();
            (*display).xcb = xcb;
            (*display).fd = fd;
            (*display).qlen = 0;
            (*display).request = 0;
            (*display).last_request_read = 0;
            (*display).async_handlers = core::ptr::null_mut();
            display
        }
    };
    connection.displays.insert(
        display as usize,
        Endpoint {
            fd,
            references: 1,
            masks: BTreeMap::new(),
            xcb_events: false,
        },
    );
    connection.users += 1;
    drop(connection);
    crate::shared::open();
    display
}

/// Release one open reference, returning whether this was the last one.
pub fn retain_shared() -> *mut crate::Display {
    let mut connection = CONNECTION.lock().unwrap();
    if let Some(endpoint) = connection
        .displays
        .get_mut(&(crate::shared_display() as usize))
    {
        endpoint.references += 1;
        connection.users += 1;
        return crate::shared_display();
    }
    drop(connection);
    unsafe { crate::XOpenDisplay(core::ptr::null()) }
}
pub(super) fn close(display: *mut crate::Display) -> Option<bool> {
    let mut connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    let endpoint = connection.displays.get_mut(&(display as usize))?;
    if endpoint.references > 1 {
        endpoint.references -= 1;
        connection.users -= 1;
        return None;
    }
    let endpoint = connection.displays.remove(&(display as usize)).unwrap();
    libc::kinakaze_abi_close(endpoint.fd);
    for window in endpoint.masks.keys() {
        let mask = connection.displays.values().fold(0, |bits, d| {
            bits | d.masks.get(window).copied().unwrap_or(0)
        });
        let _ = crate::shared::subscribe(*window, mask);
    }
    connection.users -= 1;
    if connection.users != 0 {
        return Some(false);
    }
    kinakaze_libdisplay::event::unregister_notifier(crate::notify_x11_event);
    connection.fd = -1;
    unsafe { crate::GLOBAL_DISPLAY.fd = -1 };
    drop(connection);
    crate::shared::close();
    Some(true)
}

pub fn notify() {
    // Serialize with the final close so an in-flight UI callback cannot write
    // to a descriptor which another guest thread has already reused.
    let connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    for endpoint in connection.displays.values() {
        unsafe { fdio::kinakaze_abi_eventfd_write(endpoint.fd, 1) };
    }
}

pub fn drain() {
    drain_display(xcb_display());
}
pub fn drain_display(display: *mut crate::Display) {
    let connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(endpoint) = connection.displays.get(&(display as usize)) {
        let mut count = 0;
        unsafe { fdio::kinakaze_abi_eventfd_read(endpoint.fd, &raw mut count) };
    }
}
pub fn selected(display: *mut crate::Display, window: usize) -> u32 {
    CONNECTION
        .lock()
        .unwrap()
        .displays
        .get(&(display as usize))
        .and_then(|endpoint| endpoint.masks.get(&window).copied())
        .unwrap_or(0)
}
pub fn select(display: *mut crate::Display, window: usize, mask: u32) -> Result<u32, u8> {
    let mut connection = CONNECTION.lock().unwrap();
    let mut merged = mask;
    for (&other, endpoint) in &connection.displays {
        if other == display as usize {
            continue;
        }
        let bits = endpoint.masks.get(&window).copied().unwrap_or(0);
        if bits & mask & ((1 << 20) | (1 << 18) | (1 << 2)) != 0 {
            return Err(10);
        }
        merged |= bits;
    }
    crate::shared::subscribe(window, merged)?;
    if let Some(endpoint) = connection.displays.get_mut(&(display as usize)) {
        if mask == 0 {
            endpoint.masks.remove(&window);
        } else {
            endpoint.masks.insert(window, mask);
        }
    }
    Ok(merged)
}
pub fn displays() -> impl Iterator<Item = *mut crate::Display> {
    let connection = CONNECTION.lock().unwrap();
    let mut inline = [0usize; 8];
    let overflow = if connection.displays.len() <= inline.len() {
        for (slot, &display) in inline.iter_mut().zip(connection.displays.keys()) {
            *slot = display;
        }
        None
    } else {
        Some(connection.displays.keys().copied().collect::<Vec<_>>())
    };
    inline
        .into_iter()
        .filter(|d| *d != 0)
        .chain(overflow.into_iter().flatten())
        .map(|d| d as *mut crate::Display)
}
pub fn recipients(window: usize, mask: u32) -> Vec<*mut crate::Display> {
    CONNECTION
        .lock()
        .unwrap()
        .displays
        .iter()
        .filter_map(|(&display, endpoint)| {
            (endpoint.masks.get(&window).copied().unwrap_or(0) & mask != 0)
                .then_some(display as *mut crate::Display)
        })
        .collect()
}
pub fn event_recipients(event: &crate::XEvent) -> Vec<*mut crate::Display> {
    let kind = unsafe { event.r#type };
    let mask = match kind {
        2..=5 => 1 << (kind - 2),
        6 => 1 << 6,
        7 => 1 << 4,
        8 => 1 << 5,
        9 | 10 => 1 << 21,
        12..=14 => 1 << 15,
        15 => 1 << 16,
        16 => 1 << 19,
        17..=19 | 21 | 22 => {
            if unsafe { event.xmap.event == event.xmap.window } {
                1 << 17
            } else {
                1 << 19
            }
        }
        20 | 23 | 27 => 1 << 20,
        28 => 1 << 22,
        _ => return Vec::new(),
    };
    recipients(unsafe { event.xany.window }, mask)
}
