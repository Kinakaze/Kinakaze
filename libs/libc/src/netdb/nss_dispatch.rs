//! Private NSS action lists used by glibc's compat modules. The list and module
//! names live in the guest arena, including when an ELF caller retains a list
//! across fork. Provider code is pinned by the guest dynamic linker.
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use std::{collections::HashMap, io::Read, sync::Mutex};

#[repr(C)]
pub struct Action {
    module: *mut c_char,
    bits: u32,
}
static CACHE: Mutex<Option<HashMap<(i32, String), usize>>> = Mutex::new(None);
const DATABASES: [&str; 17] = [
    "aliases",
    "ethers",
    "group",
    "group_compat",
    "gshadow",
    "hosts",
    "initgroups",
    "netgroup",
    "networks",
    "passwd",
    "passwd_compat",
    "protocols",
    "publickey",
    "rpc",
    "services",
    "shadow",
    "shadow_compat",
];

fn configuration(database: i32) -> Result<String, i32> {
    let name = DATABASES.get(database as usize).ok_or(22)?;
    let mut text = String::new();
    match super::records::Reader::open("/etc/nsswitch.conf") {
        Ok(mut reader) => {
            reader.read_to_string(&mut text).map_err(|_| 5)?;
        }
        Err(2) => {}
        Err(e) => return Err(e),
    }
    let lookup = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split('#').next()?.split_once(':')?;
            (key.trim() == name).then(|| value.trim().to_owned())
        })
    };
    if let Some(value) =
        lookup(name).or_else(|| (database == 16).then(|| lookup("passwd_compat")).flatten())
    {
        return Ok(value);
    }
    Ok(match database {
        5 => "files dns",
        3 | 10 | 16 => "nis",
        6 => "",
        _ => "files",
    }
    .into())
}

fn parse(line: &str) -> Result<Vec<(String, u32)>, i32> {
    let mut input = line.trim();
    let mut result: Vec<(String, u32)> = Vec::new();
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix('[') {
            let (rules, tail) = rest.split_once(']').ok_or(22)?;
            let (_, bits) = result.last_mut().ok_or(22)?;
            for rule in rules.split_whitespace() {
                let (status, action) = rule.split_once('=').ok_or(22)?;
                let inverted = status.starts_with('!');
                let status = match status.trim_start_matches('!').to_ascii_uppercase().as_str() {
                    "TRYAGAIN" => 0,
                    "UNAVAIL" => 1,
                    "NOTFOUND" => 2,
                    "SUCCESS" => 3,
                    _ => return Err(22),
                };
                let action = match action.to_ascii_lowercase().as_str() {
                    "continue" => 0,
                    "return" => 1,
                    "merge" => 2,
                    _ => return Err(22),
                };
                for index in 0..5 {
                    if (index == status) != inverted {
                        *bits = (*bits & !(3 << (index * 2))) | (action << (index * 2));
                    }
                }
            }
            input = tail.trim_start();
        } else {
            let end = input
                .find(|c: char| c.is_whitespace() || c == '[')
                .unwrap_or(input.len());
            let name = &input[..end];
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(22);
            }
            // SUCCESS and internal RETURN stop; other statuses continue.
            result.push((name.into(), (1 << 6) | (1 << 8)));
            input = input[end..].trim_start();
        }
    }
    Ok(result)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___nss_database_get(
    database: c_int,
    output: *mut *mut Action,
) -> bool {
    if output.is_null() {
        crate::set_errno(22);
        return false;
    }
    unsafe {
        output.write(ptr::null_mut());
    }
    let result = (|| {
        let line = configuration(database)?;
        let mut guard = CACHE.lock().map_err(|_| 5)?;
        let cache = guard.get_or_insert_with(HashMap::new);
        // Parse and allocate before publishing an immutable list. Retain old
        // configurations because NSS callers may still own their pointers.
        let key = (database, line.clone());
        let address = if let Some(&address) = cache.get(&key) {
            address
        } else {
            let records = parse(&line)?;
            if records.is_empty() {
                0
            } else {
                let header = (records.len() + 1) * size_of::<Action>();
                let size = header
                    + records
                        .iter()
                        .map(|(name, _)| name.len() + 1)
                        .sum::<usize>();
                let allocation = unsafe { kinakaze_alloc::guest::malloc(size) };
                if allocation.is_null() {
                    return Err(12);
                }
                unsafe {
                    ptr::write_bytes(allocation, 0, size);
                    let mut at = header;
                    for (index, (name, bits)) in records.iter().enumerate() {
                        ptr::copy_nonoverlapping(name.as_ptr(), allocation.add(at), name.len());
                        allocation.cast::<Action>().add(index).write(Action {
                            module: allocation.add(at).cast(),
                            bits: *bits,
                        });
                        at += name.len() + 1;
                    }
                }
                cache.insert(key, allocation as usize);
                allocation as usize
            }
        };
        unsafe {
            output.write(address as *mut Action);
        }
        Ok(())
    })();
    result.map_or_else(
        |e| {
            crate::set_errno(e);
            false
        },
        |()| true,
    )
}

