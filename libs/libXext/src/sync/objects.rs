//! SYNC alarm transitions, native idle/monotonic counters and rendering fences.
use super::*;
mod lifecycle;
use std::{
    sync::{
        Condvar, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    System::SystemInformation::GetTickCount64,
    UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
};
const SERVER_TIME: usize = 0x1f0000;
const IDLE_TIME: usize = 0x1f0001;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Trigger {
    counter: usize,
    value_type: i32,
    wait_value: XSyncValue,
    test_type: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AlarmAttributes {
    trigger: Trigger,
    delta: XSyncValue,
    events: i32,
    state: i32,
}
#[derive(Clone, Copy)]
struct Alarm {
    display: usize,
    attributes: AlarmAttributes,
    previous: i64,
}
#[derive(Default)]
struct Objects {
    alarms: HashMap<usize, Alarm>,
    fences: HashMap<usize, (usize, bool)>,
    priorities: HashMap<usize, i32>,
}
static OBJECTS: std::sync::LazyLock<Mutex<Objects>> =
    std::sync::LazyLock::new(|| Mutex::new(Objects::default()));
static CHANGED: Condvar = Condvar::new();
static TIMER: AtomicBool = AtomicBool::new(false);
fn objects() -> std::sync::MutexGuard<'static, Objects> {
    OBJECTS.lock().unwrap_or_else(|e| e.into_inner())
}
pub fn system_value(id: usize) -> Option<i64> {
    match id {
        SERVER_TIME => Some(unsafe { GetTickCount64() } as i64),
        IDLE_TIME => {
            let mut v = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            if unsafe { GetLastInputInfo(&mut v) } != 0 {
                Some((unsafe { GetTickCount64() } as u32).wrapping_sub(v.dwTime) as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}
fn value(id: usize) -> Option<i64> {
    system_value(id).or_else(|| shared_counters::value(id))
}
pub fn initialize(_d: *mut Display) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        kinakaze_libX11::xext::protocol::register_poll(|_, closing| {
            if !closing {
                changed();
            }
        });
    });
}
#[repr(C)]
pub struct SystemCounter {
    name: *mut c_char,
    counter: usize,
    resolution: XSyncValue,
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncListSystemCounters")]
pub unsafe extern "sysv64" fn XSyncListSystemCounters(
    d: *mut Display,
    count: *mut i32,
) -> *mut SystemCounter {
    initialize(d);
    if count.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let out = unsafe { guest::malloc(2 * size_of::<SystemCounter>() + 21) }.cast::<SystemCounter>();
    if out.is_null() {
        return out;
    }
    unsafe {
        let names = out.add(2).cast::<u8>();
        ptr::copy_nonoverlapping(b"SERVERTIME\0IDLETIME\0".as_ptr(), names, 20);
        out.write(SystemCounter {
            name: names.cast(),
            counter: SERVER_TIME,
            resolution: XSyncValue::from_i64(1),
        });
        out.add(1).write(SystemCounter {
            name: names.add(11).cast(),
            counter: IDLE_TIME,
            resolution: XSyncValue::from_i64(1),
        });
        *count = 2;
    }
    out
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncFreeSystemCounterList")]
pub unsafe extern "sysv64" fn XSyncFreeSystemCounterList(p: *mut SystemCounter) {
    unsafe {
        guest::free(p.cast());
    }
}
fn fail(d: *mut Display, code: u8, minor: u8, id: usize) -> i32 {
    unsafe {
        kinakaze_libX11::errors::report(d, code, 134, minor, id);
    }
    0
}
fn apply(
    mut a: AlarmAttributes,
    mask: u64,
    input: *const AlarmAttributes,
) -> Option<(AlarmAttributes, i64)> {
    if mask & !63 != 0 || mask != 0 && input.is_null() {
        return None;
    }
    if mask != 0 {
        let b = unsafe { &*input };
        if mask & 1 != 0 {
            a.trigger.counter = b.trigger.counter;
        }
        if mask & 2 != 0 {
            a.trigger.value_type = b.trigger.value_type;
        }
        if mask & 4 != 0 {
            a.trigger.wait_value = b.trigger.wait_value;
        }
        if mask & 8 != 0 {
            a.trigger.test_type = b.trigger.test_type;
        }
        if mask & 16 != 0 {
            a.delta = b.delta;
        }
        if mask & 32 != 0 {
            a.events = b.events;
        }
    }
    if !(0..=1).contains(&a.trigger.value_type)
        || !(0..=3).contains(&a.trigger.test_type)
        || !(0..=1).contains(&a.events)
    {
        return None;
    }
    let now = if a.trigger.counter == 0 {
        0
    } else {
        value(a.trigger.counter)?
    };
    if a.trigger.value_type == 1 && mask & 4 != 0 {
        a.trigger.wait_value = XSyncValue::from_i64(now.checked_add(a.trigger.wait_value.value())?);
    }
    a.trigger.value_type = 0;
    a.state = if a.trigger.counter == 0 { 1 } else { 0 };
    let previous = if matches!(a.trigger.test_type, 0 | 2) {
        a.trigger.wait_value.value().saturating_sub(1)
    } else {
        a.trigger.wait_value.value().saturating_add(1)
    };
    Some((a, previous))
}
fn condition(t: Trigger, old: i64, now: i64) -> bool {
    let v = t.wait_value.value();
    match t.test_type {
        0 => old < v && now >= v,
        1 => old > v && now <= v,
        2 => now >= v,
        3 => now <= v,
        _ => false,
    }
}
#[repr(C)]
struct AlarmEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut Display,
    alarm: usize,
    counter: XSyncValue,
    alarm_value: XSyncValue,
    time: u64,
    state: i32,
}
fn notify(id: usize, a: Alarm, now: i64) {
    kinakaze_libX11::frame_sync::trace(format_args!(
        "alarm={id:#x} counter={:#x} value={now} state={}",
        a.attributes.trigger.counter, a.attributes.state
    ));
    if a.attributes.events == 0 {
        return;
    }
    let mut event: kinakaze_libX11::XEvent = unsafe { mem::zeroed() };
    unsafe {
        (&raw mut event).cast::<AlarmEvent>().write(AlarmEvent {
            kind: 89,
            serial: 0,
            send: 0,
            display: a.display as *mut Display,
            alarm: id,
            counter: XSyncValue::from_i64(now),
            alarm_value: a.attributes.trigger.wait_value,
            time: GetTickCount64(),
            state: a.attributes.state,
        });
    }
    kinakaze_libX11::queue_extension_event(a.display as *mut Display, event);
}
pub fn changed() {
    let ids: Vec<usize> = objects()
        .alarms
        .values()
        .map(|a| a.attributes.trigger.counter)
        .collect();
    let values: HashMap<usize, i64> = ids
        .into_iter()
        .filter_map(|id| value(id).map(|v| (id, v)))
        .collect();
    let mut notifications = Vec::new();
    {
        let mut o = objects();
        for (&id, a) in o.alarms.iter_mut() {
            if a.attributes.state != 0 {
                continue;
            }
            let Some(&now) = values.get(&a.attributes.trigger.counter) else {
                shared_counters::unwatch(a.attributes.trigger.counter, id);
                a.attributes.state = 1;
                a.attributes.trigger.counter = 0;
                notifications.push((id, *a, a.previous));
                continue;
            };
            if condition(a.attributes.trigger, a.previous, now) {
                let old = a.attributes.trigger.wait_value;
                let delta = a.attributes.delta.value();
                if delta == 0 {
                    a.attributes.state = 1;
                } else {
                    let threshold = old.value() as i128;
                    let delta128 = delta as i128;
                    let positive = matches!(a.attributes.trigger.test_type, 0 | 2);
                    if positive && delta < 0 || !positive && delta > 0 {
                        a.attributes.state = 1;
                    } else {
                        let steps = if positive {
                            ((now as i128 - threshold) / delta128 + 1).max(1)
                        } else {
                            ((threshold - now as i128) / -delta128 + 1).max(1)
                        };
                        if let Ok(next) = i64::try_from(threshold + steps * delta128) {
                            a.attributes.trigger.wait_value = XSyncValue::from_i64(next);
                        } else {
                            a.attributes.state = 1;
                        }
                    }
                }
                let mut notification = *a;
                notification.attributes.trigger.wait_value = old;
                notifications.push((id, notification, now));
            }
            a.previous = now;
        }
    }
    for (id, a, now) in notifications {
        notify(id, a, now);
    }
    CHANGED.notify_all();
}
fn start_timer() {
    if TIMER
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("xsync-counters".into())
        .spawn(|| {
            loop {
                changed();
                let o = objects();
                if !o.alarms.values().any(|a| {
                    a.attributes.state == 0
                        && matches!(a.attributes.trigger.counter, SERVER_TIME | IDLE_TIME)
                }) {
                    TIMER.store(false, Ordering::Release);
                    break;
                }
                let _ = CHANGED.wait_timeout(o, Duration::from_millis(10));
            }
        });
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncCreateAlarm")]
pub unsafe extern "sysv64" fn XSyncCreateAlarm(
    d: *mut Display,
    mask: u64,
    input: *const AlarmAttributes,
) -> usize {
    initialize(d);
    let default = AlarmAttributes {
        trigger: Trigger {
            counter: 0,
            value_type: 0,
            wait_value: XSyncValue::from_i64(0),
            test_type: 2,
        },
        delta: XSyncValue::from_i64(1),
        events: 1,
        state: 1,
    };
    let Some((attributes, previous)) = apply(default, mask, input) else {
        fail(d, 2, 8, mask as usize);
        return 0;
    };
    let id = kinakaze_libX11::graphics::allocate_drawable_id();
    objects().alarms.insert(
        id,
        Alarm {
            display: d as usize,
            attributes,
            previous,
        },
    );
    shared_counters::watch(attributes.trigger.counter, id);
    changed();
    start_timer();
    id
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncChangeAlarm")]
pub unsafe extern "sysv64" fn XSyncChangeAlarm(
    d: *mut Display,
    id: usize,
    mask: u64,
    input: *const AlarmAttributes,
) -> i32 {
    let Some(old) = objects().alarms.get(&id).copied() else {
        return fail(d, 153, 9, id);
    };
    let Some((attributes, previous)) = apply(old.attributes, mask, input) else {
        return fail(d, 2, 9, id);
    };
    objects().alarms.insert(
        id,
        Alarm {
            display: old.display,
            attributes,
            previous,
        },
    );
    if old.attributes.trigger.counter != attributes.trigger.counter {
        shared_counters::unwatch(old.attributes.trigger.counter, id);
    }
    shared_counters::watch(attributes.trigger.counter, id);
    changed();
    start_timer();
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncQueryAlarm")]
pub unsafe extern "sysv64" fn XSyncQueryAlarm(
    d: *mut Display,
    id: usize,
    out: *mut AlarmAttributes,
) -> i32 {
    let a = objects().alarms.get(&id).copied();
    if let Some(a) = a {
        if out.is_null() {
            return 0;
        }
        unsafe {
            out.write(a.attributes);
        }
        1
    } else {
        fail(d, 153, 10, id)
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncDestroyAlarm")]
pub unsafe extern "sysv64" fn XSyncDestroyAlarm(d: *mut Display, id: usize) -> i32 {
    let a = objects().alarms.remove(&id);
    if let Some(mut a) = a {
        shared_counters::unwatch(a.attributes.trigger.counter, id);
        a.attributes.state = 2;
        notify(id, a, value(a.attributes.trigger.counter).unwrap_or(0));
        CHANGED.notify_all();
        1
    } else {
        fail(d, 153, 11, id)
    }
}
pub fn counter_destroyed(_d: *mut Display, id: usize) {
    let mut notifications = Vec::new();
    {
        let mut o = objects();
        for (&alarm, a) in o.alarms.iter_mut() {
            if a.attributes.trigger.counter == id {
                shared_counters::unwatch(id, alarm);
                a.attributes.state = 1;
                a.attributes.trigger.counter = 0;
                notifications.push((alarm, *a));
            }
        }
    }
    for (id, a) in notifications {
        notify(id, a, a.previous);
    }
    CHANGED.notify_all();
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncSetPriority")]
pub unsafe extern "sysv64" fn XSyncSetPriority(_d: *mut Display, id: usize, p: i32) -> i32 {
    objects().priorities.insert(id, p);
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncGetPriority")]
pub unsafe extern "sysv64" fn XSyncGetPriority(_d: *mut Display, id: usize, p: *mut i32) -> i32 {
    if p.is_null() {
        return 0;
    }
    unsafe {
        *p = objects().priorities.get(&id).copied().unwrap_or(0);
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncCreateFence")]
pub unsafe extern "sysv64" fn XSyncCreateFence(
    d: *mut Display,
    drawable: usize,
    triggered: i32,
) -> usize {
    if kinakaze_libX11::graphics::drawable_dimensions(drawable).is_none() {
        fail(d, 9, 14, drawable);
        return 0;
    }
    let id = kinakaze_libX11::graphics::allocate_drawable_id();
    objects().fences.insert(id, (d as usize, triggered != 0));
    id
}
fn fence(d: *mut Display, id: usize, minor: u8, change: Option<bool>, out: *mut i32) -> i32 {
    let mut o = objects();
    let Some(f) = o.fences.get_mut(&id) else {
        return fail(d, 154, minor, id);
    };
    if let Some(v) = change {
        f.1 = v;
    }
    if !out.is_null() {
        unsafe {
            *out = f.1 as i32;
        }
    }
    CHANGED.notify_all();
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncTriggerFence")]
pub unsafe extern "sysv64" fn XSyncTriggerFence(d: *mut Display, id: usize) -> i32 {
    unsafe {
        kinakaze_libX11::XSync(d, 0);
    }
    fence(d, id, 15, Some(true), ptr::null_mut())
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncResetFence")]
pub unsafe extern "sysv64" fn XSyncResetFence(d: *mut Display, id: usize) -> i32 {
    fence(d, id, 16, Some(false), ptr::null_mut())
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncQueryFence")]
pub unsafe extern "sysv64" fn XSyncQueryFence(d: *mut Display, id: usize, out: *mut i32) -> i32 {
    fence(d, id, 18, None, out)
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncDestroyFence")]
pub unsafe extern "sysv64" fn XSyncDestroyFence(d: *mut Display, id: usize) -> i32 {
    let removed = objects().fences.remove(&id).is_some();
    CHANGED.notify_all();
    if removed { 1 } else { fail(d, 154, 17, id) }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncAwaitFence")]
pub unsafe extern "sysv64" fn XSyncAwaitFence(
    d: *mut Display,
    ids: *const usize,
    count: i32,
) -> i32 {
    if count < 0 || count > 0 && ids.is_null() {
        return fail(d, 2, 19, count as usize);
    }
    if count == 0 {
        return 1;
    }
    let ids = unsafe { core::slice::from_raw_parts(ids, count as usize) };
    let mut o = objects();
    loop {
        for id in ids {
            if !o.fences.contains_key(id) {
                drop(o);
                return fail(d, 154, 19, *id);
            }
        }
        if ids.iter().any(|id| o.fences[id].1) {
            return 1;
        }
        o = CHANGED.wait(o).unwrap_or_else(|e| e.into_inner());
    }
}
pub fn close(d: *mut Display) {
    let mut o = objects();
    o.alarms.retain(|&id, a| {
        if a.display != d as usize {
            return true;
        }
        shared_counters::unwatch(a.attributes.trigger.counter, id);
        false
    });
    o.fences.retain(|_, f| f.0 != d as usize);
    CHANGED.notify_all();
}
