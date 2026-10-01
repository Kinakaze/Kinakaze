//! glibc-layout PI mutexes. Ownership stays in the user futex word; a pinned
//! numeric owner survives non-cooperative exit, including uncontended locks.
use super::*;
use core::cell::RefCell;
use kinakaze_vfs::{interrupt, signal};
use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::System::Threading::INFINITE;

const ROBUST: u32 = 16;
const INHERIT: u32 = 32;
const SHARED: u32 = 128;
const INCONSISTENT: u32 = i32::MAX as u32;
const NOT_RECOVERABLE: u32 = INCONSISTENT - 1;
const EBUSY: i32 = 16;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;
// Production uses the measured winner. Legacy selection exists only in the
// native test executable; graph-dependent recovery still selects its full path.
#[cfg(not(test))]
pub(super) const fn optimized() -> bool {
    true
}
#[cfg(test)]
pub(super) fn optimized() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_PTHREAD_PI_OPT").is_none_or(|v| v != "0"))
}
#[cfg(test)]
fn store_optimized() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("KINAKAZE_TEST_PTHREAD_PI_STORE_OPT").is_none_or(|v| v != "0")
    })
}
#[derive(Clone, Copy)]
struct Held {
    address: usize,
    tid: u32,
}
struct HeldLocks {
    host: u32,
    rows: Vec<Held>,
}
impl Drop for HeldLocks {
    fn drop(&mut self) {
        self.finish();
    }
}
impl HeldLocks {
    fn finish(&mut self) {
        if self.host == std::process::id() {
            for row in self.rows.drain(..) {
                let Ok(kind) = atomic_word::read(row.address + 16) else {
                    continue;
                };
                if kind & (INHERIT | ROBUST) != INHERIT | ROBUST {
                    continue;
                }
                let Ok(mut old) = atomic_word::read(row.address) else {
                    continue;
                };
                while old & TID_MASK == row.tid {
                    match atomic_word::compare_exchange(
                        row.address,
                        old,
                        (old & WAITERS) | OWNER_DIED,
                    ) {
                        Ok(value) if value == old => break,
                        Ok(value) => old = value,
                        Err(_) => break,
                    }
                }
            }
        }
        self.rows.clear();
    }
}
thread_local! {static HELD: RefCell<HeldLocks>=RefCell::new(HeldLocks{host:std::process::id(),rows:Vec::new()});}
fn held(address: usize, tid: u32) {
    HELD.with_borrow_mut(|held| {
        if held.host != std::process::id() {
            held.rows.clear();
            held.host = std::process::id();
        }
        if !held.rows.iter().any(|row| row.address == address) {
            held.rows.push(Held { address, tid });
        }
    });
}
fn unheld(address: usize) {
    let _ = HELD.try_with(|held| held.borrow_mut().rows.retain(|row| row.address != address));
}
pub(crate) fn exit_current() {
    let _ = HELD.try_with(|held| held.borrow_mut().finish());
}
pub(crate) fn install() {
    libpthread::install_pi_mutex_backend(entry);
}

