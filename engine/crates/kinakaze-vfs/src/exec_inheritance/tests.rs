use super::*;
use crate::fs::{self, O_CLOEXEC, O_NONBLOCK, O_RDWR};
use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle};
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn inherited(raw: usize) -> bool {
    let mut flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(raw as HANDLE, &mut flags) },
        0
    );
    flags & HANDLE_FLAG_INHERIT != 0
}

#[test]
fn restores_actual_native_flags_instead_of_assuming_the_baseline() {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    assert!(!handle.is_null());
    let fd = crate::install(handle as usize, FdKind::Event, FdFlags::NONE).unwrap();
    assert_ne!(
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) },
        0
    );
    crate::with_execve_handle_filter(|| assert!(inherited(handle as usize))).unwrap();
    assert!(!inherited(handle as usize));
    crate::close(fd).unwrap();
}

#[test]
fn spawn_fence_refuses_changed_cloexec_before_creating_a_process() {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let fd = crate::install(handle as usize, FdKind::Event, FdFlags::NONE).unwrap();
    let snapshot = crate::exec_descriptor_snapshot().unwrap();
    crate::with_checked_exec_handle_filter(&snapshot, || assert!(inherited(handle as usize)))
        .unwrap();
    crate::set_close_on_exec(fd, true).unwrap();
    let called = std::cell::Cell::new(false);
    let error = crate::with_checked_exec_handle_filter(&snapshot, || called.set(true)).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(1237));
    assert!(!called.get());
    assert!(inherited(handle as usize));
    let updated = crate::exec_descriptor_snapshot().unwrap();
    crate::with_checked_exec_handle_filter(&updated, || assert!(!inherited(handle as usize)))
        .unwrap();
    assert!(inherited(handle as usize));
    crate::close(fd).unwrap();
}

#[test]
fn spawn_fence_refuses_a_republished_descriptor_even_with_the_same_handle() {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let fd = crate::install(handle as usize, FdKind::Event, FdFlags::NONE).unwrap();
    let snapshot = crate::exec_descriptor_snapshot().unwrap();
    // Model descriptor/handle number reuse without depending on Windows' allocator.
    crate::table()
        .write()
        .unwrap()
        .insert_at(fd, handle as usize, FdKind::Event, FdFlags::NONE);
    let called = std::cell::Cell::new(false);
    let error = crate::with_checked_exec_handle_filter(&snapshot, || called.set(true)).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(1237));
    assert!(!called.get());
    assert!(inherited(handle as usize));
    crate::close(fd).unwrap();
}

#[test]
fn precreation_error_rolls_back_without_calling_the_operation() {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let fd = crate::install(handle as usize, FdKind::Event, FdFlags::NONE).unwrap();
    let mut table = crate::table().write().unwrap();
    let slot = table.first_free_from(0).unwrap();
    // Explicitly inject a bad table record, not an unknown live native object.
    // INVALID_HANDLE_VALUE is the current-process pseudo-handle, which cannot
    // have inheritance flags changed and never represents an owned event.
    table.insert_at(slot as i32, usize::MAX, FdKind::Event, FdFlags::NONE);
    drop(table);
    let called = std::cell::Cell::new(false);
    assert!(crate::with_exec_handle_filter(|| called.set(true)).is_err());
    assert!(!called.get());
    assert!(inherited(handle as usize));
    crate::table().write().unwrap().slots.remove(slot);
    crate::close(fd).unwrap();
}

