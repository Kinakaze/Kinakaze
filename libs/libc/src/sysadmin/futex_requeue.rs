//! futex2 requeue and the private/shared bridge used by legacy wake/requeue.
//! Lock order is domain transaction, then local queues. Migration never means
//! wake: a private waiter follows its stable token until selection or cancel.

use super::*;

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    value: u64,
    address: u64,
    flags: u32,
    reserved: u32,
}

pub(super) fn key(address: FutexAddress) -> Result<crate::futex::Key, i32> {
    address
        .shared
        .map_or_else(|| crate::futex::Key::flagged_private(address.key), Ok)
}

pub(super) fn promote(
    transaction: &mut crate::futex::Transaction,
    queues: &mut FutexQueue,
    address: FutexAddress,
    key: crate::futex::Key,
) {
    if address.shared.is_some() {
        return;
    }
    if let Some(waiters) = queues.remove(&address.key) {
        for waiter in waiters {
            let token = transaction.append_private(key, waiter.park, waiter.bitset);
            waiter.token.store(token, Ordering::Release);
        }
    }
}

pub(super) fn requeue(
    source: FutexAddress,
    target: FutexAddress,
    wake: u32,
    transfer: u32,
    comparison: Option<i32>,
) -> i64 {
    let result = (|| {
        let source_key = key(source)?;
        let target_key = key(target)?;
        // Set before acquiring either queue. False positives after an error
        // are safe; a post-publication store could miss a private wake.
        FUTEX_PRIVATE_BRIDGED.store(true, Ordering::Release);
        let mut transaction = crate::futex::Transaction::begin()?;
        let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(expected) = comparison {
            let word = futex_word(source.word).map_err(|error| -error as i32)?;
            if word.load(Ordering::SeqCst) != expected {
                return Err(EAGAIN);
            }
        }
        if wake == 0 && transfer == 0 {
            return Ok(0);
        }
        let source_rows = if source.shared.is_none() {
            queues.get(&source.key).map_or(0, VecDeque::len)
        } else {
            0
        };
        let target_rows = if target.shared.is_none() && target.key != source.key {
            queues.get(&target.key).map_or(0, VecDeque::len)
        } else {
            0
        };
        transaction.reserve(source_rows + target_rows)?;
        // Existing destination members precede incoming transfers. The same
        // VA with different PRIVATE tags denotes two different queues.
        promote(&mut transaction, &mut queues, target, target_key);
        if source_key != target_key {
            promote(&mut transaction, &mut queues, source, source_key);
        }
        let selected = transaction.wake(source_key, wake, u32::MAX)?;
        let moved = transaction.transfer(source_key, target_key, transfer)?;
        transaction.commit();
        Ok(selected + moved)
    })();
    result.map_or_else(|error| -i64::from(error), i64::from)
}

pub(super) fn wake(address: FutexAddress, count: u32, bitset: u32) -> i64 {
    let result: Result<i64, i32> = (|| {
        let key = key(address)?;
        let mut transaction = crate::futex::Transaction::begin()?;
        let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
        let selected = transaction.wake(key, count, bitset)?;
        let local = select_futex_wake(&mut queues, address.key, count - selected, bitset);
        Ok(i64::from(selected) + local as i64)
    })();
    result.unwrap_or_else(|error| -i64::from(error))
}

pub(super) fn wake_op(
    source: FutexAddress,
    count: u32,
    target: FutexAddress,
    count2: u32,
    operation: impl FnOnce() -> Result<bool, i64>,
) -> i64 {
    let result = (|| {
        let source_key = key(source).map_err(|error| -i64::from(error))?;
        let target_key = key(target).map_err(|error| -i64::from(error))?;
        let mut transaction =
            crate::futex::Transaction::begin().map_err(|error| -i64::from(error))?;
        let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
        let second = operation()?;
        let mut select = |address: FutexAddress, key, count| -> Result<i64, i64> {
            let global = transaction
                .wake(key, count, u32::MAX)
                .map_err(|error| -i64::from(error))?;
            let local = select_futex_wake(&mut queues, address.key, count - global, u32::MAX);
            Ok(i64::from(global) + local as i64)
        };
        let first = select(source, source_key, count)?;
        Ok(first
            + if second {
                select(target, target_key, count2)?
            } else {
                0
            })
    })();
    result.unwrap_or_else(|error| error)
}

