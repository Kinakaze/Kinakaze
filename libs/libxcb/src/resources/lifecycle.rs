//! Cursor pixels are serialized once even after XFreeCursor leaves window refs.
use super::*;
use std::{cell::RefCell, sync::MutexGuard};
const KEY: u64 = u64::from_le_bytes(*b"CYXCBRS1");
struct Frozen {
    _state: MutexGuard<'static, State>,
    bytes: Vec<u8>,
}
thread_local! {static FROZEN:RefCell<Option<Frozen>>=const{RefCell::new(None)};}
fn word(b: &mut Vec<u8>, v: usize) {
    b.extend_from_slice(&(v as u64).to_le_bytes());
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let Ok(state) = state().try_lock() else {
            return 11;
        };
        let mut b = Vec::new();
        word(&mut b, KEY as usize);
        word(&mut b, state.maps.len());
        for (k, v) in &state.maps {
            word(&mut b, *k as usize);
            word(&mut b, *v);
        }
        word(&mut b, state.window_maps.len());
        for (k, v) in &state.window_maps {
            word(&mut b, *k as usize);
            word(&mut b, *v as usize);
        }
        let mut unique = HashMap::new();
        let mut images = Vec::new();
        for c in state.cursors.values().chain(state.windows.values()) {
            unique.entry(Arc::as_ptr(c) as usize).or_insert_with(|| {
                let i = images.len();
                images.push(c);
                i
            });
        }
        word(&mut b, images.len());
        for c in images {
            for v in [c.width, c.height, c.x, c.y] {
                word(&mut b, v as usize);
            }
            word(&mut b, c.pixels.len());
            for pixel in c.pixels.iter() {
                b.extend_from_slice(&pixel.to_le_bytes());
            }
        }
        for map in [&state.cursors, &state.windows] {
            word(&mut b, map.len());
            for (id, c) in map {
                word(&mut b, *id as usize);
                word(&mut b, unique[&(Arc::as_ptr(c) as usize)]);
            }
        }
        *slot.borrow_mut() = Some(Frozen {
            _state: state,
            bytes: b,
        });
        0
    })
}
unsafe extern "system" fn snapshot(p: *mut u8, n: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(s) = slot.as_ref() else {
            return -22;
        };
        if !p.is_null() {
            if n < s.bytes.len() {
                return -22;
            }
            unsafe { core::ptr::copy_nonoverlapping(s.bytes.as_ptr(), p, s.bytes.len()) };
        }
        s.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_: i32) {
    FROZEN.with(|s| {
        s.borrow_mut().take();
    });
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], i32> {
        let (a, b) = self.0.split_at_checked(n).ok_or(22)?;
        self.0 = b;
        Ok(a)
    }
    fn word(&mut self) -> Result<usize, i32> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()) as usize)
    }
    fn number<T: TryFrom<usize>>(&mut self) -> Result<T, i32> {
        T::try_from(self.word()?).map_err(|_| 22)
    }
}
fn restore(b: &[u8]) -> Result<(), i32> {
    let mut r = Reader(b);
    if r.word()? != KEY as usize {
        return Err(22);
    }
    let mut s = State::default();
    for _ in 0..r.word()? {
        let id = r.number()?;
        let native = r.word()?;
        if s.maps.insert(id, native).is_some() {
            return Err(22);
        }
    }
    for _ in 0..r.word()? {
        let id = r.number()?;
        let map = r.number()?;
        if s.window_maps.insert(id, map).is_some() {
            return Err(22);
        }
    }
    let mut images = Vec::new();
    for _ in 0..r.word()? {
        let (w, h, x, y): (u16, u16, u16, u16) =
            (r.number()?, r.number()?, r.number()?, r.number()?);
        let count = r.word()?;
        if w == 0 || h == 0 || x >= w || y >= h || count != w as usize * h as usize {
            return Err(22);
        }
        let pixels = r
            .take(count.checked_mul(4).ok_or(22)?)?
            .chunks_exact(4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        images.push(cursor(w, h, x, y, pixels).map_err(|_| 12)?);
    }
    for map in [&mut s.cursors, &mut s.windows] {
        for _ in 0..r.word()? {
            let id = r.number()?;
            let image = images.get(r.word()?).ok_or(22)?.clone();
            if map.insert(id, image).is_some() {
                return Err(22);
            }
        }
    }
    if !r.0.is_empty() {
        return Err(22);
    }
    for (window, cursor) in &s.windows {
        let native = window::native(*window, 2).map_err(|_| 22)?;
        if unsafe { x11::XDefineCursor(x11::connection::xcb_display(), native, cursor.native) } == 0
        {
            return Err(5);
        }
    }
    *state().lock().unwrap() = s;
    Ok(())
}
unsafe extern "system" fn child(p: *const u8, n: usize) -> i32 {
    if p.is_null() || n > isize::MAX as usize {
        return 22;
    }
    restore(unsafe { core::slice::from_raw_parts(p, n) })
        .err()
        .unwrap_or(0)
}
unsafe fn close(_: *mut x11::Display) {
    *state().lock().unwrap() = State::default();
}
extern "C" fn register() {
    x11::register_display_close(close);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 452,
        key: KEY,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
