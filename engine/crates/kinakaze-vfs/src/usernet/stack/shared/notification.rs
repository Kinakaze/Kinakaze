//! One event per waiter, shared by name, with no polling thread. A waiter arms
//! its own event before checking readiness. Another waiter can never clear it.
use super::*;
use std::os::windows::io::{AsRawHandle, RawHandle};

const MAX_WAITERS: usize = 64;
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Record {
    id: u64,
    birth: u64,
    pid: u32,
    armed: u32,
}
#[repr(C)]
pub(super) struct Directory {
    sequence: u64,
    next_id: u64,
    records: [Record; MAX_WAITERS],
}
struct Remote {
    event: Handle,
    process: Handle,
}
unsafe impl Send for Remote {}
#[derive(Default)]
pub(super) struct Local(HashMap<u64, Remote>);

/// A subscription pins its endpoint and may be passed to native event waits.
/// Call `arm`, then inspect readiness/pump state, then wait only if still idle.
/// The handle stays owned by this subscription and must not be closed by callers.
pub struct Notification {
    pub(super) endpoint: Arc<SharedEndpoint>,
    event: Handle,
    pub(super) id: u64,
}
unsafe impl Send for Notification {}
unsafe impl Sync for Notification {}
impl AsRawHandle for Notification {
    fn as_raw_handle(&self) -> RawHandle {
        self.event.0
    }
}
impl Notification {
    pub fn arm(&self) -> Result<u64, i32> {
        let _guard = self.endpoint.acquire()?;
        if unsafe { ResetEvent(self.event.0) } == 0 {
            return Err(EIO);
        }
        let directory = unsafe { &mut *self.endpoint.directory() };
        directory
            .records
            .iter_mut()
            .find(|r| r.id == self.id)
            .ok_or(EIO)?
            .armed = 1;
        Ok(directory.sequence)
    }
    /// The arena mutex must remain held until this guard is dropped. Existing
    /// signalled state is preserved; only synchronous self-notifications mute.
    pub(super) fn mute_locked(&self) -> Result<Muted<'_>, i32> {
        let directory = unsafe { &mut *self.endpoint.directory() };
        let index = directory
            .records
            .iter()
            .position(|r| r.id == self.id)
            .ok_or(EIO)?;
        let armed = directory.records[index].armed;
        directory.records[index].armed = 0;
        Ok(Muted {
            notification: self,
            index,
            armed,
        })
    }
}
pub(super) struct Muted<'a> {
    notification: &'a Notification,
    index: usize,
    armed: u32,
}
impl Drop for Muted<'_> {
    fn drop(&mut self) {
        unsafe {
            (*self.notification.endpoint.directory()).records[self.index].armed = self.armed;
        }
    }
}
impl Drop for Notification {
    fn drop(&mut self) {
        // Even a poisoned protocol core must release its waiter registration.
        // Never inspect that core while cleaning up this disjoint directory.
        let status = unsafe { WaitForSingleObject(self.endpoint.mutex.0, INFINITE) };
        if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
            return;
        }
        let _guard = Guard(&self.endpoint);
        if status == WAIT_ABANDONED {
            unsafe {
                ptr::addr_of_mut!((*self.endpoint.view.1.0.Value.cast::<Layout>()).poison).write(1);
            }
        }
        if !self.endpoint.identity_valid_locked() {
            return;
        }
        let directory = unsafe { &mut *self.endpoint.directory() };
        for record in &mut directory.records {
            if record.id == self.id {
                *record = Record::default();
                break;
            }
        }
        self.endpoint
            .notifications
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .0
            .remove(&self.id);
    }
}

fn birth(process: HANDLE) -> Result<u64, i32> {
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(EIO);
    }
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

