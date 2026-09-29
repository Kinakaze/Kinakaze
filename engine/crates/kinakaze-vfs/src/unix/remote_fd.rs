//! `pidfd_getfd(2)` across processes.
//!
//! Only the owner of a descriptor table can describe one of its descriptors,
//! so the requester asks the target to export it. The request travels as the
//! id of a private shared object (the mailbox) appended to the target's named
//! request queue; the target's signal pump answers with an SCM_RIGHTS record
//! whose init-held resources are owned by that same mailbox, and the requester
//! installs the record exactly like a received control message.
use super::rights;
use crate::fs::object::Object;
use crate::job::ESRCH;
use crate::mount::shared::Store;
use crate::state_codec::{Reader, word};
use crate::{EBADF, EIO, EPERM};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

const QUEUE_MAGIC: u64 = u64::from_le_bytes(*b"CYGETFD1");
const REQUEST: u64 = u64::from_le_bytes(*b"GETFDREQ");
const REPLY: u64 = u64::from_le_bytes(*b"GETFDREP");
const QUEUE_BYTES: usize = 4096;
const QUEUE_SLOTS: usize = QUEUE_BYTES / 8 - 2;
const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
/// The target answers from its signal pump, which a stopped process keeps
/// running. Only a target without a pump (mid-exec) can leave us waiting.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}

fn queue_name(pid: u32, start_ticks: u64, suffix: &str) -> Vec<u16> {
    wide(&format!(
        r"Local\kinakaze.pidfd-getfd.v1.{}.{pid}.{start_ticks}{suffix}",
        kinakaze_runtime::authority::domain_id()
    ))
}

/// The request queue of one process incarnation, locked for the guard's life.
struct Queue {
    view: *mut u64,
    _section: Object,
    mutex: Object,
}

impl Queue {
    fn open(pid: u32, start_ticks: u64, create: bool) -> Result<Option<Self>, i32> {
        let name = queue_name(pid, start_ticks, "");
        let section = unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null(),
                    PAGE_READWRITE,
                    0,
                    QUEUE_BYTES as u32,
                    name.as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr())
            }
        };
        if section.is_null() {
            return if create {
                Err(crate::errno_from_win32(unsafe { GetLastError() }))
            } else {
                Ok(None)
            };
        }
        let section = Object::owned(section)?;
        let mutex_name = queue_name(pid, start_ticks, ".lock");
        let mutex = Object::owned(unsafe {
            CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr())
        })?;
        match unsafe { WaitForSingleObject(mutex.raw(), 5_000) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {}
            _ => return Err(EIO),
        }
        let view =
            unsafe { MapViewOfFile(section.raw(), FILE_MAP_ALL_ACCESS, 0, 0, QUEUE_BYTES) };
        if view.Value.is_null() {
            unsafe { ReleaseMutex(mutex.raw()) };
            return Err(EIO);
        }
        let queue = Self {
            view: view.Value.cast(),
            _section: section,
            mutex,
        };
        unsafe {
            if *queue.view != QUEUE_MAGIC {
                *queue.view = QUEUE_MAGIC;
                *queue.view.add(1) = 0;
            }
        }
        Ok(Some(queue))
    }

    fn push(&mut self, mailbox: u64) -> Result<(), i32> {
        unsafe {
            let count = *self.view.add(1) as usize;
            if count >= QUEUE_SLOTS {
                return Err(crate::EAGAIN);
            }
            *self.view.add(2 + count) = mailbox;
            *self.view.add(1) = count as u64 + 1;
        }
        Ok(())
    }

    fn take(&mut self) -> Vec<u64> {
        unsafe {
            let count = (*self.view.add(1) as usize).min(QUEUE_SLOTS);
            let taken = (0..count).map(|index| *self.view.add(2 + index)).collect();
            *self.view.add(1) = 0;
            taken
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
            ReleaseMutex(self.mutex.raw());
        }
    }
}

/// Linux requires ptrace attach rights: the same real user, or CAP_SYS_PTRACE
/// over the target's user namespace.
fn may_access(pid: u32) -> Result<(), i32> {
    let target_user = kinakaze_runtime::job::user_namespace(pid).ok_or(ESRCH)?;
    if crate::user_namespace::capable(target_user, 19) {
        return Ok(());
    }
    let identity = crate::credentials::identity();
    let ids = crate::credentials::process_ids(pid).map_err(|_| ESRCH)?;
    if ids.iter().all(|uid| *uid == identity.uids[0]) {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// Requests `target_fd` from the process incarnation `(pid, start_ticks)`.
pub(crate) fn fetch(pid: u32, start_ticks: u64, host_pid: u32, target_fd: i32) -> Result<i32, i32> {
    if target_fd < 0 {
        return Err(EBADF);
    }
    may_access(pid)?;
    let mailbox = crate::mount::shared::new_object()?;
    let mut request = Vec::new();
    word(&mut request, REQUEST);
    word(&mut request, target_fd as u64);
    mailbox.replace(&request)?;
    Queue::open(pid, start_ticks, true)?
        .ok_or(EIO)?
        .push(mailbox.id())?;
    crate::job::wake_host(host_pid);
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let (_, reply) = mailbox.read()?;
        let mut input = Reader(&reply);
        if input.word()? == REPLY {
            let error = input.word()? as i32;
            if error != 0 {
                return Err(error);
            }
            let record = rights::Record::read(&mut input)?;
            input.end()?;
            let (fds, _) = record.receive(1, MSG_CMSG_CLOEXEC);
            record.release();
            return fds.first().copied().ok_or(EIO);
        }
        if kinakaze_runtime::job::lookup(pid)
            .is_none_or(|entry| entry.start_ticks != start_ticks)
        {
            return Err(ESRCH);
        }
        if crate::signal::interrupt_pending() {
            return Err(crate::EINTR);
        }
        if Instant::now() >= deadline {
            return Err(EIO);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Answers every queued request. Runs on the signal pump of the current owner
/// of this process's registry slot.
pub(crate) fn serve(pid: u32, start_ticks: u64) {
    let Ok(Some(mut queue)) = Queue::open(pid, start_ticks, false) else {
        return;
    };
    let mailboxes = queue.take();
    drop(queue);
    for id in mailboxes {
        let Ok(mailbox) = Store::user_object(id, false) else {
            continue;
        };
        let Ok((_, request)) = mailbox.read() else {
            continue;
        };
        let mut input = Reader(&request);
        if input.word() != Ok(REQUEST) {
            continue;
        }
        let Ok(fd) = input.word() else { continue };
        let mut reply = Vec::new();
        word(&mut reply, REPLY);
        match rights::export(id, &[fd as i32]) {
            Ok(pending) => {
                word(&mut reply, 0);
                pending.record().write(&mut reply);
                if mailbox.replace(&reply).is_ok() {
                    pending.commit();
                }
            }
            Err(error) => {
                word(&mut reply, error as u64);
                let _ = mailbox.replace(&reply);
            }
        }
    }
}

#[allow(dead_code)]
fn _assert_handle_is_pointer(handle: HANDLE) -> *mut core::ffi::c_void {
    handle
}
