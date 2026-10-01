//! Proxy acquisition preserves the source token, and publishes the expected
//! destination as a second value-only record until migration commits.
use super::*;

pub(crate) fn enqueue(
    transaction: &mut Transaction,
    source: Key,
    target: Key,
    address: usize,
    expected: u32,
    tid: u32,
) -> Result<u64, i32> {
    prune(transaction, source);
    if atomic_word::read(address)? != expected {
        return Err(EAGAIN);
    }
    if source == target {
        return Err(EINVAL);
    }
    transaction.reserve(2)?;
    let token = transaction.append_private(source, park_identity()?, tid);
    let row = transaction.records.last_mut().unwrap();
    row.reserved |= WAIT | SOURCE;
    let link = Record {
        key: target,
        reserved: LINK,
        ..*row
    };
    transaction.records.push(link);
    transaction.dirty = true;
    transaction.commit();
    Ok(token)
}

fn move_row(transaction: &mut Transaction, waiter: Record, target: Key) {
    transaction.records.retain(|record| {
        !((!record.metadata() || record.reserved & LINK != 0) && record.token == waiter.token)
    });
    // Appending behind destination members maintains FIFO even when the source
    // token was allocated earlier than an existing destination waiter.
    transaction.records.push(Record {
        key: target,
        reserved: waiter.reserved & !SOURCE,
        ..waiter
    });
    transaction.dirty = true;
}

fn proxy(
    transaction: &mut Transaction,
    target: Key,
    address: usize,
    waiter: Record,
) -> Result<(), i32> {
    finish_journal(transaction, target, address)?;
    prune(transaction, target);
    if top(transaction, target).is_some_and(|row| !row.pi_wait() || row.reserved & SOURCE != 0) {
        return Err(EINVAL);
    }
    transaction.reserve(2)?; // State and recovery journal, before moving anything.
    loop {
        let old = atomic_word::read(address)?;
        if old & TID_MASK == waiter.bitset {
            return Err(EDEADLK);
        }
        let existing = state(transaction, target);
        if old & TID_MASK == 0 && top(transaction, target).is_none() {
            // Wake before publication: unlike unlock, a source waiter has no
            // prior owner-exit handle to observe a killed requeuer.
            notify(waiter, transaction.shared.domain)?;
            move_row(transaction, waiter, target);
            transaction
                .records
                .retain(|row| !(row.key == target && row.reserved & STATE != 0));
            transaction
                .records
                .push(Identity::from(waiter).record(target, 0, 0, STATE));
            // Move, state and journal share one publication. Recovery completes
            // the CAS through the waiter's own destination mapping.
            return grant(
                transaction,
                target,
                address,
                old,
                Record {
                    key: target,
                    ..waiter
                },
            );
        }
        let shared = Tasks::shared()?;
        let mut tasks = shared.load()?;
        let owner = if let Some(existing) = existing {
            if existing.reserved & DEAD == 0
                && existing.bitset != old & TID_MASK
                && !(old & TID_MASK == 0 && old & OWNER_DIED != 0)
            {
                return Err(EINVAL);
            }
            existing
        } else {
            let owner = owner(&mut tasks, namespace(), old & TID_MASK)?;
            shared.commit(&tasks);
            owner.identity.record(target, 0, owner.tid, STATE)
        };
        if owner.reserved & DEAD == 0
            && !owner.dead()
            && cycle(transaction, Identity::from(waiter), Identity::from(owner))
        {
            return Err(EDEADLK);
        }
        if atomic_word::compare_exchange(address, old, old | WAITERS)? != old {
            continue;
        }
        notify(waiter, transaction.shared.domain)?;
        move_row(transaction, waiter, target);
        if existing.is_none() {
            transaction.records.push(owner);
        }
        transaction.dirty = true;
        transaction.commit();
        return priorities(transaction, shared, &mut tasks);
    }
}

pub(crate) fn transfer(
    transaction: &mut Transaction,
    source: Key,
    target: Key,
    source_address: usize,
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
    prune(transaction, source);
    let mut moved = 0;
    while moved < count {
        let Some(waiter) = top(transaction, source) else {
            break;
        };
        if waiter.reserved & SOURCE == 0
            || !transaction.records.iter().any(|row| {
                row.reserved & LINK != 0 && row.token == waiter.token && row.key == target
            })
        {
            return Err(EINVAL);
        }
        proxy(transaction, target, target_address, waiter)?;
        moved += 1;
    }
    transaction.commit();
    Ok(moved)
}

pub(crate) enum Progress {
    Source,
    Queued,
    Owned,
}
pub(crate) fn poll_waiter(
    transaction: &mut Transaction,
    target: Key,
    address: usize,
    token: u64,
    cancel: bool,
    replay_donation: bool,
) -> Result<Progress, i32> {
    let row = transaction
        .records
        .iter()
        .find(|row| !row.metadata() && row.token == token)
        .copied()
        .ok_or(EAGAIN)?;
    if row.reserved & SOURCE != 0 {
        if cancel {
            transaction.cancel(token);
            transaction.commit();
        }
        return Ok(Progress::Source);
    }
    if row.key != target {
        return Err(EINVAL);
    }
    if replay_donation {
        let shared = Tasks::shared()?;
        let mut tasks = shared.load()?;
        priorities(transaction, shared, &mut tasks)?;
    }
    if poll(transaction, target, address, token, cancel)? {
        Ok(Progress::Owned)
    } else {
        Ok(Progress::Queued)
    }
}

#[cfg(test)]
mod tests;
