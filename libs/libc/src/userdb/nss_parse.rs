//! In-place line parsers used by glibc's compat/DB NSS modules. Strings stay in
//! the supplied line; only the group member pointer vector uses scratch space.
use super::*;

unsafe fn fields(line: *mut c_char) -> Vec<*mut c_char> {
    if line.is_null() {
        return Vec::new();
    }
    let mut result = vec![line];
    let mut at = line;
    unsafe {
        while *at != 0 {
            if *at == b'\n' as c_char {
                *at = 0;
                break;
            }
            if *at == b':' as c_char {
                *at = 0;
                result.push(at.add(1));
            }
            at = at.add(1);
        }
    }
    result
}
unsafe fn id(text: *const c_char, optional: bool) -> Option<u32> {
    if optional && unsafe { *text } == 0 {
        return Some(u32::MAX);
    }
    let mut end = ptr::null();
    let value = unsafe { crate::process::kinakaze_abi_strtoul(text, &mut end, 10) };
    (end != text && unsafe { *end } == 0).then_some(value.min(u32::MAX as u64) as u32)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__nss_files_parse_pwent(
    line: *mut c_char,
    result: *mut Passwd,
    _buffer: *mut c_char,
    _length: usize,
    _error: *mut c_int,
) -> c_int {
    if result.is_null() {
        return 0;
    }
    let fields = unsafe { fields(line) };
    if fields.is_empty() || unsafe { *fields[0] } == 0 {
        return 0;
    }
    let compat = matches!(unsafe { *line } as u8, b'+' | b'-');
    if fields.len() == 1 && compat {
        let empty = unsafe { line.add(CStr::from_ptr(line).to_bytes().len()) };
        unsafe {
            result.write(Passwd {
                pw_name: line,
                pw_passwd: empty,
                pw_uid: u32::MAX,
                pw_gid: u32::MAX,
                pw_gecos: empty,
                pw_dir: empty,
                pw_shell: empty,
            });
        }
        return 1;
    }
    if fields.len() != 7 {
        return 0;
    }
    let Some(uid) = (unsafe { id(fields[2], compat) }) else {
        return 0;
    };
    let Some(gid) = (unsafe { id(fields[3], compat) }) else {
        return 0;
    };
    unsafe {
        result.write(Passwd {
            pw_name: fields[0],
            pw_passwd: fields[1],
            pw_uid: uid,
            pw_gid: gid,
            pw_gecos: fields[4],
            pw_dir: fields[5],
            pw_shell: fields[6],
        });
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__nss_files_parse_grent(
    line: *mut c_char,
    result: *mut Group,
    buffer: *mut c_char,
    length: usize,
    error: *mut c_int,
) -> c_int {
    if line.is_null() || result.is_null() || buffer.is_null() {
        return 0;
    }
    let end = (buffer as usize).checked_add(length);
    let line_length = unsafe { CStr::from_ptr(line) }.to_bytes().len();
    let start = if line as usize >= buffer as usize && end.is_some_and(|end| (line as usize) < end)
    {
        (line as usize).checked_add(line_length + 1)
    } else {
        Some(buffer as usize)
    };
    let aligned = start.and_then(|at| at.checked_add(7)).map(|at| at & !7);
    let fields = unsafe { fields(line) };
    if fields.is_empty() || unsafe { *fields[0] } == 0 {
        return 0;
    }
    let compat = matches!(unsafe { *line } as u8, b'+' | b'-');
    let (password, gid, members) = if fields.len() == 1 && compat {
        (unsafe { line.add(line_length) }, u32::MAX, None)
    } else if fields.len() == 4 {
        let Some(gid) = (unsafe { id(fields[2], compat) }) else {
            return 0;
        };
        (fields[1], gid, Some(fields[3]))
    } else {
        return 0;
    };
    let mut pointers = Vec::new();
    if let Some(members) = members {
        let mut word = members;
        let mut at = members;
        unsafe {
            loop {
                let last = *at == 0;
                if last || *at == b',' as c_char {
                    *at = 0;
                    while (*word as u8).is_ascii_whitespace() {
                        word = word.add(1);
                    }
                    if *word != 0 {
                        pointers.push(word);
                    }
                    if last {
                        break;
                    }
                    word = at.add(1);
                }
                at = at.add(1);
            }
        }
    }
    let vector_end = aligned.and_then(|at| {
        (pointers.len() + 1)
            .checked_mul(size_of::<usize>())
            .and_then(|size| at.checked_add(size))
    });
    if !matches!((end, vector_end), (Some(end), Some(used)) if used <= end) {
        if !error.is_null() {
            unsafe {
                *error = ERANGE;
            }
        }
        return -1;
    }
    let vector = aligned.unwrap() as *mut *mut c_char;
    unsafe {
        ptr::copy_nonoverlapping(pointers.as_ptr(), vector, pointers.len());
        vector.add(pointers.len()).write(ptr::null_mut());
        result.write(Group {
            gr_name: line,
            gr_passwd: password,
            gr_gid: gid,
            gr_mem: vector,
        });
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__nss_files_parse_spent(
    line: *mut c_char,
    result: *mut Spwd,
    _buffer: *mut c_char,
    _length: usize,
    _error: *mut c_int,
) -> c_int {
    if line.is_null() || result.is_null() {
        return 0;
    }
    if let Some(record) = unsafe { super::account_files::parse_shadow(line) } {
        unsafe {
            result.write(record);
        }
        1
    } else {
        0
    }
}
