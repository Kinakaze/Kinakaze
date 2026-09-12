//! XKEYBOARD state and protocol for the native virtual core keyboard (XI id 3).
//! The keycode space is the same evdev + 8 space used by native input events.
use crate::errors::ProtocolError;
use std::sync::Mutex;

pub const OPCODE: u8 = 132;
pub const EVENT: u8 = 80;
pub const ERROR: u8 = 144;
pub const DEVICE: u8 = 3;
pub const MIN_KEY: u8 = 8;
pub const MAX_KEY: u8 = 255;
pub const TYPE_NAMES: [&str; 4] = ["ONE_LEVEL", "TWO_LEVEL", "ALPHABETIC", "KEYPAD"];
pub const TYPE_LEVELS: [u8; 4] = [1, 2, 2, 2];

#[derive(Default)]
struct State {
    locked: u8,
    lock_override: u8,
    latched: u8,
    group: u8,
    selected: u16,
    client_flags: u32,
    previous: Option<super::XkbStateRec>,
}
static STATE: Mutex<State> = Mutex::new(State {
    locked: 0,
    lock_override: 0,
    latched: 0,
    group: 0,
    selected: 0,
    client_flags: 0,
    previous: None,
});

pub fn valid_device(device: u16) -> bool {
    matches!(device, 3 | 5 | 0x100)
}
pub fn atom(name: &str) -> u32 {
    let name = std::ffi::CString::new(name).unwrap();
    unsafe { crate::XInternAtom(crate::shared_display(), name.as_ptr(), 0) as u32 }
}
pub fn modifier(key: u8) -> u8 {
    crate::keyboard::MODIFIERS
        .chunks_exact(2)
        .enumerate()
        .find(|(_, keys)| key != 0 && keys.contains(&key))
        .map_or(0, |(i, _)| 1 << i)
}
pub fn symbols(key: u8) -> [u32; 2] {
    let base = crate::linux_keycode_to_keysym(key as u32) as u32;
    let shifted = match base {
        0x61..=0x7a => base - 32,
        0x31 => 0x21,
        0x32 => 0x40,
        0x33 => 0x23,
        0x34 => 0x24,
        0x35 => 0x25,
        0x36 => 0x5e,
        0x37 => 0x26,
        0x38 => 0x2a,
        0x39 => 0x28,
        0x30 => 0x29,
        0x2d => 0x5f,
        0x3d => 0x2b,
        0x5b => 0x7b,
        0x5d => 0x7d,
        0x5c => 0x7c,
        0x3b => 0x3a,
        0x27 => 0x22,
        0x60 => 0x7e,
        0x2c => 0x3c,
        0x2e => 0x3e,
        0x2f => 0x3f,
        _ => base,
    };
    [base, shifted]
}
pub fn key_type(key: u8) -> u8 {
    let [base, shifted] = symbols(key);
    if (0x61..=0x7a).contains(&base) {
        2
    } else if base != shifted {
        1
    } else {
        0
    }
}
pub fn key_name(key: u8) -> [u8; 4] {
    let k = key as usize;
    [
        b'K',
        b'0' + (k / 100) as u8,
        b'0' + ((k / 10) % 10) as u8,
        b'0' + (k % 10) as u8,
    ]
}
pub fn state() -> super::XkbStateRec {
    let host = crate::keyboard::pointer_mask();
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let locked = ((host as u8) & 0x32 & !s.lock_override) | (s.locked & s.lock_override);
    let base = host as u8 & !0x32;
    let mods = base | locked | s.latched;
    super::XkbStateRec {
        group: s.group,
        locked_group: s.group,
        base_group: 0,
        latched_group: 0,
        mods,
        base_mods: base,
        latched_mods: s.latched,
        locked_mods: locked,
        compat_state: mods,
        grab_mods: mods,
        compat_grab_mods: mods,
        lookup_mods: mods,
        compat_lookup_mods: mods,
        ptr_buttons: (host & 0x1f00) as u16,
    }
}
pub fn lock_mods(affect: u8, values: u8, latch: bool) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if latch {
        s.latched = (s.latched & !affect) | (values & affect);
    } else {
        s.locked = (s.locked & !affect) | (values & affect);
        s.lock_override |= affect;
    }
    drop(s);
    poll(crate::shared_display(), false);
}
pub fn select(affect: u16, values: u16) {
    crate::xext::protocol::register_poll(poll);
    unsafe {
        crate::event_wire::XESetWireToEvent(
            crate::shared_display(),
            EVENT as i32,
            Some(decode_state),
        );
    }
    let now = state();
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.selected = (s.selected & !affect) | (values & affect);
    s.previous = Some(now);
}

