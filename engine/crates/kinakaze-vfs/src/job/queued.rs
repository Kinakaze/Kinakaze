//! Value-bearing process signals. Records stay in the kernel-domain store until
//! delivery, so a blocked receiver cannot evade the queue bound by draining its
//! notification event. The holder identity lets an exec replacement recover
//! pending records while fork starts with an empty queue, as Linux requires.
use super::*;
use crate::mount::shared::Store;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, OnceLock};

const MAGIC: &[u8; 8] = b"CYSIGQ01";
const ROW: usize = 48;
const LIMIT_PER_UID: usize = 1024;

#[derive(Clone, Copy)]
pub(crate) struct Record {
    target: u32,
    pub signal: i32,
    born: u64,
    pub id: u64,
    holder: u32,
    pub sender: u32,
    pub uid: u32,
    pub value: usize,
}
fn store() -> Result<Arc<Store>, i32> {
    static STORE: OnceLock<Arc<Store>> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store.clone());
    }
    let opened = Arc::new(Store::user_object(u64::MAX - 52, true)?);
    let _ = STORE.set(opened);
    Ok(STORE.get().unwrap().clone())
}
pub(super) fn initialize() -> Result<(), i32> {
    store().map(|_| ())
}

fn decode(bytes: &[u8]) -> Result<(u64, Vec<Record>), i32> {
    if bytes.is_empty() {
        return Ok((1, Vec::new()));
    }
    if bytes.len() < 16 || &bytes[..8] != MAGIC || !(bytes.len() - 16).is_multiple_of(ROW) {
        return Err(EIO);
    }
    let serial = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let records = bytes[16..]
        .chunks_exact(ROW)
        .map(|row| {
            let dword = |at| u32::from_le_bytes(row[at..at + 4].try_into().unwrap());
            let qword = |at| u64::from_le_bytes(row[at..at + 8].try_into().unwrap());
            Record {
                target: dword(0),
                signal: dword(4) as i32,
                born: qword(8),
                id: qword(16),
                holder: dword(24),
                sender: dword(28),
                uid: dword(32),
                value: qword(40) as usize,
            }
        })
        .collect();
    Ok((serial, records))
}
fn encode(serial: u64, records: &[Record]) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&serial.to_le_bytes());
    for record in records {
        bytes.extend_from_slice(&record.target.to_le_bytes());
        bytes.extend_from_slice(&record.signal.to_le_bytes());
        bytes.extend_from_slice(&record.born.to_le_bytes());
        bytes.extend_from_slice(&record.id.to_le_bytes());
        bytes.extend_from_slice(&record.holder.to_le_bytes());
        bytes.extend_from_slice(&record.sender.to_le_bytes());
        bytes.extend_from_slice(&record.uid.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&record.value.to_le_bytes());
    }
    bytes
}
fn live(record: &Record) -> bool {
    table::lookup(record.target).is_some_and(|entry| {
        entry.start_ticks == record.born && entry.flags & table::FLAG_ZOMBIE == 0
    })
}

pub fn send(pid: i32, signal: i32, value: usize) -> Result<(), i32> {
    if pid <= 0 || signal < 0 || signal as usize >= signal::NSIG {
        return Err(EINVAL);
    }
    ensure_registered();
    let pid = table::namespaces::resolve(pid as u32).ok_or(ESRCH)?;
    let target = table::resolve(pid)
        .filter(|entry| entry.flags & table::FLAG_ZOMBIE == 0)
        .ok_or(ESRCH)?;
    let identity = crate::credentials::identity();
    let target_user = table::user_namespace(target.namespace_pid).ok_or(ESRCH)?;
    let ids = crate::credentials::process_ids(target.namespace_pid);
    let permitted = crate::user_namespace::capable(target_user, 5)
        || ids.is_ok_and(|ids| {
            [identity.uids[0], identity.uids[1]]
                .iter()
                .any(|uid| *uid == ids[0] || *uid == ids[2])
        })
        || (signal == SIGCONT && target.sid == current_sid() as u32);
    if !permitted {
        return Err(EPERM);
    }
    if signal == 0 {
        return Ok(());
    }
    if matches!(signal, SIGKILL | SIGSTOP) {
        return if deliver_to(pid, signal) {
            Ok(())
        } else {
            Err(ESRCH)
        };
    }
    if namespace_init(target.namespace_pid)
        && target.namespace_pid == current_pid()
        && signal::current_action(signal).is_ok_and(|action| {
            matches!(
                action.disposition,
                signal::Disposition::Default | signal::Disposition::Ignore
            )
        })
    {
        return Ok(());
    }
    let wake = table::notifications::event("wake", target.pid).ok_or(EIO)?;
    store()?.update(|bytes| {
        let (serial, mut records) = decode(bytes)?;
        records.retain(live);
        // Standard signals coalesce and retain the first sender/value.
        if signal < 32
            && records
                .iter()
                .any(|r| r.target == target.namespace_pid && r.signal == signal)
        {
            return Ok((encode(serial, &records), ()));
        }
        if records.iter().filter(|r| r.uid == identity.uids[0]).count() >= LIMIT_PER_UID {
            return Err(crate::EAGAIN);
        }
        let next = serial.checked_add(1).ok_or(crate::EAGAIN)?;
        records.push(Record {
            target: target.namespace_pid,
            signal,
            born: target.start_ticks,
            id: serial,
            holder: 0,
            sender: current_pid(),
            uid: identity.uids[0],
            value,
        });
        // Notify before commit while holding the store mutex. A receiver that
        // wakes early waits for this transaction; an abandoned write is harmless.
        if unsafe { SetEvent(wake.as_raw_handle()) } == 0 {
            return Err(EIO);
        }
        if signal == SIGCONT {
            table::set_state(target.namespace_pid, STATE_RUNNING, 0);
            poke("cont", target.pid);
        }
        Ok((encode(next, &records), ()))
    })?;
    if target.namespace_pid == current_pid() {
        let _ = drain_external_notified(true);
    }
    Ok(())
}

pub(super) fn drain() -> Result<Vec<Record>, i32> {
    store()?.update(|bytes| {
        let (serial, mut records) = decode(bytes)?;
        records.retain(live);
        let mut result = Vec::new();
        for record in &mut records {
            if record.target == current_pid() && record.holder != current_host_pid() {
                record.holder = current_host_pid();
                result.push(*record);
            }
        }
        Ok((encode(serial, &records), result))
    })
}
pub(crate) fn complete(id: u64) {
    let result = store().and_then(|store| {
        store.update(|bytes| {
            let (serial, mut records) = decode(bytes)?;
            records.retain(|r| r.id != id);
            Ok((encode(serial, &records), ()))
        })
    });
    if result.is_err() {
        pump_failed("acknowledge queued signal");
    }
}
