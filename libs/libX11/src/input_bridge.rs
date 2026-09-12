//! The core event queue borrows extension hooks; XInput owns cookie layout and data.
use crate::{Display, XEvent};
use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

type Translate = unsafe extern "system" fn(
    *mut Display,
    *const kinakaze_libdisplay::event::Event,
    *mut XEvent,
) -> i32;
type Release = unsafe extern "system" fn(*mut c_void);
type Duplicate = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
static TRANSLATE: AtomicUsize = AtomicUsize::new(0);
static RELEASE: AtomicUsize = AtomicUsize::new(0);
static DUPLICATE: AtomicUsize = AtomicUsize::new(0);

/// The extension is retained by the native loader for this process's lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_x11_raw_input_hooks(
    translate: Translate,
    release: Release,
    duplicate: Duplicate,
) {
    DUPLICATE.store(duplicate as usize, Ordering::Release);
    RELEASE.store(release as usize, Ordering::Release);
    TRANSLATE.store(translate as usize, Ordering::Release);
}

/// A peek or put-back event owns an independent cookie payload. Freeing that
/// cookie must not invalidate the copy still waiting in the display queue.
pub(crate) fn duplicate_event(event: XEvent) -> Option<XEvent> {
    let mut copy = event;
    if unsafe { event.r#type } != crate::GenericEvent || unsafe { event.xcookie.data }.is_null() {
        return Some(copy);
    }
    let address = DUPLICATE.load(Ordering::Acquire);
    if address == 0 {
        return None;
    }
    let callback: Duplicate = unsafe { core::mem::transmute(address) };
    let data = unsafe { callback(event.xcookie.data) };
    if data.is_null() {
        return None;
    }
    copy.xcookie.data = data;
    Some(copy)
}

pub(crate) unsafe fn free_event_data(data: *mut c_void) {
    let address = RELEASE.load(Ordering::Acquire);
    if address != 0 {
        let callback: Release = unsafe { core::mem::transmute(address) };
        unsafe { callback(data) };
    }
}

/// Dispatch to each connection before allowing the core fallback. Polling on
/// Mutter's backend connection must not consume GDK's XI2 events (or vice versa).
pub(crate) fn dispatch(source: &kinakaze_libdisplay::event::Event) -> bool {
    let address = TRANSLATE.load(Ordering::Acquire);
    if address == 0 {
        return false;
    }
    let callback: Translate = unsafe { core::mem::transmute(address) };
    let mut handled = false;
    for display in crate::connection::displays() {
        let mut event = core::mem::MaybeUninit::uninit();
        match unsafe { callback(display, source, event.as_mut_ptr()) } {
            1 => {
                if crate::input_trace_enabled() {
                    crate::diagnostic!(
                        "[input] at={} event_at={} display={display:p} kind={} window={} xy={},{}",
                        crate::focus::now(),
                        source.time_ms,
                        source.kind,
                        source.window,
                        source.x,
                        source.y
                    );
                }
                crate::queue_extension_event(display, unsafe { event.assume_init() });
                handled = true;
            }
            2 => handled = true,
            _ => {}
        }
    }
    handled
}
