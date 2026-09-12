//! Linux utmp and utmpx share the same ABI; BSD login helpers use that database.
use super::*;

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setutent() {
    kinakaze_abi_setutxent();
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endutent() {
    kinakaze_abi_endutxent();
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getutent() -> *mut Utmpx {
    kinakaze_abi_getutxent()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getutline(wanted: *const Utmpx) -> *mut Utmpx {
    unsafe { kinakaze_abi_getutxline(wanted) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getutxline(wanted: *const Utmpx) -> *mut Utmpx {
    if wanted.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    loop {
        let entry = kinakaze_abi_getutxent();
        if entry.is_null() {
            return entry;
        }
        if unsafe {
            matches!((*entry).ut_type, LOGIN_PROCESS | USER_PROCESS)
                && (*entry).ut_line == (*wanted).ut_line
        } {
            return entry;
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getutid(wanted: *const Utmpx) -> *mut Utmpx {
    unsafe { kinakaze_abi_getutxid(wanted) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getutxid(wanted: *const Utmpx) -> *mut Utmpx {
    if wanted.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    loop {
        let entry = kinakaze_abi_getutxent();
        if entry.is_null() || unsafe { same_slot(&*entry, &*wanted) } {
            return entry;
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pututline(record: *const Utmpx) -> *mut Utmpx {
    unsafe { kinakaze_abi_pututxline(record) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_utmpname(path: *const c_char) -> c_int {
    unsafe { kinakaze_abi_utmpxname(path) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_updwtmp(path: *const c_char, record: *const Utmpx) {
    unsafe {
        kinakaze_abi_updwtmpx(path, record);
    }
}

fn timestamp(record: &mut Utmpx) {
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        record.ut_tv.tv_sec = now.as_secs() as i32;
        record.ut_tv.tv_usec = now.subsec_micros() as i32;
    }
}

fn field<const N: usize>(output: &mut [c_char; N], bytes: &[u8]) {
    output.fill(0);
    for (out, byte) in output.iter_mut().zip(bytes) {
        *out = *byte as c_char;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_login(entry: *const Utmpx) {
    if entry.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return;
    }
    let mut record = unsafe { *entry };
    record.ut_type = USER_PROCESS;
    record.ut_pid = kinakaze_vfs::job::process_id() as i32;
    for fd in 0..3 {
        let name = crate::term::kinakaze_abi_ttyname(fd);
        if !name.is_null() {
            let name = unsafe { CStr::from_ptr(name) }.to_bytes();
            field(
                &mut record.ut_line,
                name.strip_prefix(b"/dev/").unwrap_or(name),
            );
            break;
        }
    }
    kinakaze_abi_setutxent();
    unsafe {
        kinakaze_abi_pututxline(&record);
    }
    kinakaze_abi_endutxent();
    unsafe {
        kinakaze_abi_updwtmpx(c"/var/log/wtmp".as_ptr(), &record);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_logout(line: *const c_char) -> c_int {
    if line.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return 0;
    }
    let line = unsafe { CStr::from_ptr(line) }.to_bytes();
    let mut result = 0;
    kinakaze_abi_setutxent();
    loop {
        let entry = kinakaze_abi_getutxent();
        if entry.is_null() {
            break;
        }
        let mut record = unsafe { *entry };
        let length = record
            .ut_line
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(record.ut_line.len());
        if record.ut_type == USER_PROCESS
            && length == line.len()
            && record.ut_line[..length]
                .iter()
                .zip(line)
                .all(|(&a, &b)| a as u8 == b)
        {
            record.ut_type = DEAD_PROCESS;
            record.ut_user.fill(0);
            record.ut_host.fill(0);
            timestamp(&mut record);
            if !unsafe { kinakaze_abi_pututxline(&record) }.is_null() {
                result = 1;
            }
        }
    }
    kinakaze_abi_endutxent();
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_logwtmp(
    line: *const c_char,
    name: *const c_char,
    host: *const c_char,
) {
    if line.is_null() || name.is_null() || host.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return;
    }
    let mut record = Utmpx::zeroed();
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    record.ut_type = if name.is_empty() {
        DEAD_PROCESS
    } else {
        USER_PROCESS
    };
    record.ut_pid = kinakaze_vfs::job::process_id() as i32;
    field(
        &mut record.ut_line,
        unsafe { CStr::from_ptr(line) }.to_bytes(),
    );
    field(&mut record.ut_user, name);
    field(
        &mut record.ut_host,
        unsafe { CStr::from_ptr(host) }.to_bytes(),
    );
    timestamp(&mut record);
    unsafe {
        kinakaze_abi_updwtmpx(c"/var/log/wtmp".as_ptr(), &record);
    }
}