#[repr(C)]
struct StateEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut crate::Display,
    time: u64,
    xkb_type: i32,
    device: i32,
    changed: u32,
    group: i32,
    base_group: i32,
    latched_group: i32,
    locked_group: i32,
    mods: u32,
    base_mods: u32,
    latched_mods: u32,
    locked_mods: u32,
    compat_state: i32,
    grab_mods: u8,
    compat_grab_mods: u8,
    lookup_mods: u8,
    compat_lookup_mods: u8,
    ptr_buttons: i32,
    keycode: u8,
    event_type: u8,
    req_major: u8,
    req_minor: u8,
}
unsafe extern "sysv64" fn decode_state(
    d: *mut crate::Display,
    out: *mut crate::XEvent,
    wire: *mut u8,
) -> i32 {
    if out.is_null() || wire.is_null() {
        return 0;
    }
    let b = unsafe { core::slice::from_raw_parts(wire, 32) };
    if b[0] & 127 != EVENT || b[1] != 2 {
        return 0;
    }
    unsafe {
        out.cast::<StateEvent>().write(StateEvent {
            kind: EVENT as i32,
            serial: u16_at(b, 2) as u64,
            send: (b[0] & 128 != 0) as i32,
            display: d,
            time: u32_at(b, 4) as u64,
            xkb_type: 2,
            device: b[8] as i32,
            changed: u16_at(b, 26) as u32,
            group: b[13] as i32,
            base_group: u16_at(b, 14) as i16 as i32,
            latched_group: u16_at(b, 16) as i16 as i32,
            locked_group: b[18] as i32,
            mods: b[9] as u32,
            base_mods: b[10] as u32,
            latched_mods: b[11] as u32,
            locked_mods: b[12] as u32,
            compat_state: b[19] as i32,
            grab_mods: b[20],
            compat_grab_mods: b[21],
            lookup_mods: b[22],
            compat_lookup_mods: b[23],
            ptr_buttons: u16_at(b, 24) as i32,
            keycode: b[28],
            event_type: b[29],
            req_major: b[30],
            req_minor: b[31],
        });
    }
    1
}
fn poll(_d: *mut crate::Display, close: bool) {
    if close {
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = State::default();
        return;
    }
    let now = state();
    let (selected, old) = {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let old = s.previous.replace(now);
        (s.selected, old)
    };
    if selected & 4 == 0 || old == Some(now) {
        return;
    }
    let mut b = [0; 32];
    b[0] = EVENT;
    b[1] = 2;
    b[8] = DEVICE;
    put32(&mut b, 4, kinakaze_libdisplay::event::timestamp() as u32);
    b[9..14].copy_from_slice(&[
        now.mods,
        now.base_mods,
        now.latched_mods,
        now.locked_mods,
        now.group,
    ]);
    put16(&mut b, 14, now.base_group);
    put16(&mut b, 16, now.latched_group);
    b[18] = now.locked_group;
    b[19..24].copy_from_slice(&[
        now.compat_state,
        now.grab_mods,
        now.compat_grab_mods,
        now.lookup_mods,
        now.compat_lookup_mods,
    ]);
    put16(&mut b, 24, now.ptr_buttons);
    let changed = old.map_or(0x3fff, |old| {
        u16::from(now.mods != old.mods)
            | (u16::from(now.base_mods != old.base_mods) << 1)
            | (u16::from(now.latched_mods != old.latched_mods) << 2)
            | (u16::from(now.locked_mods != old.locked_mods) << 3)
            | (u16::from(now.group != old.group) << 4)
            | (u16::from(now.ptr_buttons != old.ptr_buttons) << 13)
    });
    put16(&mut b, 26, changed);
    crate::xext::protocol::queue_event(b);
}
pub fn detectable(value: bool) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.client_flags = (s.client_flags & !1) | u32::from(value);
}
pub fn u16_at(b: &[u8], n: usize) -> u16 {
    u16::from_ne_bytes(b[n..n + 2].try_into().unwrap())
}
pub fn u32_at(b: &[u8], n: usize) -> u32 {
    u32::from_ne_bytes(b[n..n + 4].try_into().unwrap())
}
pub fn put16(b: &mut [u8], n: usize, v: u16) {
    b[n..n + 2].copy_from_slice(&v.to_ne_bytes());
}
pub fn put32(b: &mut [u8], n: usize, v: u32) {
    b[n..n + 4].copy_from_slice(&v.to_ne_bytes());
}
pub fn append32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_ne_bytes());
}
pub fn pad(b: &mut Vec<u8>) {
    b.resize(b.len().div_ceil(4) * 4, 0);
}
pub fn reply(size: usize) -> Vec<u8> {
    let mut r = vec![0; size.max(32)];
    r[0] = 1;
    r[1] = DEVICE;
    r
}
pub fn finish(mut r: Vec<u8>) -> Vec<u8> {
    pad(&mut r);
    let len = ((r.len() - 32) / 4) as u32;
    put32(&mut r, 4, len);
    r
}
fn error(minor: u8, code: u8, resource: u32) -> ProtocolError {
    ProtocolError {
        code,
        request: OPCODE,
        minor,
        resource: resource as usize,
    }
}
fn range(
    req: &[u8],
    full: u16,
    bit: u16,
    offset: usize,
    start: u8,
    count: u8,
) -> Result<(u8, u8), ProtocolError> {
    if full & bit != 0 {
        return Ok((start, count));
    }
    let (first, n) = (req[offset], req[offset + 1]);
    if n != 0 && (first < start || first as u16 + n as u16 > start as u16 + count as u16) {
        return Err(error(8, 2, first as u32));
    }
    Ok((first, n))
}
pub fn type_entries(index: u8) -> &'static [(u8, u8)] {
    match index {
        1 => &[(1, 1)],
        2 => &[(1, 1), (2, 1), (3, 0)],
        3 => &[(1, 1), (16, 1), (17, 0)],
        _ => &[],
    }
}
pub fn type_mask(index: u8) -> u8 {
    match index {
        1 => 1,
        2 => 3,
        3 => 17,
        _ => 0,
    }
}

