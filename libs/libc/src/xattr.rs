//! Linux extended-attribute ABI over inode-backed native EAs.
//!
//! Storage is shared with the VFS, not a libc-local registry. Namespace access
//! is checked here with the guest credentials; unsupported ACL/security
//! handlers are rejected rather than stored without enforcing their meaning.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use kinakaze_vfs::fs::{self, Stat};
use kinakaze_vfs::xattr::{Attributes, ENODATA, XATTR_CREATE, XATTR_REPLACE, XATTR_SIZE_MAX};
use kinakaze_vfs::{
    EACCES, EFAULT, EINVAL, ELOOP, ENAMETOOLONG, ENOENT, EOPNOTSUPP, EPERM, ERANGE,
};

struct Credentials {
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
    capabilities: u64,
}

impl Credentials {
    fn current() -> Result<Self, i32> {
        let count = unsafe { crate::userdb::kinakaze_abi_getgroups(0, ptr::null_mut()) };
        if count < 0 {
            return Err(crate::kinakaze_errno());
        }
        let mut groups = vec![0; count as usize];
        if count > 0 {
            let actual =
                unsafe { crate::userdb::kinakaze_abi_getgroups(count, groups.as_mut_ptr()) };
            if actual < 0 {
                return Err(crate::kinakaze_errno());
            }
            groups.truncate(actual as usize);
        }
        Ok(Self {
            uid: crate::userdb::kinakaze_abi_geteuid(),
            gid: crate::userdb::kinakaze_abi_getegid(),
            groups,
            capabilities: crate::userdb::effective_capabilities()?,
        })
    }

    fn capable(&self, bit: u32) -> bool {
        self.capabilities & (1u64 << bit) != 0
    }

    fn access(&self, stat: &Stat, name: &[u8], write: bool) -> Result<(), i32> {
        let denied = if write { EPERM } else { ENODATA };
        if let Some(suffix) = name.strip_prefix(b"trusted.") {
            if !self.capable(21) {
                return Err(denied);
            } // CAP_SYS_ADMIN
            return if suffix.is_empty() {
                Err(EINVAL)
            } else {
                Ok(())
            };
        }
        let Some(suffix) = name.strip_prefix(b"user.") else {
            // In particular security.capability needs exec-time capability
            // enforcement, and system.posix_acl_* needs an actual ACL handler.
            return Err(EOPNOTSUPP);
        };
        if !matches!(stat.st_mode & fs::S_IFMT, fs::S_IFREG | fs::S_IFDIR) {
            return Err(denied);
        }
        if write
            && stat.st_mode & (fs::S_IFMT | 0o1000) == fs::S_IFDIR | 0o1000
            && self.uid != stat.st_uid
            && !self.capable(3)
        {
            // CAP_FOWNER
            return Err(EPERM);
        }
        let shift = if self.uid == stat.st_uid {
            6
        } else if self.gid == stat.st_gid || self.groups.contains(&stat.st_gid) {
            3
        } else {
            0
        };
        let required = if write { 2 } else { 4 };
        if (stat.st_mode >> shift) & required == 0
            && !self.capable(1)
            && (write || !self.capable(2))
        {
            // DAC_OVERRIDE / DAC_READ_SEARCH
            return Err(EACCES);
        }
        if suffix.is_empty() {
            return Err(EINVAL);
        }
        Ok(())
    }

    fn listed(&self, name: &[u8]) -> bool {
        name.starts_with(b"user.") || name.starts_with(b"trusted.") && self.capable(21)
    }
}

#[derive(Clone, Copy)]
enum Target {
    Path(*const c_char, bool),
    Fd(i32),
}

struct Opened {
    attributes: Attributes,
    overlay: bool,
    _write: Option<kinakaze_vfs::mount::overlay::WritePath>,
}
impl std::ops::Deref for Opened {
    type Target = Attributes;
    fn deref(&self) -> &Attributes {
        &self.attributes
    }
}
impl Opened {
    fn private(&self, name: &[u8]) -> bool {
        self.overlay
            && (name.starts_with(b"trusted.overlay.") || name.starts_with(b"user.overlay."))
    }
}

