//! Linux's descriptor based mount interface, with fault-contained user copies.
use core::ffi::{c_char, c_int, c_void};
use kinakaze_vfs::{EFAULT, EINVAL, ENAMETOOLONG, ENOENT, fs, mount::api};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn ReadProcessMemory(
        process: *mut c_void,
        source: *const c_void,
        destination: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}
pub(super) fn read_user(address: usize, bytes: &mut [u8]) -> Result<(), i32> {
    if bytes.is_empty() {
        return Ok(());
    }
    if address == 0 || address.checked_add(bytes.len()).is_none() {
        return Err(EFAULT);
    }
    let mut count = 0;
    if unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as _,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut count,
        )
    } == 0
        || count != bytes.len()
    {
        return Err(EFAULT);
    }
    Ok(())
}
pub(super) fn write_user(address: usize, bytes: &[u8]) -> Result<(), i32> {
    if bytes.is_empty() {
        return Ok(());
    }
    if address == 0 || address.checked_add(bytes.len()).is_none() {
        return Err(EFAULT);
    }
    let mut count = 0;
    // Unlike WriteProcessMemory this output-buffer probe never bypasses a
    // read-only guest mapping or races a VirtualQuery permission precheck.
    if unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            bytes.as_ptr().cast(),
            address as _,
            bytes.len(),
            &mut count,
        )
    } == 0
        || count != bytes.len()
    {
        return Err(EFAULT);
    }
    Ok(())
}
fn request(address: usize, by_fd: bool) -> Result<(u64, u64, Option<i32>), i32> {
    let mut size = [0; 4];
    read_user(address, &mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size < 24 {
        return Err(EINVAL);
    }
    if size > 4096 {
        return Err(7);
    }
    let mut bytes = vec![0; size];
    read_user(address, &mut bytes)?;
    if size > 32 && bytes[32..].iter().any(|b| *b != 0) {
        return Err(7);
    }
    let descriptor = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let id = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let param = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let ns = if size >= 32 {
        u64::from_le_bytes(bytes[24..32].try_into().unwrap())
    } else {
        0
    };
    if by_fd {
        if id != 0 || ns != 0 {
            return Err(EINVAL);
        }
        return Ok((0, param, Some(descriptor as i32)));
    }
    let current = kinakaze_vfs::mount::namespace_id()?;
    if descriptor != 0 {
        if kinakaze_vfs::mount::namespace_descriptor_id(descriptor as i32)? != current {
            return Err(kinakaze_vfs::EOPNOTSUPP);
        }
    }
    if ns != 0 && ns != current {
        return Err(ENOENT);
    }
    Ok((id, param, None))
}
pub(super) fn text(address: usize, limit: usize) -> Result<String, i32> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let start = address.checked_add(bytes.len()).ok_or(EFAULT)?;
        let count = (4096 - (start & 4095)).min(limit - bytes.len());
        let mut chunk = vec![0; count];
        read_user(start, &mut chunk)?;
        if let Some(end) = chunk.iter().position(|b| *b == 0) {
            bytes.extend_from_slice(&chunk[..end]);
            return String::from_utf8(bytes).map_err(|_| EINVAL);
        }
        bytes.extend_from_slice(&chunk);
    }
    Err(ENAMETOOLONG)
}
fn at(fd: i32, address: usize, empty: bool) -> Result<String, i32> {
    let path = if address == 0 && empty {
        String::new()
    } else {
        text(address, 4096)?
    };
    if path.is_empty() {
        if !empty {
            return Err(ENOENT);
        }
        return if fd == fs::AT_FDCWD {
            Ok(fs::getcwd())
        } else {
            crate::fsextra::directory_path(fd)
        };
    }
    crate::fsextra::resolve_at(fd, &path)
}
fn result(value: Result<i32, i32>) -> i32 {
    match value {
        Ok(n) => n,
        Err(e) => {
            crate::set_errno(e);
            -1
        }
    }
}
fn config(fd: i32, cmd: u32, key: usize, value: usize, aux: i32) -> Result<(), i32> {
    if cmd > 8 {
        return Err(kinakaze_vfs::EOPNOTSUPP);
    }
    if cmd >= 6 {
        if key != 0 || value != 0 || aux != 0 {
            return Err(EINVAL);
        }
        return api::configure(fd, cmd, None, None);
    }
    if key == 0 {
        return Err(EINVAL);
    }
    let key = text(key, 256)?;
    let string;
    let binary;
    let parameter = match cmd {
        0 => {
            if value != 0 || aux != 0 {
                return Err(EINVAL);
            }
            api::Parameter::Flag
        }
        1 => {
            if value == 0 || aux != 0 {
                return Err(EINVAL);
            }
            string = text(value, 4096)?;
            api::Parameter::String(&string)
        }
        2 => {
            if value == 0 || aux <= 0 || aux > 1024 * 1024 {
                return Err(EINVAL);
            }
            binary = {
                let mut bytes = vec![0; aux as usize];
                read_user(value, &mut bytes)?;
                bytes
            };
            api::Parameter::Binary(&binary)
        }
        3 | 4 => {
            if value == 0 {
                return Err(EINVAL);
            }
            string = text(value, 4096)?;
            api::Parameter::Path(aux, &string, cmd == 4)
        }
        5 => {
            if value != 0 || aux < 0 {
                return Err(EINVAL);
            }
            api::Parameter::Fd(aux)
        }
        _ => return Err(EINVAL),
    };
    api::configure(fd, cmd, Some(&key), Some(parameter))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fsopen(name: *const c_char, flags: u32) -> c_int {
    result(text(name as usize, 4096).and_then(|name| api::fsopen(&name, flags)))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn fsopen(name: *const c_char, flags: u32) -> c_int {
    kinakaze_abi_fsopen(name, flags)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fsconfig(
    fd: c_int,
    cmd: u32,
    key: *const c_char,
    value: *const c_void,
    aux: c_int,
) -> c_int {
    result(config(fd, cmd, key as usize, value as usize, aux).map(|_| 0))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn fsconfig(
    fd: c_int,
    cmd: u32,
    key: *const c_char,
    value: *const c_void,
    aux: c_int,
) -> c_int {
    kinakaze_abi_fsconfig(fd, cmd, key, value, aux)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fsmount(fd: c_int, flags: u32, attributes: u32) -> c_int {
    result(api::fsmount(fd, flags, attributes as u64))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn fsmount(fd: c_int, flags: u32, attributes: u32) -> c_int {
    kinakaze_abi_fsmount(fd, flags, attributes)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fspick(fd: c_int, path: *const c_char, flags: u32) -> c_int {
    result(at(fd, path as usize, flags & 8 != 0).and_then(|path| api::fspick(&path, flags)))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn fspick(fd: c_int, path: *const c_char, flags: u32) -> c_int {
    kinakaze_abi_fspick(fd, path, flags)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_open_tree(fd: c_int, path: *const c_char, flags: u32) -> c_int {
    result(at(fd, path as usize, flags & 0x1000 != 0).and_then(|path| api::open_tree(&path, flags)))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn open_tree(fd: c_int, path: *const c_char, flags: u32) -> c_int {
    kinakaze_abi_open_tree(fd, path, flags)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_move_mount(
    fromfd: c_int,
    from: *const c_char,
    tofd: c_int,
    to: *const c_char,
    flags: u32,
) -> c_int {
    result((|| {
        if flags & !0x377 != 0 || flags & 0x300 == 0x300 {
            return Err(EINVAL);
        }
        if flags & 0x300 != 0 {
            return Err(kinakaze_vfs::EOPNOTSUPP);
        }
        let target = at(tofd, to as usize, flags & 0x40 != 0)?;
        let source = at(fromfd, from as usize, flags & 4 != 0)?;
        if let Some((fd, tail)) = api::tree_reference(&source) {
            if !tail.is_empty() {
                return Err(EINVAL);
            }
            api::move_tree(fd, &target, flags)?;
        } else {
            api::move_path(&source, &target, flags)?;
        }
        Ok(0)
    })())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn move_mount(
    fromfd: c_int,
    from: *const c_char,
    tofd: c_int,
    to: *const c_char,
    flags: u32,
) -> c_int {
    kinakaze_abi_move_mount(fromfd, from, tofd, to, flags)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_statmount(
    req: *const c_void,
    buffer: *mut c_void,
    size: usize,
    flags: u32,
) -> c_int {
    result((|| {
        if flags & !1 != 0 {
            return Err(EINVAL);
        }
        let (id, mask, fd) = request(req as usize, flags & 1 != 0)?;
        let bytes = match fd {
            Some(fd) => kinakaze_vfs::mount::query::stat_fd(fd, mask)?,
            None => kinakaze_vfs::mount::query::stat(id, mask)?,
        };
        if bytes.len() > size {
            return Err(kinakaze_vfs::EOVERFLOW);
        }
        write_user(buffer as usize, &bytes)?;
        Ok(0)
    })())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn statmount(
    req: *const c_void,
    buffer: *mut c_void,
    size: usize,
    flags: u32,
) -> c_int {
    kinakaze_abi_statmount(req, buffer, size, flags)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_listmount(
    req: *const c_void,
    buffer: *mut u64,
    count: usize,
    flags: u32,
) -> isize {
    result((|| {
        if flags & !1 != 0 {
            return Err(EINVAL);
        }
        let (id, last, _) = request(req as usize, false)?;
        let ids = kinakaze_vfs::mount::query::list(id, last, count, flags & 1 != 0)?;
        let bytes: Vec<_> = ids.iter().flat_map(|id| id.to_le_bytes()).collect();
        write_user(buffer as usize, &bytes)?;
        Ok(ids.len() as i32)
    })()) as isize
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn listmount(
    req: *const c_void,
    buffer: *mut u64,
    count: usize,
    flags: u32,
) -> isize {
    kinakaze_abi_listmount(req, buffer, count, flags)
}

fn attributes(address: usize, size: usize) -> Result<[u64; 4], i32> {
    if size < 32 {
        return Err(EINVAL);
    }
    if size > 4096 {
        return Err(7);
    }
    let mut bytes = vec![0; size];
    read_user(address, &mut bytes)?;
    if bytes[32..].iter().any(|b| *b != 0) {
        return Err(7);
    }
    Ok(std::array::from_fn(|i| {
        u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap())
    }))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mount_setattr(
    fd: c_int,
    path: *const c_char,
    flags: u32,
    attr: *const c_void,
    size: usize,
) -> c_int {
    result((|| {
        let [set, clear, propagation, userns] = attributes(attr as usize, size)?;
        let path = at(fd, path as usize, flags & 0x1000 != 0)?;
        api::set_attributes(&path, flags, set, clear, propagation, userns)?;
        Ok(0)
    })())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mount_setattr(
    fd: c_int,
    path: *const c_char,
    flags: u32,
    attr: *const c_void,
    size: usize,
) -> c_int {
    kinakaze_abi_mount_setattr(fd, path, flags, attr, size)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_open_tree_attr(
    fd: c_int,
    path: *const c_char,
    flags: u32,
    attr: *const c_void,
    size: usize,
) -> c_int {
    result((|| {
        if attr.is_null() && size != 0 {
            return Err(EINVAL);
        }
        let path = at(fd, path as usize, flags & 0x1000 != 0)?;
        let tree = api::open_tree(&path, flags)?;
        if !attr.is_null() {
            let apply = (|| {
                let [set, clear, propagation, userns] = attributes(attr as usize, size)?;
                let target = crate::fsextra::directory_path(tree)?;
                api::set_attributes(
                    &target,
                    0x1000 | (flags & 0x8000),
                    set,
                    clear,
                    propagation,
                    userns,
                )
            })();
            if let Err(e) = apply {
                let _ = kinakaze_vfs::close(tree);
                return Err(e);
            }
        }
        Ok(tree)
    })())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn open_tree_attr(
    fd: c_int,
    path: *const c_char,
    flags: u32,
    attr: *const c_void,
    size: usize,
) -> c_int {
    kinakaze_abi_open_tree_attr(fd, path, flags, attr, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_guest_addresses_are_errno_not_host_faults() {
        assert_eq!(text(1, 256), Err(EFAULT));
        assert_eq!(text(usize::MAX - 2, 256), Err(EFAULT));
        let name = b"overlay\0";
        assert_eq!(text(name.as_ptr() as usize, 256).unwrap(), "overlay");
    }
}
