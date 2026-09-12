//! Per-attachment policy and crash-safe native writer ownership.
//!
//! A writer owns a sharing-denying native open. Duplicating that open into a
//! child or a file-backed mapping retains ownership until its last handle dies;
//! process crashes therefore cannot strand a software writer counter.
use crate::{EBUSY, EIO, EPERM, EROFS, errno_from_win32, fs::object::Object};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::HashMap,
    ptr,
    sync::{Arc, Mutex, OnceLock},
};
use windows_sys::Win32::Foundation::{
    ERROR_SHARING_VIOLATION, GetLastError, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_ALWAYS,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};

const MAGIC: u64 = u64::from_le_bytes(*b"CYMPOL02");
#[repr(C)]
struct Header {
    magic: AtomicU64,
    flags: AtomicU64,
    map_len: AtomicU64,
    map: [AtomicU64; 1024],
}
pub(crate) struct Policy {
    pub(crate) namespace: u64,
    pub(crate) id: u64,
    section: Object,
    mutex: Object,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    gate: Vec<u16>,
}
unsafe impl Send for Policy {}
unsafe impl Sync for Policy {}
impl Drop for Policy {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.view) };
    }
}
pub(crate) struct Guard<'a>(&'a Policy);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.mutex.raw()) };
    }
}
fn cache() -> &'static Mutex<HashMap<(u64, u64), Arc<Policy>>> {
    static CACHE: OnceLock<Mutex<HashMap<(u64, u64), Arc<Policy>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
impl Policy {
    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast::<Header>() }
    }
    pub(crate) fn flags(&self) -> u64 {
        self.header().flags.load(Ordering::Acquire)
    }
    pub(crate) fn advance_error_epoch(&self) {
        self.header().flags.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) fn mapping(&self) -> Result<Option<crate::user_namespace::Mapping>, i32> {
        if self.header().map_len.load(Ordering::Acquire) == 0 {
            return Ok(None);
        }
        let _guard = self.lock()?;
        let len = self.header().map_len.load(Ordering::Acquire) as usize;
        if len == 0 {
            return Ok(None);
        }
        if len > 8192 {
            return Err(EIO);
        }
        let mut bytes = Vec::new();
        for word in &self.header().map[..len.div_ceil(8)] {
            bytes.extend_from_slice(&word.load(Ordering::Acquire).to_le_bytes());
        }
        bytes.truncate(len);
        crate::user_namespace::Mapping::decode(&bytes).map(Some)
    }
    fn install_mapping(&self, map: Option<&crate::user_namespace::Mapping>) {
        let bytes = map.map(|m| m.encode()).unwrap_or_default();
        for (i, chunk) in bytes.chunks(8).enumerate() {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.header().map[i].store(u64::from_le_bytes(word), Ordering::Relaxed);
        }
        self.header()
            .map_len
            .store(bytes.len() as u64, Ordering::Release);
    }
    pub(crate) fn lock(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.raw(), INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }
    fn gate(&self, exclusive: bool) -> Result<Object, i32> {
        let handle = unsafe {
            CreateFileW(
                self.gate.as_ptr(),
                if exclusive {
                    FILE_WRITE_DATA
                } else {
                    FILE_READ_DATA
                },
                FILE_SHARE_READ | FILE_SHARE_DELETE | if exclusive { FILE_SHARE_WRITE } else { 0 },
                ptr::null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = unsafe { GetLastError() };
            return Err(if error == ERROR_SHARING_VIOLATION {
                EBUSY
            } else {
                errno_from_win32(error)
            });
        }
        Object::owned(handle)
    }
    pub(crate) fn writer(&self) -> Result<Arc<Object>, i32> {
        let _guard = self.lock()?;
        if self.flags() & 1 != 0 {
            return Err(EROFS);
        }
        let object = self.gate(false)?;
        Ok(Arc::new(object))
    }
}
impl Guard<'_> {
    pub(crate) fn check(&self, flags: u64) -> Result<(), i32> {
        if flags & 1 != 0 && self.0.flags() & 1 == 0 {
            self.0.gate(true)?;
        }
        Ok(())
    }
}
pub(crate) fn get(namespace: u64, id: u64, flags: u64) -> Result<Arc<Policy>, i32> {
    let mut cache = cache().lock().map_err(|_| EIO)?;
    if let Some(policy) = cache.get(&(namespace, id)) {
        return Ok(policy.clone());
    }
    let domain = kinakaze_runtime::authority::domain_id();
    let name = |suffix: &str| {
        format!(r"Local\kinakaze.mount-policy.v2.{domain}.{namespace}.{id}.{suffix}")
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let mutex = Object::owned(unsafe { CreateMutexW(ptr::null(), 0, name("guard").as_ptr()) })?;
    let section = Object::owned(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            16384,
            name("state").as_ptr(),
        )
    })?;
    let view = unsafe { MapViewOfFile(section.raw(), FILE_MAP_ALL_ACCESS, 0, 0, 16384) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let directory = std::env::temp_dir().join("kinakaze-mount-leases");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        unsafe { UnmapViewOfFile(view) };
        return Err(errno_from_win32(error.raw_os_error().unwrap_or(1) as u32));
    }
    let gate = crate::path::wide_path(&directory.join(format!("{domain}-{namespace}-{id}.lease")))?;
    let policy = Arc::new(Policy {
        namespace,
        id,
        section,
        mutex,
        view,
        gate,
    });
    if policy.header().magic.load(Ordering::Acquire) == 0 {
        let _guard = policy.lock()?;
        if policy.header().magic.load(Ordering::Acquire) == 0 {
            policy.header().flags.store(flags, Ordering::Relaxed);
            policy.header().magic.store(MAGIC, Ordering::Release);
        }
    }
    if policy.header().magic.load(Ordering::Acquire) != MAGIC {
        return Err(EIO);
    }
    crate::platform::try_set_inheritable(policy.section.raw() as usize, true)?;
    cache.insert((namespace, id), policy.clone());
    Ok(policy)
}