fn put(address: usize, value: u32) -> Result<(), i32> {
    #[cfg(test)]
    if !store_optimized() {
        let mut old = atomic_word::read(address)?;
        loop {
            let observed = atomic_word::compare_exchange(address, old, value)?;
            if observed == old {
                return Ok(());
            }
            old = observed;
        }
    }
    atomic_word::write(address, value)
}
fn first(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    tid: u32,
    try_only: bool,
    robust: bool,
) -> Result<Acquisition, i32> {
    finish_journal(transaction, key, address)?;
    prune(transaction, key);
    loop {
        let old = atomic_word::read(address)?;
        if top(transaction, key).is_none() && old & TID_MASK == 0 {
            let fast = optimized() && priority_graph_reconciled(transaction)?;
            transaction.reserve(3)?;
            // Reserve the zero-owner word against user-space CAS before
            // publishing an ownership journal. A killed claimant leaves no TID.
            if atomic_word::compare_exchange(address, old, old | WAITERS)? != old {
                continue;
            }
            let token = transaction.append_private(key, park_identity()?, tid);
            transaction.records.last_mut().unwrap().reserved |= WAIT;
            transaction
                .records
                .retain(|row| !(row.key == key && row.reserved & STATE != 0));
            transaction
                .records
                .push(Identity::current()?.record(key, 0, 0, STATE | PINNED));
            transaction.dirty = true;
            let waiter = transaction
                .records
                .iter()
                .find(|row| row.pi_wait() && row.token == token)
                .copied()
                .unwrap();
            grant_policy(transaction, key, address, old | WAITERS, waiter, !fast)?;
            if fast {
                if state(transaction, key).is_none_or(|row| row.token != token || row.bitset != tid)
                {
                    return Err(EINVAL);
                }
                transaction.cancel(token);
                let row = transaction
                    .records
                    .iter_mut()
                    .find(|row| row.key == key && row.reserved & STATE != 0)
                    .ok_or(EINVAL)?;
                row.token = 0;
                // owned() publishes the consumed token with the pthread fields.
                // The already committed owner/journal survives a killed caller.
            } else {
                assert!(poll_policy(
                    transaction,
                    key,
                    address,
                    token,
                    false,
                    robust
                )?);
            }
            #[cfg(test)]
            super::tests::crash_at("after-self-consume");
            return Ok(Acquisition::Owned);
        }
        let dead = robust
            && state(transaction, key).is_some_and(|row| row.reserved & DEAD != 0 || row.dead());
        return acquire(transaction, key, address, tid, try_only && !dead);
    }
}
fn remaining(time: &libpthread::Timespec, clock: i32) -> Result<Duration, i32> {
    if !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(EINVAL);
    }
    if time.tv_sec < 0 {
        return Ok(Duration::ZERO);
    }
    let (seconds, nanoseconds) = crate::time::read_clock(clock)?;
    let now = i128::from(seconds) * 1_000_000_000 + i128::from(nanoseconds);
    let until = i128::from(time.tv_sec) * 1_000_000_000 + i128::from(time.tv_nsec);
    let nanos = until.saturating_sub(now).max(0) as u128;
    Ok(Duration::new(
        (nanos / 1_000_000_000) as u64,
        (nanos % 1_000_000_000) as u32,
    ))
}
fn prepare(time: &libpthread::Timespec, clock: i32) -> Result<Deadline, i32> {
    let started = std::time::Instant::now();
    Ok(Deadline {
        duration: Some(remaining(time, clock)?),
        started,
        realtime: (clock == 0).then_some(i128::from(time.tv_sec) * 1_000_000_000 + i128::from(time.tv_nsec)),
    })
}
fn owned(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    tid: u32,
    kind: u32,
) -> Result<i32, i32> {
    let word = atomic_word::read(address)?;
    if word & TID_MASK != tid {
        return Err(EINVAL);
    }
    // A recovery owner can poison a mutex with waiters already queued. Pass
    // their granted futex ownership onward before reporting the persistent
    // error; none of them may enter the protected region.
    if kind & ROBUST != 0 && atomic_word::read(address + 8)? == NOT_RECOVERABLE {
        unlock(transaction, key, address, tid)?;
        return Ok(ENOTRECOVERABLE);
    }
    held(address, tid);
    let died = kind & ROBUST != 0 && word & OWNER_DIED != 0;
    if died {
        put(address, word & !OWNER_DIED)?;
    }
    put(address + 4, 1)?;
    put(address + 8, if died { INCONSISTENT } else { tid })?;
    let users = atomic_word::read(address + 12)?;
    put(
        address + 12,
        if died {
            users.max(1)
        } else {
            users.saturating_add(1)
        },
    )?;
    if let Some(row) = transaction
        .records
        .iter_mut()
        .find(|row| row.key == key && row.reserved & STATE != 0)
    {
        if !optimized() || row.reserved & PINNED == 0 {
            row.reserved |= PINNED;
            transaction.dirty = true;
        }
    }
    transaction.commit();
    Ok(if died { EOWNERDEAD } else { 0 })
}

