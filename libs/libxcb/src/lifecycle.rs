//! Stable XCB resource identifiers survive native window reconstruction.
use super::{XcbGc, XcbPixmap, XcbState, xcb_generic_event_t, xcb_state};
use core::{cell::RefCell, ptr};
use std::sync::MutexGuard;
const MAGIC: u64 = u64::from_le_bytes(*b"CYXCBST3");
struct Frozen {
    _store: MutexGuard<'static, XcbState>,
    bytes: Vec<u8>,
}
thread_local! {static FROZEN:RefCell<Option<Frozen>>=const{RefCell::new(None)};}
fn word(b: &mut Vec<u8>, v: usize) {
    b.extend_from_slice(&(v as u64).to_le_bytes())
}
fn blob(b: &mut Vec<u8>, v: &[u8]) {
    word(b, v.len());
    b.extend_from_slice(v)
}
fn encode(s: &XcbState) -> Vec<u8> {
    let mut b = Vec::new();
    word(&mut b, MAGIC as usize);
    word(&mut b, s.next_id as usize);
    word(&mut b, s.connection_error as usize);
    word(&mut b, s.wid_to_guest.len());
    for (w, id) in &s.wid_to_guest {
        word(&mut b, *w as usize);
        word(&mut b, *id as usize)
    }
    word(&mut b, s.gcs.len());
    for (id, g) in &s.gcs {
        for v in [
            *id,
            g.foreground,
            g.background,
            g.line_width as u32,
            g.depth as u32,
            g.graphics_exposures as u32,
            g.function as u32,
            g.plane_mask,
            g.clip_origin.0 as u16 as u32,
            g.clip_origin.1 as u16 as u32,
        ] {
            word(&mut b, v as usize)
        }
        word(&mut b, g.clip.is_some() as usize);
        if let Some(rectangles) = &g.clip {
            word(&mut b, rectangles.len());
            for r in rectangles.iter() {
                for v in [r.x as u16, r.y as u16, r.width, r.height] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
    word(&mut b, s.pixmaps.len());
    for (id, p) in &s.pixmaps {
        for v in [
            *id as usize,
            p.width as usize,
            p.height as usize,
            p.depth as usize,
        ] {
            word(&mut b, v)
        }
        word(&mut b, p.data.len());
        for pixel in &p.data {
            b.extend_from_slice(&pixel.to_le_bytes());
        }
    }
    word(&mut b, s.pending_events.len());
    for e in &s.pending_events {
        b.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                (e as *const xcb_generic_event_t).cast(),
                size_of::<xcb_generic_event_t>(),
            )
        })
    }
    b
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
    fn blob(&mut self) -> Result<Vec<u8>, i32> {
        let n = self.word()?;
        Ok(self.take(n)?.to_vec())
    }
}
fn decode(bytes: &[u8]) -> Result<XcbState, i32> {
    let mut r = Reader(bytes);
    if r.word()? != MAGIC as usize {
        return Err(22);
    }
    let mut s = XcbState::default();
    s.next_id = r.number()?;
    s.connection_error = r.number()?;
    if !(0..=7).contains(&s.connection_error) {
        return Err(22);
    }
    if !(0x1000_0000..=0x2000_0000).contains(&s.next_id) {
        return Err(22);
    }
    for _ in 0..r.word()? {
        let w: u32 = r.number()?;
        let id: u64 = r.number()?;
        let native = kinakaze_libdisplay::window::native_handle(id).ok_or(22)?;
        if !kinakaze_libdisplay::window::x11::bind(w, id) {
            return Err(22);
        }
        if s.wid_to_guest.insert(w, id).is_some() || s.guest_to_wid.insert(id, w).is_some() {
            return Err(22);
        }
        s.wid_to_hwnd.insert(w, native);
        s.hwnd_to_wid.insert(native, w);
    }
    for _ in 0..r.word()? {
        let id = r.number()?;
        let mut g = XcbGc {
            foreground: r.number()?,
            background: r.number()?,
            line_width: r.number()?,
            depth: r.number()?,
            graphics_exposures: match r.word()? {
                0 => false,
                1 => true,
                _ => return Err(22),
            },
            function: r.number()?,
            plane_mask: r.number()?,
            clip_origin: (r.number::<u16>()? as i16, r.number::<u16>()? as i16),
            clip: None,
        };
        if !matches!(g.depth, 1 | 24 | 32) || g.function > 15 {
            return Err(22);
        }
        match r.word()? {
            0 => {}
            1 => {
                let n = r.word()?;
                let raw = r.take(n.checked_mul(8).ok_or(22)?)?;
                let rectangles: Vec<_> = raw
                    .chunks_exact(8)
                    .map(|b| super::xcb_rectangle_t {
                        x: i16::from_le_bytes(b[0..2].try_into().unwrap()),
                        y: i16::from_le_bytes(b[2..4].try_into().unwrap()),
                        width: u16::from_le_bytes(b[4..6].try_into().unwrap()),
                        height: u16::from_le_bytes(b[6..8].try_into().unwrap()),
                    })
                    .collect();
                g.clip = Some(rectangles.into());
            }
            _ => return Err(22),
        }
        if s.gcs.insert(id, g).is_some() {
            return Err(22);
        }
    }
    for _ in 0..r.word()? {
        let id = r.number()?;
        let p = XcbPixmap {
            width: r.number()?,
            height: r.number()?,
            depth: r.number()?,
            data: {
                let count = r.word()?;
                r.take(count.checked_mul(4).ok_or(22)?)?
                    .chunks_exact(4)
                    .map(|p| u32::from_le_bytes(p.try_into().unwrap()))
                    .collect()
            },
        };
        if p.width == 0
            || p.height == 0
            || !matches!(p.depth, 1 | 24 | 32)
            || p.data.len() != p.width as usize * p.height as usize
            || s.pixmaps.insert(id, p).is_some()
        {
            return Err(22);
        }
    }
    for _ in 0..r.word()? {
        let b = r.take(size_of::<xcb_generic_event_t>())?;
        s.pending_events
            .push_back(unsafe { b.as_ptr().cast::<xcb_generic_event_t>().read_unaligned() });
    }
    if !r.0.is_empty() {
        return Err(22);
    }
    Ok(s)
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(store) = xcb_state().try_lock() else {
            return 11;
        };
        let bytes = encode(&store);
        *slot = Some(Frozen {
            _store: store,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(frozen) = slot.as_ref() else {
            return -22;
        };
        if !output.is_null() {
            if capacity < frozen.bytes.len() {
                return -22;
            }
            unsafe {
                ptr::copy_nonoverlapping(frozen.bytes.as_ptr(), output, frozen.bytes.len());
            }
        }
        frozen.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length > isize::MAX as usize {
        return 22;
    }
    match decode(unsafe { core::slice::from_raw_parts(input, length) }) {
        Ok(store) => {
            *xcb_state().lock().unwrap() = store;
            0
        }
        Err(error) => error,
    }
}
extern "C" fn register() {
    crate::x11::window_tree::register_window_resolver(|window| {
        let id = u32::try_from(window).ok()?;
        xcb_state().lock().ok()?.wid_to_hwnd.get(&id).copied()
    });
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 447,
        key: MAGIC,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
