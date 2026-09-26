//! POSIX word expansion through the guest shell. Validate the word-list grammar
//! before evaluation, including command substitutions nested in arithmetic or
//! parameter expressions. Result vectors and strings belong to the guest heap.
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use kinakaze_alloc::guest;
use std::ffi::CString;
const DOOFFS: i32 = 1;
const APPEND: i32 = 2;
const NOCMD: i32 = 4;
const REUSE: i32 = 8;
const SHOWERR: i32 = 16;
const UNDEF: i32 = 32;
const NOSPACE: i32 = 1;
const BADCHAR: i32 = 2;
const BADVAL: i32 = 3;
const CMDSUB: i32 = 4;
const SYNTAX: i32 = 5;
#[repr(C)]
pub struct Wordexp {
    count: usize,
    words: *mut *mut c_char,
    offset: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Words,
    Single,
    Double,
    Parameter,
    Command,
    Backtick,
    Arithmetic,
    Parenthesis,
}

fn validate(input: &[u8], flags: i32) -> Result<Vec<u8>, i32> {
    let mut stack = vec![Scope::Words];
    let mut output = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let ch = input[at];
        let scope = *stack.last().unwrap();
        if scope == Scope::Single {
            output.push(ch);
            at += 1;
            if ch == b'\'' {
                stack.pop();
            }
            continue;
        }
        if ch == b'\\' {
            let next = *input.get(at + 1).ok_or(SYNTAX)?;
            output.extend_from_slice(&[ch, next]);
            at += 2;
            continue;
        }
        if (scope == Scope::Double && ch == b'"') || (scope == Scope::Backtick && ch == b'`') {
            stack.pop();
            output.push(ch);
            at += 1;
            continue;
        }
        if ch == b'`' {
            if flags & NOCMD != 0 {
                return Err(CMDSUB);
            }
            stack.push(Scope::Backtick);
        } else if ch == b'$' && input.get(at + 1) == Some(&b'(') {
            if input.get(at + 2) == Some(&b'(') {
                stack.push(Scope::Arithmetic);
                output.extend_from_slice(b"$((");
                at += 3;
                continue;
            }
            if flags & NOCMD != 0 {
                return Err(CMDSUB);
            }
            stack.push(Scope::Command);
            output.extend_from_slice(b"$(");
            at += 2;
            continue;
        } else if ch == b'$' && input.get(at + 1) == Some(&b'{') {
            stack.push(Scope::Parameter);
            output.extend_from_slice(b"${");
            at += 2;
            continue;
        } else if scope != Scope::Double {
            match ch {
                b'\'' => stack.push(Scope::Single),
                b'"' => stack.push(Scope::Double),
                b'}' if scope == Scope::Parameter => {
                    stack.pop();
                }
                b'(' if matches!(scope, Scope::Arithmetic | Scope::Parenthesis) => {
                    stack.push(Scope::Parenthesis)
                }
                b')' if scope == Scope::Parenthesis => {
                    stack.pop();
                }
                b')' if scope == Scope::Arithmetic => {
                    if input.get(at + 1) != Some(&b')') {
                        return Err(SYNTAX);
                    }
                    stack.pop();
                    output.extend_from_slice(b"))");
                    at += 2;
                    continue;
                }
                b'(' if scope == Scope::Command => stack.push(Scope::Command),
                b')' if scope == Scope::Command => {
                    stack.pop();
                }
                b'\n' | b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'{' | b'}'
                    if scope == Scope::Words =>
                {
                    return Err(BADCHAR);
                }
                // In wordexp, # is an ordinary character rather than a shell
                // comment introducing another interpretation of the word list.
                b'#' if scope == Scope::Words => output.push(b'\\'),
                _ => {}
            }
        }
        output.push(ch);
        at += 1;
    }
    if stack.len() != 1 {
        return Err(SYNTAX);
    }
    Ok(output)
}

fn quote(bytes: &[u8], output: &mut Vec<u8>) {
    output.push(b'\'');
    for &byte in bytes {
        if byte == b'\'' {
            output.extend_from_slice(b"'\\''");
        } else {
            output.push(byte);
        }
    }
    output.push(b'\'');
}

