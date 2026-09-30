//! Exact AOT trap sites, retained across fork and retired with their ELF view.
//! A zero target dispatches a syscall; any other target redirects virtual GS.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::sync::{Mutex, OnceLock};

// A bounded lock-free cache for generated two-byte syscall/GS traps. The
// authoritative map still owns fork snapshots and overflow. Tombstones reuse
// retired slots, so loading/unloading JIT images never accumulates allocations.
const CACHE_SIZE: usize = 16 * 1024;
const CACHE_PROBES: usize = 32;
const TOMBSTONE: usize = usize::MAX;
struct Slot {
    version: AtomicUsize,
    address: AtomicUsize,
    target: AtomicUsize,
}
impl Slot {
    // Writers are serialized by entries(). Version both words as one record:
    // reading the address twice alone permits ABA when a slot is retired and
    // reused twice while a reader is preempted.
    fn publish(&self, address: usize, target: usize) {
        let version = self.version.fetch_add(1, Ordering::AcqRel);
        self.address.store(address, Ordering::Relaxed);
        self.target.store(target, Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(2), Ordering::Release);
    }
}
static CACHE: [Slot; CACHE_SIZE] = [const {
    Slot {
        version: AtomicUsize::new(0),
        address: AtomicUsize::new(0),
        target: AtomicUsize::new(0),
    }
}; CACHE_SIZE];

fn hash(address: usize) -> usize {
    address.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (usize::BITS - 14)
}
fn cached(address: usize) -> Option<usize> {
    let start = hash(address);
    for distance in 0..CACHE_PROBES {
        let slot = &CACHE[(start + distance) & (CACHE_SIZE - 1)];
        let version = slot.version.load(Ordering::Acquire);
        if version & 1 != 0 {
            return None;
        }
        let key = slot.address.load(Ordering::Relaxed);
        if key == 0 {
            return None;
        }
        if key == address {
            let target = slot.target.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if slot.version.load(Ordering::Relaxed) == version {
                return Some(target);
            }
            return None;
        }
    }
    None
}
// Writers hold entries(). The cache never owns executable allocations.
fn cache_insert(address: usize, target: usize) {
    if address == 0 || address == TOMBSTONE {
        return;
    }
    let start = hash(address);
    let mut vacant = None;
    for distance in 0..CACHE_PROBES {
        let index = (start + distance) & (CACHE_SIZE - 1);
        let key = CACHE[index].address.load(Ordering::Acquire);
        if key == address {
            CACHE[index].publish(address, target);
            return;
        }
        if key == TOMBSTONE && vacant.is_none() {
            vacant = Some(index);
        }
        if key == 0 {
            vacant = vacant.or(Some(index));
            break;
        }
    }
    if let Some(index) = vacant {
        CACHE[index].publish(address, target);
    }
}
fn cache_remove(address: usize) {
    let start = hash(address);
    for distance in 0..CACHE_PROBES {
        let slot = &CACHE[(start + distance) & (CACHE_SIZE - 1)];
        match slot.address.load(Ordering::Acquire) {
            0 => break,
            key if key == address => {
                slot.publish(TOMBSTONE, 0);
                break;
            }
            _ => {}
        }
    }
}
fn lookup(address: usize) -> Option<usize> {
    cached(address).or_else(|| entries().lock().ok()?.get(&address).copied())
}

fn entries() -> &'static Mutex<BTreeMap<usize, usize>> {
    static REDIRECTS: OnceLock<Mutex<BTreeMap<usize, usize>>> = OnceLock::new();
    REDIRECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn redirect(address: usize) -> Option<usize> {
    lookup(address).filter(|target| *target != 0)
}

pub(super) fn syscall(address: usize) {
    register(address, 0);
}

pub(super) fn register(address: usize, target: usize) {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.insert(address, target);
    cache_insert(address, target);
}

pub(crate) fn is_syscall(address: usize) -> bool {
    lookup(address) == Some(0)
}

