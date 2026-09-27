//! Bounded reads into disjoint, unpublished image pages. Every request has its
//! own OVERLAPPED/event; cancellation retires it before releasing its buffer.
use crate::{EINTR, EIO, fs::object::Object};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicI32, Ordering};
use windows_sys::Win32::{
    Foundation::{ERROR_HANDLE_EOF, ERROR_IO_PENDING, GetLastError, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::ReadFile,
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects},
    },
};

fn event() -> Option<OwnedHandle> {
    let raw = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
    (!raw.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(raw) })
}

struct Cancellation {
    event: OwnedHandle,
    error: AtomicI32,
}

impl Cancellation {
    fn fail(&self, error: i32) {
        let _ = self
            .error
            .compare_exchange(0, error, Ordering::AcqRel, Ordering::Acquire);
        // Manual reset broadcasts the caller's one consumed interrupt to all
        // outstanding chunks, including a thread that has not started I/O yet.
        unsafe { SetEvent(self.event.as_raw_handle()) };
    }
}

/// None means the optional parallel machinery was unavailable. The caller can
/// refill the entire unpublished buffer serially after all threads have joined.
pub(crate) fn read(object: &Object, bytes: &mut [u8]) -> Option<Result<(), i32>> {
    if bytes.len() < 16 * 1024 * 1024 {
        return None;
    }
    let threads = std::thread::available_parallelism()
        .map_or(1, |v| v.get())
        .min(4);
    if threads < 2 {
        return None;
    }
    let interrupt = crate::interrupt::current() as usize;
    if interrupt == 0 {
        return Some(Err(EIO));
    }
    let cancellation = Cancellation {
        event: event()?,
        error: AtomicI32::new(0),
    };
    let handle = object.raw() as usize;
    let chunk_size = bytes.len().div_ceil(threads * 4096) * 4096;
    // Guest signals target the calling guest thread, not these temporary native
    // helpers. Each I/O waiter observes that same caller interrupt event.
    struct Waiter;
    impl Drop for Waiter {
        fn drop(&mut self) {
            crate::signal::unregister_waiter();
        }
    }
    crate::signal::register_waiter();
    let _waiter = Waiter;
    let mut unavailable = false;
    std::thread::scope(|scope| {
        let mut chunks = bytes.chunks_mut(chunk_size).enumerate();
        let (_, first) = chunks.next().unwrap();
        let mut workers = Vec::new();
        for (index, chunk) in chunks {
            let cancellation = &cancellation;
            match std::thread::Builder::new()
                .name("image-read".into())
                .spawn_scoped(scope, move || {
                    read_chunk(handle, index * chunk_size, chunk, interrupt, cancellation);
                }) {
                Ok(worker) => workers.push(worker),
                Err(_) => unavailable = true,
            }
        }
        read_chunk(handle, 0, first, interrupt, &cancellation);
        for worker in workers {
            if worker.join().is_err() {
                cancellation.fail(EIO);
            }
        }
    });
    match cancellation.error.load(Ordering::Acquire) {
        0 if unavailable => None,
        0 => Some(Ok(())),
        error => Some(Err(error)),
    }
}

