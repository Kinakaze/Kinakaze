//! Transfer callback values only. Native Vec storage and synchronization are
//! rebuilt in the child; guest mappings and native module bases are restored
//! and checked by runtime before participant callbacks execute.
use super::{CxaHandler, ExitHandler, Handler, exit_handlers};
use core::{cell::RefCell, ffi::c_void, ptr};
use std::sync::MutexGuard;

const MAGIC: u64 = u64::from_le_bytes(*b"CYEXIT01");
struct Frozen {
    _handlers: MutexGuard<'static, Vec<Handler>>,
    bytes: Vec<u8>,
}
thread_local! {
    static FROZEN: RefCell<Option<Frozen>> = const { RefCell::new(None) };
}

fn encode(handlers: &[Handler]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + handlers.len() * 32);
    for word in [MAGIC, handlers.len() as u64] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for handler in handlers {
        let words = match *handler {
            Handler::Plain(function) => [0, function as usize as u64, 0, 0],
            Handler::Cxa {
                function,
                argument,
                dso,
            } => [1, function as usize as u64, argument as u64, dso as u64],
        };
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    bytes
}

fn decode(bytes: &[u8]) -> Result<Vec<Handler>, i32> {
    let Some((header, records)) = bytes.split_at_checked(16) else {
        return Err(kinakaze_vfs::EINVAL);
    };
    let word = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    let count = word(&header[8..]) as usize;
    if word(&header[..8]) != MAGIC || count.checked_mul(32) != Some(records.len()) {
        return Err(kinakaze_vfs::EINVAL);
    }
    let mut handlers = Vec::with_capacity(count);
    for record in records.chunks_exact(32) {
        let kind = word(&record[..8]);
        let function = word(&record[8..16]) as usize;
        let argument = word(&record[16..24]) as usize as *mut c_void;
        let dso = word(&record[24..]) as usize as *mut c_void;
        if function == 0 {
            return Err(kinakaze_vfs::EINVAL);
        }
        // The bytes are an internal parent snapshot, not guest input. The
        // registrar owns callback/argument validity until invocation.
        handlers.push(match kind {
            0 if argument.is_null() && dso.is_null() => {
                Handler::Plain(unsafe { core::mem::transmute::<usize, ExitHandler>(function) })
            }
            1 => Handler::Cxa {
                function: unsafe { core::mem::transmute::<usize, CxaHandler>(function) },
                argument,
                dso,
            },
            _ => return Err(kinakaze_vfs::EINVAL),
        });
    }
    Ok(handlers)
}

unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(handlers) = exit_handlers().try_lock() else {
            return kinakaze_vfs::EAGAIN;
        };
        let bytes = encode(&handlers);
        *slot = Some(Frozen {
            _handlers: handlers,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(frozen) = slot.as_ref() else {
            return -(kinakaze_vfs::EINVAL as isize);
        };
        if !output.is_null() {
            if capacity < frozen.bytes.len() {
                return -(kinakaze_vfs::EINVAL as isize);
            }
            unsafe {
                ptr::copy_nonoverlapping(frozen.bytes.as_ptr(), output, frozen.bytes.len());
            }
        }
        frozen.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length > isize::MAX as usize {
        return kinakaze_vfs::EINVAL;
    }
    let decoded = match decode(unsafe { core::slice::from_raw_parts(input, length) }) {
        Ok(handlers) => handlers,
        Err(error) => return error,
    };
    let Ok(mut handlers) = exit_handlers().lock() else {
        return kinakaze_vfs::EIO;
    };
    *handlers = decoded;
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 460,
        key: MAGIC,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_partial_invalid_and_trailing_records() {
        unsafe extern "sysv64" fn plain() {}
        unsafe extern "sysv64" fn cxa(_: *mut c_void) {}
        let bytes = encode(&[
            Handler::Plain(plain),
            Handler::Cxa {
                function: cxa,
                argument: 12usize as _,
                dso: 34usize as _,
            },
        ]);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err());
        }
        let mut invalid = bytes.clone();
        invalid.push(0);
        assert!(decode(&invalid).is_err());
        invalid = bytes.clone();
        invalid[16] = 2;
        assert!(decode(&invalid).is_err());
        invalid = bytes;
        invalid[24..32].fill(0);
        assert!(decode(&invalid).is_err());
    }
}