pub(super) fn forget(base: usize, length: usize) {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Invalidate before withdrawing metadata so a cache miss waits for this
    // transaction and cannot reload an entry that is being retired.
    entries.retain(|address, _| {
        if address.wrapping_sub(base) < length {
            cache_remove(*address);
            false
        } else {
            true
        }
    });
}

pub(crate) fn snapshot() -> Vec<u8> {
    let entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut bytes = Vec::with_capacity(entries.len() * 16);
    for (address, target) in entries.iter() {
        bytes.extend_from_slice(&address.to_le_bytes());
        bytes.extend_from_slice(&target.to_le_bytes());
    }
    bytes
}

pub(crate) fn restore(bytes: &[u8]) -> bool {
    if bytes.len() % 16 != 0 {
        return false;
    }
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.clear();
    for slot in &CACHE {
        slot.publish(0, 0);
    }
    for record in bytes.chunks_exact(16) {
        let address = usize::from_le_bytes(record[..8].try_into().unwrap());
        let target = usize::from_le_bytes(record[8..].try_into().unwrap());
        entries.insert(address, target);
        cache_insert(address, target);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn unloading_retires_only_the_owned_traps_before_address_reuse() {
        let _guard = TEST_LOCK.lock().unwrap();
        let base = 0x1234_5600;
        syscall(base);
        register(base + 8, base + 100);
        syscall(base + 16);
        assert_eq!(redirect(base), None);
        assert_eq!(redirect(base + 8), Some(base + 100));
        forget(base, 16);
        assert!(!is_syscall(base));
        assert_eq!(redirect(base + 8), None);
        assert!(is_syscall(base + 16));
        register(base, base + 200);
        assert!(!is_syscall(base));
        assert_eq!(redirect(base), Some(base + 200));
        forget(base, 17);
    }

    #[test]
    fn cache_overflow_falls_back_and_retired_collisions_reuse_slots() {
        let _guard = TEST_LOCK.lock().unwrap();
        let base = 0x5678_9000usize;
        let bucket = hash(base);
        let addresses: Vec<_> = (base..)
            .step_by(16)
            .filter(|address| hash(*address) == bucket)
            .take(CACHE_PROBES + 2)
            .collect();
        let length = addresses.last().unwrap() - base + 1;
        for (index, address) in addresses.iter().copied().enumerate() {
            register(address, index + 1);
        }
        assert!(cached(*addresses.last().unwrap()).is_none());
        for (index, address) in addresses.iter().copied().enumerate() {
            assert_eq!(redirect(address), Some(index + 1));
        }
        forget(base, length);
        for round in 1..=64 {
            for address in addresses[..CACHE_PROBES].iter().copied() {
                register(address, round);
            }
            assert_eq!(cached(addresses[0]), Some(round));
            for address in addresses[..CACHE_PROBES].iter().copied() {
                assert_eq!(redirect(address), Some(round));
            }
            forget(base, length);
            assert!(addresses.iter().all(|address| redirect(*address).is_none()));
        }
        assert_eq!(core::mem::size_of_val(&CACHE), CACHE_SIZE * 24);
    }

    #[test]
    fn readers_never_observe_another_traps_target_during_slot_reuse() {
        let _guard = TEST_LOCK.lock().unwrap();
        let first = 0x789a_b000usize;
        let second = (first + 16..)
            .step_by(16)
            .find(|address| hash(*address) == hash(first))
            .unwrap();
        let running = std::sync::atomic::AtomicBool::new(true);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                while running.load(Ordering::Acquire) {
                    if let Some(target) = cached(first) {
                        assert_eq!(target, 101);
                    }
                    if let Some(target) = cached(second) {
                        assert_eq!(target, 202);
                    }
                }
            });
            for _ in 0..10_000 {
                register(first, 101);
                forget(first, 1);
                register(second, 202);
                forget(second, 1);
            }
            running.store(false, Ordering::Release);
            reader.join().unwrap();
        });
    }
}
