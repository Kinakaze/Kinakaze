//! Translate Windows touch pointers without changing the shared event ABI.
use super::{Handle, Point, ScreenToClient};
use crate::event::{self, Event};

#[repr(C)]
struct PointerInfo {
    kind: u32,
    id: u32,
    frame: u32,
    flags: u32,
    source: Handle,
    target: Handle,
    pixel: Point,
    himetric: Point,
    pixel_raw: Point,
    himetric_raw: Point,
    time: u32,
    history: u32,
    input_data: i32,
    key_states: u32,
    performance_count: u64,
    button_change: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetPointerInfo(id: u32, info: *mut PointerInfo) -> i32;
}

pub(super) fn handle(window: Handle, message: u32, id: u32, logical: u64, time: u64) -> bool {
    let mut info: PointerInfo = unsafe { core::mem::zeroed() };
    if unsafe { GetPointerInfo(id, &raw mut info) } == 0 || info.kind != 2 {
        return false; // PT_TOUCH only; mouse/pen keep their normal processing.
    }
    let mut point = info.pixel;
    if unsafe { ScreenToClient(window, &raw mut point) } == 0 {
        return false;
    }
    let kind = if message == 0x247 || info.flags & 0x8000 != 0 {
        event::EVENT_TOUCH_END
    } else if message == 0x246 {
        event::EVENT_TOUCH_BEGIN
    } else {
        event::EVENT_TOUCH_UPDATE
    };
    event::publish(Event {
        kind,
        touch_id: info.id,
        x: point.x,
        y: point.y,
        time_ms: time,
        window: logical,
        ..Event::default()
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_info_matches_the_windows_x64_abi() {
        use core::mem::{offset_of, size_of};
        assert_eq!(size_of::<PointerInfo>(), 96);
        assert_eq!(offset_of!(PointerInfo, pixel), 32);
        assert_eq!(offset_of!(PointerInfo, time), 64);
        assert_eq!(offset_of!(PointerInfo, performance_count), 80);
    }
}
