//! Native playback buffers and device position. No wall-clock audio estimates.
use super::{
    MMSYSERR_NOERROR, PA_SAMPLE_FLOAT32LE, PA_SAMPLE_S16LE, PA_SAMPLE_S24_32LE, PA_SAMPLE_S24LE,
    PA_SAMPLE_S32LE, PA_SAMPLE_U8, WAVE_FORMAT_IEEE_FLOAT, frame_size_value, pa_sample_spec,
};
use core::ptr;
use windows_sys::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveOutClose, waveOutGetPosition, waveOutPause, waveOutPrepareHeader, waveOutReset,
    waveOutRestart, waveOutUnprepareHeader, waveOutWrite,
};
use windows_sys::Win32::Media::{MMTIME, TIME_BYTES, TIME_MS, TIME_SAMPLES};

#[derive(Default)]
struct Clock {
    written: u64,
    origin: u64,
    kind: u32,
    last: u32,
    wraps: u64,
}
impl Clock {
    fn units(&mut self, kind: u32, raw: u32) -> Option<u64> {
        if !matches!(kind, TIME_SAMPLES | TIME_BYTES | TIME_MS) {
            return None;
        }
        if self.kind != 0 && self.kind != kind {
            return None;
        }
        if raw < self.last {
            self.wraps = self.wraps.checked_add(1u64 << 32)?;
        }
        self.last = raw;
        self.kind = kind;
        Some(self.wraps + u64::from(raw))
    }
    fn reset(&mut self) {
        self.origin = self.written;
        self.kind = 0;
        self.last = 0;
        self.wraps = 0;
    }
}

struct WaveBuffer {
    _data: Box<[u8]>,
    header: Box<WAVEHDR>,
}

pub(super) struct WaveOut {
    handle: HWAVEOUT,
    buffers: Vec<WaveBuffer>,
    clock: Clock,
    frame: usize,
    rate: u32,
}

// waveOut handles are process handles and may be used by the stream worker.
unsafe impl Send for WaveOut {}

