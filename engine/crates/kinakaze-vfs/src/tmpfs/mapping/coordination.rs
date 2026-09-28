//! Serializes VMA enrollment/replacement with inode truncation. The mutex is
//! session scoped; records are held by the existing init-custodied map lease.
//! PID plus creation time prevents a stale record from touching a reused PID.
use super::*;
use std::marker::PhantomData;
use std::rc::Rc;
use windows_sys::Win32::Foundation::{FILETIME, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, VirtualProtectEx, VirtualQueryEx,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetProcessTimes, INFINITE, OpenProcess,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, ReleaseMutex, WaitForSingleObject,
};

static MUTEXES: Mutex<BTreeMap<(u32, u64), Arc<Object>>> = Mutex::new(BTreeMap::new());

pub struct Transaction {
    mutex: Arc<Object>,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for Transaction {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.mutex.raw()) };
    }
}
pub fn transaction() -> Result<Transaction, i32> {
    let key = (std::process::id(), kinakaze_runtime::authority::domain_id());
    let mutex = {
        let mut cache = MUTEXES.lock().map_err(|_| EIO)?;
        if let Some(mutex) = cache.get(&key) {
            mutex.clone()
        } else {
            let name: Vec<u16> = format!("Local\\kinakaze.tmpfs-vma.{}", key.1)
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mutex = Arc::new(Object::owned(unsafe {
                CreateMutexW(ptr::null(), 0, name.as_ptr())
            })?);
            cache.insert(key, mutex.clone());
            mutex
        }
    };
    match unsafe { WaitForSingleObject(mutex.raw(), INFINITE) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Transaction {
            mutex,
            _thread: PhantomData,
        }),
        _ => Err(EIO),
    }
}

fn birth(process: HANDLE) -> Result<u64, i32> {
    let mut times = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    let slots = times.as_mut_ptr();
    if unsafe { GetProcessTimes(process, slots, slots.add(1), slots.add(2), slots.add(3)) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok((times[0].dwHighDateTime as u64) << 32 | times[0].dwLowDateTime as u64)
}

#[derive(Clone, Copy)]
struct Range {
    pid: u64,
    birth: u64,
    start: u64,
    length: u64,
    offset: u64,
}
fn decode(bytes: &[u8]) -> Result<Vec<Range>, i32> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut reader = Reader(bytes);
    if reader.word()? != 0x544d504d41503031 {
        return Err(EIO);
    }
    let count = reader.word()?;
    if count > 1_000_000 {
        return Err(EIO);
    }
    let mut result = Vec::new();
    for _ in 0..count {
        let row = Range {
            pid: reader.word()?,
            birth: reader.word()?,
            start: reader.word()?,
            length: reader.word()?,
            offset: reader.word()?,
        };
        if row.pid == 0
            || row.pid > u32::MAX as u64
            || row.birth == 0
            || row.length == 0
            || row.start % PAGE != 0
            || row.length % PAGE != 0
            || row.offset % PAGE != 0
            || row.start.checked_add(row.length).is_none()
            || row.offset.checked_add(row.length).is_none()
        {
            return Err(EIO);
        }
        result.push(row);
    }
    reader.end()?;
    Ok(result)
}
fn encode(rows: &[Range]) -> Vec<u8> {
    let mut bytes = Vec::new();
    word(&mut bytes, 0x544d504d41503031);
    word(&mut bytes, rows.len() as u64);
    for row in rows {
        for value in [row.pid, row.birth, row.start, row.length, row.offset] {
            word(&mut bytes, value);
        }
    }
    bytes
}

/// Replace this process's fragments only. Empty enrollment retires its old
/// addresses before munmap can expose them to another allocator or inode.
pub fn publish(lease: u64, ranges: &[(usize, usize, u64)]) -> Result<(), i32> {
    let _transaction = transaction()?;
    let pid = std::process::id() as u64;
    let born = birth(unsafe { GetCurrentProcess() })?;
    let store = Store::user_object(lease, false)?;
    store.update(|bytes| {
        let mut rows = decode(bytes)?;
        rows.retain(|row| row.pid != pid || row.birth != born);
        for &(start, length, offset) in ranges {
            rows.push(Range {
                pid,
                birth: born,
                start: start as u64,
                length: length as u64,
                offset,
            });
        }
        Ok((encode(&rows), ()))
    })
}

pub(crate) fn revoke(mappings: &[(u64, u64, u64)], removed: u64) -> Result<(), i32> {
    let _transaction = transaction()?;
    for &(lease, _, _) in mappings {
        let store = match Store::user_object(lease, false) {
            Ok(store) => store,
            Err(ENOENT) => continue,
            Err(error) => return Err(error),
        };
        for row in decode(&store.read()?.1)? {
            let skipped = removed.saturating_sub(row.offset).min(row.length);
            if skipped == row.length {
                continue;
            }
            let process = unsafe {
                OpenProcess(
                    PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION,
                    0,
                    row.pid as u32,
                )
            };
            if process.is_null() {
                let error = unsafe { GetLastError() };
                if error == 87 {
                    continue;
                } // process has exited
                return Err(errno_from_win32(error));
            }
            let process = Object::owned(process)?;
            if birth(process.raw())? != row.birth {
                continue;
            }
            let end = (row.start + row.length) as usize;
            let mut cursor = (row.start + skipped) as usize;
            while cursor < end {
                let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
                if unsafe {
                    VirtualQueryEx(
                        process.raw(),
                        cursor as _,
                        &mut info,
                        std::mem::size_of_val(&info),
                    )
                } == 0
                {
                    // A terminated process no longer has readable VADs.
                    let error = unsafe { GetLastError() };
                    if error == 87 {
                        break;
                    }
                    return Err(errno_from_win32(error));
                }
                let limit = (info.BaseAddress as usize)
                    .checked_add(info.RegionSize)
                    .ok_or(EIO)?
                    .min(end);
                if limit <= cursor {
                    return Err(EIO);
                }
                if info.State == MEM_COMMIT {
                    let mut previous = 0;
                    if unsafe {
                        VirtualProtectEx(
                            process.raw(),
                            cursor as _,
                            limit - cursor,
                            PAGE_NOACCESS,
                            &mut previous,
                        )
                    } == 0
                    {
                        return Err(errno_from_win32(unsafe { GetLastError() }));
                    }
                }
                cursor = limit;
            }
        }
    }
    Ok(())
}
