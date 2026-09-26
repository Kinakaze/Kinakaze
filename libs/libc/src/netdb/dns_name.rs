//! DNS presentation-to-wire encoding and RFC 1035 suffix compression.
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};

fn encode(name: &[u8]) -> Result<Vec<u8>, i32> {
    if name.is_empty() || name == b"." {
        return Ok(vec![0]);
    }
    let mut wire = vec![0];
    let mut label = 0;
    let mut at = 0;
    while at < name.len() {
        let mut byte = name[at];
        at += 1;
        if byte == b'.' {
            let length = wire.len() - label - 1;
            if length == 0 || length > 63 {
                return Err(90);
            }
            wire[label] = length as u8;
            label = wire.len();
            wire.push(0);
            continue;
        }
        if byte == b'\\' {
            byte = *name.get(at).ok_or(90)?;
            at += 1;
            if byte.is_ascii_digit() {
                let digits = name.get(at..at + 2).ok_or(90)?;
                if !digits.iter().all(u8::is_ascii_digit) {
                    return Err(90);
                }
                let value = u16::from(byte - b'0') * 100
                    + u16::from(digits[0] - b'0') * 10
                    + u16::from(digits[1] - b'0');
                byte = u8::try_from(value).map_err(|_| 90)?;
                at += 2;
            }
        }
        wire.push(byte);
        if wire.len() - label - 1 > 63 || wire.len() > 254 {
            return Err(90);
        }
    }
    if wire.len() - label > 1 {
        wire[label] = (wire.len() - label - 1) as u8;
        wire.push(0);
    }
    if wire.len() > 255 {
        return Err(90);
    }
    Ok(wire)
}

/// Encode a DNS name, reusing prior message suffixes when a pointer table is
/// supplied. Tables and destination are changed only after capacity validation.
/// # Safety
/// `name` is NUL terminated; `output` has `capacity` writable bytes. A supplied
/// pointer table is NUL terminated and its entries reference earlier names in
/// the same message. `end` is the table's one-past-end pointer, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dn_comp(
    name: *const c_char,
    output: *mut u8,
    capacity: c_int,
    table: *mut *mut u8,
    end: *mut *mut u8,
) -> c_int {
    let result = (|| -> Result<usize, i32> {
        if name.is_null() || output.is_null() || capacity <= 0 {
            return Err(90);
        }
        let wire = encode(unsafe { CStr::from_ptr(name) }.to_bytes())?;
        let mut prior = Vec::new();
        let mut free_slot = ptr::null_mut();
        let mut base = ptr::null_mut();
        if !table.is_null() && (end.is_null() || table < end) {
            base = unsafe { *table };
            if !base.is_null() {
                let mut slot = unsafe { table.add(1) };
                while end.is_null() || slot < end {
                    let pointer = unsafe { *slot };
                    if pointer.is_null() {
                        free_slot = slot;
                        break;
                    }
                    prior.push(pointer);
                    slot = unsafe { slot.add(1) };
                }
            }
        }
        let message_length = (output as usize)
            .checked_sub(base as usize)
            .filter(|&length| !base.is_null() && length <= 65535);
        let message =
            message_length.map(|length| unsafe { core::slice::from_raw_parts(base, length) });
        let mut packed = Vec::new();
        let mut labels = Vec::new();
        let mut at = 0;
        while wire[at] != 0 {
            let suffix = super::expand_dns_name(&wire, at)?.0;
            let matched = message.and_then(|bytes| {
                prior.iter().find_map(|&pointer| {
                    let offset = (pointer as usize).checked_sub(base as usize)?;
                    if offset >= bytes.len() || offset >= 0x4000 {
                        return None;
                    }
                    // Table entries may point at an entire earlier name. Walk its
                    // literal labels too so a suffix can be reused independently.
                    let mut offset = offset;
                    loop {
                        if super::expand_dns_name(bytes, offset)
                            .ok()?
                            .0
                            .eq_ignore_ascii_case(&suffix)
                        {
                            return Some(offset);
                        }
                        let length = *bytes.get(offset)? as usize;
                        if length == 0 || length > 63 {
                            return None;
                        }
                        offset += length + 1;
                        if offset >= 0x4000 {
                            return None;
                        }
                    }
                })
            });
            if let Some(offset) = matched {
                packed.extend_from_slice(&[0xc0 | (offset >> 8) as u8, offset as u8]);
                break;
            }
            labels.push(packed.len());
            let count = wire[at] as usize + 1;
            packed.extend_from_slice(&wire[at..at + count]);
            at += count;
        }
        if wire[at] == 0 {
            packed.push(0);
        }
        if packed.len() > capacity as usize {
            return Err(90);
        }
        unsafe {
            ptr::copy_nonoverlapping(packed.as_ptr(), output, packed.len());
        }
        if let Some(length) = message_length {
            if !free_slot.is_null() && !end.is_null() {
                for offset in labels {
                    if length + offset >= 0x4000 || unsafe { free_slot.add(1) } >= end {
                        break;
                    }
                    unsafe {
                        free_slot.write(output.add(offset));
                        free_slot = free_slot.add(1);
                        free_slot.write(ptr::null_mut());
                    }
                }
            }
        }
        Ok(packed.len())
    })();
    match result {
        Ok(length) => length as c_int,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_compression_and_transactional_overflow() {
        assert_eq!(
            encode(br"a\.b.\000x."),
            Ok(vec![3, b'a', b'.', b'b', 2, 0, b'x', 0])
        );
        assert!(encode(b"a..b").is_err());
        assert!(encode(br"a.\999").is_err());
        assert!(encode(&[b'a'; 64]).is_err());
        let mut packet = [0u8; 128];
        let base = packet.as_mut_ptr();
        let mut table = [ptr::null_mut(); 16];
        table[0] = base;
        unsafe {
            let start = base.add(12);
            let end = table.as_mut_ptr().add(16);
            let count =
                kinakaze_abi_dn_comp(c"Example.COM".as_ptr(), start, 116, table.as_mut_ptr(), end);
            assert_eq!(count, 13);
            let output = start.add(count as usize);
            let original = table;
            assert_eq!(
                kinakaze_abi_dn_comp(
                    c"www.example.com".as_ptr(),
                    output,
                    5,
                    table.as_mut_ptr(),
                    end
                ),
                -1
            );
            assert_eq!(table, original);
            assert_eq!(
                kinakaze_abi_dn_comp(
                    c"www.example.com".as_ptr(),
                    output,
                    100,
                    table.as_mut_ptr(),
                    end
                ),
                6
            );
            assert_eq!(
                super::super::expand_dns_name(&packet[..31], 25).unwrap(),
                (b"www.Example.COM".to_vec(), 6)
            );
        }
    }
}
