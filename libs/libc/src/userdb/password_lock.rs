//! Process-scoped /etc/.pwd.lock using POSIX record locks and SIGALRM timeout.
use crate::{
    fdio::{Flock, kinakaze_abi_fcntl64},
    signal::{SigAction, SigSet},
};
use core::{
    ffi::c_int,
    ptr,
    sync::atomic::{AtomicI32, Ordering},
};
use kinakaze_vfs::{
    fs,
    signal::{SIG_SETMASK, SIG_UNBLOCK, SIGALRM},
};
use std::sync::Mutex;

static DESCRIPTOR: AtomicI32 = AtomicI32::new(-1);
static MUTEX: Mutex<()> = Mutex::new(());
unsafe extern "sysv64" fn timeout(_signal: c_int) {}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lckpwdf() -> c_int {
    if DESCRIPTOR.load(Ordering::Acquire) != -1 {
        return -1;
    }
    let _guard = MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    if DESCRIPTOR.load(Ordering::Acquire) != -1 {
        return -1;
    }
    let fd = unsafe {
        crate::fs::open(
            c"/etc/.pwd.lock".as_ptr(),
            fs::O_WRONLY | fs::O_CREAT | fs::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return -1;
    }
    DESCRIPTOR.store(fd, Ordering::Release);
    let action = SigAction {
        sa_handler: timeout as *const () as usize,
        sa_mask: SigSet {
            bits: [u64::MAX; 16],
        },
        sa_flags: 0,
        _padding: 0,
        sa_restorer: 0,
    };
    let mut saved_action = action;
    let result = unsafe {
        if crate::signal::kinakaze_abi_sigaction(SIGALRM, &action, &mut saved_action) != 0 {
            -1
        } else {
            let mut mask = SigSet { bits: [0; 16] };
            mask.bits[0] = 1 << (SIGALRM - 1);
            let mut saved_mask = mask;
            let result = if crate::signal::kinakaze_abi_sigprocmask(
                SIG_UNBLOCK,
                &mask,
                &mut saved_mask,
            ) != 0
            {
                -1
            } else {
                crate::time::kinakaze_abi_alarm(15);
                let mut lock = Flock {
                    l_type: 1,
                    ..Flock::default()
                };
                let result = kinakaze_abi_fcntl64(fd, 7, (&mut lock as *mut Flock) as usize);
                let errno = crate::kinakaze_errno();
                crate::time::kinakaze_abi_alarm(0);
                crate::signal::kinakaze_abi_sigprocmask(SIG_SETMASK, &saved_mask, ptr::null_mut());
                crate::set_errno(errno);
                result
            };
            let errno = crate::kinakaze_errno();
            crate::signal::kinakaze_abi_sigaction(SIGALRM, &saved_action, ptr::null_mut());
            crate::set_errno(errno);
            result
        }
    };
    if result < 0 {
        let errno = crate::kinakaze_errno();
        crate::kinakaze_abi_close(fd);
        DESCRIPTOR.store(-1, Ordering::Release);
        crate::set_errno(errno);
    }
    result
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ulckpwdf() -> c_int {
    let _guard = MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let fd = DESCRIPTOR.swap(-1, Ordering::AcqRel);
    if fd < 0 {
        -1
    } else {
        crate::kinakaze_abi_close(fd)
    }
}

// fork inherits libc's descriptor variable, but does not inherit ownership of
// the parent's POSIX lock. exec starts with -1 and closes the CLOEXEC descriptor.
unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    if buffer.is_null() {
        return 4;
    }
    if capacity < 4 {
        return -(kinakaze_vfs::ENOMEM as isize);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            DESCRIPTOR.load(Ordering::Acquire).to_le_bytes().as_ptr(),
            buffer,
            4,
        )
    };
    4
}
unsafe extern "system" fn child(buffer: *const u8, length: usize) -> c_int {
    if buffer.is_null() || length != 4 {
        return kinakaze_vfs::EINVAL;
    }
    let fd = i32::from_le_bytes(unsafe { ptr::read_unaligned(buffer.cast::<[u8; 4]>()) });
    if fd < -1 {
        return kinakaze_vfs::EINVAL;
    }
    DESCRIPTOR.store(fd, Ordering::Release);
    0
}
extern "C" fn initialize() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 40,
        key: u64::from_le_bytes(*b"LIBCPWDL"),
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = initialize;
