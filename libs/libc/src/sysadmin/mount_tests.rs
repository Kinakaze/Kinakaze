use super::*;
use std::ffi::CString;
use std::path::PathBuf;
use std::ptr;

struct Fixture {
    root: PathBuf,
    source: CString,
    target: CString,
}
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-mount-dispatch-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        for child in ["source", "target"] {
            std::fs::create_dir(root.join(child)).unwrap();
        }
        std::fs::write(root.join("source/data"), b"source").unwrap();
        std::fs::write(root.join("target/data"), b"target").unwrap();
        let source = CString::new(kinakaze_vfs::to_guest_path(&root.join("source"))).unwrap();
        let target = CString::new(kinakaze_vfs::to_guest_path(&root.join("target"))).unwrap();
        Self {
            root,
            source,
            target,
        }
    }
    fn unmount(&self) {
        assert_eq!(unsafe { kinakaze_abi_umount2(self.target.as_ptr(), 0) }, 0);
        assert_eq!(
            std::fs::read(self.root.join("target/data")).unwrap(),
            b"target"
        );
    }
    fn assert_mounted(&self) {
        let resolved =
            kinakaze_vfs::resolve_linux_path(&format!("{}/data", self.target.to_str().unwrap()))
                .unwrap();
        assert_eq!(std::fs::read(resolved).unwrap(), b"source");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // The fixture owns the unique mount target and both backing directories.
        let _ = unsafe { kinakaze_abi_umount2(self.target.as_ptr(), 0) };
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn bind_operation_ignores_filesystem_and_options_in_abi_and_raw_syscall() {
    let fixture = Fixture::new();
    let opaque_type = CString::new(vec![0xff]).unwrap();
    let opaque_options = [0xffu8; 4096];
    for filesystem in [
        c"overlay".as_ptr(),
        c"cgroup2".as_ptr(),
        opaque_type.as_ptr(),
        ptr::null(),
    ] {
        for raw in [false, true] {
            let result = unsafe {
                if raw {
                    kinakaze_abi_syscall_raw(
                        SYS_MOUNT,
                        fixture.source.as_ptr() as u64,
                        fixture.target.as_ptr() as u64,
                        filesystem as u64,
                        kinakaze_vfs::mount::MS_BIND,
                        opaque_options.as_ptr() as u64,
                        0,
                    )
                } else {
                    kinakaze_abi_mount(
                        fixture.source.as_ptr(),
                        fixture.target.as_ptr(),
                        filesystem,
                        kinakaze_vfs::mount::MS_BIND,
                        opaque_options.as_ptr().cast(),
                    ) as i64
                }
            };
            assert_eq!(result, 0, "raw={raw}, errno={}", kinakaze_tls::errno());
            fixture.assert_mounted();
            fixture.unmount();
        }
    }
}

#[test]
fn mount_flag_precedence_does_not_turn_remount_into_bind_or_overlay() {
    let fixture = Fixture::new();
    for flags in [32 | kinakaze_vfs::mount::MS_BIND, 32, 8192] {
        let expected = if flags & kinakaze_vfs::mount::MS_BIND != 0 {
            EINVAL
        } else {
            ENOSYS
        };
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    ptr::null(),
                    fixture.target.as_ptr(),
                    c"overlay".as_ptr(),
                    flags,
                    ptr::null(),
                )
            },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), expected);
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_MOUNT,
                    0,
                    fixture.target.as_ptr() as u64,
                    c"overlay".as_ptr() as u64,
                    flags,
                    0,
                    0,
                )
            },
            -i64::from(expected)
        );
        assert_eq!(
            unsafe { kinakaze_abi_umount2(fixture.target.as_ptr(), 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL, "no mount was published");
    }
    // Propagation requires an attachment, not merely an existing directory.
    for flags in [1 << 18, 1 << 20, (1 << 20) | kinakaze_vfs::mount::MS_REC] {
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    ptr::null(),
                    fixture.target.as_ptr(),
                    ptr::null(),
                    flags,
                    ptr::null(),
                )
            },
            -1
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    SYS_MOUNT,
                    0,
                    fixture.target.as_ptr() as u64,
                    0,
                    flags,
                    0,
                    0,
                )
            },
            -i64::from(EINVAL)
        );
    }
    // BIND precedes MOVE/propagation, and its initial readonly flag is ignored.
    for flags in [
        kinakaze_vfs::mount::MS_BIND | 8192 | (1 << 18) | 1,
        0xc0ed_0000 | kinakaze_vfs::mount::MS_BIND,
    ] {
        assert_eq!(
            unsafe {
                kinakaze_abi_mount(
                    fixture.source.as_ptr(),
                    fixture.target.as_ptr(),
                    c"overlay".as_ptr(),
                    flags,
                    ptr::null(),
                )
            },
            0
        );
        fixture.assert_mounted();
        fixture.unmount();
    }
}
