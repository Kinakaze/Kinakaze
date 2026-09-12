//! Shared metadata for the two endpoints of an anonymous pipe. The section
//! lives exactly as long as its open descriptions, including dup/fork/exec.
use crate::{
    EBADF, EIO, FdEntry, FdKind,
    fs::{self, object::Object},
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{
    collections::HashMap,
    ptr,
    sync::{Arc, Mutex, OnceLock},
};
use windows_sys::Win32::Foundation::{
    GetLastError, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};

const MAGIC: u64 = u64::from_le_bytes(*b"CYPIPIN1");
#[repr(C)]
struct Header {
    magic: u64,
    inode: u64,
    uid: AtomicU32,
    gid: AtomicU32,
    mode: AtomicU32,
}
pub(crate) struct Inode {
    section: Arc<Object>,
    mutex: Arc<Object>,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
unsafe impl Send for Inode {}
unsafe impl Sync for Inode {}
impl Drop for Inode {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}
struct Guard<'a>(&'a Inode);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.mutex.raw());
        }
    }
}
fn records() -> &'static Mutex<HashMap<u64, Arc<Inode>>> {
    static RECORDS: OnceLock<Mutex<HashMap<u64, Arc<Inode>>>> = OnceLock::new();
    RECORDS.get_or_init(Default::default)
}
impl Inode {
    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast::<Header>() }
    }
    fn locked(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.raw(), INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }
    fn map(section: Object, mutex: Object) -> Result<Arc<Self>, i32> {
        let view = unsafe { MapViewOfFile(section.raw(), FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
        if view.Value.is_null() {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Arc::new(Self {
            section: Arc::new(section),
            mutex: Arc::new(mutex),
            view,
        }))
    }
    pub(crate) fn new() -> Result<Arc<Self>, i32> {
        let section = Object::owned(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                4096,
                ptr::null(),
            )
        })?;
        let mutex = Object::owned(unsafe { CreateMutexW(ptr::null(), 0, ptr::null()) })?;
        let result = Self::map(section, mutex)?;
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(a: *mut core::ffi::c_void, b: *mut u8, n: u32, flags: u32) -> i32;
        }
        let mut id = [0u8; 8];
        if unsafe { BCryptGenRandom(ptr::null_mut(), id.as_mut_ptr(), 8, 2) } < 0 {
            return Err(EIO);
        }
        let caller = crate::credentials::filesystem();
        unsafe {
            result.view.Value.cast::<Header>().write(Header {
                magic: MAGIC,
                inode: u64::from_le_bytes(id).max(1),
                uid: AtomicU32::new(caller.uid),
                gid: AtomicU32::new(caller.gid),
                mode: AtomicU32::new(fs::S_IFIFO | 0o600),
            });
        }
        Ok(result)
    }
    fn stat(&self) -> fs::Stat {
        let h = self.header();
        fs::Stat {
            st_ino: h.inode,
            st_mode: h.mode.load(Ordering::Relaxed),
            st_uid: h.uid.load(Ordering::Relaxed),
            st_gid: h.gid.load(Ordering::Relaxed),
            st_nlink: 1,
            st_blksize: 4096,
            ..fs::Stat::default()
        }
    }
}
pub(crate) fn register(entry: FdEntry, inode: Arc<Inode>) -> Result<(), i32> {
    if entry.kind != FdKind::Pipe {
        return Err(EBADF);
    }
    crate::platform::try_set_inheritable(inode.section.raw() as usize, true)?;
    crate::platform::try_set_inheritable(inode.mutex.raw() as usize, true)?;
    records()
        .lock()
        .map_err(|_| EIO)?
        .insert(entry.description_id, inode);
    Ok(())
}
pub(crate) fn reference(entry: FdEntry) -> Result<Option<Arc<Inode>>, i32> {
    if entry.kind != FdKind::Pipe {
        return Ok(None);
    }
    Ok(records()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}
pub(crate) fn export_rights(
    inode: Option<Arc<Inode>>,
    mut duplicate: impl FnMut(u64) -> Result<u64, i32>,
) -> Result<Vec<u8>, i32> {
    let Some(inode) = inode else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    for handle in [&inode.section, &inode.mutex] {
        crate::state_codec::word(&mut bytes, duplicate(handle.raw() as u64)?);
    }
    Ok(bytes)
}
pub(crate) fn import_rights(
    entry: FdEntry,
    bytes: &[u8],
    mut duplicate: impl FnMut(u64) -> Result<Object, i32>,
) -> Result<(), i32> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut r = crate::state_codec::Reader(bytes);
    let section = duplicate(r.word()?)?;
    let mutex = duplicate(r.word()?)?;
    r.end()?;
    register(entry, Inode::map(section, mutex)?)
}
fn pin(fd: i32, deny_path: bool) -> Result<Option<Arc<Inode>>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|e| *e)
        .ok_or(EBADF)?;
    if entry.kind != FdKind::Pipe {
        return Ok(None);
    }
    if deny_path && entry.flags.contains(crate::FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    reference(entry)
}
pub(crate) fn metadata(fd: i32) -> Result<Option<fs::Stat>, i32> {
    let Some(inode) = pin(fd, false)? else {
        return Ok(None);
    };
    let _guard = inode.locked()?;
    Ok(Some(inode.stat()))
}
pub(crate) fn chown(fd: i32, allow_path: bool, owner: &fs::Ownership) -> Result<bool, i32> {
    let Some(inode) = pin(fd, !allow_path)? else {
        return Ok(false);
    };
    let _guard = inode.locked()?;
    owner.check(&inode.stat())?;
    if owner.uid != u32::MAX {
        inode.header().uid.store(owner.uid, Ordering::Relaxed);
    }
    if owner.gid != u32::MAX {
        inode.header().gid.store(owner.gid, Ordering::Relaxed);
    }
    Ok(true)
}
pub fn chmod(fd: i32, mode: u32) -> Result<bool, i32> {
    chmod_descriptor(fd, mode, false)
}
pub(crate) fn chmod_descriptor(fd: i32, mode: u32, allow_path: bool) -> Result<bool, i32> {
    let Some(inode) = pin(fd, !allow_path)? else {
        return Ok(false);
    };
    let _guard = inode.locked()?;
    let uid = crate::credentials::filesystem().uid;
    if uid != 0 && uid != inode.stat().st_uid {
        return Err(crate::EPERM);
    }
    inode
        .header()
        .mode
        .store(fs::S_IFIFO | (mode & 0o7777), Ordering::Relaxed);
    Ok(true)
}
pub(crate) fn closed(id: u64) {
    if let Ok(mut records) = records().lock() {
        if let Some(inode) = records.remove(&id) {
            if !records.values().any(|other| Arc::ptr_eq(other, &inode)) {
                let _ = crate::platform::try_set_inheritable(inode.section.raw() as usize, false);
                let _ = crate::platform::try_set_inheritable(inode.mutex.raw() as usize, false);
            }
        }
    }
}
/// Caller holds the descriptor-table lock, as in the other auxiliary registries.
pub(crate) fn auxiliary_handles() -> Result<Vec<(u64, Arc<Object>)>, i32> {
    Ok(records()
        .lock()
        .map_err(|_| EIO)?
        .iter()
        .flat_map(|(&id, inode)| [(id, inode.section.clone()), (id, inode.mutex.clone())])
        .collect())
}
pub(crate) fn serialize(retain: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let records = records().lock().map_err(|_| EIO)?;
    let ids: std::collections::HashSet<_> = table
        .slots
        .enumerated()
        .filter_map(|(fd, entry)| {
            entry
                .filter(|e| e.kind == FdKind::Pipe && retain(fd as i32))
                .map(|e| e.description_id)
        })
        .collect();
    let mut bytes = Vec::new();
    for id in ids {
        if let Some(inode) = records.get(&id) {
            for value in [id, inode.section.raw() as u64, inode.mutex.raw() as u64] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    restore_checked(bytes).is_ok()
}
fn restore_checked(bytes: &[u8]) -> Result<(), i32> {
    if !bytes.len().is_multiple_of(24) {
        return Err(EIO);
    }
    let mut restored = HashMap::new();
    let mut pins = HashMap::<(usize, usize), Arc<Inode>>::new();
    let owned: std::collections::HashSet<_> = records()
        .lock()
        .map_err(|_| EIO)?
        .values()
        .flat_map(|inode| [inode.section.raw() as usize, inode.mutex.raw() as usize])
        .collect();
    let mut inherited = HashMap::new();
    // Own each inherited pair exactly once, even if both pipe ends survived.
    for row in bytes.chunks_exact(24) {
        let word = |n| u64::from_le_bytes(row[n..n + 8].try_into().unwrap());
        let (id, section, mutex) = (word(0), word(8) as usize, word(16) as usize);
        if id == 0 || section == 0 || mutex == 0 || section == mutex || restored.contains_key(&id) {
            return Err(EIO);
        }
        let inode = if let Some(inode) = pins.get(&(section, mutex)) {
            inode.clone()
        } else {
            let copied_section = Object::duplicate(section as _)?;
            let copied_mutex = Object::duplicate(mutex as _)?;
            for raw in [section, mutex] {
                if !owned.contains(&raw) && !inherited.contains_key(&raw) {
                    inherited.insert(raw, Object::owned(raw as _)?);
                }
            }
            let inode = Inode::map(copied_section, copied_mutex)?;
            if inode.header().magic != MAGIC {
                return Err(EIO);
            }
            crate::platform::try_set_inheritable(inode.section.raw() as usize, true)?;
            crate::platform::try_set_inheritable(inode.mutex.raw() as usize, true)?;
            pins.insert((section, mutex), inode.clone());
            inode
        };
        restored.insert(id, inode);
    }
    *records().lock().map_err(|_| EIO)? = restored;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_is_shared_by_endpoints_reopens_and_restored_sections() {
        let (read, write) = crate::create_pipe(crate::FdFlags::NONE, 4096).unwrap();
        let owner = fs::Ownership {
            uid: 1234,
            gid: 2345,
            caller: 0,
            group_member: true,
        };
        fs::fchown(write, false, &owner).unwrap();
        let first = fs::fstat(read).unwrap();
        let other = fs::fstat(write).unwrap();
        assert_ne!(first.st_ino, 0);
        assert_eq!(
            (first.st_ino, first.st_uid, first.st_gid),
            (other.st_ino, 1234, 2345)
        );
        let reopened = fs::open(&format!("/proc/self/fd/{read}"), fs::O_RDONLY, 0).unwrap();
        assert_eq!(fs::fstat(reopened).unwrap().st_ino, first.st_ino);
        let denied = fs::Ownership {
            uid: 0,
            gid: 0,
            caller: 2345,
            group_member: true,
        };
        assert_eq!(fs::fchown(read, false, &denied), Err(crate::EPERM));
        assert!(chmod(write, 0o640).unwrap());
        let bytes = serialize(|_| true).unwrap();
        assert!(restore(&bytes));
        assert_eq!(fs::fstat(reopened).unwrap().st_mode, fs::S_IFIFO | 0o640);
        crate::close(read).unwrap();
        assert_eq!(crate::write(write, b"x"), Ok(1));
        let mut byte = [0];
        assert_eq!(crate::read(reopened, &mut byte), Ok(1));
        assert_eq!(byte, *b"x");
        crate::close(write).unwrap();
        assert_eq!(crate::read(reopened, &mut byte), Ok(0));
        crate::close(reopened).unwrap();
    }
}
