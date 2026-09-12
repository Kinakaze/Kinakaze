//! Server-owned XFixes regions shared by Composite and Damage.
pub mod shapes;
pub(crate) mod tracking;
use crate::xkb::server::{finish, put16, put32, reply, u16_at, u32_at};
use crate::{Display, XRectangle, errors::ProtocolError, region::RegionRec};
use std::{collections::BTreeMap, sync::Mutex};
pub const OPCODE: u8 = 135;
pub const EVENT: u8 = 92;
pub const ERROR: u8 = 156;
static REGIONS: Mutex<BTreeMap<u32, RegionRec>> = Mutex::new(BTreeMap::new());
pub fn get(id: u32) -> Option<RegionRec> {
    REGIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
}
pub fn set(id: u32, mut r: RegionRec) -> bool {
    crate::region::update_extents(&mut r);
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    if !regions.contains_key(&id) {
        return false;
    }
    regions.insert(id, r);
    true
}
pub fn create(id: u32, mut r: RegionRec) -> bool {
    if id == 0 {
        return false;
    }
    crate::region::update_extents(&mut r);
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    if regions.contains_key(&id) {
        return false;
    }
    regions.insert(id, r);
    true
}
pub fn rectangle(b: &[u8], i: usize) -> XRectangle {
    XRectangle {
        x: u16_at(b, i) as i16,
        y: u16_at(b, i + 2) as i16,
        width: u16_at(b, i + 4),
        height: u16_at(b, i + 6),
    }
}
pub fn encode(b: &mut [u8], i: usize, r: XRectangle) {
    put16(b, i, r.x as u16);
    put16(b, i + 2, r.y as u16);
    put16(b, i + 4, r.width);
    put16(b, i + 6, r.height);
}
fn error(minor: u8, code: u8, id: u32) -> ProtocolError {
    ProtocolError {
        code,
        request: OPCODE,
        minor,
        resource: id as usize,
    }
}
pub fn window_region(display: *mut Display, window: usize) -> Option<RegionRec> {
    let mut a = unsafe { core::mem::zeroed::<crate::XWindowAttributes>() };
    if unsafe { crate::XGetWindowAttributes(display, window, &mut a) } == 0 {
        return None;
    }
    let mut region = RegionRec {
        rects: vec![XRectangle {
            x: 0,
            y: 0,
            width: a.width.min(65535) as u16,
            height: a.height.min(65535) as u16,
        }],
        extents: XRectangle::default(),
    };
    crate::region::update_extents(&mut region);
    Some(region)
}
pub fn dispatch(
    display: *mut Display,
    minor: u8,
    b: &[u8],
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let need = match minor {
        0 => 12,
        1 | 6 | 8 | 9 | 12 | 17 | 18 | 23 | 26 | 27 => 12,
        2 | 7 | 13..=15 | 20 | 22 => 16,
        3 => 12,
        31 => 28,
        16 | 21 | 28 => 20,
        4 | 25 => 4,
        _ => 8,
    };
    if b.len() < need {
        return Err(error(minor, 16, b.len() as u32));
    }
    if minor == 0 {
        let mut r = reply(32);
        r[1] = 0;
        put32(&mut r, 8, u32_at(b, 4).min(5));
        put32(&mut r, 12, 0);
        return Ok(Some(finish(r)));
    }
    let id = if b.len() >= 8 { u32_at(b, 4) } else { 0 };
    let read = |id| get(id).ok_or(error(minor, ERROR, id));
    let write = |id, r| {
        if set(id, r) {
            Ok(())
        } else {
            Err(error(minor, ERROR, id))
        }
    };
    match minor {
        2..=4 | 23..=27 | 29..=32 => return tracking::dispatch(display, minor, b),
        1 => {
            return Err(error(minor, 8, id));
        } // Every hosted window belongs to this client.
        5 | 11 => {
            if (b.len() - 8) % 8 != 0 {
                return Err(error(minor, 16, id));
            }
            let r = RegionRec {
                rects: (8..b.len())
                    .step_by(8)
                    .map(|i| rectangle(b, i))
                    .filter(|r| r.width != 0 && r.height != 0)
                    .collect(),
                extents: XRectangle::default(),
            };
            if minor == 5 {
                if !create(id, r) {
                    return Err(error(minor, 14, id));
                }
            } else {
                write(id, r)?;
            }
        }
        6 => {
            let bitmap = u32_at(b, 8) as usize;
            let mut rects = Vec::new();
            let ok = crate::graphics::read_drawable(bitmap, |pixels, w, h, depth| {
                if depth != 1 {
                    return false;
                }
                for y in 0..h {
                    let mut x = 0;
                    while x < w {
                        while x < w && pixels[(y * w + x) as usize] & 1 == 0 {
                            x += 1;
                        }
                        let start = x;
                        while x < w && pixels[(y * w + x) as usize] & 1 != 0 {
                            x += 1;
                        }
                        if x > start {
                            rects.push(XRectangle {
                                x: start as i16,
                                y: y as i16,
                                width: (x - start) as u16,
                                height: 1,
                            });
                        }
                    }
                }
                true
            });
            if ok != Some(true) {
                return Err(error(minor, 8, bitmap as u32));
            }
            if !create(
                id,
                RegionRec {
                    rects,
                    extents: XRectangle::default(),
                },
            ) {
                return Err(error(minor, 14, id));
            }
        }
        7 => {
            let w = u32_at(b, 8);
            if b[12] > 2 {
                return Err(error(minor, 2, b[12] as u32));
            }
            let r = shapes::get(display, w as usize, b[12]).ok_or(error(minor, 3, w))?;
            if !create(id, r) {
                return Err(error(minor, 14, id));
            }
        }
        10 => {
            if REGIONS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
                .is_none()
            {
                return Err(error(minor, ERROR, id));
            }
        }
        12 => {
            write(u32_at(b, 8), read(id)?)?;
        }
        13..=15 => {
            let mut a = read(id)?;
            let mut other = read(u32_at(b, 8))?;
            let mut out = RegionRec::default();
            unsafe {
                match minor {
                    13 => {
                        crate::region::XUnionRegion(&mut a, &mut other, &mut out);
                    }
                    14 => {
                        crate::region::XIntersectRegion(&mut a, &mut other, &mut out);
                    }
                    _ => {
                        crate::region::XSubtractRegion(&mut a, &mut other, &mut out);
                    }
                }
            }
            write(u32_at(b, 12), out)?;
        }
        16 => {
            let a = read(id)?;
            let rect = rectangle(b, 8);
            write(
                u32_at(b, 16),
                RegionRec {
                    rects: crate::region::subtract(&[rect], &a.rects),
                    extents: XRectangle::default(),
                },
            )?;
        }
        17 => {
            let mut r = read(id)?;
            unsafe {
                crate::region::XOffsetRegion(
                    &mut r,
                    u16_at(b, 8) as i16 as i32,
                    u16_at(b, 10) as i16 as i32,
                );
            };
            write(id, r)?;
        }
        18 => {
            let r = read(id)?;
            write(
                u32_at(b, 8),
                RegionRec {
                    rects: if r.rects.is_empty() {
                        vec![]
                    } else {
                        vec![r.extents]
                    },
                    extents: r.extents,
                },
            )?;
        }
        19 => {
            let region = read(id)?;
            let mut r = reply(32);
            r[1] = 0;
            encode(&mut r, 8, region.extents);
            for rect in region.rects {
                let n = r.len();
                r.resize(n + 8, 0);
                encode(&mut r, n, rect);
            }
            return Ok(Some(finish(r)));
        }
        9 => {
            let picture = u32_at(b, 8);
            let region =
                crate::render::picture_clip(picture as usize).ok_or(error(minor, 143, picture))?;
            if !create(id, region) {
                return Err(error(minor, 14, id));
            }
        }
        21 => {
            let region = u32_at(b, 16);
            shapes::set(
                display,
                id as usize,
                b[8],
                if region == 0 {
                    None
                } else {
                    Some(read(region)?)
                },
                u16_at(b, 12) as i16,
                u16_at(b, 14) as i16,
            )
            .map_err(|code| error(minor, code, id))?;
        }
        22 => {
            let region = u32_at(b, 8);
            let mut rectangles = if region == 0 {
                None
            } else {
                Some(read(region)?.rects)
            };
            if region == 0 {
                unsafe {
                    crate::render::XRenderSetPictureClipRegion(
                        display,
                        id as usize,
                        core::ptr::null_mut(),
                    );
                }
                return Ok(None);
            }
            unsafe {
                crate::render::XRenderSetPictureClipRectangles(
                    display,
                    id as usize,
                    u16_at(b, 12) as i16 as i32,
                    u16_at(b, 14) as i16 as i32,
                    rectangles
                        .as_mut()
                        .map_or(core::ptr::null_mut(), |r| r.as_mut_ptr()),
                    rectangles.as_ref().map_or(0, |r| r.len()) as i32,
                );
            }
        }
        28 => {
            let source = read(id)?;
            let left = u16_at(b, 12) as i32;
            let right = u16_at(b, 14) as i32;
            let top = u16_at(b, 16) as i32;
            let bottom = u16_at(b, 18) as i32;
            let mut rects = Vec::new();
            for r in source.rects {
                let x = (r.x as i32 - left).max(i16::MIN as i32);
                let y = (r.y as i32 - top).max(i16::MIN as i32);
                let x2 = (r.x as i32 + r.width as i32 + right).min(i16::MAX as i32);
                let y2 = (r.y as i32 + r.height as i32 + bottom).min(i16::MAX as i32);
                let r = XRectangle {
                    x: x as i16,
                    y: y as i16,
                    width: (x2 - x).max(0) as u16,
                    height: (y2 - y).max(0) as u16,
                };
                rects.extend(crate::region::subtract(&[r], &rects));
            }
            write(
                u32_at(b, 8),
                RegionRec {
                    rects,
                    extents: XRectangle::default(),
                },
            )?;
        }
        _ => return Err(error(minor, 1, minor as u32)),
    }
    Ok(None)
}
