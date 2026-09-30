//! Legacy sys_futex's timeout-copy stage, before do_futex dispatch validation.
//! Non-timed operations treat the fourth argument as a count or ignore it.

use super::*;
use std::time::{Duration, Instant};

pub(super) struct Prepared {
    pub duration: Option<Duration>,
    pub started: Instant,
}

pub(super) fn legacy(
    command: u32,
    flags: u32,
    timeout: *const KernelTimespec,
) -> Result<Option<Prepared>, i64> {
    if !matches!(
        command,
        FUTEX_WAIT | FUTEX_WAIT_BITSET | FUTEX_LOCK_PI | FUTEX_LOCK_PI2 | FUTEX_WAIT_REQUEUE_PI
    ) {
        return Ok(None);
    }
    // LOCK_PI's absolute clock is implicitly realtime. LOCK_PI2, WAIT_BITSET
    // and WAIT_REQUEUE_PI select it with the explicit flag; WAIT is relative.
    let duration = futex_timeout(
        timeout,
        command != FUTEX_WAIT,
        command == FUTEX_LOCK_PI || flags & FUTEX_CLOCK_REALTIME != 0,
    )?;
    Ok(Some(Prepared {
        duration,
        started: Instant::now(),
    }))
}

#[cfg(test)]
pub(super) mod tests;
