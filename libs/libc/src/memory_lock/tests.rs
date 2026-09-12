use super::*;
use windows_sys::Win32::System::Memory::{
    MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};

struct Pages(usize, usize);
impl Pages {
    fn new(count: usize) -> Self {
        let length = count * geometry().0;
        let address = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                length,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!address.is_null());
        Self(address as usize, length)
    }
    fn page(&self, index: usize) -> Span {
        let start = self.0 + index * geometry().0;
        Span {
            start,
            end: start + geometry().0,
        }
    }
    fn locked(&self, index: usize) -> bool {
        !selected_runs(self.page(index), true).unwrap().is_empty()
    }
}
impl Drop for Pages {
    fn drop(&mut self) {
        assert_ne!(unsafe { VirtualFree(self.0 as _, 0, MEM_RELEASE) }, 0);
    }
}

#[test]
fn native_locks_round_pages_overlap_and_unlock_without_reference_counts() {
    let pages = Pages::new(4);
    let page = geometry().0;
    assert_eq!(lock(pages.0 + 1, page), Ok(()));
    assert!(pages.locked(0) && pages.locked(1));
    assert!(!pages.locked(2));
    assert_eq!(lock(pages.0 + page, page), Ok(()));
    assert_eq!(unlock(pages.0 + page + 1, 1), Ok(()));
    assert!(pages.locked(0));
    assert!(!pages.locked(1));
    assert_eq!(unlock(pages.0, pages.1), Ok(()));
    assert_eq!(unlock(pages.0, pages.1), Ok(()));
    assert!(!pages.locked(0));
}

#[test]
fn native_failure_rolls_back_only_new_locks() {
    let pages = Pages::new(3);
    let page = geometry().0;
    assert_eq!(lock(pages.0, page), Ok(()));
    let mut old = 0;
    assert_ne!(
        unsafe { VirtualProtect((pages.0 + 2 * page) as _, page, PAGE_NOACCESS, &mut old) },
        0
    );
    assert!(lock_spans(&[pages.page(0), pages.page(1), pages.page(2)]).is_err());
    assert!(pages.locked(0));
    assert!(!pages.locked(1));
    assert!(!pages.locked(2));
}

#[test]
fn invalid_memory_flags_and_overflow_do_not_claim_success() {
    let pages = Pages::new(1);
    let page = geometry().0;
    assert_eq!(lock(0, page), Err(ENOMEM));
    assert_eq!(unlock(0, page), Err(ENOMEM));
    assert_eq!(lock(usize::MAX - 8, 32), Err(EINVAL));
    assert_eq!(lock(0, 0), Ok(()));
    assert_eq!(lock(pages.0 + 1, 0), Ok(()));
    assert!(pages.locked(0));
    for flags in [0, MCL_ONFAULT, -1, 8] {
        assert_eq!(lock_all(flags), Err(EINVAL));
    }
    for flags in [
        MCL_FUTURE,
        MCL_CURRENT | MCL_FUTURE,
        MCL_CURRENT | MCL_ONFAULT,
    ] {
        assert_eq!(lock_all(flags), Err(EOPNOTSUPP));
        assert!(pages.locked(0));
    }
    assert_eq!(kinakaze_abi_mlock2(pages.0 as _, page, 8), -1);
    assert_eq!(crate::kinakaze_errno(), EINVAL);
}

#[test]
fn freeing_native_memory_retires_locks_without_a_shadow_registry() {
    let pages = Pages::new(2);
    assert_eq!(lock(pages.0, pages.1), Ok(()));
    let address = pages.0;
    let length = pages.1;
    drop(pages);
    let replacement = unsafe {
        VirtualAlloc(
            address as _,
            length,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    assert_eq!(replacement as usize, address);
    let replacement = Pages(address, length);
    assert!(!replacement.locked(0) && !replacement.locked(1));
}

#[test]
fn unlock_all_releases_pages_protected_after_locking() {
    let pages = Pages::new(1);
    assert_eq!(lock(pages.0, pages.1), Ok(()));
    assert!(pages.locked(0));
    let mut old = 0;
    assert_ne!(
        unsafe { VirtualProtect(pages.0 as _, pages.1, PAGE_NOACCESS, &mut old) },
        0
    );
    assert_eq!(unlock_all(), Ok(()));
    assert!(!pages.locked(0));
}

#[test]
fn raw_syscalls_share_native_locking_and_return_linux_errors() {
    let pages = Pages::new(1);
    let raw = |number, address, length, flags| unsafe {
        crate::sysadmin::kinakaze_abi_syscall_raw(number, address, length, flags, 0, 0, 0)
    };
    assert_eq!(raw(149, pages.0 as u64, pages.1 as u64, 0), 0);
    assert!(pages.locked(0));
    assert_eq!(raw(150, pages.0 as u64, pages.1 as u64, 0), 0);
    assert!(!pages.locked(0));
    assert_eq!(raw(325, pages.0 as u64, pages.1 as u64, 0), 0);
    assert!(pages.locked(0));
    assert_eq!(raw(149, 0, pages.1 as u64, 0), -i64::from(ENOMEM));
    assert_eq!(raw(151, 0, 0, 0), -i64::from(EINVAL));
}
