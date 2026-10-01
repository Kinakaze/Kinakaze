//! Shared predicate levels, including the exact space required by a sender.
//! Waiters own named events; the packet queue stores only sorted predicates.
//! Closing the last event handle retires a watch without a process registry.
use super::*;
use crate::fs::object::Object;
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, OpenEventW, ResetEvent, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
};

pub(super) const READ: u64 = LIMIT as u64 + 1;
pub(super) const CLOSED: u64 = READ + 1;
pub(super) const MAX_WATCHES: usize = 65_536;
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Watch {
    pub(super) condition: u64,
    pub(super) sender: u64,
}

#[cfg(not(test))]
pub(super) const fn enabled() -> bool {
    true
}
#[cfg(test)]
pub(super) fn enabled() -> bool {
    std::env::var_os("KINAKAZE_TEST_DGRAM_EVENTS").as_deref() != Some(std::ffi::OsStr::new("0"))
}

fn name(record: &procnet::Record, watch: Watch) -> Vec<u16> {
    let domain = kinakaze_runtime::authority::domain_id();
    format!(
        r"Local\kinakaze.dgram-ready.v1.{domain}.{}.{}.{}",
        record.id(),
        watch.condition,
        watch.sender
    )
    .encode_utf16()
    .chain(Some(0))
    .collect()
}

fn ready(queue: &Queue, watch: Watch, used: usize) -> bool {
    match watch.condition {
        READ => queue.read_closed || !queue.messages.is_empty(),
        CLOSED => queue.write_closed,
        length => {
            queue.read_closed
                || queue.peer != 0 && queue.peer != watch.sender
                || queue.messages.len() < 64 && used.saturating_add(length as usize) <= LIMIT
        }
    }
}

fn open(record: &procnet::Record, watch: Watch) -> Result<Option<Object>, i32> {
    let wide = name(record, watch);
    let raw = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            0,
            wide.as_ptr(),
        )
    };
    if raw.is_null() {
        let error = unsafe { GetLastError() };
        if error == ERROR_FILE_NOT_FOUND {
            Ok(None)
        } else {
            Err(crate::errno_from_win32(error))
        }
    } else {
        Object::owned(raw).map(Some)
    }
}

#[cfg(test)]
fn publish_fault(phase: &str) {
    if std::env::var("KINAKAZE_TEST_DGRAM_PUBLISH_FAULT").as_deref() == Ok(phase) {
        std::process::exit(0);
    }
}

pub(super) fn update<T>(
    record: &procnet::Record,
    action: impl FnOnce(&mut Queue) -> Result<T, i32>,
) -> Result<T, i32> {
    let resets = RefCell::new(Vec::new());
    record.update_data_notified(
        |input| {
            let mut queue = Queue::decode(input)?;
            let result = action(&mut queue)?;
            if !queue.watches.is_empty() {
                let used = queue.messages.iter().map(|m| m.payload.len()).sum();
                let mut retained = Vec::with_capacity(queue.watches.len());
                for &watch in &queue.watches {
                    let Some(event) = open(record, watch)? else {
                        continue; // no surviving waiter, including after a hard exit
                    };
                    if ready(&queue, watch, used) {
                        // Signal before publication: a killed publisher cannot
                        // commit a ready queue and strand a sleeping receiver.
                        // A failed commit may cause a harmless predicate retry.
                        if unsafe { SetEvent(event.raw()) } == 0 {
                            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
                        }
                    } else {
                        // Never clear the old committed readiness before the
                        // new bank is published. Retain the opened handle until
                        // reset, so another waiter cannot recreate it meanwhile.
                        resets.borrow_mut().push(event);
                    }
                    retained.push(watch);
                }
                queue.watches = retained;
            }
            #[cfg(test)]
            publish_fault("before");
            Ok((queue.encode(), result))
        },
        || {
            #[cfg(test)]
            publish_fault("after");
            for event in resets.borrow().iter() {
                unsafe { ResetEvent(event.raw()) };
            }
        },
    )
}

pub(super) fn prepare(
    record: &procnet::Record,
    condition: u64,
    sender: u64,
) -> Result<Object, i32> {
    let watch = Watch { condition, sender };
    if condition > CLOSED || (condition > LIMIT as u64 && sender != 0) {
        return Err(EINVAL);
    }
    let wide = name(record, watch);
    let event = Object::owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) })?;
    refresh(record, condition, sender)?;
    Ok(event)
}

/// Reconcile a level with committed data after an interrupted publisher. This
/// also closes the check/register race before entering the native wait.
pub(super) fn refresh(record: &procnet::Record, condition: u64, sender: u64) -> Result<(), i32> {
    let watch = Watch { condition, sender };
    update(record, |queue| {
        if let Err(index) = queue.watches.binary_search(&watch) {
            if queue.watches.len() >= MAX_WATCHES {
                // Reclaim dead predicates before enforcing the live-watch cap.
                let mut live = Vec::with_capacity(queue.watches.len());
                for &existing in &queue.watches {
                    if open(record, existing)?.is_some() {
                        live.push(existing);
                    }
                }
                queue.watches = live;
                if queue.watches.len() >= MAX_WATCHES {
                    return Err(crate::ENOSPC);
                }
                let index = queue.watches.binary_search(&watch).unwrap_err();
                queue.watches.insert(index, watch);
            } else {
                queue.watches.insert(index, watch);
            }
        }
        Ok(())
    })
}

pub(super) fn wait(events: &[HANDLE], timeout: u32) -> Result<(), i32> {
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let mut handles = [std::ptr::null_mut(); 3];
    handles[..events.len()].copy_from_slice(events);
    handles[events.len()] = interrupt;
    signal::register_waiter();
    let interrupted = WAIT_OBJECT_0 + events.len() as u32;
    let result = if signal::interrupt_pending() {
        interrupted
    } else {
        unsafe { WaitForMultipleObjects(events.len() as u32 + 1, handles.as_ptr(), 0, timeout) }
    };
    signal::unregister_waiter();
    if result == interrupted && signal::interrupt_pending() {
        Err(EINTR)
    } else if result <= interrupted || result == WAIT_TIMEOUT {
        Ok(())
    } else {
        Err(EIO)
    }
}
