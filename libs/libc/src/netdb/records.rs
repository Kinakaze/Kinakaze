//! On-demand guest text databases. Lookup retains one line, never the whole file.
pub(super) mod cursor;
pub(crate) mod returned;
use std::ffi::CString;
use std::io::{BufRead, Read};

pub(super) struct Record<'a> {
    pub name: &'a [u8],
    pub value: u32,
    pub aliases: &'a [u8],
}

impl Record<'_> {
    pub fn matches(&self, name: &[u8], ignore_case: bool) -> bool {
        let equal = |candidate: &[u8]| {
            if ignore_case {
                candidate.eq_ignore_ascii_case(name)
            } else {
                candidate == name
            }
        };
        equal(self.name)
            || self
                .aliases
                .split(u8::is_ascii_whitespace)
                .filter(|alias| !alias.is_empty())
                .any(equal)
    }
}

/// The numeric parser is specific to each database; names remain byte strings.
pub(super) fn parse(line: &[u8], number: fn(&[u8]) -> Option<u32>) -> Option<Record<'_>> {
    let line = line.split(|&byte| byte == b'#').next()?;
    if line.contains(&0) {
        return None;
    }
    let mut rest = line;
    let name = field(&mut rest)?;
    let value = number(field(&mut rest)?)?;
    Some(Record {
        name,
        value,
        aliases: rest,
    })
}

pub(super) fn field<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    *rest = rest.trim_ascii_start();
    if rest.is_empty() {
        return None;
    }
    let end = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(rest.len());
    let (value, next) = rest.split_at(end);
    *rest = next;
    Some(value)
}

pub(super) struct Names {
    pub name: CString,
    // CString allocations stay fixed when this owner moves.
    _aliases: Vec<CString>,
}

impl Names {
    pub(super) fn new(name: CString, aliases: Vec<CString>) -> Self {
        Self {
            name,
            _aliases: aliases,
        }
    }
    pub fn from_record(record: &Record<'_>) -> Self {
        // parse() already rejected embedded NUL bytes.
        let name = CString::new(record.name).unwrap();
        let aliases: Vec<_> = record
            .aliases
            .split(u8::is_ascii_whitespace)
            .filter(|alias| !alias.is_empty())
            .map(|alias| CString::new(alias).unwrap())
            .collect();
        Self {
            name,
            _aliases: aliases,
        }
    }

    pub fn aliases(&self) -> impl ExactSizeIterator<Item = &CString> + Clone {
        self._aliases.iter()
    }
}

struct GuestFile(i32);
impl Read for GuestFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        kinakaze_vfs::read(self.0, buffer).map_err(std::io::Error::from_raw_os_error)
    }
}
impl Drop for GuestFile {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.0);
    }
}

/// A buffered guest FD whose unread bytes can be handed to a fork child.
/// The kernel file position remains shared; each process retains its own buffer.
pub(super) struct Reader {
    file: GuestFile,
    buffer: Vec<u8>,
    position: usize,
    end: usize,
    eof: bool,
}

impl Reader {
    pub const CAPACITY: usize = 8192;

    pub fn open(path: &str) -> Result<Self, i32> {
        let fd = kinakaze_vfs::fs::open(
            path,
            kinakaze_vfs::fs::O_RDONLY | kinakaze_vfs::fs::O_CLOEXEC,
            0,
        )?;
        Ok(Self::from_parts(fd, &[], false))
    }

    pub fn from_parts(fd: i32, unread: &[u8], eof: bool) -> Self {
        assert!(unread.len() <= Self::CAPACITY);
        let mut buffer = vec![0; Self::CAPACITY];
        buffer[..unread.len()].copy_from_slice(unread);
        Self {
            file: GuestFile(fd),
            buffer,
            position: 0,
            end: unread.len(),
            eof,
        }
    }

    pub fn fd(&self) -> i32 {
        self.file.0
    }
    pub fn unread(&self) -> &[u8] {
        &self.buffer[self.position..self.end]
    }
    pub fn eof(&self) -> bool {
        self.eof
    }

    pub fn rewind(&mut self) -> Result<(), i32> {
        kinakaze_vfs::fs::lseek(self.file.0, 0, kinakaze_vfs::fs::SEEK_SET)?;
        self.position = 0;
        self.end = 0;
        self.eof = false;
        Ok(())
    }

