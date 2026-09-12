//! `libasound.dll`: ALSA (Advanced Linux Sound Architecture) compatibility library.
//!
//! Provides the standard `libasound.so.2` ABI for Linux guest applications,
//! bridging PCM audio playback and recording onto native Windows audio.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveOutClose, waveOutPause, waveOutPrepareHeader, waveOutReset, waveOutRestart,
    waveOutUnprepareHeader, waveOutWrite,
};

// Linux x86-64 C long is 64-bit even when the host compiler targets Windows.
type c_long = i64;
type c_ulong = u64;
mod channels;
mod control;
mod formats;
mod midi_event;
mod mixer;
mod output;
mod params;
mod polling;
mod rawmidi;
mod seq;
mod status;

pub const SND_PCM_STREAM_PLAYBACK: c_int = 0;
pub const SND_PCM_STREAM_CAPTURE: c_int = 1;

pub const SND_PCM_ACCESS_MMAP_INTERLEAVED: c_int = 0;
pub const SND_PCM_ACCESS_MMAP_NONINTERLEAVED: c_int = 1;
pub const SND_PCM_ACCESS_MMAP_COMPLEX: c_int = 2;
pub const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
pub const SND_PCM_ACCESS_RW_NONINTERLEAVED: c_int = 4;

pub const SND_PCM_FORMAT_UNKNOWN: c_int = -1;
pub const SND_PCM_FORMAT_S8: c_int = 0;
pub const SND_PCM_FORMAT_U8: c_int = 1;
pub const SND_PCM_FORMAT_S16_LE: c_int = 2;
pub const SND_PCM_FORMAT_S16_BE: c_int = 3;
pub const SND_PCM_FORMAT_U16_LE: c_int = 4;
pub const SND_PCM_FORMAT_U16_BE: c_int = 5;
pub const SND_PCM_FORMAT_S24_LE: c_int = 6;
pub const SND_PCM_FORMAT_S32_LE: c_int = 10;
pub const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
pub const SND_PCM_FORMAT_FLOAT_BE: c_int = 15;

pub const SND_PCM_STATE_OPEN: c_int = 0;
pub const SND_PCM_STATE_SETUP: c_int = 1;
pub const SND_PCM_STATE_PREPARED: c_int = 2;
pub const SND_PCM_STATE_RUNNING: c_int = 3;
pub const SND_PCM_STATE_XRUN: c_int = 4;
pub const SND_PCM_STATE_DRAINING: c_int = 5;
pub const SND_PCM_STATE_PAUSED: c_int = 6;
pub const SND_PCM_STATE_SUSPENDED: c_int = 7;
pub const SND_PCM_STATE_DISCONNECTED: c_int = 8;

const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const MMSYSERR_NOERROR: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct snd_pcm_hw_params_t {
    pub access: c_int,
    pub format: c_int,
    pub subformat: c_int,
    pub channels: c_uint,
    pub rate: c_uint,
    pub period_time: c_uint,
    pub period_size: c_ulong,
    pub periods: c_uint,
    pub buffer_time: c_uint,
    pub buffer_size: c_ulong,
    channels_min: c_uint,
    channels_max: c_uint,
    format_mask: u64,
    periods_min: c_uint,
    periods_max: c_uint,
}

