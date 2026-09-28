//! Names belong to /dev/shm; mappings use the existing VFS and fork owners.
use super::SemT;
use core::ffi::{CStr, c_char};
use kinakaze_vfs::{EEXIST, EFAULT, EINVAL, EIO, ENAMETOOLONG, ENOENT, ENOMEM, EOVERFLOW, fs};
use std::ffi::CString;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

const SIZE: usize = size_of::<SemT>();
const OPEN: i32 = fs::O_RDWR | fs::O_CLOEXEC | fs::O_NOFOLLOW;
const KEY: u64 = u64::from_le_bytes(*b"CYSEM001");

#[derive(Clone, Copy)]
struct Entry {
    device: u64,
    inode: u64,
    address: usize,
    references: usize,
}
static MAPPINGS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

unsafe fn path(name: *const c_char) -> Result<CString, i32> {
    if name.is_null() {
        return Err(EFAULT);
    }
    let name = unsafe { CStr::from_ptr(name) }
        .to_str()
        .map_err(|_| EINVAL)?;
    let name = name.trim_start_matches('/');
    if name.is_empty() || name.contains('/') {
        return Err(EINVAL);
    }
    if name.len() > 251 {
        return Err(ENAMETOOLONG);
    }
    CString::new(format!("/dev/shm/sem.{name}")).map_err(|_| EINVAL)
}

struct File(i32);
impl Drop for File {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.0);
    }
}
struct Temporary(CString);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::unlink(self.0.to_str().unwrap());
    }
}
struct Mapping(usize);
impl Drop for Mapping {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                crate::fdio::kinakaze_abi_munmap(self.0 as _, SIZE);
            }
        }
    }
}
fn map(file: &File) -> Result<Mapping, i32> {
    let address =
        unsafe { crate::fdio::kinakaze_abi_mmap64(core::ptr::null_mut(), SIZE, 3, 1, file.0, 0) };
    if address as isize == -1 {
        Err(crate::kinakaze_errno())
    } else {
        Ok(Mapping(address as usize))
    }
}

