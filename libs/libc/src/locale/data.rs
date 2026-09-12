//! Bounded reader for GNU LC_CTYPE files. Input is data, never executable code.
//! Layout: glibc locale/localeinfo.h, locale/langinfo.h and wctype/wchar-lookup.h.
use kinakaze_vfs::EINVAL;

pub(super) fn load() -> Result<Vec<u8>, i32> {
    struct Descriptor(i32);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            crate::kinakaze_abi_close(self.0);
        }
    }
    let fd = unsafe {
        crate::fsextra::kinakaze_abi_open64(
            c"/usr/lib/locale/C.utf8/LC_CTYPE".as_ptr(),
            kinakaze_vfs::fs::O_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(kinakaze_tls::errno());
    }
    let owned = Descriptor(fd);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count =
            unsafe { crate::kinakaze_abi_read(owned.0, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            let error = kinakaze_tls::errno();
            if error == kinakaze_vfs::EINTR {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            break;
        }
        if bytes.len() + count as usize > 16 * 1024 * 1024 {
            return Err(EINVAL);
        }
        bytes
            .try_reserve(count as usize)
            .map_err(|_| kinakaze_vfs::ENOMEM)?;
        bytes.extend_from_slice(&buffer[..count as usize]);
    }
    Ok(bytes)
}

pub(super) const CLASSES: [&[u8]; 12] = [
    b"upper", b"lower", b"alpha", b"digit", b"xdigit", b"space", b"print", b"graph", b"blank",
    b"cntrl", b"punct", b"alnum",
];

#[derive(Clone, Copy)]
pub(super) struct Data<'a> {
    bytes: &'a [u8],
}
fn word(bytes: &[u8], at: usize) -> Result<u32, i32> {
    let end = at.checked_add(4).ok_or(EINVAL)?;
    let value = bytes.get(at..end).ok_or(EINVAL)?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}
