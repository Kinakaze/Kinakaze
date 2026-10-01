use super::*;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

#[test]
fn absolute_timer_preserves_source_indices_and_expires_without_notifications() {
    let event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
    assert!(!event.is_null());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128;
    let started = Instant::now();
    assert_eq!(
        unsafe { any_realtime(&[event], now + 20_000_000) },
        WAIT_TIMEOUT
    );
    assert!(started.elapsed() >= Duration::from_millis(15));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_ne!(unsafe { SetEvent(event) }, 0);
    assert_eq!(
        unsafe { any_realtime(&[event], now + 10_000_000_000) },
        WAIT_OBJECT_0
    );
    // A ready source wins over an expired timer, retaining wait-any ordering.
    assert_ne!(unsafe { SetEvent(event) }, 0);
    assert_eq!(unsafe { any_realtime(&[event], 0) }, WAIT_OBJECT_0);
    assert_eq!(unsafe { any_realtime(&[event], 0) }, WAIT_TIMEOUT);
    assert_ne!(unsafe { CloseHandle(event) }, 0);
}

#[test]
fn absolute_timer_epoch_rounding_and_invalid_wait_diagnostics() {
    assert_eq!(realtime_due(0), 116_444_736_000_000_000);
    assert_eq!(realtime_due(1), 116_444_736_000_000_001);
    assert_eq!(realtime_due(100), 116_444_736_000_000_001);
    assert_eq!(realtime_due(-1), 116_444_736_000_000_000);
    assert_eq!(realtime_due(i128::MAX), i64::MAX);
    assert_eq!(realtime_due(i128::MIN), 1);
    assert_eq!(
        unsafe { any_realtime(&[ptr::null_mut(); 64], 0) },
        WAIT_FAILED
    );
    assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
    assert_eq!(unsafe { any_realtime(&[ptr::null_mut()], 0) }, WAIT_FAILED);
    assert_ne!(unsafe { GetLastError() }, 0);
}
