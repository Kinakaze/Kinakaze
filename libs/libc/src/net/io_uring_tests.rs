//! Call the real exported ABI, not just the VFS helper beneath it.
use super::*;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualProtect,
};

struct Ring(c_int);
impl Ring {
    fn new() -> Option<Self> {
        if !iouring::available() {
            return None;
        }
        let fd = kinakaze_abi_io_uring_setup(8, 0);
        assert!(fd >= 0, "setup errno {}", crate::kinakaze_errno());
        Some(Self(fd))
    }
    fn queue(&self, cookies: &[u64]) {
        for &user_data in cookies {
            let entry = SubmissionEntry {
                user_data,
                ..SubmissionEntry::default()
            };
            assert_eq!(unsafe { kinakaze_abi_io_uring_push(self.0, &entry) }, 0);
        }
        assert_eq!(
            kinakaze_abi_io_uring_enter(self.0, cookies.len() as u32, 0, 0),
            cookies.len() as c_int
        );
    }
    fn take(&self, cookie: u64) {
        let mut entry = CompletionEntry::default();
        assert_eq!(
            unsafe { kinakaze_abi_io_uring_reap(self.0, &mut entry, 1) },
            1
        );
        assert_eq!(entry.user_data, cookie);
        assert_eq!(entry.result, 0);
        assert_eq!(entry.flags, 0);
    }
}
impl Drop for Ring {
    fn drop(&mut self) {
        kinakaze_vfs::close(self.0).unwrap();
    }
}

struct Pages(usize);
impl Pages {
    fn new() -> Self {
        let address = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                8192,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        assert!(!address.is_null());
        unsafe { std::ptr::write_bytes(address.cast::<u8>(), 0xcc, 8192) };
        Self(address as usize)
    }
    fn protect(&self, offset: usize, protection: u32) {
        let mut previous = 0;
        assert_ne!(
            unsafe {
                VirtualProtect(
                    (self.0 + offset) as *const _,
                    4096,
                    protection,
                    &mut previous,
                )
            },
            0
        );
    }
}
impl Drop for Pages {
    fn drop(&mut self) {
        assert_ne!(unsafe { VirtualFree(self.0 as *mut _, 0, MEM_RELEASE) }, 0);
    }
}

#[test]
fn abi_bad_or_partial_sqe_pointers_fail_without_queuing() {
    let Some(ring) = Ring::new() else {
        return;
    };
    for address in [0usize, 1, usize::MAX - 31] {
        assert_eq!(
            unsafe { kinakaze_abi_io_uring_push(ring.0, address as *const _) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
    }
    let pages = Pages::new();
    let address = pages.0 + 4096 - 32;
    unsafe {
        std::ptr::write_unaligned(address as *mut SubmissionEntry, SubmissionEntry::default())
    };
    pages.protect(4096, PAGE_NOACCESS);
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_push(ring.0, address as *const _) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
    let address = pages.0;
    drop(pages);
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_push(ring.0, address as *const _) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
    assert_eq!(kinakaze_abi_io_uring_enter(ring.0, 0, 0, 0), 0);
    let mut out = CompletionEntry::default();
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, &mut out, 1) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EAGAIN);
    ring.queue(&[42]);
    ring.take(42);
}

#[test]
fn abi_invalid_count_and_pointer_range_preserve_the_completion() {
    let Some(ring) = Ring::new() else {
        return;
    };
    ring.queue(&[u64::MAX]);
    let mut entry = CompletionEntry::default();
    for (address, count, error) in [
        (&mut entry as *mut _ as usize, 0, kinakaze_vfs::EINVAL),
        (0, 1, kinakaze_vfs::EINVAL),
        (
            &mut entry as *mut _ as usize,
            u32::MAX,
            kinakaze_vfs::EINVAL,
        ),
        (usize::MAX - 7, 1, kinakaze_vfs::EFAULT),
        (1, 1, kinakaze_vfs::EFAULT),
    ] {
        assert_eq!(
            unsafe { kinakaze_abi_io_uring_reap(ring.0, address as *mut _, count) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), error);
    }
    ring.take(u64::MAX);
}