pub(crate) fn get_mapped(
    namespace: u64,
    id: u64,
    flags: u64,
    mapping: Option<&crate::user_namespace::Mapping>,
) -> Result<Arc<Policy>, i32> {
    let policy = get(namespace, id, flags)?;
    if let Some(map) = mapping {
        let _guard = policy.lock()?;
        if policy.mapping()?.is_none() {
            policy.install_mapping(Some(map));
        }
    }
    Ok(policy)
}

pub(crate) fn serialize() -> Result<Vec<u8>, i32> {
    let cache = cache().lock().map_err(|_| EIO)?;
    let mut bytes = Vec::with_capacity(cache.len() * 32);
    for p in cache.values() {
        for word in [p.namespace, p.id, p.flags(), p.section.raw() as u64] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    if bytes.len() % 32 != 0 {
        return false;
    }
    for record in bytes.chunks_exact(32) {
        let word = |i| u64::from_le_bytes(record[i..i + 8].try_into().unwrap());
        if get(word(0), word(8), word(16)).is_err() {
            return false;
        }
        unsafe { windows_sys::Win32::Foundation::CloseHandle(word(24) as _) };
    }
    true
}

/// Hold every selected policy lock until the namespace publication succeeds.
/// Failed recursive updates restore all policies before allowing new writers.
pub(crate) struct Changes {
    held: Vec<(Arc<Policy>, u64, Option<crate::user_namespace::Mapping>)>,
    committed: bool,
}
impl Changes {
    pub(crate) fn apply(mut policies: Vec<(Arc<Policy>, u64)>) -> Result<Self, i32> {
        policies.sort_by_key(|(p, _)| (p.namespace, p.id));
        let mut result = Self {
            held: Vec::new(),
            committed: false,
        };
        for (p, flags) in &policies {
            let guard = p.lock()?;
            guard.check(*flags)?;
            result.held.push((p.clone(), p.flags(), p.mapping()?));
            std::mem::forget(guard);
        }
        for (p, flags) in policies {
            p.header().flags.store(flags, Ordering::Release);
        }
        Ok(result)
    }
    pub(crate) fn set_mapping(&self, map: &crate::user_namespace::Mapping) -> Result<(), i32> {
        for (p, _, _) in &self.held {
            if p.mapping()?.is_some() {
                return Err(EPERM);
            }
            p.gate(true)?;
        }
        for (p, _, _) in &self.held {
            p.install_mapping(Some(map));
        }
        Ok(())
    }
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for Changes {
    fn drop(&mut self) {
        for (p, flags, map) in self.held.iter().rev() {
            if !self.committed {
                p.header().flags.store(*flags, Ordering::Release);
                p.install_mapping(map.as_ref());
            }
            unsafe { ReleaseMutex(p.mutex.raw()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialized_policy_restore_does_not_wait_on_a_frozen_parent_writer() {
        let id = crate::mount::shared::next_group().unwrap();
        let policy = get(0xfeed, id, 0).unwrap();
        cache().lock().unwrap().remove(&(0xfeed, id));
        let guard = policy.lock().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(get(0xfeed, id, 0).and_then(|p| p.mapping()))
                .unwrap();
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(1));
        drop(guard);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Ok(None));
    }
}