impl Target {
    /// # Safety
    /// Path pointers, when present, must designate readable C strings.
    unsafe fn open(self, write: bool) -> Result<Opened, i32> {
        match self {
            Self::Fd(fd) => {
                let overlay = kinakaze_vfs::mount::overlay::descriptor_path(fd)?.is_some();
                Ok(Opened {
                    attributes: Attributes::from_fd(fd, write)?,
                    overlay,
                    _write: None,
                })
            }
            Self::Path(pointer, follow) => {
                if pointer.is_null() {
                    return Err(EFAULT);
                }
                let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
                if bytes.is_empty() {
                    return Err(ENOENT);
                }
                if bytes.len() >= 4096 {
                    return Err(ENAMETOOLONG);
                }
                let absolute = fs::absolute_linux(core::str::from_utf8(bytes).map_err(|_| EINVAL)?);
                let overlay =
                    kinakaze_vfs::mount::overlay::is_overlay_path(&absolute, follow, false)?;
                if write {
                    let guard =
                        kinakaze_vfs::mount::overlay::prepare_write(&absolute, follow, false)?;
                    let attributes = Attributes::open_host(&guard, true)?;
                    return Ok(Opened {
                        attributes,
                        overlay,
                        _write: Some(guard),
                    });
                }
                let guard = kinakaze_vfs::mount::overlay::metadata_path(&absolute, follow)?;
                Ok(Opened {
                    attributes: Attributes::open_host(&guard, false)?,
                    overlay,
                    _write: Some(guard),
                })
            }
        }
    }
}

unsafe fn name_bytes<'a>(name: *const c_char) -> Result<&'a [u8], i32> {
    if name.is_null() {
        return Err(EFAULT);
    }
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    if bytes.is_empty() || bytes.len() > 255 {
        return Err(ERANGE);
    }
    Ok(bytes)
}

fn posix(result: Result<usize, i32>) -> isize {
    match result {
        Ok(length) => length as isize,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

unsafe fn copy_out(bytes: &[u8], output: *mut c_void, size: usize) -> Result<usize, i32> {
    if size != 0 {
        if size < bytes.len() {
            return Err(ERANGE);
        }
        if !bytes.is_empty() {
            if output.is_null() {
                return Err(EFAULT);
            }
            unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len()) };
        }
    }
    Ok(bytes.len())
}

unsafe fn get(target: Target, name: *const c_char, output: *mut c_void, size: usize) -> isize {
    posix((|| {
        let name = unsafe { name_bytes(name) }?;
        let attrs = unsafe { target.open(false) }?;
        if attrs.private(name) {
            return Err(ENODATA);
        }
        Credentials::current()?.access(&attrs.metadata()?, name, false)?;
        unsafe { copy_out(&attrs.get(name)?, output, size) }
    })())
}

unsafe fn set(
    target: Target,
    name: *const c_char,
    value: *const c_void,
    size: usize,
    flags: i32,
) -> c_int {
    posix((|| {
        if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
            return Err(EINVAL);
        }
        let name = unsafe { name_bytes(name) }?;
        if size > XATTR_SIZE_MAX {
            return Err(7);
        } // E2BIG
        let value = if size == 0 {
            &[]
        } else {
            if value.is_null() {
                return Err(EFAULT);
            }
            unsafe { core::slice::from_raw_parts(value.cast::<u8>(), size) }
        };
        let attrs = unsafe { target.open(true) }?;
        if attrs.private(name) {
            return Err(EPERM);
        }
        Credentials::current()?.access(&attrs.metadata()?, name, true)?;
        attrs.set(name, value, flags)?;
        Ok(0)
    })()) as c_int
}

unsafe fn list(target: Target, output: *mut c_char, size: usize) -> isize {
    posix((|| {
        let attrs = unsafe { target.open(false) }?;
        let credentials = Credentials::current()?;
        let mut names = Vec::new();
        // listxattr does not impose getxattr's read-permission check. The
        // filesystem filters private namespaces but lists ordinary user names.
        for name in attrs
            .list()?
            .into_iter()
            .filter(|name| credentials.listed(name) && !attrs.private(name))
        {
            names.extend_from_slice(&name);
            names.push(0);
        }
        unsafe { copy_out(&names, output.cast(), size) }
    })())
}

