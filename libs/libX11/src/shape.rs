//! Window shapes and per-connection notifications shared with XFixes.
use crate::{Display, XEvent, XRectangle, region::RegionRec, shared, xfixes};
use std::collections::BTreeSet;
use std::sync::Mutex;

pub const OPCODE: u8 = 141;
pub const EVENT: u8 = 116;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
}
static WATCHES: Mutex<BTreeSet<(usize, usize)>> = Mutex::new(BTreeSet::new());

#[repr(C)]
struct ShapeEvent {
    kind: i32,
    serial: u64,
    sent: i32,
    display: *mut Display,
    window: usize,
    shape_kind: i32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    time: u64,
    shaped: i32,
}

pub fn select(display: *mut Display, window: usize, mask: u64) -> Result<(), u8> {
    if mask > 1 {
        return Err(2);
    }
    xfixes::window_region(display, window).ok_or(3u8)?;
    let mut watches = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    let key = format!(
        "shape-watch/{window}/{}/{}",
        shared::pid(),
        display as usize
    );
    if mask != 0 {
        shared::set(key, Vec::new())?;
        watches.insert((display as usize, window));
    } else {
        shared::remove(&key);
        watches.remove(&(display as usize, window));
    }
    Ok(())
}
pub fn selected(display: *mut Display, window: usize) -> bool {
    WATCHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&(display as usize, window))
}
pub fn close(display: *mut Display) {
    WATCHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|&(d, window)| {
            if d != display as usize {
                return true;
            }
            shared::remove(&format!("shape-watch/{window}/{}/{d}", shared::pid()));
            false
        });
}
pub fn receive(wire: &[u8; 32]) {
    let window = u32::from_le_bytes(wire[4..8].try_into().unwrap()) as usize;
    let displays: Vec<_> = WATCHES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|&(d, w)| (w == window).then_some(d))
        .collect();
    let rect = xfixes::rectangle(wire, 8);
    for display in displays {
        let mut event: XEvent = unsafe { core::mem::zeroed() };
        unsafe {
            (&raw mut event).cast::<ShapeEvent>().write(ShapeEvent {
                kind: EVENT as i32,
                serial: 0,
                sent: 0,
                display: display as _,
                window,
                shape_kind: wire[1] as i32,
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width as u32,
                height: rect.height as u32,
                time: u32::from_le_bytes(wire[16..20].try_into().unwrap()) as u64,
                shaped: wire[20] as i32,
            });
        }
        crate::queue_extension_event(display as _, event);
    }
}
pub fn changed(window: usize, kind: u8, mut region: RegionRec, shaped: bool) {
    let prefix = format!("shape-watch/{window}/");
    let owners: BTreeSet<u32> = shared::prefix(&prefix)
        .into_iter()
        .filter_map(|(key, _)| key[prefix.len()..].split('/').next()?.parse().ok())
        .collect();
    if owners.is_empty() {
        return;
    }
    crate::region::update_extents(&mut region);
    let mut wire = [0; 32];
    wire[0] = 253;
    wire[1] = kind;
    wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
    xfixes::encode(&mut wire, 8, region.extents);
    let time = unsafe { GetTickCount64() } as u32;
    wire[16..20].copy_from_slice(&time.to_le_bytes());
    wire[20] = u8::from(shaped);
    for owner in owners {
        if owner == shared::pid() {
            receive(&wire);
        } else {
            shared::send(owner, wire);
        }
    }
}
pub unsafe extern "system" fn native_changed(window: usize) {
    // Native notifications run on the UI thread; no Xlib Display lock is taken.
    if xfixes::shapes::custom(window, 0).is_some() {
        return;
    }
    if let Some(region) = xfixes::shapes::native_bounds(window) {
        changed(
            window,
            0,
            region,
            xfixes::shapes::rounded_radius(window) > 0,
        );
    }
}

pub fn combine(
    display: *mut Display,
    window: usize,
    kind: i32,
    source: Option<RegionRec>,
    x: i32,
    y: i32,
    operation: i32,
) -> Result<(), u8> {
    if !(0..=2).contains(&kind) || !(0..=4).contains(&operation) {
        return Err(2);
    }
    let mut source = match source {
        Some(region) => region,
        None => return xfixes::shapes::set(display, window, kind as u8, None, 0, 0),
    };
    unsafe {
        crate::region::XOffsetRegion(&mut source, x, y);
    }
    let mut destination = xfixes::shapes::get(display, window, kind as u8).ok_or(3u8)?;
    let mut result = RegionRec::default();
    unsafe {
        match operation {
            0 => result = source,
            1 => {
                crate::region::XUnionRegion(&mut destination, &mut source, &mut result);
            }
            2 => {
                crate::region::XIntersectRegion(&mut destination, &mut source, &mut result);
            }
            3 => {
                crate::region::XSubtractRegion(&mut destination, &mut source, &mut result);
            }
            4 => {
                crate::region::XSubtractRegion(&mut source, &mut destination, &mut result);
            }
            _ => unreachable!(),
        }
    }
    xfixes::shapes::set(display, window, kind as u8, Some(result), 0, 0)
}
pub fn bitmap(pixmap: usize) -> Result<RegionRec, u8> {
    crate::graphics::read_drawable(pixmap, |pixels, width, height, depth| {
        if depth != 1 {
            return Err(8);
        }
        let mut result = RegionRec::default();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                while x < width && pixels[(y * width + x) as usize] & 1 == 0 {
                    x += 1;
                }
                let start = x;
                while x < width && pixels[(y * width + x) as usize] & 1 != 0 {
                    x += 1;
                }
                if x > start {
                    result.rects.push(XRectangle {
                        x: start as i16,
                        y: y as i16,
                        width: (x - start) as u16,
                        height: 1,
                    });
                }
            }
        }
        Ok(result)
    })
    .ok_or(4u8)?
}