impl WaveOut {
    pub(super) fn open(spec: pa_sample_spec) -> Option<Self> {
        let (tag, bits) = match spec.format {
            PA_SAMPLE_U8 => (WAVE_FORMAT_PCM as u16, 8),
            PA_SAMPLE_S16LE => (WAVE_FORMAT_PCM as u16, 16),
            PA_SAMPLE_S24LE => (WAVE_FORMAT_PCM as u16, 24),
            PA_SAMPLE_S24_32LE | PA_SAMPLE_S32LE => (WAVE_FORMAT_PCM as u16, 32),
            PA_SAMPLE_FLOAT32LE => (WAVE_FORMAT_IEEE_FLOAT, 32),
            _ => return None,
        };
        let channels = u16::from(spec.channels);
        let block_align = channels.checked_mul(bits / 8)?;
        let format = WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: channels,
            nSamplesPerSec: spec.rate,
            nAvgBytesPerSec: spec.rate.checked_mul(u32::from(block_align))?,
            nBlockAlign: block_align,
            wBitsPerSample: bits,
            cbSize: 0,
        };
        let mut handle = ptr::null_mut();
        let result = unsafe {
            windows_sys::Win32::Media::Audio::waveOutOpen(
                &raw mut handle,
                WAVE_MAPPER,
                &raw const format,
                0,
                0,
                CALLBACK_NULL,
            )
        };
        (result == MMSYSERR_NOERROR && !handle.is_null()).then_some(Self {
            handle,
            buffers: Vec::new(),
            clock: Clock::default(),
            frame: frame_size_value(spec),
            rate: spec.rate,
        })
    }

    fn reap(&mut self) {
        let mut index = 0;
        while index < self.buffers.len() {
            let flags =
                unsafe { ptr::addr_of!((*self.buffers[index].header).dwFlags).read_volatile() };
            if flags & WHDR_DONE == 0 {
                index += 1;
                continue;
            }
            let buffer = &mut self.buffers[index];
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &raw mut *buffer.header,
                    size_of::<WAVEHDR>() as u32,
                )
            };
            self.buffers.swap_remove(index);
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > u32::MAX as usize || bytes.len() % self.frame != 0 {
            return false;
        }
        self.reap();
        let _ = self.position();
        let mut data = bytes.to_vec().into_boxed_slice();
        let mut header = Box::new(WAVEHDR::default());
        header.lpData = data.as_mut_ptr();
        header.dwBufferLength = u32::try_from(data.len()).unwrap_or(u32::MAX);
        if unsafe {
            waveOutPrepareHeader(self.handle, &raw mut *header, size_of::<WAVEHDR>() as u32)
        } != MMSYSERR_NOERROR
        {
            return false;
        }
        if unsafe { waveOutWrite(self.handle, &raw mut *header, size_of::<WAVEHDR>() as u32) }
            != MMSYSERR_NOERROR
        {
            unsafe {
                waveOutUnprepareHeader(self.handle, &raw mut *header, size_of::<WAVEHDR>() as u32)
            };
            return false;
        }
        self.clock.written = self.clock.written.saturating_add(bytes.len() as u64);
        self.buffers.push(WaveBuffer {
            _data: data,
            header,
        });
        true
    }

    pub(super) fn pause(&self, paused: bool) -> bool {
        (unsafe {
            if paused {
                waveOutPause(self.handle)
            } else {
                waveOutRestart(self.handle)
            }
        }) == MMSYSERR_NOERROR
    }

    pub(super) fn position(&mut self) -> Option<(u64, u64)> {
        let mut time = MMTIME {
            wType: TIME_SAMPLES,
            ..Default::default()
        };
        if unsafe { waveOutGetPosition(self.handle, &mut time, size_of::<MMTIME>() as u32) }
            != MMSYSERR_NOERROR
        {
            return None;
        }
        // A driver may return another supported unit; honor the returned tag.
        let raw = unsafe { time.u.sample };
        let units = self.clock.units(time.wType, raw)?;
        let bytes = match time.wType {
            TIME_SAMPLES => units.saturating_mul(self.frame as u64),
            TIME_BYTES => units,
            TIME_MS => ((u128::from(units) * u128::from(self.rate) * self.frame as u128) / 1000)
                .min(u128::from(u64::MAX)) as u64,
            _ => return None,
        };
        let played = self
            .clock
            .origin
            .saturating_add(bytes)
            .min(self.clock.written);
        Some((played, self.clock.written))
    }

    pub(super) fn reset(&mut self) -> bool {
        if unsafe { waveOutReset(self.handle) } != MMSYSERR_NOERROR {
            return false;
        }
        for buffer in &mut self.buffers {
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &raw mut *buffer.header,
                    size_of::<WAVEHDR>() as u32,
                )
            };
        }
        self.buffers.clear();
        self.clock.reset();
        true
    }
}

impl Drop for WaveOut {
    fn drop(&mut self) {
        if !self.reset() {
            // A failed reset must not free memory still owned by the driver.
            for buffer in self.buffers.drain(..) {
                core::mem::forget(buffer);
            }
        }
        unsafe { waveOutClose(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_counter_wrap_and_reset_keep_the_stream_timeline() {
        let mut clock = Clock::default();
        assert_eq!(clock.units(TIME_SAMPLES, u32::MAX - 3), Some(0xffff_fffc));
        assert_eq!(clock.units(TIME_SAMPLES, 12), Some(0x1_0000_000c));
        assert_eq!(clock.units(TIME_MS, 13), None);
        clock.written = 0x2_0000_0100;
        clock.reset();
        assert_eq!(clock.origin, clock.written);
        assert_eq!(clock.units(TIME_BYTES, 0), Some(0));
        assert_eq!(clock.units(TIME_BYTES, 8), Some(8));
        assert_eq!(clock.units(0, 9), None);
    }
}
