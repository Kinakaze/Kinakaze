//! Linux futex_waitv: per-entry keys, one shared event and one signal event.
//! Never retain guest memory references while parked or deliver a handler with
//! outer queue entries live. Spurious notifications restart the original budget.

use super::*;
use kinakaze_vfs::{EINTR, EIO, interrupt, signal};
#[cfg(test)]
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};

const MAX: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    value: u64,
    address: u64,
    flags: u32,
    reserved: u32,
}

fn parse(address: usize, count: u32) -> Result<Vec<Entry>, i32> {
    let mut entries = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let location = address
            .checked_add(index * size_of::<Entry>())
            .ok_or(EFAULT)?;
        let entry: Entry = crate::ptrace::read_value(location)?;
        if entry.flags & !0x83 != 0
            || entry.flags & 3 != 2
            || entry.reserved != 0
            || entry.value > u64::from(u32::MAX)
        {
            return Err(EINVAL);
        }
        entries.push(entry);
    }
    Ok(entries)
}

struct Registration {
    private: Vec<(usize, Arc<FutexWaiter>)>,
    shared: Option<crate::futex::WaitGroup>,
    retired: bool,
}

impl Registration {
    fn new() -> Self {
        Self {
            private: Vec::new(),
            shared: None,
            retired: false,
        }
    }

    fn enqueue(&mut self, index: usize, entry: &Entry) -> Result<(), i32> {
        let address = FutexAddress::resolve(entry.address as _, entry.flags & 0x80 != 0)
            .map_err(|error| -error as i32)?;
        if address.shared.is_some() {
            if self.shared.is_none() {
                self.shared = Some(crate::futex::WaitGroup::new()?);
            }
            return self
                .shared
                .as_mut()
                .unwrap()
                .enqueue(index, entry.value as i32, || {
                    let key = crate::fdio::futex_key(entry.address as usize)?;
                    let word = futex_word(entry.address as _).map_err(|error| -error as i32)?;
                    Ok((key, word.load(Ordering::SeqCst)))
                });
        }
        let waiter = FutexWaiter::new(address.key, u32::MAX)?;
        let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
        let word = futex_word(address.word).map_err(|error| -error as i32)?;
        if word.load(Ordering::SeqCst) != entry.value as i32 {
            return Err(EAGAIN);
        }
        queues
            .entry(address.key)
            .or_default()
            .push_back(Arc::clone(&waiter));
        self.private.push((index, waiter));
        Ok(())
    }

    fn publish_batch(&mut self, entries: &[Entry]) -> Result<Vec<crate::futex::Key>, i32> {
        let mut queues = entries
            .iter()
            .any(|entry| entry.flags & 0x80 != 0)
            .then(|| futex_queues().lock().unwrap_or_else(|e| e.into_inner()));
        // Resolve all keys before checking any value, preserving Linux's
        // ordering of a later address fault before an earlier value mismatch.
        let addresses: Vec<_> = entries
            .iter()
            .map(|entry| {
                FutexAddress::resolve(entry.address as _, entry.flags & 0x80 != 0)
                    .map_err(|error| -error as i32)
            })
            .collect::<Result<_, _>>()?;
        let mut keys = Vec::new();
        let mut locals = Vec::new();
        for (index, (entry, address)) in entries.iter().zip(addresses).enumerate() {
            let word = futex_word(address.word).map_err(|error| -error as i32)?;
            if word.load(Ordering::SeqCst) != entry.value as i32 {
                return Err(EAGAIN);
            }
            if let Some(key) = address.shared {
                keys.push(key);
            } else {
                locals.push((index, FutexWaiter::new(address.key, u32::MAX)?));
            }
        }
        for (_, waiter) in &locals {
            queues
                .as_mut()
                .unwrap()
                .entry(waiter.address.load(Ordering::Relaxed))
                .or_default()
                .push_back(Arc::clone(waiter));
        }
        self.private = locals;
        Ok(keys)
    }

