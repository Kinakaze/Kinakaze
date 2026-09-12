//! Damage accumulation observes real backing-store writes and queues wire events.
use crate::xkb::server::{finish, put32, reply, u32_at};
use crate::{Display, XRectangle, errors::ProtocolError, region::RegionRec};
use std::{collections::BTreeMap, sync::Mutex};
pub const OPCODE: u8 = 136;
pub const EVENT: u8 = 96;
pub const ERROR: u8 = 160;
struct Damage {
    display: usize,
    drawable: usize,
    level: u8,
    region: RegionRec,
    notified: bool,
}
static DAMAGE: Mutex<BTreeMap<u32, Damage>> = Mutex::new(BTreeMap::new());
fn error(minor: u8, code: u8, id: u32) -> ProtocolError {
    ProtocolError {
        code,
        request: OPCODE,
        minor,
        resource: id as usize,
    }
}
pub fn changed(drawable: usize, rect: XRectangle, width: u16, height: u16) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    crate::shared::damage(drawable, rect, width, height);
    let mut pending = Vec::new();
    {
        let mut map = DAMAGE.lock().unwrap_or_else(|e| e.into_inner());
        for (&id, d) in map.iter_mut() {
            if d.drawable != drawable {
                continue;
            }
            let old = d.region.extents;
            let empty = d.region.rects.is_empty();
            let added = crate::region::subtract(&[rect], &d.region.rects);
            d.region.rects.extend_from_slice(&added);
            crate::region::update_extents(&mut d.region);
            let rectangles = match d.level {
                0 => vec![rect],
                1 => added,
                2 => {
                    if empty || old != d.region.extents {
                        vec![d.region.extents]
                    } else {
                        vec![]
                    }
                }
                _ => {
                    if empty {
                        vec![d.region.extents]
                    } else {
                        vec![]
                    }
                }
            };
            for (index, area) in rectangles.iter().enumerate() {
                let mut event = [0; 32];
                event[0] = EVENT;
                event[1] = d.level | if index + 1 < rectangles.len() { 128 } else { 0 };
                put32(&mut event, 4, drawable as u32);
                put32(&mut event, 8, id);
                // Event timestamps share the native display's millisecond clock.
                put32(
                    &mut event,
                    12,
                    kinakaze_libdisplay::event::timestamp() as u32,
                );
                crate::xfixes::encode(&mut event, 16, *area);
                crate::xfixes::encode(
                    &mut event,
                    24,
                    XRectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                );
                pending.push((d.display, event));
                d.notified = true;
            }
        }
    }
    for (display, event) in pending {
        crate::xext::protocol::queue_display(event, display as *mut Display);
    }
}
pub fn close(display: *mut Display) {
    DAMAGE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, damage| damage.display != display as usize);
}
pub fn dispatch(
    display: *mut Display,
    minor: u8,
    b: &[u8],
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let need = match minor {
        0 | 4 => 12,
        1 | 3 => 16,
        2 => 8,
        _ => 4,
    };
    if b.len() < need {
        return Err(error(minor, 16, b.len() as u32));
    }
    if minor == 0 {
        let mut r = reply(32);
        r[1] = 0;
        put32(&mut r, 8, 1);
        put32(&mut r, 12, 1);
        return Ok(Some(finish(r)));
    }
    let id = u32_at(b, 4);
    match minor {
        1 => {
            let drawable = u32_at(b, 8) as usize;
            let level = b[12];
            if level > 3 {
                return Err(error(minor, 2, level as u32));
            }
            let (w, h, _) = crate::graphics::drawable_dimensions(drawable).ok_or(error(
                minor,
                9,
                drawable as u32,
            ))?;
            {
                crate::shared::set(format!("d/{drawable}/{}", crate::shared::pid()), Vec::new())
                    .map_err(|code| error(minor, code, id))?;
            }
            {
                let mut map = DAMAGE.lock().unwrap_or_else(|e| e.into_inner());
                if id == 0 || map.contains_key(&id) {
                    return Err(error(minor, 14, id));
                }
                map.insert(
                    id,
                    Damage {
                        display: display as usize,
                        drawable,
                        level,
                        region: RegionRec::default(),
                        notified: false,
                    },
                );
            }
            changed(
                drawable,
                XRectangle {
                    x: 0,
                    y: 0,
                    width: w.min(65535) as u16,
                    height: h.min(65535) as u16,
                },
                w.min(65535) as u16,
                h.min(65535) as u16,
            );
        }
        2 => {
            if DAMAGE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
                .is_none()
            {
                return Err(error(minor, ERROR, id));
            }
        }
        3 => {
            let repair = u32_at(b, 8);
            let parts = u32_at(b, 12);
            let repair = if repair == 0 {
                None
            } else {
                Some(crate::xfixes::get(repair).ok_or(error(
                    minor,
                    crate::xfixes::ERROR,
                    repair,
                ))?)
            };
            if parts != 0 && crate::xfixes::get(parts).is_none() {
                return Err(error(minor, crate::xfixes::ERROR, parts));
            }
            let removed = {
                let mut map = DAMAGE.lock().unwrap_or_else(|e| e.into_inner());
                let d = map.get_mut(&id).ok_or(error(minor, ERROR, id))?;
                let mut removed = d.region.clone();
                if let Some(mut repair) = repair {
                    let mut rest = RegionRec::default();
                    unsafe {
                        crate::region::XIntersectRegion(&mut d.region, &mut repair, &mut removed);
                        crate::region::XSubtractRegion(&mut d.region, &mut repair, &mut rest);
                    }
                    d.region = rest;
                } else {
                    d.region = RegionRec::default();
                }
                d.notified = !d.region.rects.is_empty();
                removed
            };
            if parts != 0 {
                crate::xfixes::set(parts, removed);
            }
        }
        4 => {
            let region = u32_at(b, 8);
            let r = crate::xfixes::get(region).ok_or(error(minor, crate::xfixes::ERROR, region))?;
            let (w, h, _) =
                crate::graphics::drawable_dimensions(id as usize).ok_or(error(minor, 9, id))?;
            for rect in r.rects {
                changed(id as usize, rect, w as u16, h as u16);
            }
        }
        _ => return Err(error(minor, 1, minor as u32)),
    }
    Ok(None)
}
