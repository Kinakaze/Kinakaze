//! GNU gencat files contain a header, little/big-endian hash tables, then strings.
use kinakaze_vfs::EINVAL;

const MAGIC: u32 = 0x9604_08de;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub width: usize,
    pub depth: usize,
    pub strings: usize,
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

impl Layout {
    pub fn parse(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 12 {
            return Err(EINVAL);
        }
        let swapped = match word(bytes, 0) {
            MAGIC => false,
            value if value.swap_bytes() == MAGIC => true,
            _ => return Err(EINVAL),
        };
        let dimension = |offset| {
            let value = word(bytes, offset);
            (if swapped { value.swap_bytes() } else { value }) as usize
        };
        let width = dimension(4);
        let depth = dimension(8);
        if width == 0 || depth == 0 {
            return Err(EINVAL);
        }
        let entries = width.checked_mul(depth).ok_or(EINVAL)?;
        let strings = entries
            .checked_mul(24)
            .and_then(|n| n.checked_add(12))
            .ok_or(EINVAL)?;
        if strings > bytes.len() {
            return Err(EINVAL);
        }
        // Any offset before the last terminator has a bounded C string. One
        // scan avoids quadratic work on overlapping or malicious string ranges.
        let last_nul = bytes[strings..].iter().rposition(|&b| b == 0);
        for index in 0..entries {
            let offset = 12 + index * 12;
            if word(bytes, offset) != 0
                && last_nul.is_none_or(|last| word(bytes, offset + 8) as usize > last)
            {
                return Err(EINVAL);
            }
        }
        Ok(Self {
            width,
            depth,
            strings,
        })
    }

    pub fn find(&self, bytes: &[u8], set: u32, message: u32) -> Option<usize> {
        // GNU gencat multiplies two signed 32-bit IDs before converting to
        // size_t. Preserve that wrap and sign extension on Linux x86_64.
        let bucket = (set as i32).wrapping_mul(message as i32) as usize % self.width;
        for plane in 0..self.depth {
            let entry = 12 + (bucket + plane * self.width) * 12;
            if word(bytes, entry) == set && word(bytes, entry + 4) == message {
                return Some(self.strings + word(bytes, entry + 8) as usize);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_tables_and_unterminated_strings() {
        let mut bytes = Vec::new();
        for value in [MAGIC, 1, 1, 2, 1, 0] {
            bytes.extend(value.to_le_bytes());
        }
        assert!(Layout::parse(&bytes).is_err());
        bytes.extend([0; 12]);
        bytes.extend(b"message");
        assert!(Layout::parse(&bytes).is_err());
        bytes.push(0);
        let layout = Layout::parse(&bytes).unwrap();
        assert_eq!(layout.find(&bytes, 2, 1), Some(36));
        bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
        assert!(Layout::parse(&bytes).is_err());
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Layout::parse(&bytes).is_err());
    }
}
