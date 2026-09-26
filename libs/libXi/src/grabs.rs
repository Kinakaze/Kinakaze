//! Passive and active grabs on the virtual XI seat, including freeze/replay.
use super::*;
use kinakaze_libdisplay::event::Event;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{LazyLock, Mutex},
};
#[repr(C)]
pub struct Modifiers {
    pub modifiers: i32,
    pub status: i32,
}
#[derive(Clone, Copy)]
pub(crate) struct Grab {
    pub display: usize,
    pub window: Window,
    pub mask: u64,
    pub mode: i32,
    pub paired: i32,
    pub owner: bool,
    pub passive: bool,
    pub detail: i32,
}
#[derive(Default)]
struct State {
    passive: BTreeMap<(usize, Window, i32, i32, u32), Grab>,
    active: BTreeMap<i32, Grab>,
    frozen: BTreeMap<i32, bool>,
    queued: VecDeque<(usize, Event)>,
    last: BTreeMap<i32, (usize, Event)>,
    touches: BTreeMap<(usize, Window, u32), Grab>,
    contacts: BTreeMap<u32, Grab>,
    rejected: std::collections::BTreeSet<u32>,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}
fn error(d: *mut Display, code: u8, minor: u8, id: usize) -> i32 {
    unsafe {
        kinakaze_libX11::errors::report(d, code, 128, minor, id);
    }
    -1
}
fn mask(input: *const XIEventMask) -> Option<u64> {
    if input.is_null() {
        return None;
    }
    let m = unsafe { &*input };
    if m.mask_len < 0 || m.mask_len > 65535 || m.mask_len > 0 && m.mask.is_null() {
        return None;
    }
    let mut bits = 0;
    for n in 0..m.mask_len {
        let byte = unsafe { *m.mask.add(n as usize) };
        if n >= 4 {
            if byte != 0 {
                return None;
            }
        } else {
            bits |= (byte as u64) << (n * 8);
        }
    }
    if bits & !0x1fffffe != 0 {
        None
    } else {
        Some(bits)
    }
}
fn window(d: *mut Display, w: Window) -> bool {
    let mut a = unsafe { core::mem::zeroed() };
    unsafe { kinakaze_libX11::XGetWindowAttributes(d, w, &mut a) != 0 }
}
fn select(
    d: *mut Display,
    device: i32,
    detail: i32,
    w: Window,
    mode: i32,
    paired: i32,
    owner: i32,
    input: *const XIEventMask,
    count: i32,
    mods: *mut Modifiers,
    key: bool,
) -> i32 {
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some()
        || std::env::var_os("KINAKAZE_INPUT_TRACE").is_some()
    {
        kinakaze_libX11::diagnostic!(
            "[XI grab] device={device} detail={detail} window={w:#x} mode={mode} paired={paired} owner={owner} count={count} key={key}"
        );
    }
    let device = if device == 1 {
        if key { 3 } else { 2 }
    } else {
        device
    };
    if device != if key { 3 } else { 2 } {
        return error(d, 128, 54, device as usize);
    }
    if !(0..=1).contains(&mode)
        || !(0..=1).contains(&paired)
        || !(0..=1).contains(&owner)
        || count < 0
        || count > 0 && mods.is_null()
    {
        return error(d, 2, 54, w);
    }
    if !window(d, w) {
        return -1;
    }
    let Some(mask) = mask(input) else {
        return error(d, 2, 54, w);
    };
    if detail < 0 || detail > if key { 255 } else { 32 } {
        return error(d, 2, 54, detail as usize);
    }
    let mut s = state();
    let mut failed = 0;
    for n in 0..count as usize {
        let m = unsafe { &mut *mods.add(n) };
        let modifier = m.modifiers as u32;
        if modifier & !0xff != 0 && modifier != 0x80000000 {
            m.status = 2;
            failed += 1;
            continue;
        }
        s.passive.insert(
            (d as usize, w, device, detail, modifier),
            Grab {
                display: d as usize,
                window: w,
                mask,
                mode,
                paired,
                owner: owner != 0,
                passive: true,
                detail,
            },
        );
        m.status = 0;
    }
    failed
}
fn unselect(
    d: *mut Display,
    device: i32,
    detail: i32,
    w: Window,
    count: i32,
    mods: *const Modifiers,
) -> i32 {
    if !(2..=3).contains(&device) || count < 0 || count > 0 && mods.is_null() {
        return error(d, 2, 55, device as usize);
    }
    let mut s = state();
    for n in 0..count as usize {
        let modifier = unsafe { (*mods.add(n)).modifiers } as u32;
        s.passive.retain(|&(display, window, dev, key, m), _| {
            !(display == d as usize
                && window == w
                && dev == device
                && (detail == 0 || key == detail)
                && (modifier == 0x80000000 || m == modifier))
        });
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGrabButton")]
pub unsafe extern "sysv64" fn XIGrabButton(
    d: *mut Display,
    device: i32,
    button: i32,
    w: Window,
    _cursor: usize,
    mode: i32,
    paired: i32,
    owner: i32,
    mask: *mut XIEventMask,
    count: i32,
    mods: *mut Modifiers,
) -> i32 {
    select(
        d, device, button, w, mode, paired, owner, mask, count, mods, false,
    )
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGrabKeycode")]
pub unsafe extern "sysv64" fn XIGrabKeycode(
    d: *mut Display,
    device: i32,
    key: i32,
    w: Window,
    mode: i32,
    paired: i32,
    owner: i32,
    mask: *mut XIEventMask,
    count: i32,
    mods: *mut Modifiers,
) -> i32 {
    select(
        d, device, key, w, mode, paired, owner, mask, count, mods, true,
    )
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIUngrabButton")]
pub unsafe extern "sysv64" fn XIUngrabButton(
    d: *mut Display,
    device: i32,
    button: i32,
    w: Window,
    count: i32,
    mods: *const Modifiers,
) -> i32 {
    unselect(
        d,
        if device == 1 { 2 } else { device },
        button,
        w,
        count,
        mods,
    )
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIUngrabKeycode")]
pub unsafe extern "sysv64" fn XIUngrabKeycode(
    d: *mut Display,
    device: i32,
    key: i32,
    w: Window,
    count: i32,
    mods: *const Modifiers,
) -> i32 {
    unselect(d, if device == 1 { 3 } else { device }, key, w, count, mods)
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGrabDevice")]
pub unsafe extern "sysv64" fn XIGrabDevice(
    d: *mut Display,
    device: i32,
    w: Window,
    _time: usize,
    _cursor: usize,
    mode: i32,
    paired: i32,
    owner: i32,
    input: *mut XIEventMask,
) -> i32 {
    // Grab changes are sequenced by request serial: GDK ends a grab at
    // NextRequest() and matches later events against it.
    unsafe { kinakaze_libX11::issue_request(d) };
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some()
        || std::env::var_os("KINAKAZE_INPUT_TRACE").is_some()
    {
        kinakaze_libX11::diagnostic!(
            "[libXi] XIGrabDevice device={device} window={w:#x} owner={owner} mode={mode}/{paired}"
        );
    }
    if !(2..=3).contains(&device)
        || !(0..=1).contains(&mode)
        || !(0..=1).contains(&paired)
        || !(0..=1).contains(&owner)
    {
        return error(d, 2, 51, device as usize);
    }
    if !window(d, w) {
        return 3;
    }
    let Some(mask) = mask(input) else {
        return error(d, 2, 51, w);
    };
    if state()
        .active
        .get(&device)
        .is_some_and(|grab| grab.display != d as usize)
    {
        return 1;
    }
    if device == 2 && w != 1 && !kinakaze_libdisplay::ui::interaction::capture(w, true) {
        return 1;
    }
    let mut s = state();
    s.active.insert(
        device,
        Grab {
            display: d as usize,
            window: w,
            mask,
            mode,
            paired,
            owner: owner != 0,
            passive: false,
            detail: 0,
        },
    );
    s.frozen.insert(device, mode == 0);
    if paired == 0 {
        s.frozen.insert(if device == 2 { 3 } else { 2 }, true);
    }
    drop(s);
    if device == 2 {
        kinakaze_libdisplay::ui::pointer_grab::changed(w, true);
    }
    if device == 3 {
        kinakaze_libdisplay::ui::pointer_grab::keyboard_changed(w, true);
    }
    kinakaze_libdisplay::ui::desktop::grab_changed(w, device, true);
    0
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIUngrabDevice")]
pub unsafe extern "sysv64" fn XIUngrabDevice(d: *mut Display, device: i32, time: usize) -> i32 {
    unsafe { kinakaze_libX11::issue_request(d) };
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some()
        || std::env::var_os("KINAKAZE_INPUT_TRACE").is_some()
    {
        kinakaze_libX11::diagnostic!("[libXi] XIUngrabDevice device={device}");
    }
    if state()
        .active
        .get(&device)
        .is_some_and(|grab| grab.display != d as usize)
    {
        return 0;
    }
    if device == 2 {
        kinakaze_libdisplay::ui::interaction::capture(0, false);
    }
    let released = {
        let mut s = state();
        let released = s.active.remove(&device);
        s.frozen.remove(&device);
        released
    };
    if let Some(grab) = released {
        if device == 2 {
            kinakaze_libdisplay::ui::pointer_grab::changed(grab.window, false);
        }
        if device == 3 {
            kinakaze_libdisplay::ui::pointer_grab::keyboard_changed(grab.window, false);
        }
        kinakaze_libdisplay::ui::desktop::grab_changed(grab.window, device, false);
    }
    unsafe { XIAllowEvents(d, device, 3, time as u64) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIAllowTouchEvents")]
pub unsafe extern "sysv64" fn XIAllowTouchEvents(
    d: *mut Display,
    device: i32,
    touch: u32,
    w: Window,
    mode: i32,
) -> i32 {
    if device != 2 || !(0..=1).contains(&mode) {
        return error(d, 2, 53, device as usize);
    }
    let mut s = state();
    if !s.contacts.get(&touch).is_some_and(|g| g.window == w) {
        drop(s);
        return error(d, 2, 53, touch as usize);
    }
    if mode == 1 {
        s.contacts.remove(&touch);
        s.rejected.insert(touch);
    }
    0
}
pub unsafe fn touch_select(
    d: *mut Display,
    device: i32,
    w: Window,
    owner: i32,
    input: *const XIEventMask,
    count: i32,
    mods: *mut Modifiers,
    add: bool,
) -> i32 {
    if !matches!(device, 1 | 2) || count < 0 || count > 0 && mods.is_null() {
        return error(d, 2, if add { 54 } else { 55 }, device as usize);
    }
    if add && !(0..=1).contains(&owner) {
        return error(d, 2, 54, owner as usize);
    }
    for index in 0..count as usize {
        let modifiers = unsafe { (*mods.add(index)).modifiers as u32 };
        if modifiers > 0xff && modifiers != 0x80000000 {
            return error(d, 2, if add { 54 } else { 55 }, modifiers as usize);
        }
    }
    if !window(d, w) {
        return -1;
    }
    let bits = if add {
        let Some(bits) = mask(input) else {
            return error(d, 2, 54, w);
        };
        bits
    } else {
        0
    };
    let mut s = state();
    for n in 0..count as usize {
        let m = unsafe { &mut *mods.add(n) };
        if m.modifiers as u32 == 0x80000000 {
            s.touches
                .retain(|(display, window, _), _| *display != d as usize || *window != w);
        }
        if add {
            s.touches.insert(
                (d as usize, w, m.modifiers as u32),
                Grab {
                    display: d as usize,
                    window: w,
                    mask: bits,
                    mode: 1,
                    paired: 1,
                    owner: owner != 0,
                    passive: true,
                    detail: 0,
                },
            );
        } else {
            s.touches.remove(&(d as usize, w, m.modifiers as u32));
        }
        m.status = 0;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIAllowEvents")]
pub unsafe extern "sysv64" fn XIAllowEvents(
    d: *mut Display,
    device: i32,
    mode: i32,
    _time: u64,
) -> i32 {
    if !(2..=3).contains(&device) || !(0..=5).contains(&mode) {
        return error(d, 2, 53, device as usize);
    }
    let (queued, replay) = {
        let mut s = state();
        s.frozen.insert(device, false);
        if matches!(mode, 3..=5) {
            s.frozen.insert(if device == 2 { 3 } else { 2 }, false);
        }
        if matches!(mode, 0 | 3 | 4) {
            if let Some(g) = s.active.get_mut(&device) {
                g.mode = 1;
            }
        }
        let replay = if mode == 2 {
            s.active.remove(&device);
            s.last.remove(&device)
        } else {
            None
        };
        let queued = core::mem::take(&mut s.queued);
        (queued, replay)
    };
    if let Some((display, event)) = replay {
        super::device_events::queue(display as *mut Display, &event, true);
    }
    for (display, event) in queued {
        super::device_events::queue(display as *mut Display, &event, false);
    }
    0
}
pub(crate) fn route(
    d: *mut Display,
    event: &Event,
    device: i32,
    detail: i32,
    window: Window,
    press: bool,
    release: bool,
    replay: bool,
) -> Result<Option<Grab>, ()> {
    let mods = kinakaze_libX11::xkb::server::state().mods as u32;
    let mut s = state();
    if matches!(
        event.kind,
        kinakaze_libdisplay::event::EVENT_TOUCH_BEGIN
            | kinakaze_libdisplay::event::EVENT_TOUCH_UPDATE
            | kinakaze_libdisplay::event::EVENT_TOUCH_END
    ) {
        if s.rejected.contains(&event.touch_id) {
            if event.kind == kinakaze_libdisplay::event::EVENT_TOUCH_END {
                s.rejected.remove(&event.touch_id);
            }
            return Err(());
        }
        if event.kind == kinakaze_libdisplay::event::EVENT_TOUCH_BEGIN {
            let g = s
                .touches
                .get(&(d as usize, window, mods))
                .or_else(|| s.touches.get(&(d as usize, window, 0x80000000)))
                .or_else(|| s.touches.get(&(d as usize, 1, mods)))
                .or_else(|| s.touches.get(&(d as usize, 1, 0x80000000)))
                .copied();
            if let Some(g) = g {
                s.contacts.insert(event.touch_id, g);
            }
        }
        let grab = s.contacts.get(&event.touch_id).copied();
        if event.kind == kinakaze_libdisplay::event::EVENT_TOUCH_END {
            s.contacts.remove(&event.touch_id);
        }
        return Ok(grab);
    }
    if s.active
        .get(&device)
        .is_some_and(|grab| grab.display != d as usize)
    {
        return Err(());
    }
    if s.frozen.get(&device) == Some(&true) {
        s.queued.push_back((d as usize, *event));
        return Err(());
    }
    if press && !replay && !s.active.contains_key(&device) {
        let grab = s
            .passive
            .iter()
            .rev()
            .find(|&(&(display, w, dev, key, m), _)| {
                display == d as usize
                    && dev == device
                    && (w == window || w == 1)
                    && (key == 0 || key == detail)
                    && (m == mods || m == 0x80000000)
            })
            .map(|(_, g)| *g);
        if let Some(g) = grab {
            s.active.insert(device, g);
            if g.paired == 0 {
                s.frozen.insert(if device == 2 { 3 } else { 2 }, true);
            }
        }
    }
    let grab = s.active.get(&device).copied();
    if let Some(g) = grab {
        if g.mode == 0 {
            s.frozen.insert(device, true);
            s.last.insert(device, (d as usize, *event));
        }
        if release && g.passive && (g.detail == 0 || g.detail == detail) {
            s.active.remove(&device);
            s.frozen.insert(device, false);
            s.frozen.insert(if device == 2 { 3 } else { 2 }, false);
        }
    }
    Ok(grab)
}
pub(crate) fn close_display(display: *mut Display) {
    for device in 2..=3 {
        if state()
            .active
            .get(&device)
            .is_some_and(|g| g.display == display as usize)
        {
            unsafe {
                XIUngrabDevice(display, device, 0);
            }
        }
    }
    let mut s = state();
    s.passive.retain(|(d, ..), _| *d != display as usize);
    s.touches.retain(|(d, ..), _| *d != display as usize);
    s.contacts.retain(|_, g| g.display != display as usize);
    s.queued.retain(|(d, _)| *d != display as usize);
    s.last.retain(|_, (d, _)| *d != display as usize);
}
/// Grabs end with their window, as on the server; the native capture that
/// backed an active pointer grab on it is released the same way.
pub(crate) fn window_destroyed(window: Window) {
    kinakaze_libdisplay::ui::pointer_grab::changed(window, false);
    kinakaze_libdisplay::ui::pointer_grab::keyboard_changed(window, false);
    let mut s = state();
    let released_pointer = s.active.get(&2).is_some_and(|grab| grab.window == window);
    s.active.retain(|_, grab| grab.window != window);
    s.passive.retain(|(_, w, ..), _| *w != window);
    s.touches.retain(|(_, w, _), _| *w != window);
    s.contacts.retain(|_, grab| grab.window != window);
    drop(s);
    for device in 2..=3 {
        kinakaze_libdisplay::ui::desktop::grab_changed(window, device, false);
    }
    if released_pointer {
        kinakaze_libdisplay::ui::interaction::capture(0, false);
    }
}

#[cfg(test)]
pub(crate) fn close() {
    kinakaze_libdisplay::ui::interaction::capture(0, false);
    super::device_events::reset_buttons();
    let previous = core::mem::take(&mut *state());
    for (device, grab) in previous.active {
        if device == 2 {
            kinakaze_libdisplay::ui::pointer_grab::changed(grab.window, false);
        }
        if device == 3 {
            kinakaze_libdisplay::ui::pointer_grab::keyboard_changed(grab.window, false);
        }
        kinakaze_libdisplay::ui::desktop::grab_changed(grab.window, device, false);
    }
}
