//! Playback time is derived from the device cursor and accepted sample bytes.
use super::{
    Ordering, PA_STREAM_READY, StreamNotify, StreamSuccess, callback_from_usize, frame_size_value,
    lock, operation_done, pa_operation, pa_stream, pa_stream_unref,
};
use core::{
    ffi::{c_int, c_void},
    ptr,
};

pub(super) const INVALID: c_int = 3;
const BAD_STATE: c_int = 15;
const NOT_SUPPORTED: c_int = 19;
pub(super) const IO: c_int = 25;

pub(super) unsafe fn fail(stream: *const pa_stream, error: c_int) -> c_int {
    if let Some(stream) = unsafe { stream.as_ref() }
        && let Some(context) = unsafe { stream.context.as_ref() }
    {
        context.error.store(error, Ordering::Release);
    }
    -error
}

unsafe fn sample(stream: *const pa_stream) -> Result<(u64, u64), c_int> {
    let stream = unsafe { stream.as_ref() }.ok_or(INVALID)?;
    if stream.state.load(Ordering::Acquire) != PA_STREAM_READY {
        return Err(BAD_STATE);
    }
    if !stream.playback.load(Ordering::Acquire) {
        // Recording must obtain a cursor from a real capture device first.
        return Err(NOT_SUPPORTED);
    }
    let (played, written) = lock(&stream.audio)
        .as_mut()
        .ok_or(BAD_STATE)?
        .position()
        .ok_or(IO)?;
    let bytes_per_second = u64::from(stream.spec.rate) * frame_size_value(stream.spec) as u64;
    let micros = |bytes: u64| (u128::from(bytes) * 1_000_000 / u128::from(bytes_per_second)) as u64;
    Ok((micros(played), micros(written - played)))
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_ref")]
pub unsafe extern "sysv64" fn pa_stream_ref(stream: *mut pa_stream) -> *mut pa_stream {
    if let Some(value) = unsafe { stream.as_ref() } {
        value.refs.fetch_add(1, Ordering::Relaxed);
    }
    stream
}

// The completion or latency callback may release the application's reference.
struct CallbackRef(*mut pa_stream);
impl Drop for CallbackRef {
    fn drop(&mut self) {
        unsafe { pa_stream_unref(self.0) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_update_timing_info")]
pub unsafe extern "sysv64" fn pa_stream_update_timing_info(
    stream: *mut pa_stream,
    callback: Option<StreamSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    let sync = unsafe { stream.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if let Err(error) = unsafe { sample(stream) } {
        unsafe { fail(stream, error) };
        return ptr::null_mut();
    }
    let _keep_alive = CallbackRef(unsafe { pa_stream_ref(stream) });
    let notify = unsafe { (*stream).latency_update_cb.load(Ordering::Acquire) };
    let argument = unsafe { (*stream).latency_update_userdata.load(Ordering::Acquire) };
    if let Some(notify) = unsafe { callback_from_usize::<StreamNotify>(notify) } {
        unsafe { notify(stream, argument as *mut c_void) };
    }
    if let Some(callback) = callback {
        unsafe { callback(stream, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_latency")]
pub unsafe extern "sysv64" fn pa_stream_get_latency(
    stream: *const pa_stream,
    latency: *mut u64,
    negative: *mut c_int,
) -> c_int {
    if latency.is_null() {
        return unsafe { fail(stream, INVALID) };
    }
    match unsafe { sample(stream) } {
        Ok((_, pending)) => {
            unsafe {
                latency.write(pending);
                if !negative.is_null() {
                    negative.write(0);
                }
            }
            0
        }
        Err(error) => unsafe { fail(stream, error) },
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_time")]
pub unsafe extern "sysv64" fn pa_stream_get_time(
    stream: *const pa_stream,
    time: *mut u64,
) -> c_int {
    if time.is_null() {
        return unsafe { fail(stream, INVALID) };
    }
    match unsafe { sample(stream) } {
        Ok((played, _)) => {
            unsafe { time.write(played) };
            0
        }
        Err(error) => unsafe { fail(stream, error) },
    }
}