    fn enqueue_all(&mut self, entries: &[Entry]) -> Result<(), i32> {
        if crate::futex::optimized() {
            let indices: Vec<_> = entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| (entry.flags & 0x80 == 0).then_some(index))
                .collect();
            if indices.is_empty() {
                self.publish_batch(entries)?;
            } else {
                self.shared = Some(crate::futex::WaitGroup::enqueue_batch(&indices, || {
                    self.publish_batch(entries)
                })?);
            }
            Ok(())
        } else {
            // The reference setup locks and publishes one entry at a time.
            for entry in entries {
                FutexAddress::resolve(entry.address as _, entry.flags & 0x80 != 0)
                    .map_err(|error| -error as i32)?;
            }
            entries
                .iter()
                .enumerate()
                .try_for_each(|(index, entry)| self.enqueue(index, entry))
        }
    }

    /// Cancel every registration and return any selected entry's original
    /// vector index, even if its key has changed after a legacy requeue.
    fn finish(&mut self) -> Result<Option<usize>, i32> {
        let shared = self.shared.as_ref().map(|g| g.finish(true)).transpose();
        let selected = super::futex_requeue::retire(&mut self.private)?;
        self.retired = true;
        // A concurrent private selection still wins over a shared cleanup error.
        if selected.is_some() {
            Ok(selected)
        } else {
            Ok(shared?.flatten())
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.retired {
            let _ = self.finish();
        }
    }
}

pub(super) fn waitv(
    address: usize,
    count: u32,
    flags: u32,
    timeout: *const KernelTimespec,
    clock: i32,
) -> i64 {
    let result = (|| {
        // Linux validates syscall flags/count/null before timeout, then copies
        // every entry's flags/value before resolving any futex memory key.
        if flags != 0 || count == 0 || count as usize > MAX || address == 0 {
            return Err(EINVAL);
        }
        if !timeout.is_null() && !matches!(i64::from(clock), CLOCK_REALTIME | CLOCK_MONOTONIC) {
            return Err(EINVAL);
        }
        'restart: loop {
            let deadline = futex_timeout(timeout, true, i64::from(clock) == CLOCK_REALTIME)
                .map_err(|error| -error as i32)?;
            let entries = parse(address, count)?;
            loop {
                let event = interrupt::current();
                if event.is_null() {
                    return Err(EIO);
                }
                let park = crate::futex::park()?;
                let mut registration = Registration::new();
                let setup = registration.enqueue_all(&entries);
                if let Err(error) = setup {
                    let selected = registration.finish()?;
                    return selected.ok_or(error);
                }
                signal::register_waiter();
                let status = loop {
                    let pending = signal::interrupt_pending();
                    let expired = deadline.expired();
                    if pending || expired {
                        break WAIT_OBJECT_0;
                    }
                    let handles = [
                        event,
                        park,
                        registration
                            .shared
                            .as_ref()
                            .map_or(event, |group| group.event()),
                    ];
                    let sources = if registration.shared.is_some() {
                        &handles[..]
                    } else {
                        &handles[..2]
                    };
                    let status = unsafe { deadline.wait(sources) };
                    if status == WAIT_OBJECT_0 + 1
                        && matches!(
                            super::futex_requeue::selected(
                                registration.private.iter().map(|(_, waiter)| waiter)
                            ),
                            Ok(false)
                        )
                    {
                        continue; // native notification without committed removal
                    }
                    if status == WAIT_OBJECT_0 + 2
                        && let Some(group) = &registration.shared
                        && matches!(group.finish(false), Ok(None))
                    {
                        continue; // pre-commit notification from an abandoned waker
                    }
                    break status;
                };
                let selected = registration.finish();
                signal::unregister_waiter();
                let expired = deadline.expired();
                // Drop all named handles before running guest handlers, which may
                // fork or enter a nested wait. Capture no parent-only resources.
                drop(registration);
                let delivery = signal::deliver_pending();
                if let Some(index) = selected? {
                    return Ok(index);
                }
                if status != WAIT_OBJECT_0
                    && status != WAIT_OBJECT_0 + 1
                    && status != WAIT_OBJECT_0 + 2
                    && status != WAIT_TIMEOUT
                {
                    return Err(EIO);
                }
                if expired {
                    return Err(ETIMEDOUT);
                }
                if delivery == signal::Delivery::Interrupted {
                    return Err(EINTR);
                }
                if delivery == signal::Delivery::Restart {
                    continue 'restart;
                }
            }
        }
    })();
    result.map_or_else(|error| -i64::from(error), |index| index as i64)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "futex_vector/batch_tests.rs"]
mod batch_tests;
