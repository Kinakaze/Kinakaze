//! Linux fs-verity ioctl layouts and fault-contained userspace copies.
//!
//! Persistence and verification live in the VFS, not in this ABI adapter.

use core::ffi::{c_int, c_void};
use kinakaze_vfs::fs::verity;
use kinakaze_vfs::{EFAULT, EINVAL, ENOTTY, EOPNOTSUPP, EPERM};

pub(super) const ENABLE: u64 = 0x4080_6685;
pub(super) const MEASURE: u64 = 0xc004_6686;
pub(super) const READ_METADATA: u64 = 0xc028_6687;
const GETFLAGS: u64 = 0x8008_6601;
const SETFLAGS: u64 = 0x4008_6602;
const FS_VERITY_FL: u32 = 0x0010_0000;
const EMSGSIZE: i32 = 90;
const EOVERFLOW: i32 = 75;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn ReadProcessMemory(
        process: *mut c_void,
        source: *const c_void,
        destination: *mut c_void,
        length: usize,
        read: *mut usize,
    ) -> i32;
}

// Reading through the kernel copy primitive avoids dereferencing invalid guest
// pointers in Rust. A null/overflowing/nonresident userspace pointer is EFAULT,
// including buffers spanning a guard page, rather than a host access violation.
fn read_user(address: usize, destination: &mut [u8]) -> Result<(), i32> {
    if destination.is_empty() {
        return Ok(());
    }
    if address == 0 || address.checked_add(destination.len()).is_none() {
        return Err(EFAULT);
    }
    let mut copied = 0;
    if unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            destination.as_mut_ptr().cast(),
            destination.len(),
            &mut copied,
        )
    } == 0
        || copied != destination.len()
    {
        return Err(EFAULT);
    }
    Ok(())
}

fn write_user(address: usize, source: &[u8]) -> Result<(), i32> {
    if source.is_empty() {
        return Ok(());
    }
    if address == 0 || address.checked_add(source.len()).is_none() {
        return Err(EFAULT);
    }
    // Read our owned source into the guest destination using the kernel's
    // output-buffer probe. WriteProcessMemory is a debugger operation that can
    // temporarily bypass PAGE_READONLY; a VirtualQuery precheck cannot close
    // its concurrent mprotect race. This copy never changes guest protections.
    let mut copied = 0;
    if unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            source.as_ptr().cast(),
            address as *mut c_void,
            source.len(),
            &mut copied,
        )
    } == 0
        || copied != source.len()
    {
        return Err(EFAULT);
    }
    Ok(())
}

pub(super) fn handles(request: u64) -> bool {
    matches!(
        request,
        ENABLE | MEASURE | READ_METADATA | GETFLAGS | SETFLAGS
    )
}

pub(super) fn ioctl(fd: c_int, request: u64, argument: *mut c_void) -> Result<c_int, i32> {
    let opened = verity::Opened::from_fd(fd)?;
    ioctl_opened(fd, &opened, request, argument)
}

