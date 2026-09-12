//! FILE control blocks belong to guest memory; buffers and locks belong here.
//! Fork transfers values and live bytes, never a Vec/Box or a native mutex.
use super::{Buffering, File, Stream, cookie, open_streams, standard};
use core::{cell::RefCell, ptr};
use std::sync::{Mutex, MutexGuard};

const MAGIC: u64 = u64::from_le_bytes(*b"CYSTDIO4");
struct Frozen {
    // Drop stream locks before allowing registry changes again.
    _streams: Vec<MutexGuard<'static, Stream>>,
    _registry: MutexGuard<'static, Vec<usize>>,
    bytes: Vec<u8>,
}
thread_local! {
    static FROZEN: RefCell<Option<Frozen>> = const { RefCell::new(None) };
}
fn word(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn blob(bytes: &mut Vec<u8>, value: &[u8]) {
    word(bytes, value.len() as u64);
    bytes.extend_from_slice(value);
}
fn encode(bytes: &mut Vec<u8>, key: usize, stream: &Stream) {
    word(bytes, key as u64);
    word(bytes, stream.fd as i64 as u64);
    let buffering = match stream.buffering {
        Buffering::None => 0,
        Buffering::Line => 1,
        Buffering::Full => 2,
        Buffering::Undecided => 3,
    };
    word(
        bytes,
        buffering
            | ((stream.eof as u64) << 2)
            | ((stream.error as u64) << 3)
            | (((stream.orientation + 1) as u64) << 4)
            | ((stream.wide_utf8 as u64) << 6)
            | ((stream.access as u64) << 7),
    );
    word(bytes, u64::from(stream.cookie.is_some()));
    if let Some(cookie) = &stream.cookie {
        word(bytes, cookie.context as u64);
        word(
            bytes,
            cookie.functions.read.map_or(0, |f| f as usize as u64),
        );
        word(
            bytes,
            cookie.functions.write.map_or(0, |f| f as usize as u64),
        );
        word(
            bytes,
            cookie.functions.seek.map_or(0, |f| f as usize as u64),
        );
        word(
            bytes,
            cookie.functions.close.map_or(0, |f| f as usize as u64),
        );
        word(
            bytes,
            u64::from(cookie.readable)
                | (u64::from(cookie.writable) << 1)
                | (u64::from(cookie.append) << 2),
        );
    }
    blob(bytes, &stream.buffer);
    blob(bytes, &stream.input[stream.input_pos..stream.input_end]);
    blob(bytes, &stream.pushback);
}

unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35; // EDEADLK: a fork transaction is already prepared here.
        }
        // A callback may open/close another stream while holding this stream's
        // lock. Never wait for that lock with its registry pinned: fail the
        // fork transaction instead of deadlocking or snapshotting mutable data.
        let Ok(registry) = open_streams().try_lock() else {
            return kinakaze_vfs::EAGAIN;
        };
        let mut streams = Vec::with_capacity(registry.len() + 3);
        let mut bytes = Vec::new();
        word(&mut bytes, MAGIC);
        word(&mut bytes, (registry.len() + 3) as u64);
        for (key, address) in (0..3)
            .map(|i| (i, standard(i) as usize))
            .chain(registry.iter().copied().map(|address| (address, address)))
        {
            // Live FILE allocations remain pinned for their process lifetime.
            let file = unsafe { &*(address as *const File) };
            let Ok(stream) = file.stream.try_lock() else {
                return kinakaze_vfs::EAGAIN;
            };
            encode(&mut bytes, key, &stream);
            streams.push(stream);
        }
        *slot = Some(Frozen {
            _streams: streams,
            _registry: registry,
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
unsafe extern "system" fn parent(_result: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn word(&mut self) -> Result<u64, i32> {
        let (word, rest) = self.0.split_at_checked(8).ok_or(kinakaze_vfs::EINVAL)?;
        self.0 = rest;
        Ok(u64::from_le_bytes(word.try_into().unwrap()))
    }
    fn blob(&mut self) -> Result<Vec<u8>, i32> {
        let count = self.word()? as usize;
        let (bytes, rest) = self.0.split_at_checked(count).ok_or(kinakaze_vfs::EINVAL)?;
        self.0 = rest;
        Ok(bytes.to_vec())
    }
    fn stream(&mut self) -> Result<Stream, i32> {
        let fd = i32::try_from(self.word()? as i64).map_err(|_| kinakaze_vfs::EINVAL)?;
        let flags = self.word()?;
        if flags & !511 != 0 || (flags >> 4) & 3 > 2 || (flags >> 7) & 3 > 2 {
            return Err(kinakaze_vfs::EINVAL);
        }
        let buffering = match flags & 3 {
            0 => Buffering::None,
            1 => Buffering::Line,
            2 => Buffering::Full,
            _ => Buffering::Undecided,
        };
        let mut stream = Stream::new(fd, buffering).with_access(((flags >> 7) & 3) as i32);
        stream.orientation = ((flags >> 4) & 3) as i32 - 1;
        stream.wide_utf8 = flags & 64 != 0;
        stream.eof = flags & 4 != 0;
        stream.error = flags & 8 != 0;
        match self.word()? {
            0 => (),
            1 => {
                if fd != -1 {
                    return Err(kinakaze_vfs::EINVAL);
                }
                let context = self.word()? as usize;
                // These are addresses supplied by this module's authenticated
                // fork transaction, after the loader restores guest code/maps.
                let functions = cookie::Functions {
                    read: unsafe { core::mem::transmute::<usize, _>(self.word()? as usize) },
                    write: unsafe { core::mem::transmute::<usize, _>(self.word()? as usize) },
                    seek: unsafe { core::mem::transmute::<usize, _>(self.word()? as usize) },
                    close: unsafe { core::mem::transmute::<usize, _>(self.word()? as usize) },
                };
                let mode = self.word()?;
                if mode & !7 != 0 {
                    return Err(kinakaze_vfs::EINVAL);
                }
                stream.cookie = Some(Box::new(cookie::Cookie {
                    context,
                    functions,
                    readable: mode & 1 != 0,
                    writable: mode & 2 != 0,
                    append: mode & 4 != 0,
                }));
            }
            _ => return Err(kinakaze_vfs::EINVAL),
        }
        stream.buffer = self.blob()?;
        stream.input = self.blob()?;
        stream.input_end = stream.input.len();
        stream.pushback = self.blob()?;
        Ok(stream)
    }
}
fn restore(bytes: &[u8]) -> Result<(), i32> {
    let mut reader = Reader(bytes);
    if reader.word()? != MAGIC {
        return Err(kinakaze_vfs::EINVAL);
    }
    let count = reader.word()? as usize;
    // Every record requires at least four words and three empty blob lengths.
    if count < 3 || count > reader.0.len() / 56 {
        return Err(kinakaze_vfs::EINVAL);
    }
    let mut decoded = Vec::with_capacity(count);
    let mut addresses = std::collections::BTreeSet::new();
    for index in 0..count {
        let key = reader.word()? as usize;
        if index < 3 {
            if key != index {
                return Err(kinakaze_vfs::EINVAL);
            }
        } else if key % core::mem::align_of::<File>() != 0
            || !kinakaze_alloc::guest::contains(key)
            || !key
                .checked_add(core::mem::size_of::<File>() - 1)
                .is_some_and(kinakaze_alloc::guest::contains)
            || !addresses.insert(key)
        {
            return Err(kinakaze_vfs::EINVAL);
        }
        decoded.push((key, reader.stream()?));
    }
    if !reader.0.is_empty() {
        return Err(kinakaze_vfs::EINVAL);
    }
    let mut registry = open_streams().lock().map_err(|_| kinakaze_vfs::EIO)?;
    registry.clear();
    registry.reserve(count - 3);
    for (key, stream) in decoded {
        if key < 3 {
            let file = unsafe { &*standard(key) };
            file.publish_indicators(&stream);
            file._mode
                .store(stream.orientation, core::sync::atomic::Ordering::Relaxed);
            *file.stream.lock().map_err(|_| kinakaze_vfs::EIO)? = stream;
        } else {
            // The copied control block contains parent's private Vec/Box and
            // locked mutex bytes. Overwrite without dropping those stale values.
            unsafe {
                ptr::addr_of_mut!((*(key as *mut File)).stream).write(Mutex::new(stream));
            }
            registry.push(key);
        }
    }
    Ok(())
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    restore(unsafe { core::slice::from_raw_parts(input, length) })
        .err()
        .unwrap_or(0)
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 450,
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
    fn roundtrip_keeps_only_live_bytes_and_rejects_truncation() {
        let mut stream = Stream::new(42, Buffering::Line);
        stream.buffer.extend_from_slice(b"pending");
        stream.input = vec![9; super::super::BUFSIZ];
        stream.input[2..5].copy_from_slice(b"abc");
        stream.input_pos = 2;
        stream.input_end = 5;
        stream.pushback.push(b'!');
        stream.eof = true;
        let mut bytes = Vec::new();
        encode(&mut bytes, 0, &stream);
        assert!(bytes.len() < 128);
        let mut reader = Reader(&bytes);
        assert_eq!(reader.word(), Ok(0));
        let copy = reader.stream().unwrap();
        assert_eq!(copy.buffer, b"pending");
        assert_eq!(copy.input, b"abc");
        assert_eq!(copy.pushback, b"!");
        assert!(copy.eof && copy.buffering == Buffering::Line);
        for end in 8..bytes.len() {
            assert!(Reader(&bytes[8..end]).stream().is_err());
        }
    }
}
