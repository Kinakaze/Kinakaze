//! Controls belong to the virtual display; host keyboard settings stay private.
use crate::{Display, errors};
use std::sync::Mutex;
mod lifecycle;

#[repr(C)]
#[derive(Default)]
pub struct XKeyboardControl {
    pub key_click_percent: i32,
    pub bell_percent: i32,
    pub bell_pitch: i32,
    pub bell_duration: i32,
    pub led: i32,
    pub led_mode: i32,
    pub key: i32,
    pub auto_repeat_mode: i32,
}
#[repr(C)]
pub struct XKeyboardState {
    pub key_click_percent: i32,
    pub bell_percent: i32,
    pub bell_pitch: u32,
    pub bell_duration: u32,
    pub led_mask: u64,
    pub global_auto_repeat: i32,
    pub auto_repeats: [u8; 32],
}
#[derive(Clone, Copy)]
struct Controls {
    click: u8,
    bell: u8,
    pitch: u16,
    duration: u16,
    leds: u32,
    repeat: bool,
    keys: [u8; 32],
}
const DEFAULT: Controls = Controls {
    click: 0,
    bell: 50,
    pitch: 400,
    duration: 100,
    leds: 0,
    repeat: true,
    keys: [255; 32],
};
static CONTROLS: Mutex<Controls> = Mutex::new(DEFAULT);

#[unsafe(export_name = "kinakaze_engine_libX11_XChangeKeyboardControl")]
pub unsafe extern "sysv64" fn XChangeKeyboardControl(
    d: *mut Display,
    mask: u64,
    p: *const XKeyboardControl,
) -> i32 {
    let fail = |code, value| unsafe { errors::report(d, code, 102, 0, value) };
    if mask & !255 != 0 || (mask != 0 && p.is_null()) {
        return fail(2, mask as usize);
    }
    if mask == 0 {
        return 1;
    }
    let p = unsafe { &*p };
    if mask & 16 != 0 && mask & 32 == 0 || mask & 64 != 0 && mask & 128 == 0 {
        return fail(8, 0);
    }
    let mut lock = CONTROLS.lock().unwrap();
    let mut state = *lock;
    for (bit, default, max) in [
        (0, DEFAULT.click as i32, 100),
        (1, DEFAULT.bell as i32, 100),
        (2, DEFAULT.pitch as i32, 65535),
        (3, DEFAULT.duration as i32, 65535),
    ] {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let value = match bit {
            0 => p.key_click_percent,
            1 => p.bell_percent,
            2 => p.bell_pitch,
            _ => p.bell_duration,
        };
        if !(-1..=max).contains(&value) {
            drop(lock);
            return fail(2, value as usize);
        }
        let value = if value == -1 { default } else { value };
        match bit {
            0 => state.click = value as u8,
            1 => state.bell = value as u8,
            2 => state.pitch = value as u16,
            _ => state.duration = value as u16,
        }
    }
    if mask & 32 != 0 {
        if !(0..=1).contains(&p.led_mode) || (mask & 16 != 0 && !(1..=32).contains(&p.led)) {
            drop(lock);
            return fail(
                2,
                if !(0..=1).contains(&p.led_mode) {
                    p.led_mode
                } else {
                    p.led
                } as usize,
            );
        }
        let bits = if mask & 16 == 0 {
            u32::MAX
        } else {
            1 << (p.led - 1)
        };
        state.leds = if p.led_mode == 0 {
            state.leds & !bits
        } else {
            state.leds | bits
        };
    }
    if mask & 128 != 0 {
        if !(0..=2).contains(&p.auto_repeat_mode) || (mask & 64 != 0 && !(8..=255).contains(&p.key))
        {
            drop(lock);
            return fail(
                2,
                if !(0..=2).contains(&p.auto_repeat_mode) {
                    p.auto_repeat_mode
                } else {
                    p.key
                } as usize,
            );
        }
        let enabled = if p.auto_repeat_mode == 2 {
            DEFAULT.repeat
        } else {
            p.auto_repeat_mode != 0
        };
        if mask & 64 == 0 {
            state.repeat = enabled;
        } else {
            let byte = &mut state.keys[p.key as usize / 8];
            let bit = 1 << (p.key % 8);
            if enabled {
                *byte |= bit;
            } else {
                *byte &= !bit;
            }
        }
    }
    *lock = state;
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetKeyboardControl")]
pub unsafe extern "sysv64" fn XGetKeyboardControl(_d: *mut Display, p: *mut XKeyboardState) -> i32 {
    if p.is_null() {
        return 0;
    }
    let s = *CONTROLS.lock().unwrap();
    unsafe {
        p.write(XKeyboardState {
            key_click_percent: s.click as i32,
            bell_percent: s.bell as i32,
            bell_pitch: s.pitch as u32,
            bell_duration: s.duration as u32,
            led_mask: s.leds as u64,
            global_auto_repeat: s.repeat as i32,
            auto_repeats: s.keys,
        });
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAutoRepeatOn")]
pub unsafe extern "sysv64" fn XAutoRepeatOn(d: *mut Display) -> i32 {
    unsafe {
        XChangeKeyboardControl(
            d,
            128,
            &XKeyboardControl {
                auto_repeat_mode: 1,
                ..Default::default()
            },
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAutoRepeatOff")]
pub unsafe extern "sysv64" fn XAutoRepeatOff(d: *mut Display) -> i32 {
    unsafe {
        XChangeKeyboardControl(
            d,
            128,
            &XKeyboardControl {
                auto_repeat_mode: 0,
                ..Default::default()
            },
        )
    }
}

// A short PCM waveform implements per-display volume/pitch/duration without
// changing the user's global sound volume or creating a periodic audio worker.
fn tone(percent: u32, pitch: u16, duration: u16) -> bool {
    if percent == 0 || pitch == 0 || duration == 0 {
        return true;
    }
    let rate = 192_000u32; // Covers the entire protocol's 16-bit frequency range.
    let count = (rate as usize * duration as usize) / 1000;
    let size = count * 2;
    let mut wav = Vec::new();
    if wav.try_reserve_exact(44 + size).is_err() {
        return false;
    }
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + size) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(size as u32).to_le_bytes());
    let amplitude = 32767.0 * percent.min(100) as f64 / 100.0;
    for i in 0..count {
        let sample = (amplitude
            * (i as f64 * std::f64::consts::TAU * pitch as f64 / rate as f64).sin())
            as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut core::ffi::c_void, flags: u32) -> i32;
    }
    unsafe { PlaySoundW(wav.as_ptr().cast(), core::ptr::null_mut(), 4 | 2) != 0 } // MEMORY | NODEFAULT, synchronous ownership
}
pub(crate) fn bell(percent: i32) -> bool {
    let s = *CONTROLS.lock().unwrap();
    let base = s.bell as i32;
    let volume = if percent < 0 {
        base + base * percent / 100
    } else {
        base + (100 - base) * percent / 100
    };
    tone(volume as u32, s.pitch, s.duration)
}
/// Return false for a disabled repeat; initial presses and releases stay intact.
pub fn accept_key(key: u32, state: u32) -> bool {
    let s = *CONTROLS.lock().unwrap();
    if state == kinakaze_libdisplay::event::STATE_REPEAT
        && (!s.repeat || key >= 256 || s.keys[key as usize / 8] & (1 << (key % 8)) == 0)
    {
        return false;
    }
    if state != kinakaze_libdisplay::event::STATE_RELEASED && s.click != 0 {
        tone(s.click as u32, 1000, 4);
    }
    true
}
