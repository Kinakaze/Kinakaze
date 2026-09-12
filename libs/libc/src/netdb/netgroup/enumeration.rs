//! Flatten nested file-backed netgroups while retaining enumeration storage.
use super::*;
use std::{cell::RefCell, ffi::CString};
type Triple = [Option<CString>; 3];
#[derive(Default)]
struct State {
    entries: Vec<Triple>,
    next: usize,
}
thread_local! { static ENUMERATION:RefCell<State>=RefCell::new(State::default()); }

fn expand(
    group: &[u8],
    seen: &mut HashSet<Vec<u8>>,
    entries: &mut Vec<Triple>,
) -> Result<bool, i32> {
    if !seen.insert(group.to_vec()) {
        return Ok(true);
    }
    if seen.len() > 1024 {
        return Err(kinakaze_vfs::EOVERFLOW);
    }
    let Some(data) = super::definition(group)? else {
        return Ok(false);
    };
    let mut rest = data.as_slice();
    while !rest.trim_ascii_start().is_empty() {
        rest = rest.trim_ascii_start();
        if rest[0] == b'(' {
            let Some(end) = rest.iter().position(|b| *b == b')') else {
                return Err(kinakaze_vfs::EINVAL);
            };
            let parts: Vec<_> = rest[1..end]
                .split(|b| *b == b',')
                .map(<[u8]>::trim_ascii)
                .collect();
            if parts.len() != 3 {
                return Err(kinakaze_vfs::EINVAL);
            }
            let mut triple: [Option<CString>; 3] = [None, None, None];
            for n in 0..3 {
                if !parts[n].is_empty() {
                    triple[n] = Some(CString::new(parts[n]).map_err(|_| kinakaze_vfs::EINVAL)?);
                }
            }
            entries.push(triple);
            if entries.len() > 65536 {
                return Err(kinakaze_vfs::EOVERFLOW);
            }
            rest = &rest[end + 1..];
        } else if let Some(nested) = super::super::records::field(&mut rest) {
            expand(nested, seen, entries)?;
        } else {
            break;
        }
    }
    Ok(true)
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setnetgrent(group: *const c_char) -> c_int {
    ENUMERATION.with(|slot| {
        let mut state = slot.borrow_mut();
        *state = State::default();
        if group.is_null() {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return 0;
        }
        let mut entries = Vec::new();
        match expand(
            unsafe { CStr::from_ptr(group) }.to_bytes(),
            &mut HashSet::new(),
            &mut entries,
        ) {
            Ok(found) => {
                state.entries = entries;
                i32::from(found)
            }
            Err(error) => {
                crate::set_errno(error);
                0
            }
        }
    })
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endnetgrent() {
    ENUMERATION.with(|s| *s.borrow_mut() = State::default());
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getnetgrent(
    host: *mut *mut c_char,
    user: *mut *mut c_char,
    domain: *mut *mut c_char,
) -> c_int {
    if host.is_null() || user.is_null() || domain.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    }
    ENUMERATION.with(|slot| {
        let mut state = slot.borrow_mut();
        let index = state.next;
        if index >= state.entries.len() {
            return 0;
        }
        state.next += 1;
        for (output, value) in [host, user, domain].into_iter().zip(&state.entries[index]) {
            unsafe {
                *output = value
                    .as_ref()
                    .map_or(core::ptr::null_mut(), |s| s.as_ptr().cast_mut());
            }
        }
        1
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getnetgrent_r(
    host: *mut *mut c_char,
    user: *mut *mut c_char,
    domain: *mut *mut c_char,
    buffer: *mut c_char,
    length: usize,
) -> c_int {
    if host.is_null() || user.is_null() || domain.is_null() || buffer.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    }
    ENUMERATION.with(|slot| {
        let mut state = slot.borrow_mut();
        let index = state.next;
        let Some(triple) = state.entries.get(index) else {
            return 0;
        };
        let needed = triple
            .iter()
            .flatten()
            .map(|s| s.as_bytes_with_nul().len())
            .sum::<usize>();
        if needed > length {
            crate::set_errno(kinakaze_vfs::ERANGE);
            return 0;
        }
        let mut at = buffer;
        for (output, value) in [host, user, domain].into_iter().zip(triple) {
            unsafe {
                if let Some(value) = value {
                    let size = value.as_bytes_with_nul().len();
                    core::ptr::copy_nonoverlapping(value.as_ptr(), at, size);
                    *output = at;
                    at = at.add(size);
                } else {
                    *output = core::ptr::null_mut();
                }
            }
        }
        state.next += 1;
        1
    })
}
