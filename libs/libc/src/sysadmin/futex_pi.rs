//! Legacy PI syscall adapters. A signal retires the waiter before running guest
//! handlers and asks the entry wrapper to re-copy its absolute deadline even
//! without SA_RESTART (Linux ERESTARTNOINTR semantics).

use super::*;
use crate::futex::pi::{self, Acquisition};
use kinakaze_vfs::{EIO, interrupt, signal};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::INFINITE;

pub(super) const RESTART: i64 = i64::MIN;

pub(crate) fn transaction(
    address: usize,
    private: bool,
) -> Result<(crate::futex::Transaction, crate::futex::Key), i32> {
    let address = FutexAddress::resolve(address as _, private).map_err(|error| -error as i32)?;
    let (transaction, key, queues) = begin(address)?;
    drop(queues);
    Ok((transaction, key))
}

fn begin(
    address: FutexAddress,
) -> Result<
    (
        crate::futex::Transaction,
        crate::futex::Key,
        std::sync::MutexGuard<'static, FutexQueue>,
    ),
    i32,
> {
    let key = futex_requeue::key(address)?;
    if address.shared.is_none() {
        FUTEX_PRIVATE_BRIDGED.store(true, Ordering::Release);
    }
    let mut transaction = crate::futex::Transaction::begin()?;
    let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
    let count = if address.shared.is_none() {
        queues.get(&address.key).map_or(0, VecDeque::len)
    } else {
        0
    };
    transaction.reserve(count)?;
    futex_requeue::promote(&mut transaction, &mut queues, address, key);
    // Publication of migrated tokens happens while the local queue is locked.
    transaction.commit();
    Ok((transaction, key, queues))
}

pub(super) fn unlock(word: *mut c_int, private: bool) -> i64 {
    let tid = current_tid() as u32;
    let result = (|| {
        // Wrong-owner EPERM precedes key alignment and write-access checks.
        let value: u32 = crate::ptrace::read_value(word as usize)?;
        if value & 0x3fff_ffff != tid {
            return Err(EPERM);
        }
        let address = FutexAddress::resolve(word, private).map_err(|error| -error as i32)?;
        let (mut transaction, key, _queues) = begin(address)?;
        pi::unlock(&mut transaction, key, word as usize, tid)
    })();
    result.map_or_else(|error| -i64::from(error), |()| 0)
}

pub(super) fn lock(
    address: FutexAddress,
    try_only: bool,
    deadline: futex_deadline::Prepared,
) -> i64 {
    let result = attempt(address, try_only, deadline);
    result.map_or_else(
        |error| -i64::from(error),
        |restart| if restart { RESTART } else { 0 },
    )
}

fn attempt(
    address: FutexAddress,
    try_only: bool,
    deadline: futex_deadline::Prepared,
) -> Result<bool, i32> {
    let tid = current_tid() as u32;
    pi::register_current(tid as i32)?;
    let park = crate::futex::park()?;
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let (mut transaction, key, queues) = begin(address)?;
    let token = match pi::acquire(&mut transaction, key, address.word as usize, tid, try_only)? {
        Acquisition::Owned => return Ok(false),
        Acquisition::Queued(token) => token,
    };
    drop(queues);
    drop(transaction);
    signal::register_waiter();
    let outcome = loop {
        let pending = signal::interrupt_pending();
        let expired = deadline
            .duration
            .is_some_and(|duration| deadline.started.elapsed() >= duration);
        let (mut transaction, _, queues) = match begin(address) {
            Ok(value) => value,
            Err(error) => break Err(error),
        };
        match pi::poll(
            &mut transaction,
            key,
            address.word as usize,
            token,
            pending || expired,
        ) {
            Ok(true) => break Ok(true),
            Ok(false) if expired => break Err(ETIMEDOUT),
            Ok(false) if pending => break Ok(false),
            Ok(false) => {}
            Err(error) => {
                pi::cancel_error(&mut transaction, key, token);
                break Err(error);
            }
        }
        let owner = match pi::owner_handle(&transaction, key) {
            Ok(owner) => owner,
            Err(3) => {
                drop(queues);
                drop(transaction);
                continue;
            }
            Err(error) => {
                pi::cancel_error(&mut transaction, key, token);
                break Err(error);
            }
        };
        drop(queues);
        drop(transaction);
        let milliseconds = deadline.duration.map_or(INFINITE, |duration| {
            duration
                .saturating_sub(deadline.started.elapsed())
                .as_nanos()
                .div_ceil(1_000_000)
                .min(u128::from(INFINITE - 1)) as u32
        });
        let status = unsafe {
            kinakaze_vfs::deadline_wait::any(&[park, interrupt, owner.raw()], milliseconds)
        };
        if !matches!(status, WAIT_OBJECT_0 | WAIT_TIMEOUT)
            && status != WAIT_OBJECT_0 + 1
            && status != WAIT_OBJECT_0 + 2
        {
            if let Ok((mut transaction, _, _queues)) = begin(address) {
                pi::cancel_error(&mut transaction, key, token);
            }
            break Err(EIO);
        }
        // A pre-commit event, timeout, signal or native owner death is resolved
        // by the committed queue on the next iteration, under the domain guard.
    };
    signal::unregister_waiter();
    let _ = signal::deliver_pending();
    if outcome? {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