fn ioctl_opened(
    fd: c_int,
    opened: &verity::Opened,
    request: u64,
    argument: *mut c_void,
) -> Result<c_int, i32> {
    let argument = argument as usize;
    match request {
        ENABLE => {
            let mut bytes = [0u8; 128];
            read_user(argument, &mut bytes)?;
            let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            if word(0) != 1 || word(28) != 0 || bytes[40..].iter().any(|&v| v != 0) {
                return Err(EINVAL);
            }
            let block_size = word(8);
            if !block_size.is_power_of_two() {
                return Err(EINVAL);
            }
            let salt_size = word(12) as usize;
            let signature_size = word(24);
            if salt_size > 32 || signature_size > 16_128 {
                return Err(EMSGSIZE);
            }
            // Built-in signature verification is an optional Linux feature.
            // Do not accept a signature that this filesystem cannot verify.
            if signature_size != 0 {
                return Err(EOPNOTSUPP);
            }
            let mut salt = [0u8; 32];
            let salt_pointer = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
            read_user(salt_pointer, &mut salt[..salt_size])?;
            if opened.is_directory() {
                return Err(kinakaze_vfs::EISDIR);
            }
            crate::fdio::verity::reject_unverified_mappings(fd)?;
            verity::enable(fd, word(4), block_size, &salt[..salt_size])?;
            Ok(0)
        }
        MEASURE => {
            // Linux checks whether verity is enabled before reading the buffer.
            let (algorithm, digest) = opened.measure()?;
            let mut capacity = [0u8; 2];
            read_user(argument.checked_add(2).ok_or(EFAULT)?, &mut capacity)?;
            if usize::from(u16::from_le_bytes(capacity)) < digest.len() {
                return Err(EOVERFLOW);
            }
            let mut header = [0u8; 4];
            header[..2].copy_from_slice(&(algorithm as u16).to_le_bytes());
            header[2..].copy_from_slice(&(digest.len() as u16).to_le_bytes());
            write_user(argument, &header)?;
            write_user(argument.checked_add(4).ok_or(EFAULT)?, &digest)?;
            Ok(0)
        }
        READ_METADATA => {
            opened.measure()?;
            let mut bytes = [0u8; 40];
            read_user(argument, &mut bytes)?;
            let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            let kind = word(0);
            let offset = word(8);
            let length = word(16);
            if word(32) != 0 || offset.checked_add(length).is_none() {
                return Err(EINVAL);
            }
            let length = length.min(c_int::MAX as u64) as usize;
            let output = word(24) as usize;
            let mut buffer = [0u8; 16 * 1024];
            let mut done = 0;
            // Keep kernel metadata streaming bounded even for enormous lengths.
            loop {
                let wanted = buffer.len().min(length - done);
                let result = opened
                    .read_metadata(kind, offset + done as u64, &mut buffer[..wanted])
                    .and_then(|count| {
                        write_user(output.checked_add(done).ok_or(EFAULT)?, &buffer[..count])?;
                        Ok(count)
                    });
                match result {
                    Ok(count) => {
                        done += count;
                        if count < wanted || done == length {
                            break;
                        }
                    }
                    Err(error) => {
                        return if done == 0 {
                            Err(error)
                        } else {
                            Ok(done as c_int)
                        };
                    }
                }
            }
            Ok(done as c_int)
        }
        GETFLAGS => {
            let flags = if opened.descriptor()?.is_some() {
                FS_VERITY_FL
            } else {
                0
            };
            // The command encodes sizeof(long), but Linux stores an int.
            write_user(argument, &flags.to_le_bytes())?;
            Ok(0)
        }
        SETFLAGS => {
            let mut bytes = [0u8; 4];
            read_user(argument, &mut bytes)?;
            let requested = u32::from_le_bytes(bytes);
            let current = if opened.descriptor()?.is_some() {
                FS_VERITY_FL
            } else {
                0
            };
            if (requested ^ current) & FS_VERITY_FL != 0 {
                return Err(EPERM);
            }
            if requested != current {
                return Err(EOPNOTSUPP);
            }
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinakaze_vfs::EBADF;

    #[test]
    fn verity_user_copy_rejects_bad_and_readonly_pointers() {
        assert_eq!(read_user(1, &mut [0; 4]), Err(EFAULT));
        assert_eq!(read_user(usize::MAX - 1, &mut [0; 4]), Err(EFAULT));
        assert_eq!(write_user(1, &[0; 4]), Err(EFAULT));
        assert_eq!(write_user(usize::MAX - 1, &[0; 4]), Err(EFAULT));
        assert_eq!(read_user(0, &mut []), Ok(()));
        assert_eq!(write_user(0, &[]), Ok(()));
        let mut value = [1u8, 2, 3, 4];
        let mut copy = [0u8; 4];
        read_user(value.as_ptr() as usize, &mut copy).unwrap();
        assert_eq!(copy, value);
        write_user(value.as_mut_ptr() as usize, &[4, 3, 2, 1]).unwrap();
        assert_eq!(value, [4, 3, 2, 1]);
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, VirtualAlloc, VirtualFree,
        };
        let readonly = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                4096,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READONLY,
            )
        };
        assert!(!readonly.is_null());
        assert_eq!(write_user(readonly as usize, &[1, 2, 3, 4]), Err(EFAULT));
        assert_eq!(unsafe { *(readonly as *const u32) }, 0);
        assert_ne!(unsafe { VirtualFree(readonly, 0, MEM_RELEASE) }, 0);
    }

    #[test]
    fn verity_user_output_probe_respects_cross_page_and_changed_protections() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
            VirtualAlloc, VirtualFree, VirtualProtect,
        };
        let area = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                8192,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!area.is_null());
        let boundary = area as usize + 4096;
        let mut old = 0;
        for protection in [PAGE_READONLY, PAGE_NOACCESS, PAGE_READONLY] {
            assert_ne!(
                unsafe { VirtualProtect(boundary as *const c_void, 4096, protection, &mut old) },
                0
            );
            assert_eq!(write_user(boundary - 2, &[1, 2, 3, 4]), Err(EFAULT));
            assert_eq!(write_user(boundary, &[1, 2, 3, 4]), Err(EFAULT));
            assert_ne!(
                unsafe {
                    VirtualProtect(boundary as *const c_void, 4096, PAGE_READWRITE, &mut old)
                },
                0
            );
            assert_eq!(
                old, protection,
                "copy must never relax destination protection"
            );
            assert_eq!(unsafe { *(boundary as *const u32) }, 0);
            write_user(boundary, &[4, 3, 2, 1]).unwrap();
            assert_eq!(unsafe { *(boundary as *const u32) }, 0x0102_0304);
            write_user(boundary, &[0; 4]).unwrap();
        }
        assert_ne!(unsafe { VirtualFree(area, 0, MEM_RELEASE) }, 0);
        assert_eq!(write_user(area as usize, &[1; 4]), Err(EFAULT));
    }

    struct Fixture {
        root: std::path::PathBuf,
        fds: Vec<i32>,
    }

    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "kinakaze-verity-ioctl-pin-{}-{stamp}",
                std::process::id(),
            ));
            std::fs::create_dir(&root).unwrap();
            Self {
                root,
                fds: Vec::new(),
            }
        }

        fn enabled(&mut self, name: &str, byte: u8) -> i32 {
            let path = self.root.join(name);
            std::fs::write(&path, vec![byte; 1024 * 1024]).unwrap();
            let fd = kinakaze_vfs::fs::open(
                &kinakaze_vfs::path::to_guest_path(&path),
                kinakaze_vfs::fs::O_RDONLY,
                0,
            )
            .unwrap();
            self.fds.push(fd);
            // Disposable backend fixtures do not change the production ENABLE gate.
            verity::enable(fd, 1, 1024, name.as_bytes()).unwrap();
            fd
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for fd in self.fds.drain(..) {
                let _ = kinakaze_vfs::close(fd);
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn verity_ioctl_pin_keeps_measure_and_all_metadata_chunks_on_original_inode() {
        let mut fixture = Fixture::new();
        let fd = fixture.enabled("first", 0x48);
        let other = fixture.enabled("replacement", 0xaf);
        let opened = verity::Opened::from_fd(fd).unwrap();
        let original_digest = opened.measure().unwrap().1;
        let descriptor = opened.descriptor().unwrap().unwrap();
        let mut expected = vec![0; descriptor.tree_size().unwrap() as usize];
        assert!(expected.len() > 16 * 1024);
        assert_eq!(
            opened.read_metadata(1, 0, &mut expected),
            Ok(expected.len())
        );
        kinakaze_vfs::close(fd).unwrap();
        assert_eq!(crate::fdio::kinakaze_abi_dup2(other, fd), fd);
        assert_ne!(verity::measure(fd).unwrap().1, original_digest);

        let mut measured = [0u8; 68];
        measured[2..4].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(
            ioctl_opened(fd, &opened, MEASURE, measured.as_mut_ptr().cast()),
            Ok(0)
        );
        assert_eq!(&measured[4..36], original_digest);
        let mut actual = vec![0xcc; expected.len() + 100];
        let mut request = [1, 0, actual.len() as u64, actual.as_mut_ptr() as u64, 0];
        assert_eq!(
            ioctl_opened(fd, &opened, READ_METADATA, request.as_mut_ptr().cast()),
            Ok(expected.len() as c_int),
        );
        assert_eq!(&actual[..expected.len()], expected);
        assert!(actual[expected.len()..].iter().all(|byte| *byte == 0xcc));
        let mut flags = 0u32;
        assert_eq!(
            ioctl_opened(fd, &opened, GETFLAGS, (&mut flags as *mut u32).cast()),
            Ok(0)
        );
        assert_eq!(flags, FS_VERITY_FL);
        assert_eq!(
            ioctl_opened(fd, &opened, SETFLAGS, (&mut flags as *mut u32).cast()),
            Ok(0)
        );

        // A wholly inaccessible output remains fault-contained at the actual
        // ioctl boundary, not only in the memory helper's isolated tests.
        request[3] = 1;
        assert_eq!(
            ioctl_opened(fd, &opened, READ_METADATA, request.as_mut_ptr().cast()),
            Err(EFAULT)
        );
        assert_eq!(
            ioctl_opened(fd, &opened, GETFLAGS, 1usize as *mut c_void),
            Err(EFAULT)
        );
    }

    #[test]
    fn verity_ioctl_invalid_fd_precedes_pointer_access() {
        for command in [ENABLE, MEASURE, READ_METADATA, GETFLAGS, SETFLAGS] {
            assert_eq!(ioctl(-1, command, 1usize as *mut c_void), Err(EBADF));
        }
    }
}
