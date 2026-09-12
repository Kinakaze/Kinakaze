//! Output objects own guest FILE references; no host CRT FILE crosses the ABI.
use super::*;
use libc::stdio::{self, File};

#[repr(C)]
pub struct Output {
    file: *mut File,
    close: bool,
    buffer: *mut u8,
    length: usize,
    capacity: usize,
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_stdio_attach")]
pub unsafe extern "sysv64" fn snd_output_stdio_attach(
    output: *mut *mut Output,
    file: *mut File,
    close: c_int,
) -> c_int {
    if output.is_null() || file.is_null() {
        return -22;
    }
    unsafe {
        params::allocate_params(
            output,
            Output {
                file,
                close: close != 0,
                buffer: core::ptr::null_mut(),
                length: 0,
                capacity: 0,
            },
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_stdio_open")]
pub unsafe extern "sysv64" fn snd_output_stdio_open(
    output: *mut *mut Output,
    path: *const c_char,
    mode: *const c_char,
) -> c_int {
    if output.is_null() || path.is_null() || mode.is_null() {
        return -22;
    }
    unsafe {
        *output = core::ptr::null_mut();
        let file = stdio::fopen(path, mode);
        if file.is_null() {
            return -5;
        }
        let result = snd_output_stdio_attach(output, file, 1);
        if result < 0 {
            stdio::fclose(file);
        }
        result
    }
}

pub(super) unsafe fn write(output: *mut Output, bytes: &[u8]) -> c_int {
    if output.is_null() {
        return -22;
    }
    if unsafe { (*output).file.is_null() } {
        let output = unsafe { &mut *output };
        let Some(required) = output
            .length
            .checked_add(bytes.len())
            .and_then(|n| n.checked_add(1))
        else {
            return -12;
        };
        if required > output.capacity {
            let capacity = required.max(output.capacity.saturating_mul(2)).max(256);
            let buffer = unsafe { kinakaze_alloc::guest::malloc(capacity) };
            if buffer.is_null() {
                return -12;
            }
            if output.length != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(output.buffer, buffer, output.length);
                }
            }
            unsafe {
                kinakaze_alloc::guest::free(output.buffer);
            }
            output.buffer = buffer;
            output.capacity = capacity;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                output.buffer.add(output.length),
                bytes.len(),
            );
            output.length += bytes.len();
            *output.buffer.add(output.length) = 0;
        }
        return 0;
    }
    if unsafe { stdio::fwrite(bytes.as_ptr().cast(), 1, bytes.len(), (*output).file) }
        == bytes.len()
    {
        0
    } else {
        -5
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_puts")]
pub unsafe extern "sysv64" fn snd_output_puts(output: *mut Output, text: *const c_char) -> c_int {
    if text.is_null() {
        return -22;
    }
    unsafe { write(output, core::ffi::CStr::from_ptr(text).to_bytes()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_putc")]
pub unsafe extern "sysv64" fn snd_output_putc(output: *mut Output, byte: c_int) -> c_int {
    let result = unsafe { write(output, &[byte as u8]) };
    if result < 0 {
        result
    } else {
        byte as u8 as c_int
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_flush")]
pub unsafe extern "sysv64" fn snd_output_flush(output: *mut Output) -> c_int {
    if output.is_null() {
        return -22;
    }
    if unsafe { (*output).file.is_null() } {
        unsafe {
            (*output).length = 0;
            if !(*output).buffer.is_null() {
                *(*output).buffer = 0;
            }
        }
        return 0;
    }
    if unsafe { stdio::fflush((*output).file) } == 0 {
        0
    } else {
        -5
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_close")]
pub unsafe extern "sysv64" fn snd_output_close(output: *mut Output) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe {
        let result = if (*output).close {
            stdio::fclose((*output).file)
        } else {
            0
        };
        kinakaze_alloc::guest::free((*output).buffer);
        kinakaze_alloc::guest::free(output.cast());
        if result == 0 { 0 } else { -5 }
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_buffer_open")]
pub unsafe extern "sysv64" fn snd_output_buffer_open(output: *mut *mut Output) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe {
        params::allocate_params(
            output,
            Output {
                file: core::ptr::null_mut(),
                close: false,
                buffer: core::ptr::null_mut(),
                length: 0,
                capacity: 0,
            },
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_output_buffer_string")]
pub unsafe extern "sysv64" fn snd_output_buffer_string(
    output: *mut Output,
    buffer: *mut *mut c_char,
) -> usize {
    if output.is_null() || buffer.is_null() {
        return 0;
    }
    unsafe {
        *buffer = (*output).buffer.cast();
        (*output).length
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_dump")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_dump(
    params: *const snd_pcm_hw_params_t,
    output: *mut Output,
) -> c_int {
    let Some(p) = (unsafe { params.as_ref() }) else {
        return -22;
    };
    let text = format!(
        "ACCESS: {}\nFORMAT: {}\nCHANNELS: {}\nRATE: {}\nPERIOD_SIZE: {}\nBUFFER_SIZE: {}\n",
        p.access, p.format, p.channels, p.rate, p.period_size, p.buffer_size
    );
    unsafe { write(output, text.as_bytes()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_dump")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_dump(
    params: *const snd_pcm_sw_params_t,
    output: *mut Output,
) -> c_int {
    let Some(p) = (unsafe { params.as_ref() }) else {
        return -22;
    };
    let text = format!(
        "avail_min: {}\nstart_threshold: {}\nstop_threshold: {}\nboundary: {}\n",
        p.avail_min, p.start_threshold, p.stop_threshold, p.boundary
    );
    unsafe { write(output, text.as_bytes()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_dump")]
pub unsafe extern "sysv64" fn snd_pcm_dump(pcm: *mut SndPcm, output: *mut Output) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let pcm = unsafe { &*pcm };
    let p = &pcm.hw_params;
    let description = format!(
        "Windows PCM: {}\nStream: {}\nState: {}\nFormat: {}\nChannels: {}\nRate: {}\nPeriod size: {}\nBuffer size: {}\n",
        pcm.name.to_string_lossy(),
        pcm.stream,
        pcm.state.load(Ordering::Acquire),
        p.format,
        p.channels,
        p.rate,
        p.period_size,
        p.buffer_size
    );
    unsafe { write(output, description.as_bytes()) }
}
