//! Owned Xlib request buffers and replies for ELF extension client libraries.
use crate::xkb::server::{put16, u16_at};
use crate::{Display, errors::ProtocolError};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
#[derive(Default)]
struct State {
    buffer: Vec<u8>,
    active: usize,
    /// Serial of the request in `buffer`, for the reply's sequence number.
    serial: usize,
    replies: VecDeque<Vec<u8>>,
    body: Vec<u8>,
    cursor: usize,
}
/// Core `GetInputFocus`; Xlib's `XSync` and GDK's asynchronous round trips
/// use it as a no-op request that is answered in order.
const GET_INPUT_FOCUS: u8 = 43;
struct QueuedEvent {
    wire: [u8; 32],
    delivery: Option<(usize, u32)>,
    display: usize,
}
static EVENTS: Mutex<VecDeque<QueuedEvent>> = Mutex::new(VecDeque::new());
type PollHook = fn(*mut Display, bool);
static POLL_HOOKS: Mutex<Vec<PollHook>> = Mutex::new(Vec::new());
pub fn register_poll(hook: PollHook) {
    let mut hooks = POLL_HOOKS.lock().unwrap_or_else(|e| e.into_inner());
    if !hooks.iter().any(|h| *h as usize == hook as usize) {
        hooks.push(hook);
    }
}
pub fn poll(display: *mut Display, close: bool) {
    let hooks = POLL_HOOKS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for hook in hooks {
        hook(display, close);
    }
}
pub fn queue_event(event: [u8; 32]) {
    queue_delivery(event, None);
}
pub fn queue_delivery(wire: [u8; 32], delivery: Option<(usize, u32)>) {
    EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(QueuedEvent {
            wire,
            delivery,
            display: 0,
        });
    crate::notify_event();
}
pub fn queue_display(wire: [u8; 32], display: *mut Display) {
    EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(QueuedEvent {
            wire,
            delivery: None,
            display: display as usize,
        });
    crate::notify_event();
}
pub fn next_event() -> Option<[u8; 32]> {
    EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
        .map(|event| event.wire)
}
pub fn try_next_event() -> Result<Option<[u8; 32]>, i32> {
    Ok(EVENTS
        .try_lock()
        .map_err(|_| 11)?
        .pop_front()
        .map(|event| event.wire))
}
pub fn drain_events(display: *mut Display) {
    loop {
        let Some(queued) = EVENTS.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else {
            break;
        };
        let mut event = queued.wire;
        if queued.display != 0 {
            unsafe {
                crate::event_wire::_XEnq(queued.display as *mut Display, event.as_mut_ptr());
            }
            continue;
        }
        let mut decoded: crate::XEvent = unsafe { core::mem::zeroed() };
        unsafe {
            crate::event_wire::_XWireToEvent(display, &mut decoded, event.as_mut_ptr());
        }
        let recipients = queued.delivery.map_or_else(
            || crate::connection::event_recipients(&decoded),
            |(window, mask)| crate::connection::recipients(window, mask),
        );
        if recipients.is_empty() {
            unsafe {
                crate::event_wire::_XEnq(display, event.as_mut_ptr());
            }
        } else {
            for recipient in recipients {
                unsafe {
                    crate::event_wire::_XEnq(recipient, event.as_mut_ptr());
                }
            }
        }
    }
}
static STATE: Mutex<BTreeMap<usize, State>> = Mutex::new(BTreeMap::new());
pub fn close(display: *mut Display) {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(display as usize));
    EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|event| event.display != display as usize);
    crate::damage::close(display);
    crate::shape::close(display);
}
pub fn dispatch(display: *mut Display, req: &[u8]) -> Result<Option<Vec<u8>>, ProtocolError> {
    if req.len() < 4 {
        return Err(ProtocolError {
            code: 16,
            request: 0,
            minor: 0,
            resource: req.len(),
        });
    }
    match req[0] {
        GET_INPUT_FOCUS => {
            let (mut focus, mut revert) = (0usize, 0i32);
            unsafe { crate::focus::XGetInputFocus(display, &raw mut focus, &raw mut revert) };
            let mut reply = vec![0u8; 32];
            reply[0] = 1;
            reply[1] = revert as u8;
            reply[8..12].copy_from_slice(&(focus as u32).to_ne_bytes());
            Ok(Some(reply))
        }
        crate::xkb::server::OPCODE => crate::xkb::server::dispatch(req[1], req),
        crate::composite::OPCODE => crate::composite::dispatch(display, req[1], req),
        crate::damage::OPCODE => crate::damage::dispatch(display, req[1], req),
        crate::xfixes::OPCODE => crate::xfixes::dispatch(display, req[1], req),
        crate::shape::OPCODE => crate::shape::dispatch(display, req[1], req),
        _ => {
            if crate::trace_enabled() {
                crate::diagnostic!(
                    "[libX11] raw request opcode={}.{} len={} has no native handler",
                    req[0],
                    req[1],
                    req.len()
                );
            }
            Err(ProtocolError {
                code: 1,
                request: req[0],
                minor: req[1],
                resource: 0,
            })
        }
    }
}
/// Offers a 32-byte reply or error to the display's Xlib async handlers, as
/// `_XReply` does before matching it against the request being awaited.
/// Returns whether a handler consumed it.
unsafe fn offer_async(display: *mut Display, serial: usize, message: &mut [u8; 32]) -> bool {
    if display.is_null() {
        return false;
    }
    // Handlers compare against the sequence of the message being read.
    unsafe {
        (*display).last_request_read = serial;
    }
    let mut cursor = unsafe { (*display).async_handlers };
    while !cursor.is_null() {
        let handler = unsafe { &*cursor };
        let next = handler.next;
        if let Some(callback) = handler.handler
            && unsafe {
                callback(
                    display,
                    message.as_mut_ptr(),
                    message.as_mut_ptr(),
                    32,
                    handler.data,
                )
            } != 0
        {
            return true;
        }
        cursor = next;
    }
    false
}
pub unsafe fn flush(display: *mut Display) {
    let (request, serial) = {
        let mut states = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let s = states.entry(display as usize).or_default();
        if s.active == 0 {
            return;
        }
        let n = (u16_at(&s.buffer, 2) as usize * 4)
            .max(s.active)
            .min(s.buffer.len());
        let r = s.buffer[..n].to_vec();
        s.active = 0;
        (r, s.serial)
    };
    let result = dispatch(display, &request);
    match result {
        Ok(Some(mut r)) => {
            put16(&mut r, 2, serial as u16);
            let mut head = [0u8; 32];
            head.copy_from_slice(&r[..32]);
            if unsafe { offer_async(display, serial, &mut head) } {
                return;
            }
            STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(display as usize)
                .or_default()
                .replies
                .push_back(r);
        }
        Ok(None) => {}
        Err(e) => {
            // xError: type 0, code, sequence, resource, minor, major.
            let mut error = [0u8; 32];
            error[1] = e.code;
            put16(&mut error, 2, serial as u16);
            error[4..8].copy_from_slice(&(e.resource as u32).to_ne_bytes());
            put16(&mut error, 8, e.minor as u16);
            error[10] = e.request;
            if unsafe { offer_async(display, serial, &mut error) } {
                return;
            }
            unsafe {
                crate::errors::report_serial(
                    display,
                    e.code,
                    e.request,
                    e.minor,
                    e.resource,
                    serial as u64,
                );
            }
        }
    };
}
pub unsafe fn allocate(display: *mut Display, kind: u8, len: usize) -> *mut core::ffi::c_void {
    unsafe { flush(display) };
    if display.is_null() || len < 4 || len > 262140 || len % 4 != 0 {
        return core::ptr::null_mut();
    }
    let mut states = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = states.entry(display as usize).or_default();
    s.buffer.resize(262140, 0);
    s.buffer[..len].fill(0);
    s.active = len;
    s.buffer[0] = kind;
    put16(&mut s.buffer, 2, (len / 4) as u16);
    let p = s.buffer.as_mut_ptr();
    unsafe {
        (*display).request += 1;
        s.serial = (*display).request;
        (*display).last_req = p.cast();
        (*display).buffer = p.cast();
        (*display).bufptr = p.add(len).cast();
        (*display).bufmax = p.add(s.buffer.len()).cast();
    }
    p.cast()
}
pub unsafe fn append(display: *mut Display, bytes: &[u8]) {
    let mut states = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = states.entry(display as usize).or_default();
    if s.active == 0 {
        return;
    }
    let mut start = s.active;
    if !display.is_null() {
        let ptr = unsafe { (*display).bufptr } as usize;
        let base = s.buffer.as_ptr() as usize;
        if ptr >= base && ptr <= base + s.buffer.len() {
            start = start.max(ptr - base);
        }
    }
    let Some(end) = start.checked_add(bytes.len()) else {
        return;
    };
    if end > s.buffer.len() {
        return;
    }
    s.buffer[start..end].copy_from_slice(bytes);
    s.active = end.div_ceil(4) * 4;
    if !display.is_null() {
        unsafe {
            (*display).bufptr = s.buffer.as_mut_ptr().add(s.active).cast();
        }
    }
}
pub unsafe fn receive(
    display: *mut Display,
    out: *mut core::ffi::c_void,
    extra: i32,
    discard: bool,
) -> i32 {
    unsafe { flush(display) };
    if out.is_null() || extra < 0 {
        return 0;
    }
    let mut states = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = states.entry(display as usize).or_default();
    let Some(reply) = s.replies.pop_front() else {
        return 0;
    };
    let count = 32usize.saturating_add(extra as usize * 4);
    if count > reply.len() {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(reply.as_ptr(), out.cast(), count);
    }
    s.body = if discard {
        Vec::new()
    } else {
        reply[count..].to_vec()
    };
    s.cursor = 0;
    1
}
pub unsafe fn read(display: *mut Display, out: *mut u8, count: usize, padded: bool) {
    let mut states = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = states.entry(display as usize).or_default();
    let n = count.min(s.body.len().saturating_sub(s.cursor));
    if !out.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(s.body.as_ptr().add(s.cursor), out, n);
            core::ptr::write_bytes(out.add(n), 0, count - n);
        }
    }
    s.cursor = (s.cursor + if padded { count.div_ceil(4) * 4 } else { count }).min(s.body.len());
}
pub unsafe extern "sysv64" fn allocate_id(_display: *mut Display) -> usize {
    crate::graphics::allocate_drawable_id()
}