unsafe fn remove(target: Target, name: *const c_char) -> c_int {
    posix((|| {
        let name = unsafe { name_bytes(name) }?;
        let attrs = unsafe { target.open(true) }?;
        if attrs.private(name) {
            return Err(EPERM);
        }
        Credentials::current()?.access(&attrs.metadata()?, name, true)?;
        attrs.remove(name)?;
        Ok(0)
    })()) as c_int
}

/// x86-64 assigns 188..199 to four triplets: set/get/list/remove, each in
/// path/lpath/fd order. Both syscall and exported libc entry points use the
/// same operations; only the outer dispatcher converts errno to -errno.
pub(crate) unsafe fn syscall_abi_result(
    number: i64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> isize {
    let index = number - crate::sysadmin::SYS_SETXATTR;
    if !(0..12).contains(&index) {
        return posix(Err(EINVAL));
    }
    let target = match index % 3 {
        0 => Target::Path(a1 as *const c_char, true),
        1 => Target::Path(a1 as *const c_char, false),
        _ => Target::Fd(a1 as c_int),
    };
    unsafe {
        match index / 3 {
            0 => set(
                target,
                a2 as *const c_char,
                a3 as *const c_void,
                a4 as usize,
                a5 as c_int,
            ) as isize,
            1 => get(target, a2 as *const c_char, a3 as *mut c_void, a4 as usize),
            2 => list(target, a2 as *mut c_char, a3 as usize),
            _ => remove(target, a2 as *const c_char) as isize,
        }
    }
}

macro_rules! path_abi {
    ($get:ident, $set:ident, $list:ident, $remove:ident, $follow:expr) => {
        /// # Safety
        /// C strings and the output buffer must be valid for their stated sizes.
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $get(
            path: *const c_char,
            name: *const c_char,
            value: *mut c_void,
            size: usize,
        ) -> isize {
            unsafe { get(Target::Path(path, $follow), name, value, size) }
        }
        /// # Safety
        /// C strings and the input buffer must be readable for their stated sizes.
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $set(
            path: *const c_char,
            name: *const c_char,
            value: *const c_void,
            size: usize,
            flags: c_int,
        ) -> c_int {
            unsafe { set(Target::Path(path, $follow), name, value, size, flags) }
        }
        /// # Safety
        /// Path and the output buffer must be valid for their stated sizes.
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $list(
            path: *const c_char,
            value: *mut c_char,
            size: usize,
        ) -> isize {
            unsafe { list(Target::Path(path, $follow), value, size) }
        }
        /// # Safety
        /// Both arguments must designate readable C strings.
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $remove(path: *const c_char, name: *const c_char) -> c_int {
            unsafe { remove(Target::Path(path, $follow), name) }
        }
    };
}

path_abi!(
    kinakaze_abi_getxattr,
    kinakaze_abi_setxattr,
    kinakaze_abi_listxattr,
    kinakaze_abi_removexattr,
    true
);
path_abi!(
    kinakaze_abi_lgetxattr,
    kinakaze_abi_lsetxattr,
    kinakaze_abi_llistxattr,
    kinakaze_abi_lremovexattr,
    false
);

/// # Safety
/// Name and output buffer must be valid for their stated sizes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetxattr(
    fd: c_int,
    name: *const c_char,
    value: *mut c_void,
    size: usize,
) -> isize {
    unsafe { get(Target::Fd(fd), name, value, size) }
}

/// # Safety
/// Name and input buffer must be readable for their stated sizes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fsetxattr(
    fd: c_int,
    name: *const c_char,
    value: *const c_void,
    size: usize,
    flags: c_int,
) -> c_int {
    unsafe { set(Target::Fd(fd), name, value, size, flags) }
}

/// # Safety
/// Output buffer must be writable for `size` bytes, unless `size` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_flistxattr(
    fd: c_int,
    value: *mut c_char,
    size: usize,
) -> isize {
    unsafe { list(Target::Fd(fd), value, size) }
}