fn get_map(req: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let full = u16_at(req, 6);
    let present = full | u16_at(req, 8);
    if present & !255 != 0 {
        return Err(error(8, 2, present as u32));
    }
    let mut r = reply(40);
    r[10] = MIN_KEY;
    r[11] = MAX_KEY;
    put16(&mut r, 12, present);
    r[16] = 4;
    if present & 1 != 0 {
        let (first, n) = range(req, full, 1, 10, 0, 4)?;
        r[14] = first;
        r[15] = n;
        for i in first..first + n {
            let mask = type_mask(i);
            let entries = type_entries(i);
            r.extend_from_slice(&[
                mask,
                mask,
                0,
                0,
                TYPE_LEVELS[i as usize],
                entries.len() as u8,
                0,
                0,
            ]);
            for &(mods, level) in entries {
                r.extend_from_slice(&[1, mods, level, mods, 0, 0, 0, 0]);
            }
        }
    }
    if present & 2 != 0 {
        let (first, n) = range(req, full, 2, 12, MIN_KEY, 248)?;
        r[17] = first;
        r[20] = n;
        let mut total = 0;
        for k in first as u16..first as u16 + n as u16 {
            let k = k as u8;
            let typ = key_type(k);
            let width = TYPE_LEVELS[typ as usize];
            r.extend_from_slice(&[typ, 0, 0, 0, 1, width, width, 0]);
            for sym in &symbols(k)[..width as usize] {
                append32(&mut r, *sym);
                total += 1;
            }
        }
        put16(&mut r, 18, total);
    }
    if present & 16 != 0 {
        let (first, n) = range(req, full, 16, 14, MIN_KEY, 248)?;
        r[21] = first;
        r[24] = n;
        let mut actions = Vec::new();
        for k in first as u16..first as u16 + n as u16 {
            let m = modifier(k as u8);
            r.push(if m == 0 { 0 } else { 1 });
            if m != 0 {
                actions.extend_from_slice(&[
                    if m & 0x32 != 0 { 3 } else { 1 },
                    0,
                    m,
                    m,
                    0,
                    0,
                    0,
                    0,
                ]);
            }
        }
        put16(&mut r, 22, (actions.len() / 8) as u16);
        pad(&mut r);
        r.extend_from_slice(&actions);
    }
    // No per-key behaviors, virtual modifiers, or explicit overrides are needed
    // by the native core map. Their requested components have empty payloads.
    if present & 32 != 0 {
        let (f, n) = range(req, full, 32, 16, MIN_KEY, 248)?;
        r[25] = f;
        r[26] = n;
    }
    if present & 8 != 0 {
        let (f, n) = range(req, full, 8, 20, MIN_KEY, 248)?;
        r[28] = f;
        r[29] = n;
    }
    if present & 4 != 0 {
        let (f, n) = range(req, full, 4, 22, MIN_KEY, 248)?;
        r[31] = f;
        r[32] = n;
        let mut total = 0;
        for k in f as u16..f as u16 + n as u16 {
            let m = modifier(k as u8);
            if m != 0 {
                r.extend_from_slice(&[k as u8, m]);
                total += 1;
            }
        }
        r[33] = total;
        pad(&mut r);
    }
    if present & 128 != 0 {
        let (f, n) = range(req, full, 128, 24, MIN_KEY, 248)?;
        r[34] = f;
        r[35] = n;
    }
    Ok(finish(r))
}
fn get_names(which: u32) -> Vec<u8> {
    let which = which & 0x3fff;
    let mut r = reply(32);
    put32(&mut r, 8, which);
    r[12] = MIN_KEY;
    r[13] = MAX_KEY;
    for (bit, name) in ["evdev", "native", "us", "us", "complete", "complete"]
        .iter()
        .enumerate()
    {
        if which & (1 << bit) != 0 {
            append32(&mut r, atom(name));
        }
    }
    if which & 0xc0 != 0 {
        r[14] = 4;
    }
    if which & 0x40 != 0 {
        for name in TYPE_NAMES {
            append32(&mut r, atom(name));
        }
    }
    if which & 0x80 != 0 {
        r.extend_from_slice(&TYPE_LEVELS);
        put16(&mut r, 26, 7);
        for levels in TYPE_LEVELS {
            for level in 0..levels {
                append32(&mut r, atom(if level == 0 { "Base" } else { "Shift" }));
            }
        }
    }
    if which & 0x100 != 0 {
        put32(&mut r, 20, 7);
        for name in ["Caps Lock", "Num Lock", "Scroll Lock"] {
            append32(&mut r, atom(name));
        }
    }
    if which & 0x1000 != 0 {
        r[15] = 1;
        append32(&mut r, atom("English (US)"));
    }
    if which & 0x200 != 0 {
        r[18] = MIN_KEY;
        r[19] = 248;
        for k in MIN_KEY..=MAX_KEY {
            r.extend_from_slice(&key_name(k));
        }
    }
    finish(r)
}