fn lock(
    address: usize,
    op: u32,
    clock: i32,
    deadline: *const libpthread::Timespec,
    kind: u32,
) -> Result<i32, i32> {
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    register_current(tid as i32)?;
    let robust = kind & ROBUST != 0;
    let private = kind & SHARED == 0;
    let park = park()?;
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let mut time = None;
    loop {
        let (mut transaction, key) = crate::sysadmin::futex_pi::transaction(address, private)?;
        if atomic_word::read(address + 8)? == NOT_RECOVERABLE {
            return Err(ENOTRECOVERABLE);
        }
        let word = atomic_word::read(address)?;
        let self_owned = word & TID_MASK == tid;
        if self_owned && kind & 3 == 1 {
            let count = atomic_word::read(address + 4)?;
            let count = count.checked_add(1).ok_or(EAGAIN)?;
            put(address + 4, count)?;
            return Ok(0);
        }
        if self_owned && op == 2 {
            return Err(EBUSY);
        }
        if self_owned && kind & 3 == 2 {
            return Err(EDEADLK);
        }
        let attempt = if self_owned {
            Err(EAGAIN)
        } else {
            first(
                &mut transaction,
                key,
                address,
                tid,
                op == 2 || (op == 3 && time.is_none()),
                robust,
            )
        };
        let mut token = match attempt {
            Ok(Acquisition::Owned) => return owned(&mut transaction, key, address, tid, kind),
            Ok(Acquisition::Queued(token)) => Some(token),
            Err(EAGAIN) => None,
            Err(error) => return Err(error),
        };
        if op == 2 {
            if let Some(token) = token {
                if poll_policy(&mut transaction, key, address, token, true, robust)? {
                    return owned(&mut transaction, key, address, tid, kind);
                }
            }
            return Err(EBUSY);
        }
        if op == 3 && time.is_none() {
            if let Some(value) = token {
                if poll_policy(&mut transaction, key, address, value, true, robust)? {
                    return owned(&mut transaction, key, address, tid, kind);
                }
                token = None;
            }
            time = Some(prepare(&crate::ptrace::read_value::<libpthread::Timespec>(
                deadline as usize,
            )?, clock)?);
            // No source token exists on this first busy timed attempt, except
            // immediate robust death recovery handled above.
        }
        if token.is_none() {
            let wait = time.as_ref().map_or(INFINITE, Deadline::milliseconds);
            if wait == 0 {
                return Err(110);
            }
            drop(transaction);
            if signal::interrupt_pending() {
                let _ = signal::deliver_pending();
                continue;
            }
            // A busy timed first attempt now has a copied deadline and must
            // enqueue; only NORMAL self-deadlock waits without a token.
            if !self_owned {
                continue;
            }
            let status = unsafe {
                if let Some(time) = &time { time.wait(&[park, interrupt]) }
                else { kinakaze_vfs::deadline_wait::any(&[park, interrupt], wait) }
            };
            if status != WAIT_TIMEOUT && !(WAIT_OBJECT_0..WAIT_OBJECT_0 + 2).contains(&status) {
                return Err(EIO);
            }
            let _ = signal::deliver_pending();
            continue;
        }
        let token = token.unwrap();
        drop(transaction);
        signal::register_waiter();
        let result = loop {
            let wait = time.as_ref().map_or(INFINITE, Deadline::milliseconds);
            let pending = signal::interrupt_pending();
            let (mut transaction, key) =
                match crate::sysadmin::futex_pi::transaction(address, private) {
                    Ok(value) => value,
                    Err(error) => break Err(error),
                };
            match poll_policy(
                &mut transaction,
                key,
                address,
                token,
                pending || wait == 0,
                robust,
            ) {
                Ok(true) => break owned(&mut transaction, key, address, tid, kind),
                Ok(false) if wait == 0 => break Err(110),
                Ok(false) if pending => break Ok(-1),
                Ok(false) => {}
                Err(error) => {
                    cancel_error(&mut transaction, key, token);
                    break Err(error);
                }
            }
            let owner = match owner_handle(&transaction, key) {
                Ok(owner) => Some(owner),
                Err(ESRCH) if !robust => None,
                Err(ESRCH) => {
                    drop(transaction);
                    continue;
                }
                Err(error) => {
                    cancel_error(&mut transaction, key, token);
                    break Err(error);
                }
            };
            drop(transaction);
            let handles = [
                park,
                interrupt,
                owner.as_ref().map_or(ptr::null_mut(), |owner| owner.raw()),
            ];
            let status = unsafe {
                let sources = &handles[..if owner.is_some() { 3 } else { 2 }];
                if let Some(time) = &time { time.wait(sources) }
                else { kinakaze_vfs::deadline_wait::any(sources, wait) }
            };
            if status != WAIT_TIMEOUT
                && !(WAIT_OBJECT_0..WAIT_OBJECT_0 + if owner.is_some() { 3 } else { 2 })
                    .contains(&status)
            {
                if let Ok((mut transaction, key)) =
                    crate::sysadmin::futex_pi::transaction(address, private)
                {
                    cancel_error(&mut transaction, key, token);
                }
                break Err(EIO);
            }
        };
        signal::unregister_waiter();
        let _ = signal::deliver_pending();
        if result == Ok(-1) {
            continue;
        }
        return result;
    }
}