/// # Safety
/// Name must designate a readable C string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fremovexattr(fd: c_int, name: *const c_char) -> c_int {
    unsafe { remove(Target::Fd(fd), name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinakaze_vfs::path;
    use std::ffi::CString;

    struct Fixture {
        host: std::path::PathBuf,
        guest: CString,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let name = format!("xattr-test-{}-{nonce}", std::process::id());
            let host = path::system_root().unwrap().join(&name);
            std::fs::create_dir(&host).unwrap();
            Self {
                host,
                guest: CString::new(format!("/{name}/file")).unwrap(),
            }
        }
        fn file(&self) -> &CStr {
            std::fs::write(self.host.join("file"), b"payload").unwrap();
            fs::set_mode_host_path(&self.host.join("file"), 0o666).unwrap();
            &self.guest
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.host);
        }
    }

    #[test]
    fn all_twelve_abi_calls_share_persistent_inode_attributes() {
        let fixture = Fixture::new();
        let file = fixture.file();
        let fd = fs::open(file.to_str().unwrap(), fs::O_RDONLY, 0).unwrap();
        let key = c"user.binary";
        let data = b"a\0b\xff";
        let mut out = [0xadu8; 64];
        unsafe {
            assert_eq!(
                kinakaze_abi_setxattr(
                    file.as_ptr(),
                    key.as_ptr(),
                    data.as_ptr().cast(),
                    data.len(),
                    XATTR_CREATE
                ),
                0
            );
            assert_eq!(
                kinakaze_abi_getxattr(file.as_ptr(), key.as_ptr(), ptr::null_mut(), 0),
                4
            );
            assert_eq!(
                kinakaze_abi_lgetxattr(
                    file.as_ptr(),
                    key.as_ptr(),
                    out.as_mut_ptr().cast(),
                    out.len()
                ),
                4
            );
            assert_eq!(&out[..4], data);
            out.fill(0xad);
            assert_eq!(
                kinakaze_abi_fgetxattr(fd, key.as_ptr(), out.as_mut_ptr().cast(), 3),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), ERANGE);
            assert_eq!(out, [0xad; 64], "ERANGE must not partially copy");
            assert_eq!(
                kinakaze_abi_fgetxattr(fd, key.as_ptr(), out.as_mut_ptr().cast(), out.len()),
                4
            );
            assert_eq!(&out[..4], data);
            assert_eq!(
                kinakaze_abi_fsetxattr(fd, c"user.empty".as_ptr(), ptr::null(), 0, XATTR_CREATE),
                0
            );
            assert_eq!(
                kinakaze_abi_fgetxattr(fd, c"user.empty".as_ptr(), ptr::null_mut(), 0),
                0
            );
            assert_eq!(
                kinakaze_abi_lsetxattr(
                    file.as_ptr(),
                    c"user.third".as_ptr(),
                    data.as_ptr().cast(),
                    2,
                    0
                ),
                0
            );
            let length = kinakaze_abi_listxattr(file.as_ptr(), ptr::null_mut(), 0);
            assert_eq!(length, 34);
            assert_eq!(
                kinakaze_abi_llistxattr(file.as_ptr(), out.as_mut_ptr().cast(), out.len()),
                length
            );
            assert_eq!(
                &out[..length as usize],
                b"user.binary\0user.empty\0user.third\0"
            );
            assert_eq!(
                kinakaze_abi_flistxattr(fd, out.as_mut_ptr().cast(), out.len()),
                length
            );
            assert_eq!(kinakaze_abi_fremovexattr(fd, c"user.empty".as_ptr()), 0);
            assert_eq!(
                kinakaze_abi_lremovexattr(file.as_ptr(), c"user.third".as_ptr()),
                0
            );
            assert_eq!(kinakaze_abi_removexattr(file.as_ptr(), key.as_ptr()), 0);
            assert_eq!(kinakaze_abi_removexattr(file.as_ptr(), key.as_ptr()), -1);
            assert_eq!(crate::kinakaze_errno(), ENODATA);
            assert_eq!(kinakaze_abi_flistxattr(fd, ptr::null_mut(), 0), 0);
        }
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn symlink_rules_do_not_write_attributes_to_its_target() {
        let fixture = Fixture::new();
        let file = fixture.file();
        path::create_emulated_symlink(&fixture.host.join("link"), "file").unwrap();
        let link =
            CString::new(file.to_str().unwrap().trim_end_matches("file").to_owned() + "link")
                .unwrap();
        unsafe {
            assert_eq!(
                kinakaze_abi_setxattr(
                    link.as_ptr(),
                    c"user.link".as_ptr(),
                    b"v".as_ptr().cast(),
                    1,
                    0
                ),
                0
            );
            assert_eq!(
                kinakaze_abi_getxattr(file.as_ptr(), c"user.link".as_ptr(), ptr::null_mut(), 0),
                1
            );
            assert_eq!(
                kinakaze_abi_lsetxattr(
                    link.as_ptr(),
                    c"user.link".as_ptr(),
                    b"x".as_ptr().cast(),
                    1,
                    0
                ),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), EPERM);
            assert_eq!(
                kinakaze_abi_lgetxattr(link.as_ptr(), c"user.link".as_ptr(), ptr::null_mut(), 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), ENODATA);
            assert_eq!(
                kinakaze_abi_llistxattr(link.as_ptr(), ptr::null_mut(), 0),
                0
            );
        }
    }

    #[test]
    fn errors_do_not_create_or_modify_attributes() {
        let fixture = Fixture::new();
        let file = fixture.file();
        unsafe {
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 1, 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), EFAULT);
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 65537, 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), 7);
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 0, 4),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), EINVAL);
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 0, 3),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), ENODATA);
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 0, 1),
                0
            );
            assert_eq!(
                kinakaze_abi_setxattr(file.as_ptr(), c"user.test".as_ptr(), ptr::null(), 0, 3),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EEXIST);
            assert_eq!(
                kinakaze_abi_getxattr(file.as_ptr(), c"".as_ptr(), ptr::null_mut(), 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), ERANGE);
            assert_eq!(
                kinakaze_abi_getxattr(c"".as_ptr(), c"user.test".as_ptr(), ptr::null_mut(), 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), ENOENT);
            assert_eq!(
                kinakaze_abi_getxattr(
                    file.as_ptr(),
                    c"security.capability".as_ptr(),
                    ptr::null_mut(),
                    0
                ),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), EOPNOTSUPP);
            assert_eq!(
                kinakaze_abi_fgetxattr(-1, c"user.test".as_ptr(), ptr::null_mut(), 0),
                -1
            );
            assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EBADF);
        }
        let fd = fs::open(file.to_str().unwrap(), fs::O_PATH, 0).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_fgetxattr(fd, c"user.test".as_ptr(), ptr::null_mut(), 0) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EBADF);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn namespace_permissions_use_capabilities_and_file_mode_not_root_shortcuts() {
        let mut creds = Credentials {
            uid: 42,
            gid: 50,
            groups: vec![100],
            capabilities: 0,
        };
        let mut stat = Stat {
            st_mode: fs::S_IFREG | 0o640,
            st_uid: 42,
            st_gid: 100,
            ..Stat::default()
        };
        assert_eq!(creds.access(&stat, b"user.a", true), Ok(()));
        creds.uid = 43;
        assert_eq!(creds.access(&stat, b"user.a", false), Ok(()));
        assert_eq!(creds.access(&stat, b"user.a", true), Err(EACCES));
        creds.uid = 0;
        assert_eq!(creds.access(&stat, b"trusted.a", false), Err(ENODATA));
        assert_eq!(creds.access(&stat, b"trusted.a", true), Err(EPERM));
        assert!(!creds.listed(b"trusted.a"));
        creds.capabilities = 1 << 21;
        assert_eq!(creds.access(&stat, b"trusted.a", true), Ok(()));
        assert!(creds.listed(b"trusted.a"));
        assert_eq!(
            creds.access(&stat, b"system.posix_acl_access", true),
            Err(EOPNOTSUPP)
        );
        stat.st_mode = fs::S_IFDIR | 0o1777;
        assert_eq!(creds.access(&stat, b"user.a", true), Err(EPERM));
        creds.capabilities |= 1 << 3;
        assert_eq!(creds.access(&stat, b"user.a", true), Ok(()));
    }
}
