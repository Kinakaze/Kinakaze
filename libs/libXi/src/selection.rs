//! Per-window/device XI masks. Raw input is selected on the virtual root.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
pub(crate) static MUTATION: Mutex<()> = Mutex::new(());
pub(crate) fn masks() -> &'static Mutex<BTreeMap<(usize, usize, i32), u64>> {
    static MAP: OnceLock<Mutex<BTreeMap<(usize, usize, i32), u64>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub(crate) fn selected(display: *mut Display, event: i32) -> bool {
    let device = if event <= XI_RAW_KEY_RELEASE { 3 } else { 2 };
    let map = masks().lock().unwrap_or_else(|e| e.into_inner());
    [0, 1, device, device + 2].into_iter().any(|id| {
        map.get(&(display as usize, 1, id))
            .is_some_and(|bits| bits & (1u64 << event) != 0)
    })
}
#[unsafe(export_name = "kinakaze_engine_libXi_XISelectEvents")]
pub unsafe extern "sysv64" fn XISelectEvents(
    display: *mut Display,
    window: Window,
    input: *mut XIEventMask,
    count: c_int,
) -> Status {
    unsafe { kinakaze_libX11::issue_request(display) };
    if count < 0 || count > 0 && input.is_null() {
        return 2;
    }
    // The virtual seat has six selectors. Repeated selectors replace the
    // earlier mask without allocating a temporary request list.
    let mut updates = [None; 6];
    for index in 0..count as usize {
        unsafe {
            let mask = &*input.add(index);
            if !(0..=5).contains(&mask.deviceid)
                || mask.mask_len < 0
                || mask.mask_len > 0 && mask.mask.is_null()
            {
                return 2;
            }
            let mut bits = 0u64;
            for i in 0..mask.mask_len as usize {
                let byte = *mask.mask.add(i);
                if i >= 4 {
                    if byte != 0 {
                        return 2;
                    }
                } else {
                    bits |= (byte as u64) << (i * 8);
                }
            }
            if bits & !0x7fffffe != 0 {
                return 2;
            } // XI 2.3 event numbers 1..26.
            if window != 1 && bits & 0x3e000 != 0 {
                return kinakaze_libX11::errors::report(display, 2, 128, 46, window);
            }
            updates[mask.deviceid as usize] = Some(bits);
        }
    }
    // An X error invokes application code, which may reenter XI or fork. Only
    // acquire the mutation guard after validation has completed.
    let _mutation = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = masks().lock().unwrap_or_else(|e| e.into_inner());
    for (device, value) in updates.into_iter().enumerate() {
        let Some(value) = value else {
            continue;
        };
        let key = (display as usize, window, device as i32);
        if value == 0 {
            map.remove(&key);
        } else {
            map.insert(key, value);
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGetSelectedEvents")]
pub unsafe extern "sysv64" fn XIGetSelectedEvents(
    display: *mut Display,
    window: Window,
    count: *mut c_int,
) -> *mut XIEventMask {
    if count.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let map = masks().lock().unwrap_or_else(|e| e.into_inner());
    let n = map
        .range((display as usize, window, i32::MIN)..=(display as usize, window, i32::MAX))
        .count();
    if n == 0 {
        return core::ptr::null_mut();
    }
    let size = core::mem::size_of::<XIEventMask>();
    let output = unsafe { kinakaze_alloc::guest::malloc(n * (size + 4)) }.cast::<XIEventMask>();
    if output.is_null() {
        return output;
    }
    let bytes = unsafe { output.add(n).cast::<u8>() };
    for (index, ((_, _, device), bits)) in map
        .range((display as usize, window, i32::MIN)..=(display as usize, window, i32::MAX))
        .enumerate()
    {
        unsafe {
            let mask = bytes.add(index * 4);
            core::ptr::copy_nonoverlapping(bits.to_le_bytes().as_ptr(), mask, 4);
            output.add(index).write(XIEventMask {
                deviceid: *device,
                mask_len: 4,
                mask,
            });
        }
    }
    unsafe {
        *count = n as i32;
    }
    output
}
