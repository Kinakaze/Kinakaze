//! Parse shadow records from a guest FILE into caller-owned storage.
use super::*;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetspent_r(
    file: *mut crate::stdio::File,
    entry: *mut Spwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Spwd,
) -> c_int {
    if result.is_null() {
        return EINVAL;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if file.is_null() || entry.is_null() || buffer.is_null() {
        return EINVAL;
    }
    loop {
        let start = crate::stdio::ftell(file);
        let mut line = Vec::new();
        loop {
            let byte = crate::stdio::fgetc(file);
            if byte < 0 || byte == 10 {
                break;
            }
            line.push(byte as u8);
        }
        if crate::stdio::ferror(file) != 0 {
            return kinakaze_vfs::EIO;
        }
        if line.is_empty() && crate::stdio::feof(file) != 0 {
            return ENOENT;
        }
        let line = line.trim_ascii_end();
        if line.is_empty() || line.starts_with(b"#") || line.contains(&0) {
            continue;
        }
        let fields: Vec<_> = line.split(|b| *b == b':').collect();
        if fields.len() != 9 || fields[0].is_empty() {
            continue;
        }
        let mut numbers = [-1i64; 6];
        let mut valid = true;
        for n in 0..6 {
            if !fields[n + 2].is_empty() {
                match std::str::from_utf8(fields[n + 2])
                    .ok()
                    .and_then(|v| v.parse::<i64>().ok())
                {
                    Some(value) => numbers[n] = value,
                    None => valid = false,
                }
            }
        }
        let flag = if fields[8].is_empty() {
            Some(u64::MAX)
        } else {
            std::str::from_utf8(fields[8])
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
        };
        let Some(flag) = flag.filter(|_| valid) else {
            continue;
        };
        let needed = fields[0].len() + fields[1].len() + 2;
        if needed > length {
            if start >= 0 {
                crate::stdio::fseek(file, start, 0);
            }
            crate::set_errno(ERANGE);
            return ERANGE;
        }
        unsafe {
            ptr::copy_nonoverlapping(fields[0].as_ptr(), buffer.cast(), fields[0].len());
            *buffer.add(fields[0].len()) = 0;
            let password = buffer.add(fields[0].len() + 1);
            ptr::copy_nonoverlapping(fields[1].as_ptr(), password.cast(), fields[1].len());
            *password.add(fields[1].len()) = 0;
            entry.write(Spwd {
                sp_namp: buffer,
                sp_pwdp: password,
                sp_lstchg: numbers[0],
                sp_min: numbers[1],
                sp_max: numbers[2],
                sp_warn: numbers[3],
                sp_inact: numbers[4],
                sp_expire: numbers[5],
                sp_flag: flag,
            });
            *result = entry;
        }
        return 0;
    }
}
