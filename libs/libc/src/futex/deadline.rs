//! A copied futex deadline. Realtime remains an absolute wall-clock target;
//! relative and translated monotonic waits retain their original budget.
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::INFINITE;

#[derive(Clone, Copy)]
pub(crate) struct Deadline {
    pub duration: Option<Duration>,
    pub started: Instant,
    pub realtime: Option<i128>,
}

impl Deadline {
    #[cfg(test)]
    pub(crate) fn poll_optimized() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::env::var_os("KINAKAZE_FUTEX_REALTIME_POLL_OPT").is_none_or(|value| value != "0")
        })
    }

    #[cfg(not(test))]
    fn poll_optimized() -> bool {
        true
    }

    pub(crate) fn relative(duration: Option<Duration>, started: Instant) -> Self {
        Self {
            duration,
            started,
            realtime: None,
        }
    }

    pub(crate) fn remaining(&self) -> Option<Duration> {
        if let Some(target) = self.realtime {
            // CLOCK_REALTIME is the precise host FILETIME and cannot fail.
            let (seconds, nanos) = crate::time::read_clock(0).unwrap();
            return Some(Self::wall_remaining(target, seconds, nanos));
        }
        self.duration
            .map(|limit| limit.saturating_sub(self.started.elapsed()))
    }

    fn wall_remaining(target: i128, seconds: i64, nanos: i64) -> Duration {
        let current = i128::from(seconds) * 1_000_000_000 + i128::from(nanos);
        let left = target.saturating_sub(current).max(0) as u128;
        Duration::new((left / 1_000_000_000) as u64, (left % 1_000_000_000) as u32)
    }

    pub(crate) fn expired(&self) -> bool {
        self.remaining().is_some_and(|left| left.is_zero())
    }

    pub(crate) fn milliseconds(&self) -> u32 {
        self.remaining().map_or(INFINITE, |left| {
            left.as_nanos()
                .div_ceil(1_000_000)
                .min(u128::from(INFINITE - 1)) as u32
        })
    }

    /// # Safety
    /// The handles must remain valid throughout the native wait.
    pub(crate) unsafe fn wait(&self, handles: &[HANDLE]) -> u32 {
        if let Some(target) = self.realtime {
            // An expired deadline needs only a wait-any poll. This keeps a
            // ready wake/signal ahead of timeout without allocating a timer.
            // A backward clock step is resolved by the caller's next expiry
            // check, using the original absolute target.
            if Self::poll_optimized() && self.expired() {
                return unsafe { kinakaze_vfs::deadline_wait::any(handles, 0) };
            }
            unsafe { kinakaze_vfs::deadline_wait::any_realtime(handles, target) }
        } else {
            unsafe { kinakaze_vfs::deadline_wait::any(handles, self.milliseconds()) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_deadline_tracks_forward_and_backward_clock_steps() {
        let target = 100 * 1_000_000_000;
        assert_eq!(
            Deadline::wall_remaining(target, 90, 0),
            Duration::from_secs(10)
        );
        assert_eq!(Deadline::wall_remaining(target, 110, 0), Duration::ZERO);
        assert_eq!(
            Deadline::wall_remaining(target, 80, 0),
            Duration::from_secs(20)
        );
        assert_eq!(
            Deadline::wall_remaining(target, 99, 999_999_999),
            Duration::from_nanos(1)
        );
    }

    #[test]
    fn expired_realtime_poll_preserves_ready_source_order_and_handle_count() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            CreateEventW, GetCurrentProcess, GetProcessHandleCount, SetEvent,
        };
        let event = unsafe { CreateEventW(core::ptr::null(), 0, 0, core::ptr::null()) };
        assert!(!event.is_null());
        let deadline = Deadline {
            duration: Some(Duration::ZERO),
            started: Instant::now(),
            realtime: Some(0),
        };
        assert_ne!(unsafe { SetEvent(event) }, 0);
        assert_eq!(unsafe { deadline.wait(&[event]) }, WAIT_OBJECT_0);
        let mut before = 0;
        assert_ne!(
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut before) },
            0
        );
        for _ in 0..128 {
            assert_eq!(unsafe { deadline.wait(&[event]) }, WAIT_TIMEOUT);
        }
        let mut after = 0;
        assert_ne!(
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut after) },
            0
        );
        assert_eq!(after, before);
        assert_ne!(unsafe { CloseHandle(event) }, 0);
    }
}
