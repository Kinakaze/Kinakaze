//! Process-facing scheduling uses the same supported policies as pthreads.
use super::{EFAULT, EINVAL, affinity_process, names_self};
use core::ffi::c_int;
use std::os::windows::io::OwnedHandle;

pub use libpthread::SchedParam;

fn target(pid: c_int, write: bool) -> Result<Option<OwnedHandle>, i32> {
    if pid < 0 {
        return Err(EINVAL);
    }
    if names_self(pid) {
        Ok(None)
    } else {
        affinity_process(pid, write).map(Some)
    }
}

fn finish(result: Result<c_int, i32>) -> c_int {
    result.unwrap_or_else(|error| {
        crate::set_errno(error);
        -1
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_getparam(
    pid: c_int,
    param: *mut SchedParam,
) -> c_int {
    finish((|| {
        if param.is_null() {
            return Err(EINVAL);
        }
        let _target = target(pid, false)?;
        super::mount_api::write_user(param as usize, &0i32.to_ne_bytes())?;
        Ok(0)
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_setparam(
    pid: c_int,
    param: *const SchedParam,
) -> c_int {
    unsafe { kinakaze_abi_sched_setscheduler(pid, libpthread::sched::SCHED_OTHER, param) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_getscheduler(pid: c_int) -> c_int {
    finish(target(pid, false).map(|_| libpthread::sched::SCHED_OTHER))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_setscheduler(
    pid: c_int,
    policy: c_int,
    param: *const SchedParam,
) -> c_int {
    finish((|| {
        if param.is_null() {
            return Err(EINVAL);
        }
        if pid < 0 {
            return Err(EINVAL);
        }
        let mut bytes = [0; 4];
        super::mount_api::read_user(param as usize, &mut bytes)?;
        let priority = c_int::from_ne_bytes(bytes);
        let _target = target(pid, false)?;
        libpthread::sched::validate(policy, priority)?;
        if !names_self(pid) {
            let _ = affinity_process(pid, true)?;
        }
        // Every admitted policy is OTHER/0, including after fork/exec. There
        // is no private "requested policy" that could disagree with the host.
        Ok(0)
    })())
}

fn priority_range(policy: c_int) -> Result<(c_int, c_int), i32> {
    match policy {
        0 | 3 | 5 | 6 => Ok((0, 0)), // OTHER, BATCH, IDLE, DEADLINE
        1 | 2 => Ok((1, 99)),        // FIFO, RR
        _ => Err(EINVAL),
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_get_priority_min(policy: c_int) -> c_int {
    finish(priority_range(policy).map(|range| range.0))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sched_get_priority_max(policy: c_int) -> c_int {
    finish(priority_range(policy).map(|range| range.1))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sched_rr_get_interval(
    pid: c_int,
    tp: *mut crate::time::TimeSpec,
) -> c_int {
    finish((|| {
        if tp.is_null() {
            return Err(EFAULT);
        }
        let _target = target(pid, false)?;
        // The host supplies no Linux round-robin quantum for OTHER threads.
        Err(kinakaze_vfs::EOPNOTSUPP)
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysadmin::*;

    #[test]
    fn scheduling_wrappers_and_raw_syscalls_agree_on_policy_and_errors() {
        let zero = SchedParam { sched_priority: 0 };
        let realtime = SchedParam { sched_priority: 1 };
        let raw = |number, policy, param: &SchedParam| unsafe {
            kinakaze_abi_syscall_raw(number, 0, policy, param as *const _ as u64, 0, 0, 0)
        };
        assert_eq!(unsafe { kinakaze_abi_sched_setscheduler(0, 0, &zero) }, 0);
        assert_eq!(raw(SYS_SCHED_SETSCHEDULER, 0, &zero), 0);
        assert_eq!(
            unsafe { kinakaze_abi_sched_setscheduler(0, 1, &realtime) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EPERM);
        assert_eq!(
            raw(SYS_SCHED_SETSCHEDULER, 1, &realtime),
            -i64::from(kinakaze_vfs::EPERM)
        );
        assert_eq!(raw(SYS_SCHED_SETSCHEDULER, 1, &zero), -i64::from(EINVAL));
        assert_eq!(kinakaze_abi_sched_getscheduler(-1), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        assert_eq!(kinakaze_abi_sched_getscheduler(0x7fff_fff0), -1);
        assert_eq!(kinakaze_tls::errno(), super::super::ESRCH);
        assert_eq!(
            unsafe { kinakaze_abi_sched_getparam(0, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        for (policy, minimum, maximum) in [
            (0, 0, 0),
            (1, 1, 99),
            (2, 1, 99),
            (3, 0, 0),
            (5, 0, 0),
            (6, 0, 0),
        ] {
            assert_eq!(kinakaze_abi_sched_get_priority_min(policy), minimum);
            assert_eq!(kinakaze_abi_sched_get_priority_max(policy), maximum);
        }
        assert_eq!(kinakaze_abi_sched_get_priority_max(4), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }
}