#[test]
fn abi_readonly_noaccess_and_unmapped_cqe_outputs_can_be_retried() {
    let Some(ring) = Ring::new() else {
        return;
    };
    for protection in [PAGE_READONLY, PAGE_NOACCESS] {
        ring.queue(&[protection as u64]);
        let pages = Pages::new();
        pages.protect(0, protection);
        assert_eq!(
            unsafe { kinakaze_abi_io_uring_reap(ring.0, pages.0 as *mut _, 1) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
        pages.protect(0, PAGE_READONLY);
        assert_eq!(
            unsafe { *(pages.0 as *const u8) },
            0xcc,
            "no debugger-style write through readonly pages"
        );
        ring.take(protection as u64);
    }
    ring.queue(&[7]);
    let pages = Pages::new();
    let address = pages.0;
    drop(pages);
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, address as *mut _, 1) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
    ring.take(7);
}

#[test]
fn abi_torn_cqe_is_retained_and_only_complete_prefix_entries_are_consumed() {
    let Some(ring) = Ring::new() else {
        return;
    };
    let pages = Pages::new();
    pages.protect(4096, PAGE_NOACCESS);
    ring.queue(&[0x1234]);
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, (pages.0 + 4088) as *mut _, 1) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EFAULT);
    ring.take(0x1234);
    ring.queue(&[11, 22]);
    let address = pages.0 + 4096 - 24;
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, address as *mut _, 2) },
        1
    );
    let first = unsafe { std::ptr::read_unaligned(address as *const CompletionEntry) };
    assert_eq!(first.user_data, 11);
    assert_eq!(first.result, 0);
    ring.take(22);
}

#[test]
fn abi_valid_unaligned_sqe_and_cqe_addresses_are_supported() {
    let Some(ring) = Ring::new() else {
        return;
    };
    let mut input = [0u8; 65];
    let address = unsafe { input.as_mut_ptr().add(1) };
    unsafe {
        std::ptr::write_unaligned(
            address.cast::<SubmissionEntry>(),
            SubmissionEntry {
                user_data: 99,
                ..SubmissionEntry::default()
            },
        )
    };
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_push(ring.0, address.cast()) },
        0
    );
    assert_eq!(kinakaze_abi_io_uring_enter(ring.0, 1, 0, 0), 1);
    let mut output = [0u8; 17];
    let address = unsafe { output.as_mut_ptr().add(1) };
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, address.cast(), 1) },
        1
    );
    let completion = unsafe { std::ptr::read_unaligned(address.cast::<CompletionEntry>()) };
    assert_eq!(completion.user_data, 99);
    assert_eq!(completion.result, 0);
}

#[test]
fn abi_enter_respects_zero_submission_and_rejects_partial_or_unknown_flags_without_consuming() {
    let Some(ring) = Ring::new() else {
        return;
    };
    for user_data in [100, 200] {
        let entry = SubmissionEntry {
            user_data,
            ..SubmissionEntry::default()
        };
        assert_eq!(unsafe { kinakaze_abi_io_uring_push(ring.0, &entry) }, 0);
    }
    let mut entries = [CompletionEntry::default(); 2];
    assert_eq!(kinakaze_abi_io_uring_enter(ring.0, 0, 999, 0), 0);
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, entries.as_mut_ptr(), 2) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EAGAIN);
    assert_eq!(kinakaze_abi_io_uring_enter(ring.0, 1, 0, 0), -1);
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EOPNOTSUPP);
    for flags in [2u32, 4, 0x8000_0000, u32::MAX] {
        assert_eq!(kinakaze_abi_io_uring_enter(ring.0, 2, 0, flags), -1);
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EINVAL);
    }
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, entries.as_mut_ptr(), 2) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EAGAIN);
    assert_eq!(
        kinakaze_abi_io_uring_enter(ring.0, 3, 2, iouring::IORING_ENTER_GETEVENTS),
        2
    );
    assert_eq!(
        unsafe { kinakaze_abi_io_uring_reap(ring.0, entries.as_mut_ptr(), 2) },
        2
    );
    assert_eq!(entries[0].user_data, 100);
    assert_eq!(entries[1].user_data, 200);
}

#[test]
fn abi_enter_without_getevents_does_not_wait_for_min_complete() {
    let Some(ring) = Ring::new() else {
        return;
    };
    let fd = ring.0;
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(kinakaze_abi_io_uring_enter(fd, 0, u32::MAX, 0))
            .unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(1));
    // Keep failure bounded too: close wakes an incorrectly blocking enter.
    drop(ring);
    worker.join().unwrap();
    assert_eq!(result.expect("flags=0 must not wait"), 0);
}