unsafe fn expand(words: &[u8], flags: i32) -> Result<Vec<Vec<u8>>, i32> {
    let mut command = Vec::new();
    if flags & UNDEF != 0 {
        command.extend_from_slice(b"set -u\n");
    }
    let ifs = unsafe { crate::process::kinakaze_abi_getenv(c"IFS".as_ptr()) };
    if !ifs.is_null() {
        command.extend_from_slice(b"IFS=");
        quote(unsafe { CStr::from_ptr(ifs) }.to_bytes(), &mut command);
        command.push(b'\n');
    }
    command.extend_from_slice(b"set -- ");
    command.extend_from_slice(words);
    command.extend_from_slice(b"\nfor kinakaze_word do printf '%s\\0' \"$kinakaze_word\"; done\n");
    let command = CString::new(command).map_err(|_| SYNTAX)?;
    let bytes = unsafe { capture(&command, flags) }?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.last() != Some(&0) {
        return Err(SYNTAX);
    }
    Ok(bytes[..bytes.len() - 1]
        .split(|&b| b == 0)
        .map(Vec::from)
        .collect())
}

// Spawn copies arguments into a fresh guest image. A native Rust frame and its
// allocations must never be resumed in the guest fork child used by popen.
unsafe fn capture(command: &CStr, flags: i32) -> Result<Vec<u8>, i32> {
    use crate::exec::spawn::*;
    let mut pipe = [-1; 2];
    if unsafe { crate::fdio::kinakaze_abi_pipe2(pipe.as_mut_ptr(), 0o2000000) } < 0 {
        return Err(NOSPACE);
    }
    // No file actions: the managed spawn backend can enter a fresh ELF worker.
    // Redirection is performed by the guest shell, with no parent stdio changes.
    // The write end is inherited only until the shell finishes this expansion.
    if let Err(error) = kinakaze_vfs::set_close_on_exec(pipe[1], false) {
        crate::kinakaze_abi_close(pipe[0]);
        crate::kinakaze_abi_close(pipe[1]);
        crate::set_errno(error);
        return Err(NOSPACE);
    }
    let mut wrapper = format!("exec 1>/proc/self/fd/{}\n", pipe[1]);
    if flags & SHOWERR == 0 {
        wrapper.push_str("exec 2>/dev/null\n");
    }
    wrapper.push_str("eval \"$1\"");
    let wrapper = CString::new(wrapper).unwrap();
    let argv = [
        c"sh".as_ptr(),
        c"-c".as_ptr(),
        wrapper.as_ptr(),
        c"wordexp".as_ptr(),
        command.as_ptr(),
        ptr::null(),
    ];
    let mut child = 0;
    let error = unsafe {
        kinakaze_abi_posix_spawn(
            &mut child,
            c"/bin/sh".as_ptr(),
            ptr::null(),
            ptr::null(),
            argv.as_ptr(),
            crate::process::kinakaze_abi_environ.get().cast(),
        )
    };
    crate::kinakaze_abi_close(pipe[1]);
    if error != 0 {
        crate::kinakaze_abi_close(pipe[0]);
        crate::set_errno(error);
        return Err(NOSPACE);
    }
    let mut bytes = Vec::new();
    let mut block = [0u8; 4096];
    let mut read_error = None;
    loop {
        let count =
            unsafe { crate::kinakaze_abi_read(pipe[0], block.as_mut_ptr().cast(), block.len()) };
        if count > 0 {
            bytes.extend_from_slice(&block[..count as usize]);
        } else if count == 0 {
            break;
        } else if kinakaze_tls::errno() != kinakaze_vfs::EINTR {
            read_error = Some(kinakaze_tls::errno());
            crate::signal::kinakaze_abi_kill(child, kinakaze_vfs::signal::SIGKILL);
            break;
        }
    }
    crate::kinakaze_abi_close(pipe[0]);
    let mut status = 0;
    let waited = loop {
        let result = unsafe { crate::exec::kinakaze_abi_waitpid(child, &mut status, 0) };
        if result >= 0 || kinakaze_tls::errno() != kinakaze_vfs::EINTR {
            break result;
        }
    };
    if let Some(error) = read_error {
        crate::set_errno(error);
        return Err(NOSPACE);
    }
    if waited < 0 {
        return Err(NOSPACE);
    }
    if status != 0 {
        return Err(if flags & UNDEF != 0 { BADVAL } else { SYNTAX });
    }
    Ok(bytes)
}