impl SharedEndpoint {
    fn directory(&self) -> *mut Directory {
        unsafe { ptr::addr_of_mut!((*self.view.0.Value.cast::<Layout>()).notifications) }
    }
    fn notification_name(&self, id: u64) -> Vec<u16> {
        Self::notification_name_for(&self.name, id)
    }
    fn notification_name_for(name: &str, id: u64) -> Vec<u16> {
        format!("{name}.Wake.{id:016x}")
            .encode_utf16()
            .chain([0])
            .collect()
    }
    pub fn subscribe(self: &Arc<Self>) -> Result<Notification, i32> {
        let _guard = self.acquire()?;
        let directory = unsafe { &mut *self.directory() };
        // Reclaim subscriptions left by processes which have exited. This is
        // an on-demand check at registration/publication, never a timer poll.
        Self::refresh_notifications_locked(directory, true, &self.name, &self.notifications);
        let record = directory
            .records
            .iter_mut()
            .find(|r| r.id == 0)
            .ok_or(crate::ENOBUFS)?;
        let id = directory.next_id.checked_add(1).ok_or(crate::ENOBUFS)?;
        let event = Handle::new(unsafe {
            CreateEventW(ptr::null(), 1, 0, self.notification_name(id).as_ptr())
        })?;
        let owner = birth(unsafe { GetCurrentProcess() })?;
        *record = Record {
            id,
            birth: owner,
            pid: std::process::id(),
            armed: 0,
        };
        directory.next_id = id;
        Ok(Notification {
            endpoint: self.clone(),
            event,
            id,
        })
    }
    fn refresh_notifications_locked(
        directory: &mut Directory,
        reap: bool,
        name: &str,
        cache: &Mutex<Local>,
    ) {
        let mut local = cache.lock().unwrap_or_else(|e| e.into_inner());
        local
            .0
            .retain(|id, _| directory.records.iter().any(|r| r.id == *id));
        for record in &mut directory.records {
            if record.id == 0 || (!reap && record.armed == 0) {
                continue;
            }
            if !local.0.contains_key(&record.id) {
                let remote = (|| {
                    let process = Handle::new(unsafe {
                        OpenProcess(
                            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                            0,
                            record.pid,
                        )
                    })?;
                    if birth(process.0)? != record.birth {
                        return Err(EIO);
                    }
                    let event = Handle::new(unsafe {
                        OpenEventW(
                            EVENT_MODIFY_STATE,
                            0,
                            Self::notification_name_for(name, record.id).as_ptr(),
                        )
                    })?;
                    Ok::<_, i32>(Remote { process, event })
                })();
                match remote {
                    Ok(remote) => {
                        local.0.insert(record.id, remote);
                    }
                    Err(_) => {
                        *record = Record::default();
                        continue;
                    }
                }
            }
            let remote = &local.0[&record.id];
            if reap && unsafe { WaitForSingleObject(remote.process.0, 0) } != WAIT_TIMEOUT {
                local.0.remove(&record.id);
                *record = Record::default();
            }
        }
    }
    pub(super) fn notify_locked(&self) {
        self.notify_except_locked(0);
    }
    pub(super) fn notify_except_locked(&self, exclude: u64) {
        let directory = unsafe { &mut *self.directory() };
        Self::notify_directory_locked(directory, &self.name, &self.notifications, exclude);
        if self.identity.is_some() {
            let directory = unsafe {
                &mut *ptr::addr_of_mut!((*self.view.1.0.Value.cast::<Layout>()).notifications)
            };
            let root_name = self.name.rsplit_once(".C").unwrap().0;
            Self::notify_directory_locked(directory, root_name, &self.root_notifications, 0);
        }
    }
    fn notify_directory_locked(
        directory: &mut Directory,
        name: &str,
        cache: &Mutex<Local>,
        exclude: u64,
    ) {
        directory.sequence = directory.sequence.wrapping_add(1);
        if !directory
            .records
            .iter()
            .any(|r| r.id != 0 && r.armed != 0 && r.id != exclude)
        {
            return;
        }
        Self::refresh_notifications_locked(directory, false, name, cache);
        let local = cache.lock().unwrap_or_else(|e| e.into_inner());
        for record in &mut directory.records {
            if record.id == 0 || record.armed == 0 || record.id == exclude {
                continue;
            }
            let remote = &local.0[&record.id];
            // All events are held by owned handles with MODIFY_STATE access.
            unsafe {
                SetEvent(remote.event.0);
            }
            // Further writes before this waiter rearms are coalesced. The hot
            // path does not repeatedly signal an already notified waiter or
            // query its owner's process handle for every received packet.
            record.armed = 0;
        }
    }
}

#[cfg(test)]
mod tests;
