//! Ordered cookies and owned protocol replies. Only returned buffers use guest
//! memory; queued data is private and serialized across fork.
use super::{c_void, x11, xcb_generic_error_t};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
mod lifecycle;

pub(super) struct Store {
    next: u32,
    pending: BTreeMap<u32, Vec<u8>>,
    events: VecDeque<Vec<u8>>,
}
pub(super) static STORE: Mutex<Store> = Mutex::new(Store {
    next: 1,
    pending: BTreeMap::new(),
    events: VecDeque::new(),
});

pub fn reply(extra: usize) -> Vec<u8> {
    let mut bytes = vec![0; 32 + extra.div_ceil(4) * 4];
    bytes[0] = 1;
    put32(&mut bytes, 4, extra.div_ceil(4) as u32);
    bytes
}
pub fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
}
pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}
pub fn error(code: u8, request: u8, resource: u32) -> x11::errors::ProtocolError {
    x11::errors::ProtocolError {
        code,
        request,
        minor: 0,
        resource: resource as usize,
    }
}

pub fn submit(
    checked: bool,
    has_reply: bool,
    operation: impl FnOnce() -> Result<Option<Vec<u8>>, x11::errors::ProtocolError>,
) -> u32 {
    let mut store = STORE.lock().unwrap();
    let sequence = store.next;
    store.next = store.next.wrapping_add(1).max(1);
    match operation() {
        Ok(Some(mut bytes)) => {
            put16(&mut bytes, 2, sequence as u16);
            store.pending.insert(sequence, bytes);
        }
        Ok(None) => {}
        Err(error) => {
            let mut bytes = vec![0; 36];
            bytes[1] = error.code;
            put16(&mut bytes, 2, sequence as u16);
            put32(&mut bytes, 4, error.resource as u32);
            put16(&mut bytes, 8, error.minor as u16);
            bytes[10] = error.request;
            put32(&mut bytes, 32, sequence);
            if checked {
                store.pending.insert(sequence, bytes);
            } else {
                store.events.push_back(bytes);
                x11::notify_event();
                if has_reply {
                    store.pending.insert(sequence, Vec::new());
                }
            }
        }
    }
    sequence
}
pub fn command(checked: bool, operation: impl FnOnce()) -> super::xcb_void_cookie_t {
    let sequence = submit(checked, false, || {
        x11::errors::capture(operation).map(|()| None)
    });
    super::xcb_void_cookie_t { sequence }
}
pub unsafe fn copy_guest(bytes: &[u8]) -> *mut c_void {
    if bytes.is_empty() {
        return core::ptr::null_mut();
    }
    let output = unsafe { kinakaze_alloc::c::malloc(bytes.len()) };
    if !output.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    }
    output.cast()
}
pub unsafe fn take(sequence: u32, error: *mut *mut xcb_generic_error_t) -> *mut c_void {
    if !error.is_null() {
        unsafe { error.write(core::ptr::null_mut()) };
    }
    let Some(bytes) = STORE.lock().unwrap().pending.remove(&sequence) else {
        return core::ptr::null_mut();
    };
    if bytes.first() == Some(&0) {
        if !error.is_null() {
            unsafe { error.write(copy_guest(&bytes).cast()) };
        }
        core::ptr::null_mut()
    } else {
        unsafe { copy_guest(&bytes) }
    }
}
pub fn discard(sequence: u32) {
    STORE.lock().unwrap().pending.remove(&sequence);
}
pub fn ready(sequence: u32) -> bool {
    STORE.lock().unwrap().pending.contains_key(&sequence)
}
pub unsafe fn event() -> *mut super::xcb_generic_event_t {
    let bytes = STORE.lock().unwrap().events.pop_front();
    bytes.map_or(core::ptr::null_mut(), |bytes| unsafe {
        copy_guest(&bytes).cast()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookies_retain_independent_results_and_checked_errors() {
        let first = submit(true, true, || {
            let mut r = reply(0);
            put32(&mut r, 8, 71);
            Ok(Some(r))
        });
        let second = submit(true, true, || Err(error(5, 17, 92)));
        assert_ne!(first, second);
        let mut failure = core::ptr::null_mut();
        assert!(unsafe { take(second, &raw mut failure) }.is_null());
        assert!(!failure.is_null());
        unsafe {
            assert_eq!(((*failure).error_code, (*failure).resource_id), (5, 92));
            kinakaze_alloc::c::free(failure.cast());
        }
        let value = unsafe { take(first, &raw mut failure) };
        assert!(!value.is_null() && failure.is_null());
        unsafe {
            assert_eq!(value.cast::<u32>().add(2).read(), 71);
            kinakaze_alloc::c::free(value.cast());
        }
        assert!(!ready(first) && !ready(second));
    }
}