pub(super) fn retire(waiters: &mut Vec<(usize, Arc<FutexWaiter>)>) -> Result<Option<usize>, i32> {
    if waiters.is_empty() {
        return Ok(None);
    }
    let mut transaction = None;
    let mut queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
    if waiters
        .iter()
        .any(|(_, waiter)| waiter.token.load(Ordering::Acquire) != 0)
    {
        drop(queues);
        transaction = Some(crate::futex::Transaction::begin()?);
        queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
    }
    let mut selected = None;
    for (index, waiter) in waiters.drain(..) {
        let token = waiter.token.load(Ordering::Acquire);
        let removed = if token != 0 {
            transaction.as_mut().unwrap().cancel(token)
        } else {
            let address = waiter.address.load(Ordering::Acquire);
            let removed = queues.get_mut(&address).is_some_and(|queue| {
                queue
                    .iter()
                    .position(|other| Arc::ptr_eq(other, &waiter))
                    .and_then(|index| queue.remove(index))
                    .is_some()
            });
            remove_empty_futex_queue(&mut queues, address);
            removed
        };
        if !removed {
            selected = Some(index);
        }
    }
    Ok(selected)
}

/// A native park event can precede an abandoned waker's bank publication. Only
/// committed removal constitutes a wake. Acquire domain before local mutex.
pub(super) fn selected<'a>(
    waiters: impl Iterator<Item = &'a Arc<FutexWaiter>>,
) -> Result<bool, i32> {
    let transaction = crate::futex::Transaction::begin()?;
    let queues = futex_queues().lock().unwrap_or_else(|e| e.into_inner());
    for waiter in waiters {
        let token = waiter.token.load(Ordering::Acquire);
        if token != 0 {
            if !transaction.contains(token) {
                return Ok(true);
            }
        } else {
            let address = waiter.address.load(Ordering::Acquire);
            if !queues
                .get(&address)
                .is_some_and(|queue| queue.iter().any(|other| Arc::ptr_eq(other, waiter)))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn syscall(address: usize, flags: u32, wake: i32, transfer: i32) -> i64 {
    let result = (|| {
        if flags != 0 || address == 0 {
            return Err(EINVAL);
        }
        let mut entries = [Entry {
            value: 0,
            address: 0,
            flags: 0,
            reserved: 0,
        }; 2];
        for (index, slot) in entries.iter_mut().enumerate() {
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
            *slot = entry;
        }
        if wake < 0 || transfer < 0 {
            return Err(EINVAL);
        }
        let source = FutexAddress::resolve(entries[0].address as _, entries[0].flags & 0x80 != 0)
            .map_err(|error| -error as i32)?;
        let target = FutexAddress::resolve(entries[1].address as _, entries[1].flags & 0x80 != 0)
            .map_err(|error| -error as i32)?;
        let result = if let (Some(key), Some(destination)) = (source.shared, target.shared) {
            let word = futex_word(source.word).map_err(|error| -error as i32)?;
            crate::futex::requeue(
                key,
                destination,
                wake as u32,
                transfer as u32,
                Some((word, entries[0].value as i32)),
            )
            .map_or_else(|error| -i64::from(error), i64::from)
        } else if source.shared.is_none()
            && target.shared.is_none()
            && !FUTEX_PRIVATE_BRIDGED.load(Ordering::Acquire)
        {
            futex_syscall(
                source.word,
                (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
                wake as u32,
                transfer as usize as _,
                target.word,
                entries[0].value as u32,
            )
        } else {
            requeue(
                source,
                target,
                wake as u32,
                transfer as u32,
                Some(entries[0].value as i32),
            )
        };
        if result < 0 {
            Err(-result as i32)
        } else {
            Ok(result)
        }
    })();
    result.unwrap_or_else(|error| -i64::from(error))
}

#[cfg(test)]
mod tests;
