//! Wide stream I/O with the encoding captured at orientation. The decoder is shared
//! with mbrtowc; stream operations keep one lock across a complete character.
use super::{File, Stream, standard, with_oriented, with_stream};
use core::{ffi::c_int, ptr};
const WEOF: u32 = u32::MAX;
const EILSEQ: i32 = 84;

fn read_scalar(stream: &mut Stream) -> u32 {
    let mut state = crate::uchar::MbState::default();
    let mut started = false;
    loop {
        let mut byte = [0u8];
        match stream.read(&mut byte) {
            Ok(0) => {
                if started {
                    stream.error = true;
                    crate::set_errno(EILSEQ);
                }
                return WEOF;
            }
            Err(error) => {
                crate::set_errno(error);
                return WEOF;
            }
            Ok(_) => (),
        }
        started = true;
        match unsafe {
            crate::uchar::decode_with_encoding(
                byte.as_ptr().cast(),
                1,
                &mut state,
                stream.wide_utf8,
            )
        } {
            crate::uchar::Decoded::Scalar(scalar, _) => return scalar,
            crate::uchar::Decoded::Incomplete => (),
            crate::uchar::Decoded::Invalid => {
                stream.error = true;
                crate::set_errno(EILSEQ);
                return WEOF;
            }
        }
    }
}
fn write_scalar(stream: &mut Stream, scalar: u32) -> u32 {
    let mut encoded = [0u8; 4];
    let count = unsafe {
        crate::uchar::encode_with_encoding(
            encoded.as_mut_ptr().cast(),
            scalar,
            &mut crate::uchar::MbState::default(),
            stream.wide_utf8,
        )
    };
    if count == usize::MAX {
        stream.error = true;
        return WEOF;
    }
    let encoded = &encoded[..count];
    match stream.write(encoded) {
        Ok(count) if count == encoded.len() => scalar,
        Ok(_) => {
            stream.error = true;
            WEOF
        }
        Err(error) => {
            crate::set_errno(error);
            WEOF
        }
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fgetwc(file: *mut File) -> u32 {
    with_oriented(file, 1, WEOF, read_scalar)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fputwc(scalar: u32, file: *mut File) -> u32 {
    with_oriented(file, 1, WEOF, |stream| write_scalar(stream, scalar))
}
pub(super) fn ungetwc(scalar: u32, file: *mut File) -> u32 {
    if scalar == WEOF {
        return WEOF;
    }
    with_oriented(file, 1, WEOF, |stream| {
        let mut bytes = [0u8; 4];
        let count = unsafe {
            crate::uchar::encode_with_encoding(
                bytes.as_mut_ptr().cast(),
                scalar,
                &mut crate::uchar::MbState::default(),
                stream.wide_utf8,
            )
        };
        if count == usize::MAX {
            return WEOF;
        }
        let bytes = &bytes[..count];
        stream.pushback.extend(bytes.iter().rev());
        stream.eof = false;
        scalar
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetws(
    buffer: *mut i32,
    count: c_int,
    file: *mut File,
) -> *mut i32 {
    if buffer.is_null() || count <= 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    with_oriented(file, 1, ptr::null_mut(), |stream| {
        let mut filled = 0;
        while filled + 1 < count as usize {
            let scalar = read_scalar(stream);
            if scalar == WEOF {
                if filled == 0 || stream.error {
                    return ptr::null_mut();
                }
                break;
            }
            unsafe {
                buffer.add(filled).write(scalar as i32);
            }
            filled += 1;
            if scalar == b'\n' as u32 {
                break;
            }
        }
        unsafe {
            buffer.add(filled).write(0);
        }
        buffer
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fputws(text: *const i32, file: *mut File) -> c_int {
    if text.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    with_oriented(file, 1, -1, |stream| {
        let mut cursor = text;
        loop {
            let scalar = unsafe { cursor.read() } as u32;
            if scalar == 0 {
                return 0;
            }
            if write_scalar(stream, scalar) == WEOF {
                return -1;
            }
            cursor = unsafe { cursor.add(1) };
        }
    })
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fwide(file: *mut File, mode: c_int) -> c_int {
    with_stream(file, 0, |stream| {
        if stream.orientation == 0 && mode != 0 {
            stream.orientation = mode.signum();
            stream.wide_utf8 = mode > 0 && crate::locale::utf8();
            unsafe {
                (*file)
                    ._mode
                    .store(stream.orientation, core::sync::atomic::Ordering::Relaxed);
            }
        }
        stream.orientation
    })
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getwchar() -> u32 {
    kinakaze_abi_fgetwc(standard(0))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_putwchar(scalar: u32) -> u32 {
    kinakaze_abi_fputwc(scalar, standard(1))
}
