//! Allocation-free MAC parsing/formatting; non-reentrant ABI owns thread storage.
use core::{
    ffi::{CStr, c_char},
    ptr,
};

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse(text: &[u8]) -> Option<[u8; 6]> {
    let mut address = [0; 6];
    let mut cursor = 0;
    for (index, octet) in address.iter_mut().enumerate() {
        *octet = hex(*text.get(cursor)?)?;
        cursor += 1;
        let next = text.get(cursor).copied();
        // The last component may be followed by whitespace and a host name,
        // as in /etc/ethers. A complete two-digit final component is a prefix.
        let one_digit = if index < 5 {
            next == Some(b':')
        } else {
            next.is_none() || next.is_some_and(|b| b.is_ascii_whitespace())
        };
        if !one_digit {
            *octet = (*octet << 4) | hex(next?)?;
            cursor += 1;
        }
        if index < 5 {
            if text.get(cursor) != Some(&b':') {
                return None;
            }
            cursor += 1;
        }
    }
    Some(address)
}

fn format(address: &[u8; 6], output: &mut [u8; 18]) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut cursor = 0;
    for (index, &byte) in address.iter().enumerate() {
        if index != 0 {
            output[cursor] = b':';
            cursor += 1;
        }
        if byte >= 16 {
            output[cursor] = DIGITS[(byte >> 4) as usize];
            cursor += 1;
        }
        output[cursor] = DIGITS[(byte & 15) as usize];
        cursor += 1;
    }
    output[cursor] = 0;
    cursor + 1
}

super::records::returned::returned_record!(*b"CYETHR01");

fn storage() -> Option<*mut u8> {
    if let Err(error) = register_returned() {
        crate::set_errno(error);
        return None;
    }
    RETURNED.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let bytes = unsafe { kinakaze_alloc::guest::malloc(24) };
            if bytes.is_null() {
                crate::set_errno(12);
                return None;
            }
            unsafe { ptr::write_bytes(bytes, 0, 24) };
            *slot = Some(super::records::returned::Record(bytes as usize));
        }
        Some(slot.as_ref().unwrap().0 as *mut u8)
    })
}

/// # Safety
/// text is NUL-terminated; destination is writable for six bytes, or null.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ether_aton_r(
    text: *const c_char,
    destination: *mut u8,
) -> *mut u8 {
    if text.is_null() || destination.is_null() {
        return ptr::null_mut();
    }
    let Some(address) = parse(unsafe { CStr::from_ptr(text) }.to_bytes()) else {
        return ptr::null_mut();
    };
    unsafe { ptr::copy_nonoverlapping(address.as_ptr(), destination, address.len()) };
    destination
}

/// # Safety
/// text is null or a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ether_aton(text: *const c_char) -> *mut u8 {
    if text.is_null() {
        return ptr::null_mut();
    }
    storage().map_or(ptr::null_mut(), |output| unsafe {
        kinakaze_abi_ether_aton_r(text, output)
    })
}

/// # Safety
/// address is readable for six bytes; output is writable for eighteen bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ether_ntoa_r(
    address: *const u8,
    output: *mut c_char,
) -> *mut c_char {
    if address.is_null() || output.is_null() {
        return ptr::null_mut();
    }
    let mut bytes = [0u8; 18];
    let count = format(unsafe { &*address.cast::<[u8; 6]>() }, &mut bytes);
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), count) };
    output
}

/// # Safety
/// address is null or readable for six bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ether_ntoa(address: *const u8) -> *mut c_char {
    if address.is_null() {
        return ptr::null_mut();
    }
    storage().map_or(ptr::null_mut(), |output| unsafe {
        kinakaze_abi_ether_ntoa_r(address, output.add(6).cast())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_prefix_parser_and_unpadded_lowercase_formatter() {
        let value = [0, 1, 10, 171, 205, 255];
        for input in [
            b"0:1:a:AB:cd:Ff".as_slice(),
            b"00:01:0a:ab:cd:ff host",
            b"0:1:a:ab:cd:ff:trailing",
        ] {
            assert_eq!(parse(input), Some(value));
        }
        assert_eq!(parse(b"0:1:2:3:4:5 host"), Some([0, 1, 2, 3, 4, 5]));
        for input in [
            b"".as_slice(),
            b"1:2:3:4:5",
            b"1:2:3:4:5:",
            b"1:2:3:4:5:gg",
            b"000:1:2:3:4:5",
            b"1:2:3:4:5:6x",
        ] {
            assert!(parse(input).is_none(), "{input:?}");
        }
        let mut output = [0xcc; 18];
        let count = format(&value, &mut output);
        assert_eq!(&output[..count], b"0:1:a:ab:cd:ff\0");
        assert!(output[count..].iter().all(|&b| b == 0xcc));
        assert_eq!(format(&[255; 6], &mut output), 18);
        assert_eq!(&output, b"ff:ff:ff:ff:ff:ff\0");
    }
}