    /// An undersized caller buffer must not consume an enumeration record.
    pub fn retry_range<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, i32>,
    ) -> Result<T, i32> {
        let offset = i64::try_from(kinakaze_vfs::fs::lseek(
            self.file.0,
            0,
            kinakaze_vfs::fs::SEEK_CUR,
        )?)
        .map_err(|_| kinakaze_vfs::EOVERFLOW)?;
        let unread = self.unread().to_vec();
        let eof = self.eof;
        let result = operation(self);
        if matches!(&result, Err(error) if *error == kinakaze_vfs::ERANGE) {
            kinakaze_vfs::fs::lseek(self.file.0, offset, kinakaze_vfs::fs::SEEK_SET)?;
            self.buffer[..unread.len()].copy_from_slice(&unread);
            self.position = 0;
            self.end = unread.len();
            self.eof = eof;
        }
        result
    }
}

/// Pack names and an aligned, terminated alias table into caller-owned bytes.
pub(super) unsafe fn copy_names(
    names: &Names,
    buffer: *mut core::ffi::c_char,
    capacity: usize,
) -> Result<(*mut core::ffi::c_char, *mut *mut core::ffi::c_char), i32> {
    use core::{ffi::c_char, ptr};
    let padding = buffer.addr().wrapping_neg() & (core::mem::align_of::<*mut c_char>() - 1);
    let table = (names.aliases().len() + 1)
        .checked_mul(core::mem::size_of::<*mut c_char>())
        .and_then(|n| n.checked_add(padding))
        .ok_or(kinakaze_vfs::ERANGE)?;
    let strings = std::iter::once(&names.name).chain(names.aliases());
    let needed = strings
        .clone()
        .try_fold(table, |n, s| n.checked_add(s.as_bytes_with_nul().len()))
        .ok_or(kinakaze_vfs::ERANGE)?;
    if needed > capacity || buffer.is_null() {
        return Err(kinakaze_vfs::ERANGE);
    }
    let aliases = unsafe { buffer.byte_add(padding).cast::<*mut c_char>() };
    let mut at = table;
    for (index, text) in strings.enumerate() {
        let target = unsafe { buffer.byte_add(at) };
        unsafe { ptr::copy_nonoverlapping(text.as_ptr(), target, text.as_bytes_with_nul().len()) };
        if index > 0 {
            unsafe { aliases.add(index - 1).write(target) };
        }
        at += text.as_bytes_with_nul().len();
    }
    unsafe { aliases.add(names.aliases().len()).write(ptr::null_mut()) };
    Ok((unsafe { buffer.byte_add(table) }, aliases))
}

impl BufRead for Reader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.position == self.end && !self.eof {
            self.end = self.file.read(&mut self.buffer)?;
            self.position = 0;
            self.eof = self.end == 0;
        }
        Ok(self.unread())
    }
    fn consume(&mut self, amount: usize) {
        self.position += amount.min(self.end - self.position);
    }
}

impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let input = self.fill_buf()?;
        let length = input.len().min(output.len());
        output[..length].copy_from_slice(&input[..length]);
        self.consume(length);
        Ok(length)
    }
}

pub(super) fn lookup(
    path: &str,
    number: fn(&[u8]) -> Option<u32>,
    matches: impl Fn(&Record<'_>) -> bool,
) -> Result<Option<(u32, Names)>, i32> {
    find(&mut Reader::open(path)?, number, matches)
}

pub(super) fn find(
    reader: &mut impl BufRead,
    number: fn(&[u8]) -> Option<u32>,
    matches: impl Fn(&Record<'_>) -> bool,
) -> Result<Option<(u32, Names)>, i32> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.raw_os_error().unwrap_or(kinakaze_vfs::EIO))?
            == 0
        {
            return Ok(None);
        }
        if let Some(record) = parse(&line, number)
            && matches(&record)
        {
            return Ok(Some((record.value, Names::from_record(&record))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_lookup_skips_bad_records_and_keeps_byte_aliases() {
        let number = |text: &[u8]| std::str::from_utf8(text).ok()?.parse::<u32>().ok();
        assert!(!parse(b"named 3   ", number).unwrap().matches(b"", false));
        let mut input = std::io::Cursor::new(
            b"# comment\nbad xx\nzero\0 2\nfirst 3 ALIAS \xff # ignored\nlast 4\n",
        );
        let (value, names) = find(&mut input, number, |r| r.matches(b"alias", true))
            .unwrap()
            .unwrap();
        assert_eq!(value, 3);
        assert_eq!(names.name.as_bytes(), b"first");
        assert_eq!(names._aliases[1].as_bytes(), b"\xff");
        assert_eq!(names.aliases().count(), 2);
        assert_eq!(names.aliases().next().unwrap().as_bytes(), b"ALIAS");
        let (value, _) = find(&mut input, number, |r| r.matches(b"last", false))
            .unwrap()
            .unwrap();
        assert_eq!(value, 4);
        assert!(find(&mut input, number, |_| true).unwrap().is_none());
    }
}
