//! Signed 64-bit SYNC values and owned counters. Alarms/await are not advertised.
use super::*;
use std::{collections::HashMap, sync::Mutex};
mod lifecycle;
mod objects;
mod shared_counters;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XSyncValue {
    pub hi: i32,
    pub lo: u32,
}
impl XSyncValue {
    fn from_i64(v: i64) -> Self {
        Self {
            hi: (v >> 32) as i32,
            lo: v as u32,
        }
    }
    fn value(self) -> i64 {
        ((self.hi as i64) << 32) | self.lo as i64
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncIntToValue")]
pub unsafe extern "sysv64" fn XSyncIntToValue(out: *mut XSyncValue, v: i32) {
    unsafe {
        out.write(XSyncValue::from_i64(v as i64));
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncIntsToValue")]
pub unsafe extern "sysv64" fn XSyncIntsToValue(out: *mut XSyncValue, lo: u32, hi: i32) {
    unsafe {
        out.write(XSyncValue { hi, lo });
    }
}
macro_rules! compare {
    ($name:ident,$export:literal,$op:tt) => {
        #[unsafe(export_name=$export)] pub extern "sysv64" fn $name(a:XSyncValue,b:XSyncValue)->Bool { (a.value() $op b.value()) as Bool }
    }
}
compare!(XSyncValueGreaterThan,"kinakaze_engine_libXext_XSyncValueGreaterThan",>);
compare!(XSyncValueLessThan,"kinakaze_engine_libXext_XSyncValueLessThan",<);
compare!(XSyncValueGreaterOrEqual,"kinakaze_engine_libXext_XSyncValueGreaterOrEqual",>=);
compare!(XSyncValueLessOrEqual,"kinakaze_engine_libXext_XSyncValueLessOrEqual",<=);
compare!(XSyncValueEqual,"kinakaze_engine_libXext_XSyncValueEqual",==);
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueIsNegative")]
pub extern "sysv64" fn XSyncValueIsNegative(v: XSyncValue) -> Bool {
    (v.hi < 0) as Bool
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueIsPositive")]
pub extern "sysv64" fn XSyncValueIsPositive(v: XSyncValue) -> Bool {
    (v.hi >= 0) as Bool
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueIsZero")]
pub extern "sysv64" fn XSyncValueIsZero(v: XSyncValue) -> Bool {
    (v.value() == 0) as Bool
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueLow32")]
pub extern "sysv64" fn XSyncValueLow32(v: XSyncValue) -> u32 {
    v.lo
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueHigh32")]
pub extern "sysv64" fn XSyncValueHigh32(v: XSyncValue) -> i32 {
    v.hi
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncMaxValue")]
pub unsafe extern "sysv64" fn XSyncMaxValue(out: *mut XSyncValue) {
    unsafe {
        out.write(XSyncValue::from_i64(i64::MAX));
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncMinValue")]
pub unsafe extern "sysv64" fn XSyncMinValue(out: *mut XSyncValue) {
    unsafe {
        out.write(XSyncValue::from_i64(i64::MIN));
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueAdd")]
pub unsafe extern "sysv64" fn XSyncValueAdd(
    out: *mut XSyncValue,
    a: XSyncValue,
    b: XSyncValue,
    overflow: *mut Bool,
) {
    let (v, o) = a.value().overflowing_add(b.value());
    unsafe {
        out.write(XSyncValue::from_i64(v));
        overflow.write(o as Bool);
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncValueSubtract")]
pub unsafe extern "sysv64" fn XSyncValueSubtract(
    out: *mut XSyncValue,
    a: XSyncValue,
    b: XSyncValue,
    overflow: *mut Bool,
) {
    let (v, o) = a.value().overflowing_sub(b.value());
    unsafe {
        out.write(XSyncValue::from_i64(v));
        overflow.write(o as Bool);
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncQueryExtension")]
pub unsafe extern "sysv64" fn XSyncQueryExtension(
    _: *mut Display,
    event: *mut c_int,
    error: *mut c_int,
) -> Bool {
    unsafe {
        if !event.is_null() {
            *event = 88;
        }
        if !error.is_null() {
            *error = 152;
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncInitialize")]
pub unsafe extern "sysv64" fn XSyncInitialize(
    d: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Status {
    objects::initialize(d);
    unsafe {
        if !major.is_null() {
            *major = 3;
        }
        if !minor.is_null() {
            *minor = 1;
        }
    }
    1
}
#[derive(Clone, Copy)]
struct Counter {
    display: usize,
    value: i64,
}
struct Counters {
    next: usize,
    values: HashMap<usize, Counter>,
}
static COUNTERS: Mutex<Option<Counters>> = Mutex::new(None);
fn state(guard: &mut Option<Counters>) -> &mut Counters {
    guard.get_or_insert_with(|| Counters {
        next: 0x20_0000,
        values: HashMap::new(),
    })
}
unsafe fn invalid(d: *mut Display, id: usize, minor: u8) -> Status {
    unsafe {
        kinakaze_libX11::errors::report(d, 152, 134, minor, id);
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncCreateCounter")]
pub unsafe extern "sysv64" fn XSyncCreateCounter(d: *mut Display, value: XSyncValue) -> usize {
    let Some(id) = shared_counters::create(d as usize, value.value()) else {
        return 0;
    };
    let mut guard = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
    let s = state(&mut guard);
    s.next = s.next.max(id + 1);
    s.values.insert(
        id,
        Counter {
            display: d as usize,
            value: value.value(),
        },
    );
    id
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncDestroyCounter")]
pub unsafe extern "sysv64" fn XSyncDestroyCounter(d: *mut Display, id: usize) -> Status {
    let found = shared_counters::destroy(id, false);
    if let Some(s) = COUNTERS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.values.remove(&id);
    }
    if found {
        objects::counter_destroyed(d, id);
        1
    } else {
        unsafe { invalid(d, id, 6) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncSetCounter")]
pub unsafe extern "sysv64" fn XSyncSetCounter(
    d: *mut Display,
    id: usize,
    value: XSyncValue,
) -> Status {
    kinakaze_libX11::frame_sync::counter_changing(id, value.value());
    match shared_counters::set(id, value.value(), false) {
        Ok(value) => {
            if let Some(c) = state(&mut COUNTERS.lock().unwrap_or_else(|e| e.into_inner()))
                .values
                .get_mut(&id)
            {
                c.value = value;
            }
            objects::changed();
            1
        }
        Err(code) => {
            unsafe {
                kinakaze_libX11::errors::report(d, code, 134, 3, id);
            }
            0
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncQueryCounter")]
pub unsafe extern "sysv64" fn XSyncQueryCounter(
    d: *mut Display,
    id: usize,
    value: *mut XSyncValue,
) -> Status {
    if value.is_null() {
        return 0;
    }
    if let Some(v) = objects::system_value(id) {
        unsafe {
            value.write(XSyncValue::from_i64(v));
        }
        return 1;
    }
    if let Some(v) = shared_counters::value(id) {
        unsafe {
            value.write(XSyncValue::from_i64(v));
        }
        1
    } else {
        unsafe { invalid(d, id, 5) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncChangeCounter")]
pub unsafe extern "sysv64" fn XSyncChangeCounter(
    d: *mut Display,
    id: usize,
    value: XSyncValue,
) -> Status {
    if let Some(next) = shared_counters::value(id).and_then(|old| old.checked_add(value.value())) {
        kinakaze_libX11::frame_sync::counter_changing(id, next);
    }
    match shared_counters::set(id, value.value(), true) {
        Ok(value) => {
            if let Some(c) = state(&mut COUNTERS.lock().unwrap_or_else(|e| e.into_inner()))
                .values
                .get_mut(&id)
            {
                c.value = value;
            }
            objects::changed();
            1
        }
        Err(code) => {
            unsafe {
                kinakaze_libX11::errors::report(d, code, 134, 4, id);
            }
            0
        }
    }
}
unsafe fn close(d: *mut Display) {
    let ids = {
        let mut guard = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
        let s = state(&mut guard);
        let ids: Vec<_> = s
            .values
            .iter()
            .filter_map(|(&id, c)| (c.display == d as usize).then_some(id))
            .collect();
        for id in &ids {
            s.values.remove(id);
        }
        ids
    };
    objects::close(d);
    for id in ids {
        if shared_counters::destroy(id, true) {
            objects::counter_destroyed(d, id);
        }
    }
}
