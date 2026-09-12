//! Enforced process limits. State lives in runtime's shared process row so fork
//! inherits it and exec retains it without copying a module's private heap.
use crate::{EAGAIN, EINVAL, EIO, EOPNOTSUPP, EPERM};
pub const FSIZE: u32 = 1;
pub const NPROC: u32 = 6;

pub fn limits(pid: u32, resource: u32, value: Option<(u64, u64)>) -> Result<(u64, u64), i32> {
    crate::job::ensure_registered();
    let target = if pid == 0 {
        kinakaze_runtime::job::current_pid()
    } else {
        pid
    };
    let old =
        kinakaze_runtime::job::file_process_limits(target, resource, None).map_err(|_| EIO)?;
    if let Some((soft, hard)) = value {
        if soft > hard {
            return Err(EINVAL);
        }
        if hard > old.1 && !crate::user_namespace::current_capable(24) {
            return Err(EPERM);
        }
        // Zero is a complete creation prohibition for an unprivileged task.
        // Finite UID-wide quotas need atomic thread/process accounting; reject
        // them until that accounting exists instead of pretending to enforce it.
        if resource == NPROC && soft != 0 && soft != u64::MAX {
            return Err(EOPNOTSUPP);
        }
        kinakaze_runtime::job::file_process_limits(target, resource, Some((soft, hard)))
            .map_err(|_| EIO)?;
    }
    Ok(old)
}

pub fn task_creation_errno() -> i32 {
    if crate::credentials::real_uid() == 0
        || crate::user_namespace::current_capable(24)
        || crate::user_namespace::current_capable(21)
    {
        return 0;
    }
    match limits(0, NPROC, None) {
        Ok((0, _)) => EAGAIN,
        Ok(_) => 0,
        Err(error) => error,
    }
}

fn exceeded() -> i32 {
    let _ = crate::signal::raise_signal(25); // SIGXFSZ; delivered after VFS locks unwind.
    27 // EFBIG
}

pub fn write_length(offset: u64, length: usize) -> Result<usize, i32> {
    if length == 0 {
        return Ok(0);
    }
    let (limit, _) = limits(0, FSIZE, None)?;
    if offset >= limit {
        return Err(exceeded());
    }
    Ok(length.min((limit - offset).min(usize::MAX as u64) as usize))
}

pub fn truncate(length: u64) -> Result<(), i32> {
    if length > limits(0, FSIZE, None)?.0 {
        return Err(exceeded());
    }
    Ok(())
}
