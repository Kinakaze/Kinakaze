use super::*;

unsafe fn key(event: *mut XKeyEvent) -> (KeySym, Option<char>) {
    if event.is_null() || unsafe { (*event).r#type } != crate::KeyPress {
        return (0, None);
    }
    let mut symbol = unsafe { crate::XLookupKeysym(event, 0) };
    let modifiers = unsafe { (*event).state };
    let shift = modifiers & 1 != 0;
    let caps = modifiers & 2 != 0;
    if (b'a' as usize..=b'z' as usize).contains(&symbol) {
        if shift ^ caps {
            symbol -= 32;
        }
    } else if shift {
        let plain = b"1234567890-=[]\\;',./`";
        let shifted = b"!@#$%^&*()_+{}|:\"<>?~";
        if let Some(index) = plain.iter().position(|&value| value as usize == symbol) {
            symbol = shifted[index] as usize;
        }
    }
    let mut character = match symbol {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(symbol as u32),
        0x01000100..=0x0110ffff => char::from_u32((symbol & 0x00ffffff) as u32),
        0xff08 => Some('\u{8}'),
        0xff09 => Some('\t'),
        0xff0d => Some('\r'),
        0xff1b => Some('\u{1b}'),
        0xffff => Some('\u{7f}'),
        _ => None,
    };
    if modifiers & 4 != 0 {
        if let Some(c) = character {
            character = match c {
                '@'..='_' | 'a'..='z' => char::from_u32((c as u32) & 0x1f),
                ' ' | '2' => Some('\0'),
                '?' => Some('\u{7f}'),
                _ => Some(c),
            };
        }
    }
    (symbol, character)
}
unsafe fn lookup(
    context: *mut InputContext,
    event: *mut XKeyEvent,
    buffer: *mut c_void,
    capacity: c_int,
    keysym: *mut KeySym,
    status: *mut Status,
    wide: bool,
) -> c_int {
    let (symbol, character) = if context.is_null() {
        (0, None)
    } else {
        unsafe { key(event) }
    };
    let mut bytes = [0; 4];
    let length = character.map_or(0, |c| {
        if wide {
            1
        } else {
            c.encode_utf8(&mut bytes).len()
        }
    });
    let overflow = length > capacity.max(0) as usize;
    if !status.is_null() {
        unsafe {
            *status = if overflow {
                -1
            } else if length > 0 {
                if symbol != 0 { 4 } else { 2 }
            } else if symbol != 0 {
                3
            } else {
                1
            };
        }
    }
    if !overflow {
        if !keysym.is_null() {
            unsafe {
                *keysym = symbol;
            }
        }
        if !buffer.is_null() && length != 0 {
            unsafe {
                if wide {
                    buffer.cast::<i32>().write(character.unwrap() as i32);
                } else {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast(), length);
                }
            }
        }
    }
    length as c_int
}
macro_rules! lookup_entry {
    ($name:ident, $char:ty, $wide:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libX11_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            context: *mut InputContext,
            event: *mut XKeyEvent,
            buffer: *mut $char,
            capacity: c_int,
            keysym: *mut KeySym,
            status: *mut Status,
        ) -> c_int {
            unsafe {
                lookup(
                    context,
                    event,
                    buffer.cast(),
                    capacity,
                    keysym,
                    status,
                    $wide,
                )
            }
        }
    };
}
lookup_entry!(Xutf8LookupString, c_char, false);
lookup_entry!(XmbLookupString, c_char, false);
lookup_entry!(XwcLookupString, i32, true);