fn read_chunk(
    handle: usize,
    offset: usize,
    bytes: &mut [u8],
    interrupt: usize,
    cancel: &Cancellation,
) {
    struct Unwind<'a>(&'a Cancellation, bool);
    impl Drop for Unwind<'_> {
        fn drop(&mut self) {
            if !self.1 {
                self.0.fail(EIO);
            }
        }
    }
    let mut unwind = Unwind(cancel, false);
    let result = (|| {
        let completion = event().ok_or(EIO)?;
        let mut done = 0;
        while done < bytes.len() {
            if cancel.error.load(Ordering::Acquire) != 0 {
                return Ok(());
            }
            let position = (offset as u64).checked_add(done as u64).ok_or(EIO)?;
            let amount = (bytes.len() - done).min(u32::MAX as usize) as u32;
            let mut request = OVERLAPPED::default();
            request.hEvent = completion.as_raw_handle();
            request.Anonymous.Anonymous.Offset = position as u32;
            request.Anonymous.Anonymous.OffsetHigh = (position >> 32) as u32;
            let raw = handle as HANDLE;
            if unsafe { ResetEvent(completion.as_raw_handle()) } == 0 {
                return Err(EIO);
            }
            // These slices do not overlap. The file is private to the image
            // reader, and each request/event remains live until completion.
            let started = unsafe {
                ReadFile(
                    raw,
                    bytes[done..].as_mut_ptr(),
                    amount,
                    core::ptr::null_mut(),
                    &raw mut request,
                )
            };
            if started == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(if error == ERROR_HANDLE_EOF {
                        EIO
                    } else {
                        crate::errno_from_win32(error)
                    });
                }
                let events = [
                    completion.as_raw_handle(),
                    cancel.event.as_raw_handle(),
                    interrupt as HANDLE,
                ];
                let waited = unsafe { WaitForMultipleObjects(3, events.as_ptr(), 0, INFINITE) };
                if waited != WAIT_OBJECT_0 {
                    cancel.fail(
                        if waited == WAIT_OBJECT_0 + 1 || waited == WAIT_OBJECT_0 + 2 {
                            EINTR
                        } else {
                            EIO
                        },
                    );
                    // Cancellation is a request, not completion. Always drain
                    // this exact OVERLAPPED before its stack/buffer can expire.
                    unsafe {
                        CancelIoEx(raw, &request);
                        let mut transferred = 0;
                        GetOverlappedResult(raw, &request, &mut transferred, 1);
                        if request.Internal == 0x103 {
                            // A failed wait must never unwind a live kernel
                            // request. These private handles cannot expire here.
                            std::process::abort();
                        }
                    }
                    return Ok(());
                }
            }
            let mut transferred = 0;
            if unsafe { GetOverlappedResult(raw, &request, &mut transferred, 0) } == 0 {
                return Err(crate::errno_from_win32(unsafe { GetLastError() }));
            }
            if transferred == 0 {
                return Err(EIO);
            }
            done += transferred as usize;
        }
        Ok(())
    })();
    if let Err(error) = result {
        cancel.fail(error);
    }
    unwind.1 = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_interrupt_cancels_and_retires_every_pending_chunk() {
        let (reader, writer) = crate::platform::create_overlapped_pipe_pair(4096).unwrap();
        let reader = Object::owned(reader as HANDLE).unwrap();
        let writer = Object::owned(writer as HANDLE).unwrap();
        let interrupt = crate::interrupt::current() as usize;
        assert_ne!(interrupt, 0);
        let cancel = Cancellation {
            event: event().unwrap(),
            error: AtomicI32::new(0),
        };
        let raw = reader.raw() as usize;
        let mut bytes = [0xcc; 512];
        std::thread::scope(|scope| {
            let workers: Vec<_> = bytes
                .chunks_mut(128)
                .map(|chunk| {
                    let cancel = &cancel;
                    scope.spawn(move || read_chunk(raw, 0, chunk, interrupt, cancel))
                })
                .collect();
            scope.spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                assert_ne!(unsafe { SetEvent(interrupt as HANDLE) }, 0);
            });
            for worker in workers {
                worker.join().unwrap();
            }
        });
        assert_eq!(cancel.error.load(Ordering::Acquire), EINTR);
        assert_eq!(bytes, [0xcc; 512]);
        // An abandoned earlier read would steal these bytes or continue using
        // an expired request/buffer. The fresh read must own all of them.
        assert_eq!(writer.write_at(0, b"after-cancel").unwrap(), 12);
        let mut next = [0; 12];
        assert_eq!(reader.read_at(0, &mut next).unwrap(), 12);
        assert_eq!(&next, b"after-cancel");
    }
}
