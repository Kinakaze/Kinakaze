//! WAIT_REQUEUE_PI/CMP_REQUEUE_PI entry and signal boundaries. Source and
//! destination queues use one domain transaction before the local queue lock.

use super::*;
use crate::futex::pi::{self, RequeueStatus};
use kinakaze_vfs::{EIO, interrupt, signal};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};

fn write_key(word: *mut c_int, private: bool) -> Result<FutexAddress, i64> {
    let address = FutexAddress::resolve(word, private)?;
    // Linux PRIVATE keys use access_ok and alignment, not a mapped-page/write
    // probe. Shared keys pin the destination for write before any value check.
    if !private {
        futex_access(word as usize, 4, true)?;
    }
    Ok(address)
}

fn begin(
    source: FutexAddress,
    target: FutexAddress,
) -> Result<
    (
        crate::futex::Transaction,
        crate::futex::Key,
        crate::futex::Key,
        std::sync::MutexGuard<'static, FutexQueue>,
    ),
    i32,
> {
    let source_key = futex_requeue::key(source)?;
    let target_key = futex_requeue::key(target)?;
    if source.shared.is_none() || target.shared.is_none() {
        FUTEX_PRIVATE_BRIDGED.store(true, Ordering::Release);
    }
    let mut transaction = crate::futex::Transaction::begin()?;
    let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
    let count = [source, target]
        .into_iter()
        .filter(|address| address.shared.is_none())
        .map(|address| queues.get(&address.key).map_or(0, VecDeque::len))
        .sum();
    transaction.reserve(count)?;
    futex_requeue::promote(&mut transaction, &mut queues, target, target_key);
    if source_key != target_key {
        futex_requeue::promote(&mut transaction, &mut queues, source, source_key);
    }
    transaction.commit();
    Ok((transaction, source_key, target_key, queues))
}

pub(super) fn compare(
    source: *mut c_int,
    target: *mut c_int,
    private: bool,
    expected: u32,
    count: u32,
) -> i64 {
    let result = (|| {
        let source = FutexAddress::resolve(source, private).map_err(|error| -error as i32)?;
        let target = write_key(target, private).map_err(|error| -error as i32)?;
        let (mut transaction, key, target_key, _queues) = begin(source, target)?;
        pi::compare_requeue(
            &mut transaction,
            key,
            source.word as usize,
            target_key,
            target.word as usize,
            expected,
            count,
        )
    })();
    result.map_or_else(
        |error| {
            if error == pi::RETRY {
                futex_pi::RESTART
            } else {
                -i64::from(error)
            }
        },
        i64::from,
    )
}

pub(super) fn wait(
    source: *mut c_int,
    target: *mut c_int,
    private: bool,
    expected: u32,
    deadline: futex_deadline::Prepared,
) -> i64 {
    // sys_futex has copied the timeout before this same-VA refusal. Resolve the
    // destination first; same-key aliases are checked after the source value.
    if source == target {
        return -i64::from(EINVAL);
    }
    let target = match write_key(target, private) {
        Ok(target) => target,
        Err(error) => return error,
    };
    let source = match FutexAddress::resolve(source, private) {
        Ok(source) => source,
        Err(error) => return error,
    };
    attempt(source, target, expected, deadline).unwrap_or_else(|error| {
        if error == pi::RETRY {
            futex_pi::RESTART
        } else {
            -i64::from(error)
        }
    })
}

fn attempt(
    source: FutexAddress,
    target: FutexAddress,
    expected: u32,
    deadline: futex_deadline::Prepared,
) -> Result<i64, i32> {
    let tid = current_tid() as u32;
    pi::register_current(tid as i32)?;
    let park = crate::futex::park()?;
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let (mut transaction, key, target_key, queues) = begin(source, target)?;
    let token = pi::enqueue_requeue(
        &mut transaction,
        key,
        source.word as usize,
        expected,
        target_key,
        tid,
    )?;
    drop(queues);
    drop(transaction);
    signal::register_waiter();
    let outcome = loop {
        let pending = signal::interrupt_pending();
        let expired = deadline.expired();
        let (mut transaction, _, _, queues) = match begin(source, target) {
            Ok(value) => value,
            Err(error) => break Err(error),
        };
        let stage = match pi::poll_requeue(
            &mut transaction,
            key,
            target_key,
            target.word as usize,
            token,
            pending || expired,
        ) {
            Ok(stage) => stage,
            Err(error) => {
                pi::retire_requeue(&mut transaction, target_key, token);
                break Err(error);
            }
        };
        if matches!(stage, RequeueStatus::Owned) {
            break Ok(0);
        }
        if expired {
            break Err(ETIMEDOUT);
        }
        if pending {
            break if matches!(stage, RequeueStatus::Source) {
                Ok(futex_pi::RESTART)
            } else {
                Err(EAGAIN)
            };
        }
        let owner = if matches!(stage, RequeueStatus::Moved) {
            match pi::owner_handle(&transaction, target_key) {
                Ok(owner) => Some(owner),
                Err(3) => {
                    drop(queues);
                    drop(transaction);
                    continue;
                }
                Err(error) => {
                    pi::retire_requeue(&mut transaction, target_key, token);
                    break Err(error);
                }
            }
        } else {
            None
        };
        drop(queues);
        drop(transaction);
        let handles = [
            park,
            interrupt,
            owner.as_ref().map_or(park, |owner| owner.raw()),
        ];
        let handles = &handles[..if owner.is_some() { 3 } else { 2 }];
        let status = unsafe { deadline.wait(handles) };
        if status != WAIT_TIMEOUT
            && !(WAIT_OBJECT_0..WAIT_OBJECT_0 + handles.len() as u32).contains(&status)
        {
            if let Ok((mut transaction, _, _, _queues)) = begin(source, target) {
                pi::retire_requeue(&mut transaction, target_key, token);
            }
            break Err(EIO);
        }
    };
    signal::unregister_waiter();
    let _ = signal::deliver_pending();
    outcome
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod prior_tests;
