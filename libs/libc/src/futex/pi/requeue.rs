//! PI-aware source queues and proxy acquisition. A target relation is a value
//! record, not a pointer into the sleeping process. Both phases keep one token.

use super::*;

fn relation(transaction: &Transaction, token: u64) -> Option<Key> {
    transaction
        .records
        .iter()
        .find(|record| record.token == token && record.reserved & TARGET != 0)
        .map(|record| record.key)
}

fn remove_relation(transaction: &mut Transaction, token: u64) {
    let before = transaction.records.len();
    transaction
        .records
        .retain(|record| !(record.token == token && record.reserved & TARGET != 0));
    transaction.dirty |= before != transaction.records.len();
}

fn prune_source(transaction: &mut Transaction, source: Key) {
    let dead: Vec<u64> = transaction
        .records
        .iter()
        .filter(|record| record.key == source && record.pi_source() && record.dead())
        .map(|record| record.token)
        .collect();
    for token in dead {
        transaction.cancel(token);
        remove_relation(transaction, token);
    }
}

pub(crate) fn enqueue_requeue(
    transaction: &mut Transaction,
    source: Key,
    address: usize,
    expected: u32,
    target: Key,
    tid: u32,
) -> Result<u64, i32> {
    // The value check wins over same-key aliases, as futex_wait_setup does.
    if atomic_word::read(address)? != expected {
        return Err(EAGAIN);
    }
    if source == target {
        return Err(EINVAL);
    }
    prune_source(transaction, source);
    transaction.reserve(2)?;
    let identity = park_identity()?;
    let token = transaction.append_private(source, identity, tid);
    transaction.records.last_mut().unwrap().reserved |= REQUEUE;
    transaction.records.push(
        Identity {
            host: identity.host,
            thread: identity.thread,
            born: identity.born,
        }
        .record(target, token, tid, TARGET),
    );
    transaction.dirty = true;
    transaction.commit();
    Ok(token)
}

pub(crate) enum RequeueStatus {
    Source,
    Moved,
    Owned,
}

/// The faultable CAS did not write the target word. Restore the single proxy
/// head to its source before returning an error or retrying atomic contention.
pub(super) fn rollback_proxy(transaction: &mut Transaction, target: Key, token: u64) {
    let source = transaction
        .records
        .iter()
        .find(|record| record.token == token && record.reserved & SOURCE_JOURNAL != 0)
        .map(|record| record.key);
    if let Some(source) = source {
        if let Some(index) = transaction
            .records
            .iter()
            .position(|record| !record.metadata() && record.token == token)
        {
            let mut record = transaction.records.remove(index);
            record.key = source;
            record.reserved = hybrid::PRIVATE_PARK | REQUEUE;
            let position = transaction
                .records
                .iter()
                .position(|record| record.key == source && !record.metadata())
                .unwrap_or(transaction.records.len());
            transaction.records.insert(position, record);
        }
    }
    transaction.records.retain(|record| {
        !(record.token == token && record.reserved & SOURCE_JOURNAL != 0)
            && !(record.key == target
                && record.token == token
                && record.reserved & (STATE | JOURNAL) != 0)
    });
    transaction.dirty = true;
    transaction.commit();
}

pub(crate) fn retire_requeue(transaction: &mut Transaction, target: Key, token: u64) {
    let moved = transaction
        .records
        .iter()
        .any(|record| !record.metadata() && record.token == token && record.pi_wait());
    if moved {
        cancel_error(transaction, target, token);
    } else {
        transaction.cancel(token);
    }
    remove_relation(transaction, token);
    transaction.commit();
}

