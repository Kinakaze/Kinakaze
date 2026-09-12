//! Owned PulseAudio property lists. Returned strings remain valid until mutation.
use super::*;
use std::ffi::CString;

#[derive(Clone, Default)]
pub struct pa_proplist {
    entries: Vec<(CString, Vec<u8>)>,
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_new")]
pub extern "sysv64" fn pa_proplist_new() -> *mut pa_proplist {
    Box::into_raw(Box::new(pa_proplist::default()))
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_free")]
pub unsafe extern "sysv64" fn pa_proplist_free(list: *mut pa_proplist) {
    if !list.is_null() {
        unsafe { drop(Box::from_raw(list)) };
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_set")]
pub unsafe extern "sysv64" fn pa_proplist_set(
    list: *mut pa_proplist,
    key: *const c_char,
    data: *const c_void,
    length: usize,
) -> c_int {
    if list.is_null() || key.is_null() || (data.is_null() && length != 0) {
        return -1;
    }
    let key = unsafe { CStr::from_ptr(key) };
    if key.to_bytes().is_empty()
        || !key
            .to_bytes()
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
    {
        return -1;
    }
    let value = if length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec()
    };
    let list = unsafe { &mut *list };
    if let Some(entry) = list.entries.iter_mut().find(|(k, _)| k.as_c_str() == key) {
        entry.1 = value;
    } else {
        list.entries.push((key.to_owned(), value));
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_sets")]
pub unsafe extern "sysv64" fn pa_proplist_sets(
    list: *mut pa_proplist,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    if value.is_null() {
        return -1;
    }
    let bytes = unsafe { CStr::from_ptr(value) }.to_bytes_with_nul();
    unsafe { pa_proplist_set(list, key, bytes.as_ptr().cast(), bytes.len()) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_gets")]
pub unsafe extern "sysv64" fn pa_proplist_gets(
    list: *const pa_proplist,
    key: *const c_char,
) -> *const c_char {
    if list.is_null() || key.is_null() {
        return ptr::null();
    }
    let key = unsafe { CStr::from_ptr(key) };
    unsafe { &*list }
        .entries
        .iter()
        .find(|(k, _)| k.as_c_str() == key)
        .filter(|(_, v)| CStr::from_bytes_with_nul(v).is_ok())
        .map_or(ptr::null(), |(_, v)| v.as_ptr().cast())
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_iterate")]
pub unsafe extern "sysv64" fn pa_proplist_iterate(
    list: *const pa_proplist,
    state: *mut *mut c_void,
) -> *const c_char {
    if list.is_null() || state.is_null() {
        return ptr::null();
    }
    let index = unsafe { *state } as usize;
    let Some((key, _)) = (unsafe { &*list }).entries.get(index) else {
        return ptr::null();
    };
    unsafe { *state = (index + 1) as *mut c_void };
    key.as_ptr()
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_proplist_copy")]
pub unsafe extern "sysv64" fn pa_proplist_copy(list: *const pa_proplist) -> *mut pa_proplist {
    unsafe { list.as_ref() }.map_or(ptr::null_mut(), |p| Box::into_raw(Box::new(p.clone())))
}
