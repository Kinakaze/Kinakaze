//! Walk-owned memory survives a guest callback which forks and then returns.
use core::ptr;
use kinakaze_alloc::guest;
use kinakaze_vfs::{ENOMEM, EOVERFLOW, fs};

pub(crate) struct Bytes {
    pub pointer: *mut u8,
    pub length: usize,
}
impl Bytes {
    pub fn zeroed(length: usize) -> Result<Self, i32> {
        let pointer = unsafe { guest::malloc(length.max(1)) };
        if pointer.is_null() {
            return Err(ENOMEM);
        }
        unsafe { ptr::write_bytes(pointer, 0, length) };
        Ok(Self { pointer, length })
    }
    pub fn text(value: &str) -> Result<Self, i32> {
        let bytes = Self::zeroed(value.len().checked_add(1).ok_or(EOVERFLOW)?)?;
        unsafe { ptr::copy_nonoverlapping(value.as_ptr(), bytes.pointer, value.len()) };
        Ok(bytes)
    }
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.pointer, self.length) }
    }
    pub fn as_str(&self) -> &str {
        // Only constructed from checked UTF-8 paths and VFS entry names.
        unsafe { core::str::from_utf8_unchecked(&self.as_slice()[..self.length - 1]) }
    }
    pub fn join(&self, name: &[u8]) -> Result<Self, i32> {
        let prefix = &self.as_slice()[..self.length - 1];
        let slash = usize::from(!prefix.ends_with(b"/"));
        let size = prefix
            .len()
            .checked_add(slash)
            .and_then(|n| n.checked_add(name.len() + 1))
            .ok_or(EOVERFLOW)?;
        let result = Self::zeroed(size)?;
        unsafe {
            ptr::copy_nonoverlapping(prefix.as_ptr(), result.pointer, prefix.len());
            if slash != 0 {
                result.pointer.add(prefix.len()).write(b'/');
            }
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                result.pointer.add(prefix.len() + slash),
                name.len(),
            );
        }
        Ok(result)
    }
    pub fn directory(path: &str) -> Result<Self, i32> {
        let fd = fs::open(path, fs::O_RDONLY | fs::O_DIRECTORY | fs::O_CLOEXEC, 0)?;
        let entries = fs::read_directory_fd(fd);
        let _ = kinakaze_vfs::close(fd);
        let entries = entries?;
        let names = entries.iter().filter(|e| e.name != "." && e.name != "..");
        let size = names
            .clone()
            .try_fold(0usize, |n, e| n.checked_add(e.name.len() + 1))
            .ok_or(EOVERFLOW)?;
        let result = Self::zeroed(size)?;
        let mut offset = 0;
        for entry in names {
            unsafe {
                ptr::copy_nonoverlapping(
                    entry.name.as_ptr(),
                    result.pointer.add(offset),
                    entry.name.len(),
                )
            };
            offset += entry.name.len() + 1;
        }
        // Native VFS objects are gone before any guest callback runs.
        Ok(result)
    }
}
impl Drop for Bytes {
    fn drop(&mut self) {
        unsafe { guest::free(self.pointer) };
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Entry {
    device: u64,
    inode: u64,
    used: bool,
}
#[derive(Default)]
pub(super) struct Seen {
    storage: Option<Bytes>,
    count: usize,
}
impl Seen {
    fn slot(device: u64, inode: u64, mask: usize) -> usize {
        // SplitMix64 finalizer; no process-local hash seed needs fork repair.
        let mut hash = inode ^ device.rotate_left(31);
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (hash ^ (hash >> 31)) as usize & mask
    }
    fn place(storage: &Bytes, entry: Entry) -> bool {
        let capacity = storage.length / size_of::<Entry>();
        let mut index = Self::slot(entry.device, entry.inode, capacity - 1);
        loop {
            let slot = unsafe { &mut *storage.pointer.cast::<Entry>().add(index) };
            if !slot.used {
                *slot = entry;
                return true;
            }
            if slot.device == entry.device && slot.inode == entry.inode {
                return false;
            }
            index = (index + 1) & (capacity - 1);
        }
    }
    pub fn insert(&mut self, device: u64, inode: u64) -> Result<bool, i32> {
        let capacity = self
            .storage
            .as_ref()
            .map_or(0, |s| s.length / size_of::<Entry>());
        if self.count >= capacity / 2 {
            let size = capacity
                .max(8)
                .checked_mul(2 * size_of::<Entry>())
                .ok_or(ENOMEM)?;
            let replacement = Bytes::zeroed(size)?;
            if let Some(old) = &self.storage {
                for i in 0..capacity {
                    let entry = unsafe { old.pointer.cast::<Entry>().add(i).read() };
                    if entry.used {
                        Self::place(&replacement, entry);
                    }
                }
            }
            self.storage = Some(replacement);
        }
        let inserted = Self::place(
            self.storage.as_ref().unwrap(),
            Entry {
                device,
                inode,
                used: true,
            },
        );
        self.count += usize::from(inserted);
        Ok(inserted)
    }
}