pub(crate) fn poll_requeue(
    transaction: &mut Transaction,
    source: Key,
    target: Key,
    address: usize,
    token: u64,
    cancel: bool,
) -> Result<RequeueStatus, i32> {
    if relation(transaction, token) != Some(target) {
        return Err(EINVAL);
    }
    let record = transaction
        .records
        .iter()
        .copied()
        .find(|record| !record.metadata() && record.token == token)
        .ok_or(EINVAL)?;
    if record.pi_source() && record.key == source {
        if cancel {
            transaction.cancel(token);
            remove_relation(transaction, token);
            transaction.commit();
        }
        return Ok(RequeueStatus::Source);
    }
    if !record.pi_wait() || record.key != target {
        return Err(EINVAL);
    }
    let owned = match poll(transaction, target, address, token, cancel) {
        Ok(owned) => owned,
        Err(error)
            if transaction.records.iter().any(|record| {
                record.token == token && record.pi_source() && record.key == source
            }) =>
        {
            // A killed requeuer's proxy intent failed before its CAS. The
            // original WAIT_REQUEUE_PI remains a source wait, not an EFAULT.
            let _ = error;
            if cancel {
                transaction.cancel(token);
                remove_relation(transaction, token);
                transaction.commit();
            }
            return Ok(RequeueStatus::Source);
        }
        Err(error) => return Err(error),
    };
    if owned || cancel {
        remove_relation(transaction, token);
        transaction.commit();
    }
    Ok(if owned {
        RequeueStatus::Owned
    } else {
        RequeueStatus::Moved
    })
}

fn move_waiter(transaction: &mut Transaction, token: u64, target: Key) -> Result<(), i32> {
    let index = transaction
        .records
        .iter()
        .position(|record| !record.metadata() && record.token == token)
        .ok_or(EINVAL)?;
    let mut record = transaction.records.remove(index);
    record.key = target;
    record.reserved = hybrid::PRIVATE_PARK | WAIT;
    // Requeued members follow the existing target FIFO even if their original
    // source token was allocated earlier than the destination members' tokens.
    transaction.records.push(record);
    transaction.dirty = true;
    Ok(())
}

pub(crate) fn compare_requeue(
    transaction: &mut Transaction,
    source: Key,
    source_address: usize,
    target: Key,
    target_address: usize,
    expected: u32,
    count: u32,
) -> Result<u32, i32> {
    if source == target {
        return Err(EINVAL);
    }
    if atomic_word::read(source_address)? != expected {
        return Err(EAGAIN);
    }
    // futex_proxy_trylock_atomic reads the target before checking for an empty
    // source queue. An unmapped PRIVATE target must not become a false zero.
    atomic_word::read(target_address)?;
    prune_source(transaction, source);
    finish_journal(transaction, target, target_address)?;
    prune(transaction, target);
    let candidates: Vec<Record> = transaction
        .records
        .iter()
        .copied()
        .filter(|record| record.key == source && !record.metadata())
        .collect();
    if candidates.is_empty() {
        return Ok(0);
    }
    let shared = Tasks::shared()?;
    let mut tasks = shared.load()?;
    let mut moved = 0;
    let mut error = None;
    let mut proxy_owner = None;
    let mut free = false;
    let mut owner_tid = 0;
    let mut provisional = None;
    for record in candidates {
        if moved >= count + 1 {
            break;
        }
        let token = record.token;
        if record.dead() {
            transaction.cancel(token);
            remove_relation(transaction, token);
            continue;
        }
        if !record.pi_source() || relation(transaction, token) != Some(target) {
            error = Some(EINVAL);
            break;
        }
        let identity = Identity::from(record);
        if moved == 0 {
            // CAS contention retries before moving this first source record.
            loop {
                let mut old = atomic_word::read(target_address)?;
                if old & TID_MASK == record.bitset {
                    error = Some(EDEADLK);
                    break;
                }
                let queued = top(transaction, target);
                if queued.is_some_and(|record| !record.pi_wait()) {
                    error = Some(EINVAL);
                    break;
                }
                free = queued.is_none() && old & TID_MASK == 0;
                if free {
                    remove_unused_state(transaction, target);
                    transaction.reserve(3)?;
                    let requeuer = Identity::current()?;
                    // A journal protects the proxy's pending ownership until
                    // the CAS. Its token also marks the head as a grantee for
                    // cycle detection while preparing a whole requeue batch.
                    transaction
                        .records
                        .push(identity.record(target, token, 0, STATE));
                    transaction.records.push(identity.record(
                        target,
                        token,
                        old,
                        JOURNAL | if count == 0 { NO_WAITERS } else { 0 },
                    ));
                    transaction
                        .records
                        .push(requeuer.record(source, token, 0, SOURCE_JOURNAL));
                    provisional = Some(token);
                    transaction.dirty = true;
                    proxy_owner = Some(identity);
                    owner_tid = record.bitset;
                    break;
                }
                if atomic_word::compare_exchange(target_address, old, old | WAITERS)? != old {
                    continue;
                }
                old |= WAITERS;
                owner_tid = old & TID_MASK;
                if let Some(existing) = state(transaction, target) {
                    if existing.reserved & DEAD == 0
                        && existing.bitset != old & TID_MASK
                        && !(old & TID_MASK == 0 && old & OWNER_DIED != 0)
                    {
                        error = Some(EINVAL);
                        break;
                    }
                    proxy_owner = Some(Identity::from(existing));
                } else {
                    let task = match owner(&mut tasks, namespace(), old & TID_MASK) {
                        Ok(task) => task,
                        Err(value) => {
                            error = Some(value);
                            break;
                        }
                    };
                    shared.commit(&tasks);
                    transaction.reserve(1)?;
                    transaction
                        .records
                        .push(task.identity.record(target, 0, task.tid, STATE));
                    transaction.dirty = true;
                    proxy_owner = Some(task.identity);
                }
                break;
            }
            if error.is_some() {
                break;
            }
        } else if record.bitset == owner_tid {
            error = Some(EDEADLK);
            break;
        }
        if moved != 0 || !free {
            if cycle(transaction, identity, proxy_owner.ok_or(EINVAL)?) {
                error = Some(EDEADLK);
                break;
            }
        }
        // Wake the original source park BEFORE publishing migration or its
        // ownership journal. An abandoned requeuer therefore cannot leave a
        // migrated waiter asleep without observing the target owner or CAS.
        if let Err(value) = notify(record, transaction.shared.domain) {
            error = Some(value);
            break;
        }
        #[cfg(test)]
        super::tests::crash_at("before-journal");
        move_waiter(transaction, token, target)?;
        moved += 1;
        // The measured release route batches followers. Keep the slower
        // per-waiter control only in native test builds for paired benchmarks.
        if (cfg!(test) && !optimized()) || (free && moved == 1) {
            transaction.commit();
            finish_journal(transaction, target, target_address)?;
            priorities(transaction, shared, &mut tasks)?;
        }
    }
    if moved == 0 {
        // The first candidate can fail before owning/queuing anything. Remove
        // only the provisional state; never consume that source registration.
        remove_unused_state(transaction, target);
        transaction.records.retain(|record| {
            !(Some(record.token) == provisional && record.reserved & SOURCE_JOURNAL != 0)
        });
    } else {
        transaction.commit();
        finish_journal(transaction, target, target_address)?;
        priorities(transaction, shared, &mut tasks)?;
    }
    error.map_or(Ok(moved), Err)
}

