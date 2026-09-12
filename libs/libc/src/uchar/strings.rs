//! Wide strings share the validated scalar codec and honor explicit mbstate.
use super::*;
use core::ffi::c_int;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbrtowc(
    out: *mut i32,
    input: *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe { mbrtowc(out, input, count, state) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcrtomb(
    out: *mut c_char,
    scalar: i32,
    state: *mut MbState,
) -> usize {
    unsafe {
        with_state(state, &ENCODEWIDE, |state| {
            encode(out, scalar as u32, state)
        })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbsrtowcs(
    out: *mut i32,
    source: *mut *const c_char,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe {
        with_state(state, &STRDECODE, |state| {
            let mut cursor = *source;
            let mut scratch = *state;
            let state = if out.is_null() { &mut scratch } else { state };
            let mut filled = 0;
            while out.is_null() || filled < count {
                // The codec stops on a NUL or invalid continuation before reading
                // past the source string; no strlen scan or temporary allocation.
                match decode(cursor, 4, state) {
                    Decoded::Scalar(value, bytes) => {
                        if !out.is_null() {
                            out.add(filled).write(value as i32);
                        }
                        if value == 0 {
                            if !out.is_null() {
                                *source = ptr::null();
                            }
                            return filled;
                        }
                        filled += 1;
                        cursor = cursor.add(bytes);
                    }
                    _ => {
                        if !out.is_null() {
                            *source = cursor;
                        }
                        return illegal();
                    }
                }
            }
            *source = cursor;
            filled
        })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcsrtombs(
    out: *mut c_char,
    source: *mut *const i32,
    count: usize,
    state: *mut MbState,
) -> usize {
    unsafe {
        with_state(state, &STRENCODE, |state| {
            let mut cursor = *source;
            let mut scratch = *state;
            let state = if out.is_null() { &mut scratch } else { state };
            let mut filled = 0;
            while out.is_null() || filled < count {
                let scalar = cursor.read() as u32;
                let mut trial = *state;
                let mut bytes = [0u8; 4];
                let size = encode(bytes.as_mut_ptr().cast(), scalar, &mut trial);
                if size == INVALID {
                    if !out.is_null() {
                        *source = cursor;
                    }
                    return INVALID;
                }
                if !out.is_null() {
                    if size > count - filled {
                        break;
                    }
                    ptr::copy_nonoverlapping(bytes.as_ptr(), out.add(filled).cast(), size);
                }
                *state = trial;
                if scalar == 0 {
                    if !out.is_null() {
                        *source = ptr::null();
                    }
                    return filled;
                }
                filled += size;
                cursor = cursor.add(1);
            }
            *source = cursor;
            filled
        })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbstowcs(
    out: *mut i32,
    mut input: *const c_char,
    count: usize,
) -> usize {
    unsafe { kinakaze_abi_mbsrtowcs(out, &mut input, count, &mut MbState::ZERO) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcstombs(
    out: *mut c_char,
    mut input: *const i32,
    count: usize,
) -> usize {
    unsafe { kinakaze_abi_wcsrtombs(out, &mut input, count, &mut MbState::ZERO) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mbtowc(
    out: *mut i32,
    input: *const c_char,
    count: usize,
) -> c_int {
    // Both supported encodings are stateless; incomplete legacy conversions
    // fail rather than carrying a partial prefix to the next call.
    if input.is_null() {
        return 0;
    }
    let result = unsafe { mbrtowc(out, input, count, &mut MbState::ZERO) };
    if result >= INCOMPLETE {
        illegal();
        -1
    } else {
        result as c_int
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mblen(input: *const c_char, count: usize) -> c_int {
    unsafe { kinakaze_abi_mbtowc(ptr::null_mut(), input, count) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wctomb(out: *mut c_char, scalar: i32) -> c_int {
    if out.is_null() {
        return 0;
    }
    let result = unsafe { encode(out, scalar as u32, &mut MbState::ZERO) };
    if result == INVALID {
        -1
    } else {
        result as c_int
    }
}