pub fn dispatch(
    display: *mut Display,
    minor: u8,
    bytes: &[u8],
) -> Result<Option<Vec<u8>>, crate::errors::ProtocolError> {
    use crate::xkb::server::{finish, put16, put32, reply, u16_at, u32_at};
    let fail = |code, resource| crate::errors::ProtocolError {
        code,
        request: OPCODE,
        minor,
        resource,
    };
    let required = match minor {
        0 => 4,
        1 | 4 => 16,
        2 | 3 => 20,
        5 | 7 => 8,
        6 | 8 => 12,
        _ => return Err(fail(1, 0)),
    };
    if bytes.len() < required {
        return Err(fail(16, bytes.len()));
    }
    if minor == 0 {
        let mut out = reply(32);
        put16(&mut out, 8, 1);
        put16(&mut out, 10, 1);
        return Ok(Some(out));
    }
    let window = u32_at(bytes, if minor <= 4 { 8 } else { 4 }) as usize;
    xfixes::window_region(display, window).ok_or(fail(3, window))?;
    if minor <= 4 {
        let kind = bytes[if minor == 4 { 4 } else { 5 }] as i32;
        if kind > 2 {
            return Err(fail(2, kind as usize));
        }
        let x = u16_at(bytes, 12) as i16 as i32;
        let y = u16_at(bytes, 14) as i16 as i32;
        let operation = if minor == 4 { 0 } else { bytes[4] as i32 };
        let source = match minor {
            1 => {
                if bytes[6] > 3 {
                    return Err(fail(2, bytes[6] as usize));
                }
                if (bytes.len() - 16) % 8 != 0 {
                    return Err(fail(16, bytes.len()));
                }
                Some(RegionRec {
                    rects: (16..bytes.len())
                        .step_by(8)
                        .map(|i| xfixes::rectangle(bytes, i))
                        .collect(),
                    extents: XRectangle::default(),
                })
            }
            2 => {
                let id = u32_at(bytes, 16) as usize;
                if id == 0 {
                    None
                } else {
                    Some(bitmap(id).map_err(|code| fail(code, id))?)
                }
            }
            3 => {
                if bytes[6] > 2 {
                    return Err(fail(2, bytes[6] as usize));
                }
                let source = u32_at(bytes, 16) as usize;
                Some(xfixes::shapes::get(display, source, bytes[6]).ok_or(fail(3, source))?)
            }
            4 => {
                let Some(region) = xfixes::shapes::custom(window, kind as u8) else {
                    return Ok(None);
                };
                Some(region)
            }
            _ => unreachable!(),
        };
        combine(display, window, kind, source, x, y, operation)
            .map_err(|code| fail(code, window))?;
        return Ok(None);
    }
    let mut out = reply(32);
    match minor {
        5 => {
            for kind in 0..2 {
                let region = xfixes::shapes::get(display, window, kind).ok_or(fail(3, window))?;
                out[8 + kind as usize] = u8::from(
                    xfixes::shapes::custom(window, kind).is_some()
                        || kind == 0 && xfixes::shapes::rounded_radius(window) > 0,
                );
                xfixes::encode(&mut out, 12 + kind as usize * 8, region.extents);
            }
        }
        6 => {
            select(display, window, bytes[8] as u64).map_err(|code| fail(code, window))?;
            return Ok(None);
        }
        7 => out[1] = u8::from(selected(display, window)),
        8 => {
            if bytes[8] > 2 {
                return Err(fail(2, bytes[8] as usize));
            }
            let region = xfixes::shapes::get(display, window, bytes[8]).ok_or(fail(3, window))?;
            out.resize(32 + region.rects.len() * 8, 0);
            put32(&mut out, 8, region.rects.len() as u32);
            for (i, rect) in region.rects.iter().enumerate() {
                xfixes::encode(&mut out, 32 + i * 8, *rect);
            }
        }
        _ => unreachable!(),
    }
    Ok(Some(finish(out)))
}