impl<'a> Data<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, i32> {
        if word(bytes, 0)? != 0x2009_0720 {
            return Err(EINVAL);
        }
        let count = word(bytes, 4)? as usize;
        if count < 19 || count > (bytes.len().saturating_sub(8)) / 4 {
            return Err(EINVAL);
        }
        let data = Self { bytes };
        for index in 0..count {
            let offset = word(bytes, 8 + index * 4)? as usize;
            if offset < 8 + count * 4 || offset >= bytes.len() {
                return Err(EINVAL);
            }
        }
        if !data.item(14)?.starts_with(b"UTF-8\0") {
            return Err(EINVAL);
        }
        let names: Vec<_> = data
            .item(10)?
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        if !names.starts_with(&CLASSES) {
            return Err(EINVAL);
        }
        if !data.item(11)?.starts_with(b"toupper\0tolower\0") {
            return Err(EINVAL);
        }
        for class in 0..CLASSES.len() {
            Table::parse(data.class(class)?, Kind::Bits)?;
        }
        for upper in [true, false] {
            Table::parse(data.mapping(upper)?, Kind::Delta)?;
        }
        Table::parse(data.item(12)?, Kind::Width)?;
        Ok(data)
    }
    fn item(self, index: usize) -> Result<&'a [u8], i32> {
        let count = word(self.bytes, 4)? as usize;
        if index >= count {
            return Err(EINVAL);
        }
        let start = word(self.bytes, 8 + index * 4)? as usize;
        let mut end = self.bytes.len();
        // Empty or shared items can have the same offset. Select the next
        // physical boundary rather than assuming the directory is sorted.
        for i in 0..count {
            let offset = word(self.bytes, 8 + i * 4)? as usize;
            if offset > start {
                end = end.min(offset);
            }
        }
        self.bytes.get(start..end).ok_or(EINVAL)
    }
    fn class(self, index: usize) -> Result<&'a [u8], i32> {
        let base = word(self.item(17)?, 0)? as usize;
        self.item(base.checked_add(index).ok_or(EINVAL)?)
    }
    fn mapping(self, upper: bool) -> Result<&'a [u8], i32> {
        let base = word(self.item(18)?, 0)? as usize;
        self.item(base.checked_add(usize::from(!upper)).ok_or(EINVAL)?)
    }
    pub fn classify(self, scalar: u32, class: usize) -> bool {
        let base = self.raw_item(17).and_then(|item| word(item, 0)).unwrap() as usize;
        self.raw_item(base + class)
            .ok()
            .and_then(|table| Table(table).lookup(scalar, Kind::Bits))
            .unwrap_or(0)
            != 0
    }
    pub fn map(self, scalar: u32, upper: bool) -> u32 {
        let base = self.raw_item(18).and_then(|item| word(item, 0)).unwrap() as usize;
        let delta = self
            .raw_item(base + usize::from(!upper))
            .ok()
            .and_then(|table| Table(table).lookup(scalar, Kind::Delta))
            .unwrap_or(0);
        scalar.wrapping_add(delta)
    }
    pub fn width(self, scalar: u32) -> i32 {
        match self
            .raw_item(12)
            .ok()
            .and_then(|table| Table(table).lookup(scalar, Kind::Width))
        {
            Some(value) if value != 255 => value as i32,
            _ => -1,
        }
    }
    fn raw_item(self, index: usize) -> Result<&'a [u8], i32> {
        if index >= word(self.bytes, 4)? as usize {
            return Err(EINVAL);
        }
        let start = word(self.bytes, 8 + index * 4)? as usize;
        self.bytes.get(start..).ok_or(EINVAL)
    }
    /// The owner validates the immutable buffer once before publishing it.
    pub unsafe fn validated(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Bits,
    Delta,
    Width,
}
struct Table<'a>(&'a [u8]);
impl<'a> Table<'a> {
    fn parse(bytes: &'a [u8], kind: Kind) -> Result<Self, i32> {
        let shift = word(bytes, 0)?;
        let count = word(bytes, 4)? as usize;
        let second_shift = word(bytes, 8)?;
        let second_mask = word(bytes, 12)? as usize;
        let last_mask = word(bytes, 16)? as usize;
        let tail_shift = if matches!(kind, Kind::Bits) { 5 } else { 0 };
        if shift >= 32
            || second_shift > shift
            || second_shift < tail_shift
            || second_mask as u64 + 1 != 1u64 << (shift - second_shift)
            || last_mask as u64 + 1 != 1u64 << (second_shift - tail_shift)
            || count > (bytes.len().saturating_sub(20)) / 4
        {
            return Err(EINVAL);
        }
        let stride = if matches!(kind, Kind::Width) { 1 } else { 4 };
        for first in 0..count {
            let offset = word(bytes, 20 + first * 4)? as usize;
            if offset == 0 {
                continue;
            }
            let end = offset
                .checked_add((second_mask + 1).checked_mul(4).ok_or(EINVAL)?)
                .ok_or(EINVAL)?;
            if offset % 4 != 0 || end > bytes.len() {
                return Err(EINVAL);
            }
            for second in 0..=second_mask {
                let last = word(bytes, offset + second * 4)? as usize;
                if last == 0 {
                    continue;
                }
                let end = last
                    .checked_add((last_mask + 1).checked_mul(stride).ok_or(EINVAL)?)
                    .ok_or(EINVAL)?;
                if last % stride != 0 || end > bytes.len() {
                    return Err(EINVAL);
                }
            }
        }
        Ok(Self(bytes))
    }
    fn lookup(&self, scalar: u32, kind: Kind) -> Option<u32> {
        let shift = word(self.0, 0).ok()?;
        let first = scalar.checked_shr(shift)?;
        if first >= word(self.0, 4).ok()? {
            return None;
        }
        let second = word(self.0, 20 + first as usize * 4).ok()? as usize;
        if second == 0 {
            return None;
        }
        let index = scalar.checked_shr(word(self.0, 8).ok()?)? & word(self.0, 12).ok()?;
        let last = word(self.0, second + index as usize * 4).ok()? as usize;
        if last == 0 {
            return None;
        }
        let shift = if matches!(kind, Kind::Bits) { 5 } else { 0 };
        let index = (scalar >> shift) & word(self.0, 16).ok()?;
        match kind {
            Kind::Bits => {
                Some((word(self.0, last + index as usize * 4).ok()? >> (scalar & 31)) & 1)
            }
            Kind::Delta => word(self.0, last + index as usize * 4).ok(),
            Kind::Width => self.0.get(last + index as usize).copied().map(u32::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    #[test]
    fn directory_rejects_bad_magic_counts_and_offsets() {
        for data in [
            vec![],
            words(&[0, 19]),
            words(&[0x20090720, u32::MAX]),
            words(&[0x20090720, 19]),
        ] {
            assert!(Data::parse(&data).is_err());
        }
        let mut data = words(&[0x20090720, 19]);
        data.resize(128, 0);
        assert!(Data::parse(&data).is_err());
        for i in 0..19 {
            data[8 + i * 4..12 + i * 4].copy_from_slice(&128u32.to_le_bytes());
        }
        assert!(Data::parse(&data).is_err());
    }
    #[test]
    fn sparse_tables_bound_unicode_and_preserve_signed_deltas() {
        let bits = words(&[5, 1, 5, 0, 0, 24, 28, 1 << 17]);
        let t = Table::parse(&bits, Kind::Bits).unwrap();
        assert_eq!(t.lookup(17, Kind::Bits), Some(1));
        assert_eq!(t.lookup(16, Kind::Bits), Some(0));
        assert_eq!(t.lookup(32, Kind::Bits), None);
        let mapping = words(&[0, 1, 0, 0, 0, 24, 28, (-32i32) as u32]);
        let t = Table::parse(&mapping, Kind::Delta).unwrap();
        assert_eq!(t.lookup(0, Kind::Delta), Some((-32i32) as u32));
        let mut width = words(&[0, 1, 0, 0, 0, 24, 28]);
        width.push(2);
        assert_eq!(
            Table::parse(&width, Kind::Width)
                .unwrap()
                .lookup(0, Kind::Width),
            Some(2)
        );
    }
    #[test]
    fn table_rejects_every_truncation_invalid_shift_and_unaligned_offset() {
        let valid = words(&[5, 1, 5, 0, 0, 24, 28, 0]);
        for len in 0..valid.len() {
            assert!(Table::parse(&valid[..len], Kind::Bits).is_err(), "{len}");
        }
        for (field, value) in [
            (0, 32),
            (1, u32::MAX),
            (2, 6),
            (3, 1),
            (4, 1),
            (5, 25),
            (6, 29),
            (6, u32::MAX),
        ] {
            let mut bad = valid.clone();
            bad[field * 4..field * 4 + 4].copy_from_slice(&value.to_le_bytes());
            assert!(Table::parse(&bad, Kind::Bits).is_err(), "{field}/{value}");
        }
    }
}
