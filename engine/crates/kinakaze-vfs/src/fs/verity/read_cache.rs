//! Reuse an independently opened read capability, never cached verity answers.
use super::{EBADF, EIO, FILE_READ_DATA, GENERIC_READ, HANDLE, Object, QUERY_ACCESS};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct ReadCache {
    idle: Mutex<Option<Reader>>,
}
struct Reader {
    // Retire the named capability before releasing its last inode pin.
    mutex: Option<crate::xattr::InodeMutex>,
    object: Object,
    key: crate::xattr::InodeKey,
}

fn retain_mutex() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_READ_MUTEX").is_none_or(|v| v != "0"))
}

impl ReadCache {
    /// The caller pins this exact open description before close/slot reuse can
    /// intervene. An idle private handle has no pending operation or EA cursor.
    pub(crate) fn read(
        &self,
        original: HANDLE,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize, i32> {
        let idle = self.idle.lock().map_err(|_| EIO)?.take();
        let reader = match idle {
            Some(reader) => reader,
            None => {
                // A reopen must never turn a write-only imported descriptor
                // into a reader using the host identity's broader privileges.
                if Object::granted_access(original)? & FILE_READ_DATA == 0 {
                    return Err(EBADF);
                }
                let object = Object::reopen(original, GENERIC_READ | QUERY_ACCESS)?;
                let key = crate::xattr::InodeKey::from_handle(object.raw())?;
                let mutex = if retain_mutex() {
                    crate::xattr::InodeMutex::open(&key).ok()
                } else {
                    None
                };
                Reader { object, key, mutex }
            }
        };
        // Ownership is exclusive throughout all asynchronous I/O. Concurrent
        // pread operations open their own handle instead of waiting for this
        // cache or sharing a completion status. Cancellation retires only the
        // current request, before the handle can be returned to the cache.
        let result = if let Some(mutex) = &reader.mutex {
            let _lock = mutex.acquire()?;
            super::read_locked_object(&reader.object, offset, bytes)?.ok_or(EIO)
        } else {
            let _lock = reader.key.acquire()?;
            super::read_locked_object(&reader.object, offset, bytes)?.ok_or(EIO)
        };
        let mut idle = self.idle.lock().map_err(|_| EIO)?;
        if idle.is_none() {
            *idle = Some(reader);
        }
        result
    }
}