fn create(path: &CStr, mode: u32, value: u32) -> Result<(File, Mapping), i32> {
    if value > super::VALUE_MAX {
        return Err(EINVAL);
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (file, temporary) = loop {
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let name =
            CString::new(format!("/dev/shm/.sem-{}-{nonce:016x}", std::process::id())).unwrap();
        match fs::open(
            name.to_str().unwrap(),
            OPEN | fs::O_CREAT | fs::O_EXCL,
            crate::fsextra::creation_mode(mode),
        ) {
            Ok(fd) => break (File(fd), Temporary(name)),
            Err(EEXIST) => continue,
            Err(error) => return Err(error),
        }
    };
    // Publish an initialized inode with one atomic link. A concurrent opener
    // never sees an empty file or accidentally resets an existing semaphore.
    let mut bytes = [0u8; SIZE];
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    let mut written = 0;
    while written < SIZE {
        let count = kinakaze_vfs::write(file.0, &bytes[written..])?;
        if count == 0 {
            return Err(EIO);
        }
        written += count;
    }
    let mapping = map(&file)?;
    if unsafe { crate::fsextra::kinakaze_abi_link(temporary.0.as_ptr(), path.as_ptr()) } != 0 {
        return Err(crate::kinakaze_errno());
    }
    Ok((file, mapping))
}

fn open(path: &CStr, flags: i32, mode: u32, value: u32) -> Result<usize, i32> {
    // The same topology gate protects mmap metadata and this process's open
    // references, so fork cannot copy one without the other.
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let (file, candidate) = loop {
        if flags & (fs::O_CREAT | fs::O_EXCL) != (fs::O_CREAT | fs::O_EXCL) {
            match fs::open(path.to_str().unwrap(), OPEN, 0) {
                Ok(fd) => break (File(fd), None),
                Err(ENOENT) if flags & fs::O_CREAT != 0 => (),
                Err(error) => return Err(error),
            }
        }
        match create(path, mode, value) {
            Ok((file, mapping)) => break (file, Some(mapping)),
            Err(EEXIST) if flags & fs::O_EXCL == 0 => continue,
            Err(error) => return Err(error),
        }
    };
    let stat = fs::fstat(file.0)?;
    if stat.st_size < SIZE as i64 || stat.st_mode & fs::S_IFMT != fs::S_IFREG {
        return Err(EINVAL);
    }
    let mut entries = MAPPINGS.lock().map_err(|_| EIO)?;
    if let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.device == stat.st_dev && entry.inode == stat.st_ino)
    {
        entry.references = entry.references.checked_add(1).ok_or(EOVERFLOW)?;
        return Ok(entry.address);
    }
    let mut mapping = match candidate {
        Some(mapping) => mapping,
        None => map(&file)?,
    };
    let address = mapping.0;
    entries.push(Entry {
        device: stat.st_dev,
        inode: stat.st_ino,
        address,
        references: 1,
    });
    mapping.0 = 0;
    Ok(address)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_open(
    name: *const c_char,
    flags: i32,
    mode: u32,
    value: u32,
) -> *mut SemT {
    match unsafe { path(name) }.and_then(|path| open(&path, flags, mode, value)) {
        Ok(address) => address as _,
        Err(error) => {
            crate::set_errno(error);
            core::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_close(sem: *mut SemT) -> i32 {
    let Some(_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        return super::failed(ENOMEM);
    };
    let Ok(mut entries) = MAPPINGS.lock() else {
        return super::failed(EIO);
    };
    let Some(index) = entries
        .iter()
        .position(|entry| entry.address == sem as usize)
    else {
        return super::failed(EINVAL);
    };
    if entries[index].references > 1 {
        entries[index].references -= 1;
    } else {
        if unsafe { crate::fdio::kinakaze_abi_munmap(sem.cast(), SIZE) } != 0 {
            return -1;
        }
        entries.swap_remove(index);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_unlink(name: *const c_char) -> i32 {
    match unsafe { path(name) }.and_then(|path| fs::unlink(path.to_str().unwrap())) {
        Ok(()) => 0,
        Err(error) => super::failed(error),
    }
}

// Only process references travel through fork. File contents and wait queues
// keep their existing shared owners; exec starts with an empty local registry.
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    let Ok(entries) = MAPPINGS.lock() else {
        return -(EIO as isize);
    };
    let Some(length) = entries
        .len()
        .checked_mul(32)
        .and_then(|size| size.checked_add(8))
    else {
        return -(EOVERFLOW as isize);
    };
    if output.is_null() {
        return length as isize;
    }
    if capacity < length {
        return -(EINVAL as isize);
    }
    let output = unsafe { core::slice::from_raw_parts_mut(output, length) };
    output[..8].copy_from_slice(&KEY.to_le_bytes());
    for (entry, bytes) in entries.iter().zip(output[8..].chunks_exact_mut(32)) {
        for (value, field) in [
            entry.device,
            entry.inode,
            entry.address as u64,
            entry.references as u64,
        ]
        .into_iter()
        .zip(bytes.chunks_exact_mut(8))
        {
            field.copy_from_slice(&value.to_le_bytes());
        }
    }
    length as isize
}

unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length < 8 || (length - 8) % 32 != 0 || length > isize::MAX as usize {
        return EINVAL;
    }
    let bytes = unsafe { core::slice::from_raw_parts(input, length) };
    if bytes[..8] != KEY.to_le_bytes() {
        return EINVAL;
    }
    let mut entries = Vec::<Entry>::new();
    for row in bytes[8..].chunks_exact(32) {
        let word = |offset| u64::from_le_bytes(row[offset..offset + 8].try_into().unwrap());
        let entry = Entry {
            device: word(0),
            inode: word(8),
            address: word(16) as usize,
            references: word(24) as usize,
        };
        if entry.address == 0
            || entry.address % 4096 != 0
            || entry.references == 0
            || entries.iter().any(|other| {
                other.address == entry.address
                    || (other.device == entry.device && other.inode == entry.inode)
            })
        {
            return EINVAL;
        }
        entries.push(entry);
    }
    let Ok(mut registry) = MAPPINGS.lock() else {
        return EIO;
    };
    *registry = entries;
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 40,
        key: KEY,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
