//! POSIX realtime ABI backed by the worker's canonical libc services.
//!
//! Clocks, descriptor tables and queues are owned once by the runtime engine.
//! POSIX timers share one scheduler and the canonical signal/pthread backends.
//! The legacy success-only AIO placeholders are absent.

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod timers;

#[cfg(all(windows, target_arch = "x86_64"))]
pub mod windows {
    use core::ffi::{CStr, c_char, c_int};
    pub use libc::time::TimeSpec as Timespec;

    fn fail(error: i32) -> i32 {
        unsafe {
            *libc::kinakaze_abi___errno_location() = error;
        }
        -1
    }

    #[unsafe(export_name = "kinakaze_engine_librt_clock_gettime")]
    pub unsafe extern "sysv64" fn clock_gettime(clock: c_int, value: *mut Timespec) -> c_int {
        unsafe { libc::time::kinakaze_abi_clock_gettime(clock, value) }
    }

    #[unsafe(export_name = "kinakaze_engine_librt_clock_getres")]
    pub unsafe extern "sysv64" fn clock_getres(clock: c_int, value: *mut Timespec) -> c_int {
        unsafe { libc::time::kinakaze_abi_clock_getres(clock, value) }
    }

    #[unsafe(export_name = "kinakaze_engine_librt_clock_settime")]
    pub unsafe extern "sysv64" fn clock_settime(clock: c_int, value: *const Timespec) -> c_int {
        unsafe { libc::time::kinakaze_abi_clock_settime(clock, value) }
    }

    #[unsafe(export_name = "kinakaze_engine_librt_clock_nanosleep")]
    pub unsafe extern "sysv64" fn clock_nanosleep(
        clock: c_int,
        flags: c_int,
        request: *const Timespec,
        remaining: *mut Timespec,
    ) -> c_int {
        unsafe { libc::time::kinakaze_abi_clock_nanosleep(clock, flags, request, remaining) }
    }

    fn shared_object_path(name: &[u8]) -> Result<Vec<u8>, i32> {
        // Linux permits leading slashes but no slash inside the object name.
        let name = name
            .iter()
            .position(|b| *b != b'/')
            .map_or(&[][..], |n| &name[n..]);
        if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
            return Err(22);
        }
        if name.len() > 255 {
            return Err(36);
        }
        let mut path = b"/dev/shm/".to_vec();
        path.extend_from_slice(name);
        path.push(0);
        Ok(path)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_shm_open")]
    pub unsafe extern "sysv64" fn shm_open(name: *const c_char, flags: c_int, mode: u32) -> c_int {
        if name.is_null() {
            return fail(14);
        }
        let path = match shared_object_path(unsafe { CStr::from_ptr(name).to_bytes() }) {
            Ok(path) => path,
            Err(error) => return fail(error),
        };
        // POSIX shm descriptors always have FD_CLOEXEC set.
        unsafe { libc::fs::open(path.as_ptr().cast(), flags | 0x80000, mode) }
    }

    #[unsafe(export_name = "kinakaze_engine_librt_shm_unlink")]
    pub unsafe extern "sysv64" fn shm_unlink(name: *const c_char) -> c_int {
        if name.is_null() {
            return fail(14);
        }
        let path = match shared_object_path(unsafe { CStr::from_ptr(name).to_bytes() }) {
            Ok(path) => path,
            Err(error) => return fail(error),
        };
        unsafe { libc::fs::unlink(path.as_ptr().cast()) }
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_open")]
    pub extern "sysv64" fn mq_open(name: usize, flags: i32, mode: u32, attributes: usize) -> i32 {
        libc::sysadmin::mqueue::mq_open(name, flags, mode, attributes)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_close")]
    pub extern "sysv64" fn mq_close(fd: i32) -> i32 {
        libc::sysadmin::mqueue::mq_close(fd)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_unlink")]
    pub extern "sysv64" fn mq_unlink(name: usize) -> i32 {
        libc::sysadmin::mqueue::mq_unlink(name)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_send")]
    pub extern "sysv64" fn mq_send(fd: i32, data: usize, length: usize, priority: u32) -> i32 {
        libc::sysadmin::mqueue::mq_send(fd, data, length, priority)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_receive")]
    pub extern "sysv64" fn mq_receive(
        fd: i32,
        data: usize,
        length: usize,
        priority: usize,
    ) -> isize {
        libc::sysadmin::mqueue::mq_receive(fd, data, length, priority)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_timedsend")]
    pub extern "sysv64" fn mq_timedsend(
        fd: i32,
        data: usize,
        length: usize,
        priority: u32,
        time: usize,
    ) -> i32 {
        libc::sysadmin::mqueue::mq_timedsend(fd, data, length, priority, time)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_timedreceive")]
    pub extern "sysv64" fn mq_timedreceive(
        fd: i32,
        data: usize,
        length: usize,
        priority: usize,
        time: usize,
    ) -> isize {
        libc::sysadmin::mqueue::mq_timedreceive(fd, data, length, priority, time)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_notify")]
    pub extern "sysv64" fn mq_notify(fd: i32, notification: usize) -> i32 {
        libc::sysadmin::mqueue::mq_notify(fd, notification)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_getattr")]
    pub extern "sysv64" fn mq_getattr(fd: i32, attributes: usize) -> i32 {
        libc::sysadmin::mqueue::mq_setattr(fd, 0, attributes)
    }

    #[unsafe(export_name = "kinakaze_engine_librt_mq_setattr")]
    pub extern "sysv64" fn mq_setattr(fd: i32, attributes: usize, old: usize) -> i32 {
        libc::sysadmin::mqueue::mq_setattr(fd, attributes, old)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn shared_names_stay_in_posix_shm_namespace() {
            assert_eq!(
                shared_object_path(b"/sample").unwrap(),
                b"/dev/shm/sample\0"
            );
            assert_eq!(
                shared_object_path(b"///sample").unwrap(),
                b"/dev/shm/sample\0"
            );
            for invalid in [b"".as_slice(), b"/", b"..", b"/../escape", b"/dir/file"] {
                assert_eq!(shared_object_path(invalid), Err(22));
            }
            assert_eq!(shared_object_path(&[b'x'; 256]), Err(36));
        }
        #[test]
        fn clocks_validate_actual_linux_clock_ids() {
            assert_eq!(unsafe { clock_getres(-1, core::ptr::null_mut()) }, -1);
            assert_eq!(unsafe { *libc::kinakaze_abi___errno_location() }, 22);
            let mut value = Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            assert_eq!(unsafe { clock_gettime(1, &mut value) }, 0);
            assert!(value.tv_sec >= 0 && (0..1_000_000_000).contains(&value.tv_nsec));
            let request = Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            assert_eq!(
                unsafe { clock_nanosleep(1, 2, &request, core::ptr::null_mut()) },
                22
            );
        }
    }
}

/// Direct Linux timer syscall service owned by librt.
/// # Safety
/// Guest pointers must satisfy the syscall's Linux ABI.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_posix_timer_syscall(
    number: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
) -> i64 {
    unsafe { timers::raw_syscall(number, a1, a2, a3, a4) }
}

#[cfg(windows)]
extern "C" fn install_timer_service() {
    kinakaze_runtime::services::install_timers(kinakaze_process_posix_timer_syscall);
}
#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static TIMER_SERVICE: extern "C" fn() = install_timer_service;

mod object_layout;
