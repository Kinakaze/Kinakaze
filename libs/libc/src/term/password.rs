//! getpass storage is guest-owned; terminal handles and locks stay native.
use crate::{set_errno, stdio};
use core::{ffi::c_char, ptr};
use kinakaze_vfs::{
    pty::{ECHO, ECHONL, ISIG, TCSAFLUSH, Termios},
    tty,
};
use std::sync::Mutex;
mod lifecycle;

#[derive(Default)]
struct Buffer {
    address: usize,
    capacity: usize,
}
static BUFFER: Mutex<Buffer> = Mutex::new(Buffer {
    address: 0,
    capacity: 0,
});
impl Buffer {
    fn clear(&self) {
        // Do not leave an older, longer password in reusable capacity.
        for offset in 0..self.capacity {
            unsafe {
                (self.address as *mut u8).add(offset).write_volatile(0);
            }
        }
    }
}
struct Terminal {
    stream: *mut stdio::File,
    owned: bool,
    original: Option<Termios>,
}
impl Terminal {
    fn restore(&mut self) -> Result<(), i32> {
        if let Some(original) = self.original.take() {
            tty::tcsetattr(stdio::fileno(self.stream), TCSAFLUSH, &original)?;
        }
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let error = unsafe { *crate::kinakaze___errno_location() };
        let _ = self.restore();
        if self.owned {
            unsafe {
                stdio::fclose(self.stream);
            }
        }
        set_errno(error);
    }
}

/// # Safety
/// `prompt` is a readable NUL-terminated string. The returned static storage is
/// invalidated by the next getpass call and must not be freed by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getpass(prompt: *const c_char) -> *mut c_char {
    if prompt.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    let Ok(mut buffer) = BUFFER.lock() else {
        set_errno(kinakaze_vfs::EIO);
        return ptr::null_mut();
    };
    buffer.clear();
    let opened = unsafe { stdio::fopen(c"/dev/tty".as_ptr(), c"r+e".as_ptr()) };
    // GNU specifies stdin/stderr when the process has no controlling terminal.
    let input = if opened.is_null() {
        stdio::exports::kinakaze_stdin()
    } else {
        opened
    };
    let output = if opened.is_null() {
        stdio::exports::kinakaze_stderr()
    } else {
        opened
    };
    let mut terminal = Terminal {
        stream: input,
        owned: !opened.is_null(),
        original: None,
    };
    if let Ok(original) = tty::tcgetattr(stdio::fileno(input)) {
        let mut hidden = original;
        hidden.c_lflag &= !(ECHO | ECHONL | ISIG);
        if let Err(error) = tty::tcsetattr(stdio::fileno(input), TCSAFLUSH, &hidden) {
            set_errno(error);
            return ptr::null_mut();
        }
        terminal.original = Some(original);
    }
    if unsafe { stdio::fputs(prompt, output) } < 0 || unsafe { stdio::fflush(output) } != 0 {
        return ptr::null_mut();
    }
    let mut line = buffer.address as *mut c_char;
    let mut capacity = buffer.capacity;
    let count =
        unsafe { crate::fdio::kinakaze_abi_getline(&raw mut line, &raw mut capacity, input) };
    buffer.address = line as usize;
    buffer.capacity = capacity;
    if count < 0 {
        buffer.clear();
        if stdio::ferror(input) != 0 {
            return ptr::null_mut();
        }
    } else if count > 0 && unsafe { *line.add(count as usize - 1) } == b'\n' as c_char {
        unsafe {
            *line.add(count as usize - 1) = 0;
        }
        if terminal.original.is_some() {
            unsafe {
                stdio::fputs(c"\n".as_ptr(), output);
                stdio::fflush(output);
            }
        }
    }
    if let Err(error) = terminal.restore() {
        buffer.clear();
        set_errno(error);
        return ptr::null_mut();
    }
    line
}