/// # Safety
/// The record comes from wordexp and has not been freed, or has a null vector.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wordfree(record: *mut Wordexp) {
    if record.is_null() {
        return;
    }
    let record = unsafe { &mut *record };
    if !record.words.is_null() {
        for n in 0..record.count {
            unsafe {
                guest::free((*record.words.add(record.offset + n)).cast());
            }
        }
        unsafe {
            guest::free(record.words.cast());
        }
    }
    record.words = ptr::null_mut();
    record.count = 0;
}

/// # Safety
/// words is NUL terminated, record is writable; APPEND/REUSE require a result
/// previously initialized by wordexp, with consistent DOOFFS usage.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wordexp(
    words: *const c_char,
    record: *mut Wordexp,
    flags: c_int,
) -> c_int {
    if words.is_null() || record.is_null() || flags & !63 != 0 {
        return SYNTAX;
    }
    if flags & REUSE != 0 {
        unsafe {
            kinakaze_abi_wordfree(record);
        }
    }
    let record = unsafe { &mut *record };
    if flags & APPEND == 0 {
        record.count = 0;
        record.words = ptr::null_mut();
    }
    if flags & DOOFFS == 0 {
        record.offset = 0;
    }
    let result = (|| {
        let validated = validate(unsafe { CStr::from_ptr(words) }.to_bytes(), flags)?;
        let expanded = unsafe { expand(&validated, flags) }?;
        let total = record.count.checked_add(expanded.len()).ok_or(NOSPACE)?;
        let slots = record
            .offset
            .checked_add(total)
            .and_then(|n| n.checked_add(1))
            .ok_or(NOSPACE)?;
        let size = slots.checked_mul(size_of::<*mut c_char>()).ok_or(NOSPACE)?;
        let vector = unsafe { guest::malloc(size).cast::<*mut c_char>() };
        if vector.is_null() {
            return Err(NOSPACE);
        }
        unsafe {
            ptr::write_bytes(vector, 0, slots);
        }
        if !record.words.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(record.words, vector, record.offset + record.count);
            }
        }
        for (i, word) in expanded.iter().enumerate() {
            let value = unsafe { guest::malloc(word.len() + 1) };
            if value.is_null() {
                unsafe {
                    for n in 0..i {
                        guest::free((*vector.add(record.offset + record.count + n)).cast());
                    }
                    guest::free(vector.cast());
                }
                return Err(NOSPACE);
            }
            unsafe {
                ptr::copy_nonoverlapping(word.as_ptr(), value, word.len());
                value.add(word.len()).write(0);
                vector
                    .add(record.offset + record.count + i)
                    .write(value.cast());
            }
        }
        unsafe {
            guest::free(record.words.cast());
        }
        record.words = vector;
        record.count = total;
        Ok(())
    })();
    result.err().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_execution_outside_substitutions_and_nested_nocmd() {
        for input in [
            b"ok; echo bad".as_slice(),
            b"x\ny",
            b"x > file",
            b"(echo bad)",
        ] {
            assert_eq!(validate(input, 0), Err(BADCHAR));
        }
        for input in [
            b"$(id)".as_slice(),
            b"\"`id`\"",
            b"$((1+$(id)))",
            b"${x:-$(id)}",
        ] {
            assert_eq!(validate(input, NOCMD), Err(CMDSUB));
        }
        assert!(validate(b"'$(id)' \"$((1+(2)))\"", NOCMD).is_ok());
        assert_eq!(validate(b"'unterminated", 0), Err(SYNTAX));
        assert!(validate(b"$(printf '%s' 'x;y')", 0).is_ok());
    }
}
