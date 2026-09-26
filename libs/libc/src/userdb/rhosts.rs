//! Legacy host-based authorization against guest files only. No host Windows
//! account or trust file participates in this check.
use super::*;
use kinakaze_vfs::fs;

fn addresses(name: &CStr, family: i32) -> Vec<Vec<u8>> {
    let hints = crate::netdb::AddrInfo {
        ai_flags: 0,
        ai_family: family,
        ai_socktype: 1,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: ptr::null_mut(),
        ai_canonname: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };
    let mut list = ptr::null_mut();
    let mut result = Vec::new();
    unsafe {
        if crate::netdb::kinakaze_abi_getaddrinfo(name.as_ptr(), ptr::null(), &hints, &mut list)
            != 0
        {
            return result;
        }
        let mut at = list;
        while !at.is_null() {
            let record = &*at;
            let range = match record.ai_family {
                2 => 4..8,
                10 => 8..24,
                _ => 0..0,
            };
            if !range.is_empty()
                && !record.ai_addr.is_null()
                && record.ai_addrlen as usize >= range.end
            {
                result.push(
                    core::slice::from_raw_parts(
                        record.ai_addr.cast::<u8>().add(range.start),
                        range.len(),
                    )
                    .to_vec(),
                );
            }
            at = record.ai_next;
        }
        crate::netdb::kinakaze_abi_freeaddrinfo(list);
    }
    result
}
fn trusted_file(path: &str, uid: u32) -> Result<Vec<u8>, i32> {
    let before = fs::lstat(path)?;
    if before.st_mode & fs::S_IFMT != fs::S_IFREG {
        return Err(13);
    }
    let fd = fs::open(path, fs::O_RDONLY | fs::O_CLOEXEC | fs::O_NOFOLLOW, 0)?;
    let result = (|| {
        let stat = fs::fstat(fd)?;
        if stat.st_mode & fs::S_IFMT != fs::S_IFREG
            || (stat.st_uid != 0 && stat.st_uid != uid)
            || stat.st_mode & 0o022 != 0
            || stat.st_nlink != 1
            || stat.st_dev != before.st_dev
            || stat.st_ino != before.st_ino
        {
            return Err(13);
        }
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = match kinakaze_vfs::read(fd, &mut buffer) {
                Err(4) => continue,
                other => other?,
            };
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        Ok(bytes)
    })();
    let _ = kinakaze_vfs::close(fd);
    result
}
fn rules(
    bytes: &[u8],
    host: &CStr,
    remote: &CStr,
    local: &CStr,
    family: i32,
    peers: &[Vec<u8>],
) -> bool {
    for line in bytes.split(|&c| c == b'\n') {
        if line.contains(&0) {
            continue;
        }
        let mut fields = line
            .split(|c| c.is_ascii_whitespace())
            .filter(|s| !s.is_empty());
        let Some(host_rule) = fields.next() else {
            continue;
        };
        if host_rule.starts_with(b"#") {
            continue;
        }
        let user_rule = fields.next().unwrap_or(local.to_bytes());
        let matched = |rule: &[u8], is_host: bool| -> i32 {
            let sign = if rule.starts_with(b"-") { -1 } else { 1 };
            let rule = if rule.starts_with(b"-") || rule.starts_with(b"+") {
                &rule[1..]
            } else {
                rule
            };
            let yes = if rule.is_empty() {
                true
            } else if rule.starts_with(b"@") {
                let Ok(group) = CString::new(&rule[1..]) else {
                    return 0;
                };
                unsafe {
                    crate::netdb::innetgr_for_rhosts(
                        group.as_ptr(),
                        if is_host { host.as_ptr() } else { ptr::null() },
                        if is_host {
                            ptr::null()
                        } else {
                            remote.as_ptr()
                        },
                    ) != 0
                }
            } else if is_host {
                CString::new(rule).ok().is_some_and(|name| {
                    addresses(&name, family)
                        .iter()
                        .any(|address| peers.contains(address))
                })
            } else {
                rule == remote.to_bytes()
            };
            if yes { sign } else { 0 }
        };
        let host_match = matched(host_rule, true);
        if host_match < 0 {
            return false;
        }
        if host_match == 0 {
            continue;
        }
        match matched(user_rule, false) {
            -1 => return false,
            1 => return true,
            _ => {}
        }
    }
    false
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ruserok_af(
    host: *const c_char,
    superuser: c_int,
    remote: *const c_char,
    local: *const c_char,
    family: u16,
) -> c_int {
    if host.is_null() || remote.is_null() || local.is_null() {
        crate::set_errno(EINVAL);
        return -1;
    }
    if !matches!(family, 0 | 2 | 10) {
        crate::set_errno(97);
        return -1;
    }
    let (host, remote, local) = unsafe {
        (
            CStr::from_ptr(host),
            CStr::from_ptr(remote),
            CStr::from_ptr(local),
        )
    };
    let peers = addresses(host, family as i32);
    if peers.is_empty() {
        return -1;
    }
    if superuser == 0 {
        if let Ok(bytes) = trusted_file("/etc/hosts.equiv", 0) {
            if rules(&bytes, host, remote, local, family as i32, &peers) {
                return 0;
            }
        }
    }
    let Some(account) = local.to_str().ok().and_then(find_user_by_name) else {
        return -1;
    };
    let path = format!("{}/.rhosts", account.dir.trim_end_matches('/'));
    if let Ok(bytes) = trusted_file(&path, account.uid) {
        if rules(&bytes, host, remote, local, family as i32, &peers) {
            return 0;
        }
    }
    -1
}
