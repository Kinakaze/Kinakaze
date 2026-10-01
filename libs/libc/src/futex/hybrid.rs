//! Value-only bridge records for private/shared requeues. A private waiter
//! retains its token after migration; no guest/Rust pointer crosses processes.
//! Named park events are per thread, rather than per futex or vector member.

use super::*;
use core::cell::RefCell;

pub(super) const PRIVATE_PARK: u32 = 1 << 31;

#[derive(Clone, Copy)]
pub(crate) struct ParkIdentity {
    pub host: u32,
    pub thread: u32,
    pub born: u64,
}

struct Park {
    identity: ParkIdentity,
    domain: u64,
    event: Handle,
}

thread_local! {
    static PARK: RefCell<Option<Park>> = const { RefCell::new(None) };
}

pub(super) fn park_name(domain: u64, host: u32, thread: u32, born: u64) -> Vec<u16> {
    wide(&format!(
        r"Local\kinakaze.futex.park.v5.{domain:016x}.{host:08x}.{thread:08x}.{born:016x}"
    ))
}

fn with_park<T>(read: impl FnOnce(&Park) -> T) -> Result<T, i32> {
    let host = std::process::id();
    let thread = unsafe { GetCurrentThreadId() };
    let domain = kinakaze_runtime::authority::domain_id();
    PARK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|park| {
            park.identity.host != host || park.identity.thread != thread || park.domain != domain
        }) {
            // A fork-restored handle value belongs to the parent. Never close
            // it in the child, where the same integer can name another object.
            core::mem::forget(slot.take());
        }
        if slot.is_none() {
            let identity = ParkIdentity {
                host,
                thread,
                born: current_thread_birth()?,
            };
            let event = Handle::new(unsafe {
                CreateEventW(
                    ptr::null(),
                    0,
                    0,
                    park_name(domain, host, thread, identity.born).as_ptr(),
                )
            })?;
            *slot = Some(Park {
                identity,
                domain,
                event,
            });
        }
        Ok(read(slot.as_ref().unwrap()))
    })
}

pub(crate) fn park() -> Result<HANDLE, i32> {
    with_park(|park| park.event.0)
}

pub(crate) fn park_identity() -> Result<ParkIdentity, i32> {
    with_park(|park| park.identity)
}

pub(super) fn reset_after_fork() {
    let _ = PARK.try_with(|slot| core::mem::forget(slot.borrow_mut().take()));
}

pub(crate) struct Transaction {
    pub(super) shared: &'static Shared,
    _guard: Guard<'static>,
    pub(super) records: Vec<Record>,
    pub(super) dirty: bool,
}

impl Transaction {
    /// Always acquire this guard before the process-local queue mutex. A token
    /// cancellation drops a local-only guard before entering this transaction.
    pub(crate) fn begin() -> Result<Self, i32> {
        let shared = shared()?;
        let guard = shared.acquire()?;
        Self::from_guard(shared, guard)
    }

    pub(super) fn from_guard(shared: &'static Shared, guard: Guard<'static>) -> Result<Self, i32> {
        let records = shared.load()?;
        Ok(Self {
            shared,
            _guard: guard,
            records,
            dirty: false,
        })
    }

    pub(crate) fn reserve(&mut self, count: usize) -> Result<(), i32> {
        self.shared.reserve(&mut self.records, count)?;
        self.records.reserve(count);
        Ok(())
    }

    /// Caller already reserved capacity and holds the local queue lock.
    pub(crate) fn append_private(&mut self, key: Key, identity: ParkIdentity, bitset: u32) -> u64 {
        // Local records use zero to mean "not migrated". A newly created
        // domain (or a wrapped counter) must never publish that sentinel.
        let token = self.shared.token();
        self.records.push(Record {
            key,
            token,
            host: identity.host,
            thread: identity.thread,
            born: identity.born,
            bitset,
            reserved: PRIVATE_PARK,
        });
        self.dirty = true;
        token
    }

    pub(crate) fn wake(&mut self, key: Key, count: u32, bitset: u32) -> Result<u32, i32> {
        if count == 0 {
            return Ok(0);
        }
        // Selection can remove dead rows or select earlier wakes before a
        // later error. Commit that progress even when returning an error.
        self.dirty = true;
        self.shared.select(&mut self.records, key, count, bitset)
    }

    pub(crate) fn transfer(&mut self, source: Key, target: Key, count: u32) -> Result<u32, i32> {
        let mut moved = 0;
        let mut transfers = Vec::new();
        if !optimized() {
            let mut index = 0;
            while index < self.records.len() && moved < count {
                if self.records[index].metadata() || self.records[index].key != source {
                    index += 1;
                    continue;
                }
                let record = self.records[index];
                if record.special_wait() {
                    self.records.extend(transfers);
                    return Err(kinakaze_vfs::EINVAL);
                }
                self.records.remove(index);
                self.dirty = true;
                if record.dead() {
                    continue;
                }
                transfers.push(Record {
                    key: target,
                    ..record
                });
                moved += 1;
            }
            self.records.extend(transfers);
            return Ok(moved);
        }
        let mut error = None;
        self.records.retain(|&record| {
            if error.is_some() || moved >= count || record.metadata() || record.key != source {
                return true;
            }
            if record.special_wait() {
                error = Some(kinakaze_vfs::EINVAL);
                return true;
            }
            if record.dead() {
                self.dirty = true;
                return false;
            }
            transfers.push(Record {
                key: target,
                ..record
            });
            moved += 1;
            false
        });
        self.dirty |= moved != 0;
        self.records.extend(transfers);
        error.map_or(Ok(moved), Err)
    }

    /// true: removed a still-queued token; false: a wake already selected it.
    pub(crate) fn cancel(&mut self, token: u64) -> bool {
        let length = self.records.len();
        self.records
            .retain(|record| record.metadata() || record.token != token);
        let removed = self.records.len() != length;
        self.dirty |= removed;
        removed
    }

    pub(crate) fn contains(&self, token: u64) -> bool {
        self.records
            .iter()
            .any(|record| !record.metadata() && record.token == token)
    }

    pub(crate) fn commit(&mut self) {
        if self.dirty {
            self.shared.commit(&self.records);
            self.dirty = false;
        }
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        self.commit();
    }
}
