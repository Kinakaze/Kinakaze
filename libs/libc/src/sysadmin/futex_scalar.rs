//! Linux 6.12 futex2 single-address wait (raw 455).
//! Parse widths and the immutable absolute timeout before resolving a key.

use super::*;

pub(super) fn wait(
    address: usize,
    value: u64,
    mask: u64,
    flags: u32,
    timeout: *const KernelTimespec,
    clock: i32,
) -> i64 {
    if flags & !0x83 != 0
        || flags & 3 != 2
        || value > u64::from(u32::MAX)
        || mask > u64::from(u32::MAX)
    {
        return -i64::from(EINVAL);
    }
    loop {
        if !timeout.is_null() && !matches!(i64::from(clock), CLOCK_REALTIME | CLOCK_MONOTONIC) {
            return -i64::from(EINVAL);
        }
        let duration = match futex_timeout(timeout, true, i64::from(clock) == CLOCK_REALTIME) {
            Ok(duration) => duration,
            Err(error) => return error,
        };
        let started = std::time::Instant::now();
        #[cfg(test)]
        tests::pause_after_timeout_copy(address);
        // __futex_wait checks an empty mask after timeout setup, before key lookup.
        if mask == 0 {
            return -i64::from(EINVAL);
        }
        let address = match FutexAddress::resolve(address as _, flags & 0x80 != 0) {
            Ok(address) => address,
            Err(error) => return error,
        };
        match futex_wait_attempt(
            address,
            value as u32 as i32,
            duration,
            started,
            mask as u32,
            crate::futex::RestartPolicy::Reparse,
        ) {
            Ok(true) => return 0,
            Ok(false) => continue,
            Err(error) => return -i64::from(error),
        }
    }
}

#[cfg(test)]
mod tests;
