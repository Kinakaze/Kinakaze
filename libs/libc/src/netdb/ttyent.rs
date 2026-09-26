//! BSD/glibc tty database, read from the guest's /etc/ttys.
use super::records;
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::io::BufRead;
const PATH: &str = "/etc/ttys";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYTTYC01");
}
super::records::returned::returned_record!(*b"CYTTYR01");

#[repr(C)]
pub struct Ttyent {
    name: *mut c_char,
    getty: *mut c_char,
    kind: *mut c_char,
    status: c_int,
    window: *mut c_char,
    comment: *mut c_char,
}
struct Entry {
    strings: [Vec<u8>; 5],
    status: i32,
}

fn parse(line: &[u8]) -> Option<Entry> {
    if line.contains(&0) {
        return None;
    }
    let mut fields = Vec::new();
    let mut at = 0;
    let mut comment = Vec::new();
    while at < line.len() {
        if line[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if line[at] == b'#' {
            comment = line[at + 1..].trim_ascii().to_vec();
            break;
        }
        let mut quoted = false;
        let mut word = Vec::new();
        while at < line.len() {
            let ch = line[at];
            if !quoted && (ch.is_ascii_whitespace() || ch == b'#') {
                break;
            }
            at += 1;
            if ch == b'"' {
                quoted = !quoted;
            } else if ch == b'\\' && line.get(at) == Some(&b'"') {
                word.push(b'"');
                at += 1;
            } else {
                word.push(ch);
            }
        }
        if quoted {
            return None;
        }
        fields.push(word);
    }
    if fields.len() < 3 {
        return None;
    }
    let mut status = 0;
    let mut window = Vec::new();
    for value in &fields[3..] {
        match value.as_slice() {
            b"on" => status |= 1,
            b"off" => status &= !1,
            b"secure" => status |= 2,
            value if value.starts_with(b"window=") => window = value[7..].to_vec(),
            _ => {}
        }
    }
    Some(Entry {
        strings: [
            fields[0].clone(),
            fields[1].clone(),
            fields[2].clone(),
            window,
            comment,
        ],
        status,
    })
}
fn next(reader: &mut impl BufRead) -> Result<Option<Entry>, i32> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.raw_os_error().unwrap_or(5))?
            == 0
        {
            return Ok(None);
        }
        if let Some(entry) = parse(&line) {
            return Ok(Some(entry));
        }
    }
}
fn publish(entry: Entry) -> *mut Ttyent {
    if let Err(error) = register_returned() {
        crate::set_errno(error);
        return ptr::null_mut();
    }
    let size = core::mem::size_of::<Ttyent>();
    let bytes = unsafe {
        kinakaze_alloc::guest::malloc(
            size + entry.strings.iter().map(|s| s.len() + 1).sum::<usize>(),
        )
    };
    if bytes.is_null() {
        crate::set_errno(12);
        return ptr::null_mut();
    }
    let mut fields = [ptr::null_mut(); 5];
    let output = bytes.cast::<Ttyent>();
    unsafe {
        let mut data = bytes.add(size);
        for (i, s) in entry.strings.iter().enumerate() {
            ptr::copy_nonoverlapping(s.as_ptr(), data, s.len());
            data.add(s.len()).write(0);
            if i < 3 || !s.is_empty() {
                fields[i] = data.cast();
            }
            data = data.add(s.len() + 1);
        }
        output.write(Ttyent {
            name: fields[0],
            getty: fields[1],
            kind: fields[2],
            status: entry.status,
            window: fields[3],
            comment: fields[4],
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(records::returned::Record(bytes as usize)));
    output
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setttyent() -> c_int {
    match cursor::rewind() {
        Ok(()) => 1,
        Err(e) => {
            crate::set_errno(e);
            0
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endttyent() -> c_int {
    match cursor::close() {
        Ok(()) => 1,
        Err(e) => {
            crate::set_errno(e);
            0
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getttyent() -> *mut Ttyent {
    match cursor::next(next) {
        Ok(Some(entry)) => publish(entry),
        Ok(None) => ptr::null_mut(),
        Err(error) => {
            crate::set_errno(error);
            ptr::null_mut()
        }
    }
}
/// # Safety
/// `name` is a readable NUL-terminated tty name (without /dev/).
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getttynam(name: *const c_char) -> *mut Ttyent {
    if name.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    if kinakaze_abi_setttyent() == 0 {
        return ptr::null_mut();
    }
    let wanted = unsafe { CStr::from_ptr(name) }.to_bytes();
    let found = loop {
        match cursor::next(next) {
            Ok(Some(entry)) if entry.strings[0] == wanted => break publish(entry),
            Ok(Some(_)) => {}
            Ok(None) => break ptr::null_mut(),
            Err(error) => {
                crate::set_errno(error);
                break ptr::null_mut();
            }
        }
    };
    kinakaze_abi_endttyent();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_command_options_and_comment() {
        let entry =
            parse(br#"tty1 "/sbin/getty -L" vt100 off secure on window="/bin/x -n" # console"#)
                .unwrap();
        assert_eq!(entry.status, 3);
        assert_eq!(entry.strings[1], b"/sbin/getty -L");
        assert_eq!(entry.strings[3], b"/bin/x -n");
        assert_eq!(entry.strings[4], b"console");
        assert!(parse(b"# no record").is_none());
        assert!(parse(b"tty1 \"unterminated").is_none());
    }
}
