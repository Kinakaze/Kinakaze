//! Stream vectors retain one inode pin and one open-description transaction.
//! Data checks and native completion remain per segment; no gather buffer is
//! allocated and guest handlers run only after every pin/position lock unwinds.

use crate::{EBADF, EFAULT, ESPIPE, FdFlags, FdKind};

/// Transfer an already length-validated vector on a seekable native file.
///
/// # Safety
/// `item` returns stable vector metadata with buffers valid for the requested
/// direction. `limit` is at most Linux's MAX_RW_COUNT and the aggregate length.
pub unsafe fn transfer(
    fd: i32,
    count: usize,
    limit: usize,
    writing: bool,
    mut item: impl FnMut(usize) -> (*mut u8, usize),
) -> Result<usize, i32> {
    let mut size_limit_signal = false;
    let result = crate::ofd::with(fd, || {
        let (mut entry, pin) = crate::pin_native_fd(fd, |entry| {
            if entry.flags.contains(FdFlags::PATH_ONLY)
                || (writing
                    && entry.flags.contains(FdFlags::READ_ACCESS)
                    && !entry.flags.contains(FdFlags::WRITE_ACCESS))
                || (!writing
                    && entry.flags.contains(FdFlags::WRITE_ACCESS)
                    && !entry.flags.contains(FdFlags::READ_ACCESS))
            {
                return Err(EBADF);
            }
            if entry.kind != FdKind::File || !entry.flags.contains(FdFlags::SEEKABLE) {
                return Err(ESPIPE);
            }
            Ok(())
        })?;
        let mut total = 0usize;
        let mut outcome = Ok(0);
        for index in 0..count {
            if total == limit {
                break;
            }
            let (buffer, length) = item(index);
            let wanted = length.min(limit - total);
            if wanted == 0 {
                continue;
            }
            let result = if buffer.is_null() {
                Err(EFAULT)
            } else if writing {
                // Keep write-rights/verity checks and the existing append,
                // resource-limit, pending-I/O and overlay-error boundaries.
                crate::platform::write(&entry, unsafe {
                    core::slice::from_raw_parts(buffer, wanted)
                })
            } else {
                unsafe { pin.read_once(entry, buffer, wanted) }
            };
            match result {
                Ok(bytes) => {
                    total += bytes;
                    entry.offset = entry.offset.saturating_add(bytes as u64);
                    if bytes < wanted {
                        break;
                    }
                }
                Err(error) => {
                    size_limit_signal |= error == 27;
                    outcome = Err(error);
                    break;
                }
            }
        }
        if writing && entry.flags.contains(FdFlags::APPEND) && total != 0 {
            if let Ok(size) = crate::platform::file_size(entry) {
                crate::set_offset(fd, entry.generation, size);
            }
        } else {
            crate::advance_offset(fd, entry.generation, total);
        }
        // ofd::with dispatches/restarts only an operation that moved no bytes.
        // NativePin is retired before that dispatch, including on early errors.
        drop(pin);
        if total != 0 { Ok(total) } else { outcome }
    });
    // A later segment may hit RLIMIT_FSIZE after an earlier segment succeeded.
    // Preserve delivery even though the syscall returns its partial byte count.
    if size_limit_signal {
        crate::signal::deliver_pending();
    }
    result
}
