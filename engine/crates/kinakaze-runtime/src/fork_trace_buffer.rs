//! Formatting while the arena/threads are frozen must not acquire stdio locks.
//! Keep bounded host-private bytes and flush only after both freezes end.
use std::{
    cell::Cell,
    fmt::{self, Write as _},
    io::Write as _,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};

const DISABLED: *mut Buffer = 1usize as *mut Buffer;
thread_local! { static ACTIVE: Cell<*mut Buffer> = const { Cell::new(std::ptr::null_mut()) }; }

#[repr(C)]
struct Buffer {
    len: usize,
    overflow: bool,
    bytes: [u8; 65536],
}
impl fmt::Write for Buffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let mut count = value.len().min(self.bytes.len() - self.len);
        while !value.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        self.overflow |= count != value.len();
        Ok(())
    }
}

pub(super) struct Guard {
    buffer: *mut Buffer,
    previous: *mut Buffer,
}
impl Guard {
    /// Declare this before allocator/thread guards, so early errors thaw first.
    pub fn new() -> Self {
        let buffer = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                std::mem::size_of::<Buffer>(),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .cast::<Buffer>();
        let previous =
            ACTIVE.with(|active| active.replace(if buffer.is_null() { DISABLED } else { buffer }));
        Self { buffer, previous }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.set(self.previous));
        if self.buffer.is_null() {
            return;
        }
        let buffer = unsafe { &*self.buffer };
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(&buffer.bytes[..buffer.len]);
        if buffer.overflow {
            let _ = stderr.write_all(b"kinakaze fork: frozen diagnostic buffer truncated\n");
        }
        unsafe {
            VirtualFree(self.buffer.cast(), 0, MEM_RELEASE);
        }
    }
}

pub(super) fn line(arguments: fmt::Arguments<'_>) {
    ACTIVE.with(|active| {
        let buffer = active.get();
        if buffer == DISABLED {
            return;
        }
        if let Some(buffer) = unsafe { buffer.as_mut() } {
            let _ = writeln!(buffer, "{arguments}");
        } else {
            eprintln!("{arguments}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_records_do_not_wait_for_a_siblings_stdio_lock() {
        let (held, received) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let sibling = std::thread::spawn(move || {
            let _lock = std::io::stderr().lock();
            held.send(()).unwrap();
            released.recv().unwrap();
        });
        received.recv().unwrap();
        let guard = Guard::new();
        assert!(!guard.buffer.is_null());
        line(format_args!("frozen diagnostic {}", 7));
        let buffer = unsafe { &*guard.buffer };
        assert_eq!(&buffer.bytes[..buffer.len], b"frozen diagnostic 7\n");
        release.send(()).unwrap();
        sibling.join().unwrap();
        drop(guard);
        ACTIVE.with(|active| assert!(active.get().is_null()));
    }
}