unsafe extern "sysv64" fn entry(
    op: u32,
    mutex: *mut usize,
    clock: i32,
    deadline: *const libpthread::Timespec,
    flags: i32,
) -> i32 {
    let result = (|| {
        let address = mutex as usize;
        if op == 0 {
            crate::sysadmin::futex_access(address, 40, true).map_err(|error| -error as i32)?;
            let kind = (flags as u32 & 3)
                | INHERIT
                | if flags as u32 & (1 << 30) != 0 {
                    ROBUST
                } else {
                    0
                }
                | if flags as u32 & ((1 << 31) | (1 << 30)) != 0 {
                    SHARED
                } else {
                    0
                };
            unsafe { ptr::write_bytes(mutex.cast::<u8>(), 0, 40) };
            put(address + 16, kind)?;
            // Keep the domain section alive in the initializing participant,
            // even when its first owner is a different process that is killed.
            let _ = crate::sysadmin::futex_pi::transaction(address, kind & SHARED == 0)?;
            return Ok(0);
        }
        let kind = atomic_word::read(address + 16)?;
        if kind & INHERIT == 0 {
            return Err(EINVAL);
        }
        if matches!(op, 1..=3) {
            return lock(address, op, clock, deadline, kind);
        }
        let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
        let (mut transaction, key) =
            crate::sysadmin::futex_pi::transaction(address, kind & SHARED == 0)?;
        let word = atomic_word::read(address)?;
        if op == 5 {
            if word & TID_MASK != 0 || top(&transaction, key).is_some() {
                return Err(EBUSY);
            }
            transaction
                .records
                .retain(|row| row.key != key || !row.metadata());
            transaction.dirty = true;
            put(address + 16, u32::MAX)?;
            return Ok(0);
        }
        if word & TID_MASK != tid {
            return Err(if op == 6 { EINVAL } else { EPERM });
        }
        if op == 6 {
            if kind & ROBUST == 0 || atomic_word::read(address + 8)? != INCONSISTENT {
                return Err(EINVAL);
            }
            put(address + 8, tid)?;
            return Ok(0);
        }
        if op != 4 {
            return Err(EINVAL);
        }
        let count = atomic_word::read(address + 4)?;
        if kind & 3 == 1 && count > 1 {
            put(address + 4, count - 1)?;
            return Ok(0);
        }
        let poison = kind & ROBUST != 0 && atomic_word::read(address + 8)? == INCONSISTENT;
        put(address + 4, 0)?;
        put(address + 8, if poison { NOT_RECOVERABLE } else { 0 })?;
        let users = atomic_word::read(address + 12)?;
        put(address + 12, users.saturating_sub(1))?;
        unlock(&mut transaction, key, address, tid)?;
        unheld(address);
        Ok(0)
    })();
    result.unwrap_or_else(|error| error)
}

#[cfg(test)]
mod tests;
