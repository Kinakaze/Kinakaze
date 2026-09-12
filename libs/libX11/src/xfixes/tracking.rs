//! Selection and cursor tracking, cursor visibility and pointer barriers.
use super::*;
use std::sync::{Arc, LazyLock};
use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::*};
#[derive(Default)]
struct State {
    selections: BTreeMap<(u32, u32), u32>,
    cursors: BTreeMap<u32, u32>,
    names: BTreeMap<usize, (u32, Vec<u8>)>,
    bindings: BTreeMap<usize, usize>,
    hidden: BTreeMap<usize, u32>,
    serial: u32,
    last: usize,
    image: Option<Arc<crate::xcursor::CursorImage>>,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn define(window: usize, cursor: usize) {
    let mut s = state();
    s.bindings.insert(window, cursor);
    drop(s);
    crate::notify_event();
}
pub fn selection(selection: usize, owner: usize, time: u32, subtype: u8) {
    let s = state();
    for (&(window, atom), &mask) in &s.selections {
        if atom == selection as u32 && mask & (1 << subtype) != 0 {
            let mut e = [0; 32];
            e[0] = EVENT;
            e[1] = subtype;
            put32(&mut e, 4, window);
            put32(&mut e, 8, owner as u32);
            put32(&mut e, 12, atom);
            put32(&mut e, 16, crate::focus::now());
            put32(&mut e, 20, time);
            crate::xext::protocol::queue_event(e);
        }
    }
}
pub fn forget(window: usize) {
    let mut s = state();
    s.selections.retain(|(w, _), _| *w != window as u32);
    s.cursors.remove(&(window as u32));
    s.bindings.remove(&window);
    s.hidden.remove(&window);
}
fn cursor(s: &mut State) -> (POINT, usize, u32, Vec<u8>) {
    let mut info = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        GetCursorInfo(&mut info);
    }
    let mut window = unsafe { WindowFromPoint(info.ptScreenPos) };
    let mut binding = None;
    while !window.is_null() {
        if let Some(c) = s.bindings.get(&(window as usize)) {
            binding = Some(*c);
            break;
        }
        window = unsafe { GetParent(window) };
    }
    let binding = binding.or_else(|| s.bindings.get(&1).copied()).unwrap_or(0);
    let handle = if info.hCursor.is_null() {
        unsafe { LoadCursorW(core::ptr::null_mut(), IDC_ARROW) }
    } else {
        info.hCursor
    };
    let key = if binding != 0 {
        binding
    } else {
        handle as usize
    };
    if key != s.last || s.image.is_none() {
        s.last = key;
        s.serial = s.serial.wrapping_add(1).max(1);
        s.image = crate::xcursor::cursor_image(binding)
            .or_else(|| crate::xcursor::native_snapshot(handle as usize));
    }
    let (atom, name) = s.names.get(&binding).cloned().unwrap_or_default();
    (info.ptScreenPos, key, atom, name)
}
pub fn poll(_d: *mut Display, close: bool) {
    if close {
        *state() = State::default();
        super::shapes::clear();
        super::REGIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        kinakaze_libdisplay::ui::barriers::clear();
        kinakaze_libdisplay::ui::hide_root_cursor(false);
        return;
    }
    let mut s = state();
    if s.cursors.is_empty() {
        return;
    }
    let old = s.serial;
    let (_, _, name, _) = cursor(&mut s);
    if old != s.serial {
        for (&w, &mask) in &s.cursors {
            if mask != 0 {
                let mut e = [0; 32];
                e[0] = EVENT + 1;
                put32(&mut e, 4, w);
                put32(&mut e, 8, s.serial);
                put32(&mut e, 12, crate::focus::now());
                put32(&mut e, 16, name);
                crate::xext::protocol::queue_event(e);
            }
        }
    }
}
pub fn dispatch(d: *mut Display, minor: u8, b: &[u8]) -> Result<Option<Vec<u8>>, ProtocolError> {
    crate::xext::protocol::register_poll(poll);
    let id = if b.len() >= 8 { u32_at(b, 4) } else { 0 };
    let window = |id: u32| {
        if super::window_region(d, id as usize).is_some() {
            Ok(())
        } else {
            Err(error(minor, 3, id))
        }
    };
    match minor {
        2 => {
            window(id)?;
            let atom = u32_at(b, 8);
            let mask = u32_at(b, 12);
            if mask & !7 != 0 {
                return Err(error(minor, 2, mask));
            }
            let mut s = state();
            if mask == 0 {
                s.selections.remove(&(id, atom));
            } else {
                s.selections.insert((id, atom), mask);
            }
        }
        3 => {
            window(id)?;
            let mask = u32_at(b, 8);
            if mask & !1 != 0 {
                return Err(error(minor, 2, mask));
            }
            let mut s = state();
            if mask == 0 {
                s.cursors.remove(&id);
            } else {
                s.cursors.insert(id, mask);
            }
        }
        4 | 25 => {
            let mut s = state();
            let (p, _, atom, name) = cursor(&mut s);
            let image = s.image.as_ref().ok_or(error(minor, 11, 0))?;
            let mut r = reply(32);
            r[1] = 0;
            put16(&mut r, 8, p.x as u16);
            put16(&mut r, 10, p.y as u16);
            put16(&mut r, 12, image.width);
            put16(&mut r, 14, image.height);
            put16(&mut r, 16, image.x);
            put16(&mut r, 18, image.y);
            put32(&mut r, 20, s.serial);
            if minor == 25 {
                put32(&mut r, 24, atom);
                put16(&mut r, 28, name.len() as u16);
            }
            for pixel in image.pixels.iter() {
                r.extend_from_slice(&pixel.to_le_bytes());
            }
            if minor == 25 {
                r.extend_from_slice(&name);
            }
            return Ok(Some(finish(r)));
        }
        23 | 27 => {
            if !crate::xcursor::exists(id as usize) {
                return Err(error(minor, 6, id));
            }
            let n = u16_at(b, 8) as usize;
            if b.len() < 12 + n {
                return Err(error(minor, 16, id));
            }
            let name = b[12..12 + n].to_vec();
            if minor == 23 {
                let c = std::ffi::CString::new(name.clone()).map_err(|_| error(minor, 2, id))?;
                let atom = unsafe { crate::XInternAtom(d, c.as_ptr(), 0) } as u32;
                state().names.insert(id as usize, (atom, name));
            } else {
                let dest: Vec<_> = state()
                    .names
                    .iter()
                    .filter(|(_, (_, n))| *n == name)
                    .map(|(id, _)| *id)
                    .collect();
                for destination in dest {
                    change(id as usize, destination)?;
                }
            }
        }
        24 => {
            if !crate::xcursor::exists(id as usize) {
                return Err(error(minor, 6, id));
            }
            let (atom, name) = state()
                .names
                .get(&(id as usize))
                .cloned()
                .unwrap_or_default();
            let mut r = reply(32);
            r[1] = 0;
            put32(&mut r, 8, atom);
            put16(&mut r, 12, name.len() as u16);
            r.extend_from_slice(&name);
            return Ok(Some(finish(r)));
        }
        26 => change(id as usize, u32_at(b, 8) as usize)?,
        29 | 30 => {
            window(id)?;
            let mut s = state();
            let count = s.hidden.entry(id as usize).or_default();
            if minor == 29 {
                *count = count.saturating_add(1);
            } else {
                if *count == 0 {
                    return Err(error(minor, 8, id));
                }
                *count -= 1;
            }
            let hidden = *count != 0;
            drop(s);
            if id == 1 {
                kinakaze_libdisplay::ui::hide_root_cursor(hidden);
            } else {
                kinakaze_libdisplay::ui::set_cursor_visible(id as usize, !hidden);
            }
        }
        31 => {
            window(u32_at(b, 8))?;
            let x1 = u16_at(b, 12) as i16 as i32;
            let y1 = u16_at(b, 14) as i16 as i32;
            let x2 = u16_at(b, 16) as i16 as i32;
            let y2 = u16_at(b, 18) as i16 as i32;
            if (x1 == x2) == (y1 == y2) {
                return Err(error(minor, 2, id));
            }
            let count = u16_at(b, 26) as usize;
            if b.len() < 28 + count * 2 {
                return Err(error(minor, 16, id));
            }
            for n in 0..count {
                let dev = u16_at(b, 28 + n * 2);
                if dev > 3 {
                    return Err(error(minor, 128, dev as u32));
                }
            }
            let applies = count == 0 || (0..count).any(|n| u16_at(b, 28 + n * 2) <= 2);
            let barrier = kinakaze_libdisplay::ui::barriers::Barrier {
                window: u32_at(b, 8) as usize,
                x1,
                y1,
                x2,
                y2,
                directions: if applies { u32_at(b, 20) } else { 15 },
            };
            if !kinakaze_libdisplay::ui::barriers::create(id, barrier) {
                return Err(error(minor, 14, id));
            }
        }
        32 => {
            if !kinakaze_libdisplay::ui::barriers::destroy(id) {
                return Err(error(minor, ERROR + 1, id));
            }
        }
        _ => return Err(error(minor, 1, id)),
    }
    Ok(None)
}
fn change(source: usize, dest: usize) -> Result<(), ProtocolError> {
    if !crate::xcursor::replace(source, dest) {
        return Err(error(26, 6, dest as u32));
    }
    let windows: Vec<_> = state()
        .bindings
        .iter()
        .filter(|(_, c)| **c == dest)
        .map(|(w, _)| *w)
        .collect();
    for w in windows {
        crate::xcursor::define(w, dest);
    }
    state().last = 0;
    Ok(())
}