pub fn dispatch(minor: u8, req: &[u8]) -> Result<Option<Vec<u8>>, ProtocolError> {
    let required = match minor {
        0 | 4 | 6 | 12 => 8,
        1 | 10 | 13 | 17 => 12,
        3 => 28,
        5 | 24 => 16,
        8 => 28,
        21 => 28,
        _ => 4,
    };
    if req.len() < required {
        return Err(error(minor, 16, req.len() as u32));
    }
    if minor != 0 && req.len() >= 6 && !valid_device(u16_at(req, 4)) {
        return Err(error(minor, ERROR, u16_at(req, 4) as u32));
    }
    let mut r = reply(32);
    match minor {
        0 => {
            r[1] = u8::from(u16_at(req, 4) == 1);
            put16(&mut r, 8, 1);
            put16(&mut r, 10, 0);
        }
        1 => {
            let affect = u16_at(req, 6);
            let clear = u16_at(req, 8);
            let all = u16_at(req, 10);
            select(affect, !clear | all);
            return Ok(None);
        }
        3 => {
            unsafe {
                crate::XBell(crate::shared_display(), req[9] as i8 as i32);
            };
            return Ok(None);
        }
        4 => {
            let s = state();
            r[8..14].copy_from_slice(&[
                s.mods,
                s.base_mods,
                s.latched_mods,
                s.locked_mods,
                s.group,
                s.locked_group,
            ]);
            r[18..23].fill(s.mods);
            put16(&mut r, 24, s.ptr_buttons);
        }
        5 => {
            if req[8] != 0 && req[9] != 0 || req[13] != 0 && u16_at(req, 14) != 0 {
                return Err(error(minor, 2, req[9] as u32));
            }
            lock_mods(req[6], req[7], false);
            lock_mods(req[10], req[11], true);
            return Ok(None);
        }
        6 => {
            r.resize(92, 0);
            r[8] = 1;
            r[9] = 1;
            put16(&mut r, 20, 660);
            put16(&mut r, 22, 40);
            let mut controls = unsafe { core::mem::zeroed::<crate::keyboard::XKeyboardState>() };
            unsafe {
                crate::XGetKeyboardControl(crate::shared_display(), &mut controls);
            }
            put32(&mut r, 56, u32::from(controls.global_auto_repeat != 0));
            r[60..92].copy_from_slice(&controls.auto_repeats);
        }
        8 => return get_map(req).map(Some),
        10 => {
            r[8] = req[6] & 15;
            for _ in 0..r[8].count_ones() {
                r.extend_from_slice(&[0; 4]);
            }
        }
        12 => {
            let s = state();
            let bits = u32::from(s.locked_mods & 2 != 0)
                | (u32::from(s.locked_mods & 16 != 0) << 1)
                | (u32::from(s.locked_mods & 32 != 0) << 2);
            put32(&mut r, 8, bits);
        }
        13 => {
            let which = u32_at(req, 8);
            put32(&mut r, 8, which);
            put32(&mut r, 12, 7);
            r[16] = which.count_ones() as u8;
            for i in 0..32 {
                if which & (1 << i) != 0 {
                    let m = match i {
                        0 => 2,
                        1 => 16,
                        2 => 32,
                        _ => 0,
                    };
                    r.extend_from_slice(&[0, 0, 0, 4, m, m, 0, 0, 0, 0, 0, 0]);
                }
            }
        }
        17 => return Ok(Some(get_names(u32_at(req, 8)))),
        21 => {
            let change = u32_at(req, 8);
            let value = u32_at(req, 12);
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            s.client_flags = (s.client_flags & !change) | (value & change & 1);
            put32(&mut r, 8, 1);
            put32(&mut r, 12, s.client_flags);
        }
        24 => {
            put16(&mut r, 12, u16_at(req, 6));
            r[21] = 1;
            put16(&mut r, 22, 0);
            put16(&mut r, 24, 0);
            put32(&mut r, 28, atom("KEYBOARD"));
            let name = b"Windows virtual core keyboard";
            r.extend_from_slice(&(name.len() as u16).to_ne_bytes());
            r.extend_from_slice(name);
        }
        _ => return Err(error(minor, 1, minor as u32)),
    }
    Ok(Some(finish(r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_map_ranges_actions_and_reply_lengths() {
        let mut req = vec![0; 28];
        put16(&mut req, 4, 0x100);
        put16(&mut req, 6, 255);
        let r = dispatch(8, &req)
            .unwrap_or_else(|e| panic!("protocol error {}", e.code))
            .unwrap();
        assert_eq!(r.len(), 32 + u32_at(&r, 4) as usize * 4);
        assert_eq!(
            (r[1], r[10], r[11], r[15], r[20], r[24]),
            (3, 8, 255, 4, 248, 248)
        );
        assert_eq!(symbols(38), [b'a' as u32, b'A' as u32]);
        assert_eq!(modifier(105), 4);
        put16(&mut req, 6, 0);
        put16(&mut req, 8, 2);
        req[12] = 255;
        req[13] = 2;
        assert_eq!(dispatch(8, &req).unwrap_err().code, 2);
        assert_eq!(dispatch(8, &req[..27]).unwrap_err().code, 16);
        put16(&mut req, 4, 99);
        assert_eq!(dispatch(8, &req).unwrap_err().code, ERROR);
    }
}
