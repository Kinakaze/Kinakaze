//! The source handle is asynchronous and also owns a pending oplock. Use one
//! event per read; neither std's synchronous seek_read nor the file's shared
//! completion state can determine when our particular buffer is safe to free.
use super::*;

struct Pending<'a> {
    file: &'a File,
    _event: OwnedHandle,
    request: Box<UnsafeCell<OVERLAPPED>>,
    submitted: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.submitted {
            let request = self.request.get();
            let mut count = 0;
            unsafe {
                CancelIoEx(self.file.as_raw_handle(), request);
                GetOverlappedResult(self.file.as_raw_handle(), request, &mut count, 1);
                if (*request).Internal == 0x103 {
                    std::process::abort();
                }
            }
        }
    }
}

pub(super) fn at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
    read_with(file, bytes, offset, |_| {})
}

fn read_with(
    file: &File,
    bytes: &mut [u8],
    offset: u64,
    submitted: impl FnOnce(bool),
) -> io::Result<usize> {
    let event = unsafe { owned(CreateEventW(null(), 1, 0, null()))? };
    let mut request = OVERLAPPED::default();
    request.hEvent = event.as_raw_handle();
    request.Anonymous.Anonymous.Offset = offset as u32;
    request.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    let mut pending = Pending {
        file,
        _event: event,
        request: Box::new(UnsafeCell::new(request)),
        submitted: false,
    };
    let started = unsafe {
        ReadFile(
            file.as_raw_handle(),
            bytes.as_mut_ptr(),
            bytes.len().min(u32::MAX as usize) as u32,
            std::ptr::null_mut(),
            pending.request.get(),
        )
    };
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_HANDLE_EOF {
            return Ok(0);
        }
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }
    pending.submitted = true;
    submitted(started == 0);
    let mut count = 0;
    let result =
        unsafe { GetOverlappedResult(file.as_raw_handle(), pending.request.get(), &mut count, 1) };
    let error = io::Error::last_os_error();
    if unsafe { (*pending.request.get()).Internal } != 0x103 {
        pending.submitted = false;
    }
    if result == 0 {
        return Err(error);
    }
    Ok(count as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::Pipes::*;
    fn pipe() -> (File, File) {
        let name = std::ffi::OsString::from(format!(
            r"\\.\pipe\kinakaze-cache-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let name: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
        let server = unsafe {
            owned(CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                null(),
            ))
            .unwrap()
        };
        let writer = unsafe {
            owned(CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            ))
            .unwrap()
        };
        (File::from(server), File::from(writer))
    }
    #[test]
    fn pending_read_uses_its_own_completion_event() {
        let (file, mut writer) = pipe();
        let mut bytes = [0u8; 4];
        let count = read_with(&file, &mut bytes, 0, |pending| {
            assert!(pending);
            writer.write_all(b"data").unwrap();
        })
        .unwrap();
        assert_eq!(count, 4);
        assert_eq!(&bytes, b"data");
    }
    #[test]
    fn unwinding_cancels_and_retires_the_exact_pending_read() {
        let (file, mut writer) = pipe();
        let mut bytes = [0u8; 4];
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = read_with(&file, &mut bytes, 0, |pending| {
                assert!(pending);
                panic!("injected after submission");
            });
        }));
        assert!(result.is_err());
        writer.write_all(b"next").unwrap();
        assert_eq!(at(&file, &mut bytes, 0).unwrap(), 4);
        assert_eq!(&bytes, b"next");
    }
}
