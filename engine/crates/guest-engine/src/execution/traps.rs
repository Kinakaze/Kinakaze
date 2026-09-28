//! Exact AOT trap sites, retained across fork and retired with their ELF view.
//! A zero target dispatches a syscall; any other target redirects virtual GS.
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

fn entries() -> &'static Mutex<BTreeMap<usize, usize>> {
    static REDIRECTS: OnceLock<Mutex<BTreeMap<usize, usize>>> = OnceLock::new();
    REDIRECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn redirect(address: usize) -> Option<usize> {
    entries()
        .lock()
        .ok()?
        .get(&address)
        .copied()
        .filter(|target| *target != 0)
}

pub(super) fn syscall(address: usize) {
    register(address, 0);
}

pub(super) fn register(address: usize, target: usize) {
    entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(address, target);
}

pub(crate) fn is_syscall(address: usize) -> bool {
    entries()
        .lock()
        .is_ok_and(|entries| entries.get(&address) == Some(&0))
}

pub(super) fn forget(base: usize, length: usize) {
    entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|address, _| address.wrapping_sub(base) >= length);
}

pub(crate) fn snapshot() -> Vec<u8> {
    entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .flat_map(|(address, target)| [address.to_le_bytes(), target.to_le_bytes()].concat())
        .collect()
}

pub(crate) fn restore(bytes: &[u8]) -> bool {
    if bytes.len() % 16 != 0 {
        return false;
    }
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.clear();
    for record in bytes.chunks_exact(16) {
        let address = usize::from_le_bytes(record[..8].try_into().unwrap());
        let target = usize::from_le_bytes(record[8..].try_into().unwrap());
        entries.insert(address, target);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unloading_retires_only_the_owned_traps_before_address_reuse() {
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
}