unsafe fn status(code: i32, found: bool, error: *mut i32) -> i32 {
    if code != 0 && !error.is_null() {
        unsafe {
            error.write(code);
        }
    }
    match code {
        0 if found => 1,
        0 | 2 => 0,
        12 | 34 | 11 => -2,
        _ => -1,
    }
}
macro_rules! lookup {
    ($name:ident, $key:ty, $record:ty, $call:path) => {
        unsafe extern "sysv64" fn $name(
            key: $key,
            record: *mut $record,
            buffer: *mut c_char,
            length: usize,
            error: *mut i32,
        ) -> i32 {
            let mut result = ptr::null_mut();
            let code = unsafe { $call(key, record, buffer, length, &mut result) };
            unsafe { status(code, !result.is_null(), error) }
        }
    };
}
lookup!(
    pwnam,
    *const c_char,
    crate::userdb::Passwd,
    crate::userdb::kinakaze_abi_getpwnam_r
);
lookup!(
    pwuid,
    u32,
    crate::userdb::Passwd,
    crate::userdb::kinakaze_abi_getpwuid_r
);
lookup!(
    grnam,
    *const c_char,
    crate::userdb::Group,
    crate::userdb::kinakaze_abi_getgrnam_r
);
lookup!(
    grgid,
    u32,
    crate::userdb::Group,
    crate::userdb::kinakaze_abi_getgrgid_r
);
lookup!(
    spnam,
    *const c_char,
    crate::userdb::Spwd,
    super::shadow_database::getspnam_r
);
macro_rules! enumeration {
    ($name:ident, $record:ty, $call:path) => {
        unsafe extern "sysv64" fn $name(
            record: *mut $record,
            buffer: *mut c_char,
            length: usize,
            error: *mut i32,
        ) -> i32 {
            let mut result = ptr::null_mut();
            let code = unsafe { $call(record, buffer, length, &mut result) };
            unsafe { status(code, !result.is_null(), error) }
        }
    };
}
enumeration!(
    pwent,
    crate::userdb::Passwd,
    crate::userdb::kinakaze_abi_getpwent_r
);
enumeration!(
    grent,
    crate::userdb::Group,
    crate::userdb::kinakaze_abi_getgrent_r
);
enumeration!(
    spent,
    crate::userdb::Spwd,
    super::shadow_database::kinakaze_abi_getspent_r
);
macro_rules! control {
    ($name:ident, $call:path) => {
        unsafe extern "sysv64" fn $name() -> i32 {
            $call();
            1
        }
    };
}
control!(setpw, crate::userdb::kinakaze_abi_setpwent);
control!(endpw, crate::userdb::kinakaze_abi_endpwent);
control!(setgr, crate::userdb::kinakaze_abi_setgrent);
control!(endgr, crate::userdb::kinakaze_abi_endgrent);
control!(setsp, super::shadow_database::setspent);
control!(endsp, super::shadow_database::endspent);

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___nss_lookup_function(
    action: *const Action,
    function: *const c_char,
) -> *mut c_void {
    if action.is_null() || function.is_null() || unsafe { (*action).module.is_null() } {
        return ptr::null_mut();
    }
    let module = unsafe { CStr::from_ptr((*action).module) }.to_bytes();
    let function = unsafe { CStr::from_ptr(function) }.to_bytes();
    if module == b"files" {
        return match function {
            b"getpwnam_r" => pwnam as *mut c_void,
            b"getpwuid_r" => pwuid as _,
            b"getgrnam_r" => grnam as _,
            b"getgrgid_r" => grgid as _,
            b"getspnam_r" => spnam as _,
            b"getpwent_r" => pwent as _,
            b"getgrent_r" => grent as _,
            b"getspent_r" => spent as _,
            b"setpwent" => setpw as _,
            b"endpwent" => endpw as _,
            b"setgrent" => setgr as _,
            b"endgrent" => endgr as _,
            b"setspent" => setsp as _,
            b"endspent" => endsp as _,
            _ => ptr::null_mut(),
        };
    }
    use kinakaze_link::process::{kinakaze_process_dlopen, kinakaze_process_dlsym};
    let Ok(module) = std::str::from_utf8(module) else {
        return ptr::null_mut();
    };
    let Ok(function) = std::str::from_utf8(function) else {
        return ptr::null_mut();
    };
    let library = std::ffi::CString::new(format!("libnss_{module}.so.2")).unwrap();
    let symbol = std::ffi::CString::new(format!("_nss_{module}_{function}")).unwrap();
    unsafe {
        let handle = kinakaze_process_dlopen(library.as_ptr(), 2 | 0x1000);
        if handle.is_null() {
            ptr::null_mut()
        } else {
            kinakaze_process_dlsym(handle, symbol.as_ptr())
        }
    }
}

/// Called only during libc teardown, once no NSS caller retains a list.
pub(crate) fn cleanup() {
    if let Ok(mut cache) = CACHE.lock() {
        if let Some(cache) = cache.take() {
            for address in cache.into_values() {
                unsafe {
                    kinakaze_alloc::guest::free(address as *mut u8);
                }
            }
        }
    }
}
