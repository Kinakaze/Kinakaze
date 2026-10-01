//! The source sleep and PI destination share one domain guard. A source signal
//! restarts with freshly copied arguments; after migration it returns EAGAIN.
use super::*;
use crate::futex::{
    Key, Transaction,
    pi::{
        self,
        requeue::{self, Progress},
    },
};
use kinakaze_vfs::{EIO, interrupt, signal};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::INFINITE;

fn begin(
    source: FutexAddress,
    target: FutexAddress,
) -> Result<
    (
        Transaction,
        Key,
        Key,
        std::sync::MutexGuard<'static, FutexQueue>,
    ),
    i32,
> {
    let source_key = futex_requeue::key(source)?;
    let target_key = futex_requeue::key(target)?;
    if source.shared.is_none() || target.shared.is_none() {
        FUTEX_PRIVATE_BRIDGED.store(true, Ordering::Release);
    }
    let mut transaction = Transaction::begin()?;
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

pub(super) fn transfer(
    source: FutexAddress,
    target: FutexAddress,
    expected: u32,
    count: u32,
) -> i64 {
    let result = (|| {
        let (mut transaction, source_key, target_key, _queues) = begin(source, target)?;
        // The mandatory first waiter moves even when the extra count is zero.
        requeue::transfer(
            &mut transaction,
            source_key,
            target_key,
            source.word as usize,
            target.word as usize,
            expected,
            count + 1,
        )
    })();
    result.map_or_else(|error| -i64::from(error), i64::from)
}

pub(super) fn wait(
    source: FutexAddress,
    target: FutexAddress,
    expected: u32,
    deadline: futex_deadline::Prepared,
) -> i64 {
    attempt(source, target, expected, deadline).unwrap_or_else(|error| -i64::from(error))
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
    let (mut transaction, source_key, target_key, queues) = begin(source, target)?;
    let token = requeue::enqueue(
        &mut transaction,
        source_key,
        target_key,
        source.word as usize,
        expected,
        tid,
    )?;
    drop(queues);
    drop(transaction);
    signal::register_waiter();
    let mut target_observed = false;
    let outcome = loop {
        let pending = signal::interrupt_pending();
        let expired = deadline
            .duration
            .is_some_and(|duration| deadline.started.elapsed() >= duration);
        let (mut transaction, _, _, queues) = match begin(source, target) {
            Ok(value) => value,
            Err(error) => break Err(error),
        };
        let progress = match requeue::poll_waiter(
            &mut transaction,
            target_key,
            target.word as usize,
            token,
            pending || expired,
            !target_observed,
        ) {
            Ok(value) => value,
            Err(error) => {
                pi::cancel_error(&mut transaction, target_key, token);
                break Err(error);
            }
        };
        let owner = match progress {
            Progress::Owned => break Ok(0),
            Progress::Source => {
                if expired {
                    break Err(ETIMEDOUT);
                }
                if pending {
                    break Ok(futex_pi::RESTART);
                }
                None
            }
            Progress::Queued => {
                target_observed = true;
                if expired {
                    break Err(ETIMEDOUT);
                }
                if pending {
                    break Err(EAGAIN);
                }
                match pi::owner_handle(&transaction, target_key) {
                    Ok(owner) => Some(owner),
                    Err(3) => {
                        drop(queues);
                        drop(transaction);
                        continue;
                    }
                    Err(error) => {
                        pi::cancel_error(&mut transaction, target_key, token);
                        break Err(error);
                    }
                }
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
        let handles = [
            park,
            interrupt,
            owner
                .as_ref()
                .map_or(core::ptr::null_mut(), |owner| owner.raw()),
        ];
        let status = unsafe {
            kinakaze_vfs::deadline_wait::any(
                &handles[..if owner.is_some() { 3 } else { 2 }],
                milliseconds,
            )
        };
        if status != WAIT_TIMEOUT
            && !(WAIT_OBJECT_0..WAIT_OBJECT_0 + if owner.is_some() { 3 } else { 2 })
                .contains(&status)
        {
            if let Ok((mut transaction, _, _, _queues)) = begin(source, target) {
                pi::cancel_error(&mut transaction, target_key, token);
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