#[cfg(test)]
mod tests;

// Preserve the existing native crash helpers and microbenchmark entry points.
#[cfg(test)]
fn enqueue(
    transaction: &mut Transaction,
    source: Key,
    target: Key,
    address: usize,
    expected: u32,
    tid: u32,
) -> Result<u64, i32> {
    enqueue_requeue(transaction, source, address, expected, target, tid)
}
#[cfg(test)]
fn transfer(
    transaction: &mut Transaction,
    source: Key,
    target: Key,
    source_address: usize,
    target_address: usize,
    expected: u32,
    count: u32,
) -> Result<u32, i32> {
    if count == 0 {
        return Ok(0);
    }
    compare_requeue(
        transaction,
        source,
        source_address,
        target,
        target_address,
        expected,
        count - 1,
    )
}
#[cfg(test)]
enum Progress {
    Source,
    Queued,
    Owned,
}
#[cfg(test)]
fn poll_waiter(
    transaction: &mut Transaction,
    target: Key,
    address: usize,
    token: u64,
    cancel: bool,
    _replay_donation: bool,
) -> Result<Progress, i32> {
    let source = transaction
        .records
        .iter()
        .find(|record| record.token == token && record.reserved & SOURCE_JOURNAL != 0)
        .or_else(|| {
            transaction
                .records
                .iter()
                .find(|record| record.token == token && !record.metadata())
        })
        .map(|record| record.key)
        .ok_or(EAGAIN)?;
    poll_requeue(transaction, source, target, address, token, cancel).map(|status| match status {
        RequeueStatus::Source => Progress::Source,
        RequeueStatus::Moved => Progress::Queued,
        RequeueStatus::Owned => Progress::Owned,
    })
}
#[cfg(test)]
mod additional_tests;