#[test]
fn fifo_tokens_and_auxiliary_markers_follow_the_same_exec_policy() {
    let path = std::env::temp_dir().join(format!("kinakaze-exec-fifo-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let fifo_path = crate::to_guest_path(&path.join("fifo"));
    fs::create_fifo(&fifo_path, 0o600).unwrap();
    let retained = fs::open(&fifo_path, O_RDWR | O_NONBLOCK, 0).unwrap();
    let excluded = fs::open(&fifo_path, O_RDWR | O_NONBLOCK | O_CLOEXEC, 0).unwrap();
    let first = crate::get(retained).unwrap().raw;
    let second = crate::get(excluded).unwrap().raw;
    let pins = fifo::auxiliary_handles().unwrap();
    let first_marker = pins.iter().find(|pin| pin.fd == retained).unwrap().raw();
    let second_marker = pins.iter().find(|pin| pin.fd == excluded).unwrap().raw();
    let all = [first, second, first_marker, second_marker];
    assert!(all.iter().all(|&handle| inherited(handle)));
    crate::with_exec_handle_filter(|| assert!(all.iter().all(|&handle| !inherited(handle))))
        .unwrap();
    assert!(all.iter().all(|&handle| inherited(handle)));
    crate::with_execve_handle_filter(|| {
        assert!(inherited(first));
        assert!(inherited(first_marker));
        assert!(!inherited(second));
        assert!(!inherited(second_marker));
    })
    .unwrap();
    assert!(all.iter().all(|&handle| inherited(handle)));
    crate::close(retained).unwrap();
    crate::close(excluded).unwrap();
    // Operation-local pins can extend marker lifetime but cannot leak them into
    // an unrelated native child after the descriptors have closed.
    assert!(!inherited(first_marker));
    assert!(!inherited(second_marker));
    drop(pins);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn optional_native_handles_are_inherited_by_every_install_entrypoint() {
    let (source_fd, peer) =
        crate::unix::socketpair(crate::socket::SOCK_STREAM | crate::socket::SOCK_CLOEXEC).unwrap();
    let source = crate::get(source_fd).unwrap();
    assert_ne!(source.raw, 0);
    assert!(
        inherited(source.raw),
        "socketpair's plain install retains its native object"
    );
    assert!(inherited(crate::get(peer).unwrap().raw));
    for mode in 0..6 {
        let mut duplicate = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    source.raw as HANDLE,
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let raw = duplicate as usize;
        assert!(!inherited(raw));
        let flags = source.flags.union(FdFlags::CLOSE_ON_EXEC);
        let exact = crate::table()
            .read()
            .unwrap()
            .slots
            .iter()
            .position(Option::is_none)
            .unwrap() as i32;
        let fd = match mode {
            0 => crate::install(raw, source.kind, flags),
            1 => crate::install_at_least(raw, source.kind, flags, 0),
            2 => crate::install_exact(raw, source.kind, flags, exact),
            3 => crate::install_duplicate(raw, source.kind, flags, source),
            4 => crate::install_duplicate_at_least(raw, source.kind, flags, 0, source),
            5 => crate::install_duplicate_exact(raw, source.kind, flags, exact, source),
            _ => unreachable!(),
        }
        .unwrap();
        crate::unix::duplicate(source_fd, fd, raw).unwrap();
        assert!(
            inherited(raw),
            "installation mode {mode} omitted a real Unix handle"
        );
        assert!(
            crate::get(fd)
                .unwrap()
                .flags
                .contains(FdFlags::CLOSE_ON_EXEC)
        );
        crate::close(fd).unwrap();
    }
    crate::close(source_fd).unwrap();
    crate::close(peer).unwrap();
}

#[test]
fn publication_failure_restores_original_native_inheritance() {
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    assert!(!handle.is_null());
    assert!(!inherited(handle as usize));
    let called = std::cell::Cell::new(false);
    let result = crate::install_with(handle as usize, FdKind::Event, FdFlags::NONE, |_, _| {
        called.set(true);
        assert!(inherited(handle as usize));
        Err(crate::EIO)
    });
    assert_eq!(result, Err(crate::EIO));
    assert!(called.get());
    assert!(
        !inherited(handle as usize),
        "unpublished handle still belongs to caller"
    );
    assert_ne!(unsafe { CloseHandle(handle) }, 0);
}

#[test]
fn invalid_owned_handle_fails_before_descriptor_publication() {
    let called = std::cell::Cell::new(false);
    let result = crate::install_with(
        usize::MAX - 4095,
        FdKind::UnixSocket,
        FdFlags::NONE,
        |_, _| {
            called.set(true);
            Ok(())
        },
    );
    assert_eq!(result, Err(crate::EBADF));
    assert!(!called.get());
}

#[test]
fn invalid_handle_replacement_keeps_the_published_descriptor() {
    let (fd, peer) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
    let before = crate::get(fd).unwrap();
    assert_eq!(crate::set_raw(fd, usize::MAX - 4095), Err(crate::EBADF));
    let after = crate::get(fd).unwrap();
    assert_eq!(after.raw, before.raw);
    assert_eq!(after.generation, before.generation);
    assert!(inherited(after.raw));
    crate::close(fd).unwrap();
    crate::close(peer).unwrap();
}