impl Default for snd_pcm_hw_params_t {
    fn default() -> Self {
        Self {
            access: SND_PCM_ACCESS_RW_INTERLEAVED,
            format: SND_PCM_FORMAT_S16_LE,
            subformat: 0,
            channels: 2,
            rate: 44100,
            period_time: 20000,
            period_size: 1024,
            periods: 4,
            buffer_time: 80000,
            buffer_size: 4096,
            channels_min: 1,
            channels_max: 8,
            format_mask: [0, 1, 2, 6, 10, 14, 32]
                .into_iter()
                .fold(0, |m, f| m | (1u64 << f)),
            periods_min: 2,
            periods_max: 16,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct snd_pcm_sw_params_t {
    pub tstamp_mode: c_int,
    pub period_step: c_uint,
    pub sleep_min: c_uint,
    pub avail_min: c_ulong,
    pub xfer_align: c_ulong,
    pub start_threshold: c_ulong,
    pub stop_threshold: c_ulong,
    pub silence_threshold: c_ulong,
    pub silence_size: c_ulong,
    pub boundary: c_ulong,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct snd_pcm_info_t {
    device: c_uint,
    subdevice: c_uint,
    stream: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct snd_ctl_card_info_t {
    card: c_int,
}

pub struct SndPcm {
    pub name: std::ffi::CString,
    pub stream: c_int,
    pub mode: c_int,
    pub state: AtomicI32,
    pub hw_params: snd_pcm_hw_params_t,
    pub sw_params: snd_pcm_sw_params_t,
    audio: Mutex<Option<WaveOutState>>,
    poll_fd: i32,
}

impl Drop for SndPcm {
    fn drop(&mut self) {
        // Stop native callbacks before releasing their notification descriptor.
        *self.audio.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = kinakaze_vfs::close(self.poll_fd);
    }
}

struct WaveBuffer {
    frames: u64,
    _data: Box<[u8]>,
    header: Box<WAVEHDR>,
}

struct WaveOutState {
    handle: HWAVEOUT,
    buffers: Vec<WaveBuffer>,
}

impl WaveOutState {
    fn open(params: &snd_pcm_hw_params_t, poll_fd: i32) -> Option<Self> {
        let (tag, bits) = match params.format {
            SND_PCM_FORMAT_U8 | SND_PCM_FORMAT_S8 => (WAVE_FORMAT_PCM as u16, 8u16),
            SND_PCM_FORMAT_S16_LE => (WAVE_FORMAT_PCM as u16, 16u16),
            SND_PCM_FORMAT_S24_LE | 32 => (WAVE_FORMAT_PCM as u16, 24u16),
            SND_PCM_FORMAT_S32_LE => (WAVE_FORMAT_PCM as u16, 32u16),
            SND_PCM_FORMAT_FLOAT_LE => (WAVE_FORMAT_IEEE_FLOAT, 32u16),
            _ => return None,
        };
        let channels = u16::try_from(params.channels).ok()?.clamp(1, 8);
        let block_align = channels.checked_mul(bits / 8)?;
        let format = WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: channels,
            nSamplesPerSec: params.rate,
            nAvgBytesPerSec: params.rate.checked_mul(u32::from(block_align))?,
            nBlockAlign: block_align,
            wBitsPerSample: bits,
            cbSize: 0,
        };
        let mut handle: HWAVEOUT = core::ptr::null_mut();
        let result = unsafe {
            windows_sys::Win32::Media::Audio::waveOutOpen(
                &raw mut handle,
                WAVE_MAPPER,
                &raw const format,
                polling::completed as *const () as usize,
                poll_fd as usize,
                windows_sys::Win32::Media::Audio::CALLBACK_FUNCTION,
            )
        };
        (result == MMSYSERR_NOERROR && !handle.is_null()).then_some(Self {
            handle,
            buffers: Vec::new(),
        })
    }

    fn buffer_done(buffer: &WaveBuffer) -> bool {
        let flags = unsafe { core::ptr::addr_of!((*buffer.header).dwFlags).read_unaligned() };
        flags & WHDR_DONE != 0
    }

    fn reap(&mut self) {
        let mut index = 0;
        while index < self.buffers.len() {
            if Self::buffer_done(&self.buffers[index]) {
                let buffer = &mut self.buffers[index];
                unsafe {
                    waveOutUnprepareHeader(
                        self.handle,
                        &raw mut *buffer.header,
                        core::mem::size_of::<WAVEHDR>() as u32,
                    )
                };
                self.buffers.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn queue(&mut self, bytes: &[u8], params: &snd_pcm_hw_params_t) -> bool {
        self.reap();
        let frames = (bytes.len() / bytes_per_frame(params)) as u64;
        let mut data = match params.format {
            SND_PCM_FORMAT_S8 => bytes.iter().map(|b| b ^ 0x80).collect::<Vec<_>>(),
            SND_PCM_FORMAT_S24_LE => bytes
                .chunks_exact(4)
                .flat_map(|s| s[..3].iter().copied())
                .collect::<Vec<_>>(),
            _ => bytes.to_vec(),
        }
        .into_boxed_slice();
        let mut header = Box::new(WAVEHDR::default());
        header.lpData = data.as_mut_ptr();
        header.dwBufferLength = u32::try_from(data.len()).unwrap_or(u32::MAX);
        let prepared = unsafe {
            waveOutPrepareHeader(
                self.handle,
                &raw mut *header,
                core::mem::size_of::<WAVEHDR>() as u32,
            )
        };
        if prepared != MMSYSERR_NOERROR {
            return false;
        }
        if unsafe {
            waveOutWrite(
                self.handle,
                &raw mut *header,
                core::mem::size_of::<WAVEHDR>() as u32,
            )
        } != MMSYSERR_NOERROR
        {
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &raw mut *header,
                    core::mem::size_of::<WAVEHDR>() as u32,
                )
            };
            return false;
        }
        self.buffers.push(WaveBuffer {
            frames,
            _data: data,
            header,
        });
        true
    }

    fn reset(&mut self) {
        unsafe { waveOutReset(self.handle) };
        for buffer in &mut self.buffers {
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &raw mut *buffer.header,
                    core::mem::size_of::<WAVEHDR>() as u32,
                )
            };
        }
        self.buffers.clear();
    }
}

impl Drop for WaveOutState {
    fn drop(&mut self) {
        self.reset();
        unsafe { waveOutClose(self.handle) };
    }
}

fn bytes_per_frame(params: &snd_pcm_hw_params_t) -> usize {
    let bytes = match params.format {
        SND_PCM_FORMAT_U8 | SND_PCM_FORMAT_S8 => 1,
        SND_PCM_FORMAT_S16_LE => 2,
        SND_PCM_FORMAT_S24_LE => 4,
        32 => 3,
        SND_PCM_FORMAT_S32_LE | SND_PCM_FORMAT_FLOAT_LE => 4,
        _ => 0,
    };
    bytes * params.channels as usize
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_open")]
pub unsafe extern "sysv64" fn snd_pcm_open(
    pcmp: *mut *mut SndPcm,
    name: *const c_char,
    stream: c_int,
    mode: c_int,
) -> c_int {
    if pcmp.is_null() {
        return -22; // -EINVAL
    }
    unsafe {
        *pcmp = core::ptr::null_mut();
    }
    if stream != SND_PCM_STREAM_PLAYBACK {
        return if stream == SND_PCM_STREAM_CAPTURE {
            -38
        } else {
            -22
        };
    }
    let dev_name = if !name.is_null() {
        unsafe { std::ffi::CStr::from_ptr(name) }.to_owned()
    } else {
        c"default".to_owned()
    };
    let poll_fd = match kinakaze_vfs::eventfd::create_eventfd(1, 0x80800) {
        Ok(fd) => fd,
        Err(error) => return -error,
    };
    let pcm = Box::new(SndPcm {
        name: dev_name,
        stream,
        mode,
        state: AtomicI32::new(SND_PCM_STATE_OPEN),
        hw_params: snd_pcm_hw_params_t::default(),
        sw_params: snd_pcm_sw_params_t::default(),
        audio: Mutex::new(None),
        poll_fd,
    });
    unsafe { *pcmp = Box::into_raw(pcm) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_name")]
pub unsafe extern "sysv64" fn snd_pcm_name(pcm: *mut SndPcm) -> *const c_char {
    if pcm.is_null() {
        core::ptr::null()
    } else {
        unsafe { (*pcm).name.as_ptr() }
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_get_params")]
pub unsafe extern "sysv64" fn snd_pcm_get_params(
    pcm: *mut SndPcm,
    buffer: *mut u64,
    period: *mut u64,
) -> c_int {
    if pcm.is_null() || buffer.is_null() || period.is_null() {
        return -22;
    }
    if unsafe { (*pcm).state.load(Ordering::Acquire) } == SND_PCM_STATE_OPEN {
        return -77;
    }
    unsafe {
        *buffer = (*pcm).hw_params.buffer_size;
        *period = (*pcm).hw_params.period_size;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_can_resume")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_can_resume(
    params: *const snd_pcm_hw_params_t,
) -> c_int {
    i32::from(!params.is_null()) // waveOutRestart resumes the retained queue
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_close")]
pub unsafe extern "sysv64" fn snd_pcm_close(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe { drop(Box::from_raw(pcm)) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_sizeof")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_sizeof() -> usize {
    core::mem::size_of::<snd_pcm_hw_params_t>()
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_malloc")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_malloc(
    ptr: *mut *mut snd_pcm_hw_params_t,
) -> c_int {
    if ptr.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(ptr, snd_pcm_hw_params_t::default()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_free")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_free(ptr: *mut snd_pcm_hw_params_t) {
    unsafe {
        kinakaze_alloc::guest::free(ptr.cast());
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_any")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_any(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    unsafe { *params = snd_pcm_hw_params_t::default() };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_access")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_access(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    access: c_int,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    // The waveOut bridge exposes one interleaved host buffer. Rejecting mmap and
    // non-interleaved modes makes ALSA clients select their ordinary writei path.
    if access != SND_PCM_ACCESS_RW_INTERLEAVED {
        return -22;
    }
    unsafe { (*params).access = access };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_format")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_format(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    format: c_int,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    if !formats::supported(format) || unsafe { (*params).format_mask } & (1u64 << format) == 0 {
        return -22;
    }
    unsafe {
        (*params).format = format;
        (*params).format_mask = 1u64 << format;
    };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_channels")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_channels(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    val: c_uint,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    if !(unsafe { (*params).channels_min }..=unsafe { (*params).channels_max }).contains(&val) {
        return -22;
    }
    unsafe {
        (*params).channels = val;
        (*params).channels_min = val;
        (*params).channels_max = val;
    };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_rate_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_rate_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    val: *mut c_uint,
    _dir: *mut c_int,
) -> c_int {
    if params.is_null() || val.is_null() {
        return -22;
    }
    let rate = unsafe { *val }.clamp(8000, 192000);
    unsafe {
        (*params).rate = rate;
        *val = rate;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_buffer_size_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_buffer_size_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    val: *mut c_ulong,
) -> c_int {
    if params.is_null() || val.is_null() {
        return -22;
    }
    let size = unsafe { *val }.clamp(256, 65536);
    unsafe {
        (*params).buffer_size = size;
        *val = size;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_period_size_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_period_size_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    val: *mut c_ulong,
    _dir: *mut c_int,
) -> c_int {
    if params.is_null() || val.is_null() {
        return -22;
    }
    let size = unsafe { *val }.clamp(64, 16384);
    unsafe {
        (*params).period_size = size;
        *val = size;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_current")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_current(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
) -> c_int {
    if pcm.is_null() || params.is_null() {
        return -22;
    }
    unsafe { *params = (*pcm).hw_params };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_access")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_access(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe { *value = (*params).access };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_buffer_size")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_buffer_size(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_ulong,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe { *value = (*params).buffer_size };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_channels")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_channels(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_uint,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe { *value = (*params).channels };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_period_size")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_period_size(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_ulong,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = (*params).period_size;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_periods")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_periods(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = (*params).periods;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

unsafe fn write_time_range(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
    period: bool,
    maximum: bool,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    let configured = if period {
        unsafe { (*params).period_time }
    } else {
        unsafe { (*params).buffer_time }
    };
    unsafe {
        *value = if maximum {
            configured.max(if period { 1_000_000 } else { 4_000_000 })
        } else {
            configured.min(1_000)
        };
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

macro_rules! time_getter {
    ($name:ident, $period:expr, $maximum:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libasound_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            params: *const snd_pcm_hw_params_t,
            value: *mut c_uint,
            direction: *mut c_int,
        ) -> c_int {
            unsafe { write_time_range(params, value, direction, $period, $maximum) }
        }
    };
}

time_getter!(snd_pcm_hw_params_get_buffer_time_min, false, false);
time_getter!(snd_pcm_hw_params_get_buffer_time_max, false, true);
time_getter!(snd_pcm_hw_params_get_period_time_min, true, false);
time_getter!(snd_pcm_hw_params_get_period_time_max, true, true);

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_buffer_size_min")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_buffer_size_min(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_ulong,
) -> c_int {
    unsafe { snd_pcm_hw_params_set_buffer_size_near(pcm, params, value) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_buffer_time_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_buffer_time_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    let requested = unsafe { *value }.clamp(1_000, 4_000_000);
    unsafe {
        (*params).buffer_time = requested;
        (*params).buffer_size = (u64::from((*params).rate) * u64::from(requested) / 1_000_000)
            .clamp(256, 65_536) as c_ulong;
        *value = requested;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_channels_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_channels_near(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
) -> c_int {
    if value.is_null() {
        return -22;
    }
    let channels = unsafe { *value }.clamp(1, 8);
    unsafe { *value = channels };
    unsafe { snd_pcm_hw_params_set_channels(pcm, params, channels) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_period_time_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_period_time_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    let requested = unsafe { *value }.clamp(500, 1_000_000);
    unsafe {
        (*params).period_time = requested;
        (*params).period_size = (u64::from((*params).rate) * u64::from(requested) / 1_000_000)
            .clamp(64, 16_384) as c_ulong;
        *value = requested;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_periods_near")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_periods_near(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    let periods = unsafe { *value }.clamp(unsafe { (*params).periods_min }, unsafe {
        (*params).periods_max
    });
    unsafe {
        (*params).periods = periods;
        (*params).periods_min = periods;
        (*params).periods_max = periods;
        *value = periods;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_rate")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_rate(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    rate: c_uint,
    _direction: c_int,
) -> c_int {
    if params.is_null() || !(8_000..=192_000).contains(&rate) {
        return -22;
    }
    unsafe { (*params).rate = rate };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_rate_resample")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_rate_resample(
    _pcm: *mut SndPcm,
    _params: *mut snd_pcm_hw_params_t,
    _enabled: c_uint,
) -> c_int {
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_test_channels")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_test_channels(
    _pcm: *mut SndPcm,
    _params: *mut snd_pcm_hw_params_t,
    channels: c_uint,
) -> c_int {
    if (1..=8).contains(&channels) { 0 } else { -22 }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_test_format")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_test_format(
    _pcm: *mut SndPcm,
    _params: *mut snd_pcm_hw_params_t,
    format: c_int,
) -> c_int {
    if _params.is_null() || !formats::supported(format) {
        return -22;
    }
    if unsafe { (*_params).format_mask } & (1u64 << format) != 0 {
        0
    } else {
        -22
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
) -> c_int {
    if pcm.is_null() || params.is_null() {
        return -22;
    }
    unsafe {
        (*pcm).hw_params = *params;
        (*pcm).state.store(SND_PCM_STATE_SETUP, Ordering::Release);
    }
    if unsafe { (*pcm).stream } == SND_PCM_STREAM_PLAYBACK {
        let Some(output) =
            WaveOutState::open(unsafe { &(*pcm).hw_params }, unsafe { (*pcm).poll_fd })
        else {
            return -19; // -ENODEV
        };
        let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
        *audio = Some(output);
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_prepare")]
pub unsafe extern "sysv64" fn snd_pcm_prepare(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe {
        (*pcm)
            .state
            .store(SND_PCM_STATE_PREPARED, Ordering::Release)
    };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_start")]
pub unsafe extern "sysv64" fn snd_pcm_start(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe { (*pcm).state.store(SND_PCM_STATE_RUNNING, Ordering::Release) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_drop")]
pub unsafe extern "sysv64" fn snd_pcm_drop(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe { (*pcm).state.store(SND_PCM_STATE_SETUP, Ordering::Release) };
    let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    if let Some(output) = audio.as_mut() {
        output.reset();
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_drain")]
pub unsafe extern "sysv64" fn snd_pcm_drain(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe {
        (*pcm)
            .state
            .store(SND_PCM_STATE_DRAINING, Ordering::Release)
    };
    loop {
        polling::clear(unsafe { (*pcm).poll_fd });
        let empty = {
            let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
            if let Some(output) = audio.as_mut() {
                if unsafe { waveOutRestart(output.handle) } != MMSYSERR_NOERROR {
                    return -5;
                }
                output.reap();
                output.buffers.is_empty()
            } else {
                true
            }
        };
        if empty {
            unsafe {
                (*pcm).state.store(SND_PCM_STATE_SETUP, Ordering::Release);
            }
            polling::signal(unsafe { (*pcm).poll_fd });
            return 0;
        }
        if unsafe { (*pcm).mode } & 1 != 0 {
            return -11;
        }
        let mut fd = libc::fdio::PollFd {
            fd: unsafe { (*pcm).poll_fd },
            events: 1,
            revents: 0,
        };
        if unsafe { libc::fdio::kinakaze_abi_poll(&raw mut fd, 1, -1) } < 0 {
            return -kinakaze_tls::errno();
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_pause")]
pub unsafe extern "sysv64" fn snd_pcm_pause(pcm: *mut SndPcm, enable: c_int) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let new_state = if enable != 0 {
        SND_PCM_STATE_PAUSED
    } else {
        SND_PCM_STATE_RUNNING
    };
    let audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    if let Some(output) = audio.as_ref() {
        unsafe {
            if enable != 0 {
                waveOutPause(output.handle)
            } else {
                waveOutRestart(output.handle)
            }
        };
    }
    unsafe { (*pcm).state.store(new_state, Ordering::Release) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_writei")]
pub unsafe extern "sysv64" fn snd_pcm_writei(
    pcm: *mut SndPcm,
    buffer: *const c_void,
    size: c_ulong,
) -> c_long {
    if pcm.is_null() || buffer.is_null() {
        return -22;
    }
    let frame_bytes = bytes_per_frame(unsafe { &(*pcm).hw_params });
    let Some(byte_count) = (size as usize).checked_mul(frame_bytes) else {
        return -22;
    };
    if frame_bytes == 0 || byte_count > u32::MAX as usize {
        return -22;
    }
    loop {
        let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
        if audio.is_none() {
            *audio = WaveOutState::open(unsafe { &(*pcm).hw_params }, unsafe { (*pcm).poll_fd });
        }
        let Some(output) = audio.as_mut() else {
            return -19;
        };
        polling::clear(unsafe { (*pcm).poll_fd });
        output.reap();
        let queued = output.buffers.iter().map(|b| b.frames).sum::<u64>();
        let available = unsafe { (*pcm).hw_params.buffer_size }.saturating_sub(queued);
        if available == 0 {
            drop(audio);
            if unsafe { (*pcm).mode } & 1 != 0 {
                return -11;
            }
            let ready = unsafe { polling::snd_pcm_wait(pcm, -1) };
            if ready < 0 {
                return ready as c_long;
            }
            continue;
        }
        let accepted = size.min(available);
        let bytes = unsafe {
            core::slice::from_raw_parts(buffer.cast::<u8>(), accepted as usize * frame_bytes)
        };
        if !output.queue(bytes, unsafe { &(*pcm).hw_params }) {
            return -5;
        }
        if available - accepted >= unsafe { (*pcm).sw_params.avail_min.max(1) } {
            polling::signal(unsafe { (*pcm).poll_fd });
        }
        unsafe { (*pcm).state.store(SND_PCM_STATE_RUNNING, Ordering::Release) };
        return accepted as c_long;
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_readi")]
pub unsafe extern "sysv64" fn snd_pcm_readi(
    pcm: *mut SndPcm,
    buffer: *mut c_void,
    size: c_ulong,
) -> c_long {
    if pcm.is_null() || buffer.is_null() {
        return -22;
    }
    let _ = size;
    -38 // Capture is unavailable; never fabricate silent input.
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_avail_update")]
pub unsafe extern "sysv64" fn snd_pcm_avail_update(pcm: *mut SndPcm) -> c_long {
    if pcm.is_null() {
        return -22;
    }
    let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    let queued = if let Some(output) = audio.as_mut() {
        output.reap();
        output.buffers.iter().map(|b| b.frames).sum::<u64>()
    } else {
        0
    };
    unsafe { (*pcm).hw_params.buffer_size.saturating_sub(queued) as c_long }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_delay")]
pub unsafe extern "sysv64" fn snd_pcm_delay(pcm: *mut SndPcm, delayp: *mut c_long) -> c_int {
    if pcm.is_null() || delayp.is_null() {
        return -22;
    }
    let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    let frames = if let Some(output) = audio.as_mut() {
        output.reap();
        output
            .buffers
            .iter()
            .map(|buffer| buffer.frames)
            .sum::<u64>()
    } else {
        0
    };
    unsafe { *delayp = frames as c_long };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_state")]
pub unsafe extern "sysv64" fn snd_pcm_state(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return SND_PCM_STATE_DISCONNECTED;
    }
    unsafe { (*pcm).state.load(Ordering::Acquire) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_frames_to_bytes")]
pub unsafe extern "sysv64" fn snd_pcm_frames_to_bytes(pcm: *mut SndPcm, frames: c_long) -> c_long {
    if pcm.is_null() || frames < 0 {
        return -22;
    }
    frames.saturating_mul(bytes_per_frame(unsafe { &(*pcm).hw_params }) as c_long)
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_bytes_to_frames")]
pub unsafe extern "sysv64" fn snd_pcm_bytes_to_frames(pcm: *mut SndPcm, bytes: c_long) -> c_long {
    if pcm.is_null() || bytes < 0 {
        return -22;
    }
    let frame_bytes = bytes_per_frame(unsafe { &(*pcm).hw_params });
    if frame_bytes == 0 {
        -22
    } else {
        bytes / frame_bytes as c_long
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_nonblock")]
pub unsafe extern "sysv64" fn snd_pcm_nonblock(pcm: *mut SndPcm, nonblock: c_int) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    unsafe {
        (*pcm).mode = ((*pcm).mode & !1) | i32::from(nonblock != 0);
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_recover")]
pub unsafe extern "sysv64" fn snd_pcm_recover(
    pcm: *mut SndPcm,
    _error: c_int,
    _silent: c_int,
) -> c_int {
    unsafe { snd_pcm_prepare(pcm) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_reset")]
pub unsafe extern "sysv64" fn snd_pcm_reset(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    if let Some(output) = audio.as_mut() {
        output.reset();
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_resume")]
pub unsafe extern "sysv64" fn snd_pcm_resume(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    if let Some(output) = audio.as_ref() {
        unsafe { waveOutRestart(output.handle) };
    }
    unsafe { (*pcm).state.store(SND_PCM_STATE_RUNNING, Ordering::Release) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_mmap_begin")]
pub unsafe extern "sysv64" fn snd_pcm_mmap_begin(
    _pcm: *mut SndPcm,
    _areas: *mut *const c_void,
    _offset: *mut c_ulong,
    _frames: *mut c_ulong,
) -> c_int {
    -38 // -ENOSYS; set_access already steers clients to writei.
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_mmap_commit")]
pub unsafe extern "sysv64" fn snd_pcm_mmap_commit(
    _pcm: *mut SndPcm,
    _offset: c_ulong,
    _frames: c_ulong,
) -> c_long {
    -38
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_malloc")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_malloc(
    output: *mut *mut snd_pcm_sw_params_t,
) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(output, snd_pcm_sw_params_t::default()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_free")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_free(params: *mut snd_pcm_sw_params_t) {
    unsafe {
        kinakaze_alloc::guest::free(params.cast());
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_current")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_current(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
) -> c_int {
    if pcm.is_null() || params.is_null() {
        return -22;
    }
    unsafe { *params = (*pcm).sw_params };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
) -> c_int {
    if pcm.is_null() || params.is_null() {
        return -22;
    }
    unsafe { (*pcm).sw_params = *params };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_set_avail_min")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_set_avail_min(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
    value: c_ulong,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    unsafe { (*params).avail_min = value };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_set_stop_threshold")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_set_stop_threshold(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
    value: c_ulong,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    unsafe { (*params).stop_threshold = value };
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_strerror")]
pub unsafe extern "sysv64" fn snd_strerror(_errnum: c_int) -> *const c_char {
    if _errnum < 0 {
        c"Kinakaze ALSA bridge error".as_ptr()
    } else {
        c"Success".as_ptr()
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_config_update_free_global")]
pub extern "sysv64" fn snd_config_update_free_global() -> c_int {
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_name")]
pub unsafe extern "sysv64" fn snd_pcm_format_name(format: c_int) -> *const c_char {
    match format {
        SND_PCM_FORMAT_S16_LE => c"S16_LE".as_ptr(),
        SND_PCM_FORMAT_S32_LE => c"S32_LE".as_ptr(),
        SND_PCM_FORMAT_FLOAT_LE => c"FLOAT_LE".as_ptr(),
        SND_PCM_FORMAT_U8 => c"U8".as_ptr(),
        _ => c"UNKNOWN".as_ptr(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_description")]
pub unsafe extern "sysv64" fn snd_pcm_format_description(format: c_int) -> *const c_char {
    match format {
        SND_PCM_FORMAT_S16_LE => c"Signed 16 bit Little Endian".as_ptr(),
        SND_PCM_FORMAT_S32_LE => c"Signed 32 bit Little Endian".as_ptr(),
        SND_PCM_FORMAT_FLOAT_LE => c"Float 32 bit Little Endian".as_ptr(),
        SND_PCM_FORMAT_U8 => c"Unsigned 8 bit".as_ptr(),
        _ => c"Unknown format".as_ptr(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_open")]
pub unsafe extern "sysv64" fn snd_ctl_open(
    ctlp: *mut *mut c_void,
    name: *const c_char,
    mode: c_int,
) -> c_int {
    if ctlp.is_null() || name.is_null() || mode & !7 != 0 {
        return -22;
    }
    unsafe {
        *ctlp = core::ptr::null_mut();
    }
    let name = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
    if !matches!(name, b"default" | b"hw:0" | b"hw:Kinakaze") {
        return -19;
    }
    unsafe { params::allocate_params(ctlp.cast(), control::Control { card: 0, mode }) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_close")]
pub unsafe extern "sysv64" fn snd_ctl_close(ctl: *mut c_void) -> c_int {
    if ctl.is_null() {
        return -22;
    }
    unsafe {
        kinakaze_alloc::guest::free(ctl.cast());
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_malloc")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_malloc(
    output: *mut *mut snd_ctl_card_info_t,
) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(output, snd_ctl_card_info_t::default()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_free")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_free(info: *mut snd_ctl_card_info_t) {
    if !info.is_null() {
        unsafe { kinakaze_alloc::guest::free(info.cast()) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info")]
pub unsafe extern "sysv64" fn snd_ctl_card_info(
    ctl: *mut c_void,
    info: *mut snd_ctl_card_info_t,
) -> c_int {
    if ctl.is_null() || info.is_null() {
        return -22;
    }
    unsafe {
        (*info).card = (*ctl.cast::<control::Control>()).card;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_get_id")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_get_id(
    _info: *const snd_ctl_card_info_t,
) -> *const c_char {
    c"Kinakaze".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_get_name")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_get_name(
    _info: *const snd_ctl_card_info_t,
) -> *const c_char {
    c"Windows Default Audio".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_pcm_next_device")]
pub unsafe extern "sysv64" fn snd_ctl_pcm_next_device(
    ctl: *mut c_void,
    device: *mut c_int,
) -> c_int {
    if ctl.is_null() || device.is_null() {
        return -22;
    }
    unsafe {
        *device = if *device < 0 { 0 } else { -1 };
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_pcm_info")]
pub unsafe extern "sysv64" fn snd_ctl_pcm_info(
    ctl: *mut c_void,
    info: *mut snd_pcm_info_t,
) -> c_int {
    if ctl.is_null() || info.is_null() {
        return -22;
    }
    let info = unsafe { &*info };
    // The PCM implementation currently opens playback only.
    if info.device != 0 || info.subdevice != 0 || info.stream != SND_PCM_STREAM_PLAYBACK {
        -2
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_malloc")]
pub unsafe extern "sysv64" fn snd_pcm_info_malloc(output: *mut *mut snd_pcm_info_t) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(output, snd_pcm_info_t::default()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_free")]
pub unsafe extern "sysv64" fn snd_pcm_info_free(info: *mut snd_pcm_info_t) {
    if !info.is_null() {
        unsafe { kinakaze_alloc::guest::free(info.cast()) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_get_name")]
pub unsafe extern "sysv64" fn snd_pcm_info_get_name(_info: *const snd_pcm_info_t) -> *const c_char {
    c"Windows Default Audio".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_set_device")]
pub unsafe extern "sysv64" fn snd_pcm_info_set_device(info: *mut snd_pcm_info_t, device: c_uint) {
    if !info.is_null() {
        unsafe { (*info).device = device };
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_set_subdevice")]
pub unsafe extern "sysv64" fn snd_pcm_info_set_subdevice(
    info: *mut snd_pcm_info_t,
    subdevice: c_uint,
) {
    if !info.is_null() {
        unsafe { (*info).subdevice = subdevice };
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_set_stream")]
pub unsafe extern "sysv64" fn snd_pcm_info_set_stream(info: *mut snd_pcm_info_t, stream: c_int) {
    if !info.is_null() {
        unsafe { (*info).stream = stream };
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_card_load")]
pub unsafe extern "sysv64" fn snd_card_load(card: c_int) -> c_int {
    i32::from(card == 0)
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_card_next")]
pub unsafe extern "sysv64" fn snd_card_next(card: *mut c_int) -> c_int {
    if card.is_null() {
        return -22;
    }
    let current = unsafe { *card };
    if current < 0 {
        unsafe { *card = 0 };
    } else {
        unsafe { *card = -1 };
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_card_get_name")]
pub unsafe extern "sysv64" fn snd_card_get_name(card: c_int, name: *mut *mut c_char) -> c_int {
    unsafe { control::card_name(card, name, c"Kinakaze Audio") }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_card_get_longname")]
pub unsafe extern "sysv64" fn snd_card_get_longname(card: c_int, name: *mut *mut c_char) -> c_int {
    unsafe { control::card_name(card, name, c"Kinakaze Virtual Audio Device on Windows") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_lifecycle_and_param_configuration() {
        unsafe {
            let mut pcm: *mut SndPcm = core::ptr::null_mut();
            let ret = snd_pcm_open(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            assert_eq!(ret, 0);
            assert!(!pcm.is_null());

            let mut params: *mut snd_pcm_hw_params_t = core::ptr::null_mut();
            assert_eq!(snd_pcm_hw_params_malloc(&mut params), 0);
            assert!(!params.is_null());

            assert_eq!(snd_pcm_hw_params_any(pcm, params), 0);
            assert_eq!(
                snd_pcm_hw_params_set_access(pcm, params, SND_PCM_ACCESS_RW_INTERLEAVED),
                0
            );
            assert_eq!(
                snd_pcm_hw_params_set_format(pcm, params, SND_PCM_FORMAT_S16_LE),
                0
            );
            assert_eq!(snd_pcm_hw_params_set_channels(pcm, params, 2), 0);
            let mut rate = 48000;
            assert_eq!(
                snd_pcm_hw_params_set_rate_near(pcm, params, &mut rate, core::ptr::null_mut()),
                0
            );
            assert_eq!(rate, 48000);

            assert_eq!(snd_pcm_hw_params(pcm, params), 0);
            assert_eq!(snd_pcm_prepare(pcm), 0);

            let buffer = [0i16; 1024];
            let written = snd_pcm_writei(pcm, buffer.as_ptr() as *const c_void, 512);
            assert_eq!(written, 512);

            assert_eq!(snd_pcm_drain(pcm), 0);
            snd_pcm_hw_params_free(params);
            assert_eq!(snd_pcm_close(pcm), 0);
        }
    }
}

mod object_layout;
