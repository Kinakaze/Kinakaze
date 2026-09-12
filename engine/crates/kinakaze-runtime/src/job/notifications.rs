//! Event publication and exact process handles for the signal pump.
//!
//! Publishers hold the PID-table mutex, signal BEFORE committing state, and
//! consumers reset under that same mutex. A publisher dying at either side of
//! the store therefore cannot leave a committed change without a notification.

use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

pub fn event(kind: &str, host_pid: u32) -> Option<OwnedHandle> {
    let name: Vec<u16> = format!("Local\\kinakaze.job.{kind}.{host_pid}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe { CreateEventW(core::ptr::null(), 1, 0, name.as_ptr()) };
    (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn notify(kind: &str, host_pid: u32) -> bool {
    let Some(event) = event(kind, host_pid) else {
        return false;
    };
    unsafe { SetEvent(event.as_raw_handle()) != 0 }
}

pub(super) unsafe fn signal(entry: *mut u8) -> bool {
    notify("wake", unsafe { load32(entry, SLOT_PID) })
}

/// Notify the process whose child set/host identity is about to change.
/// Missing rows have no subscriber; a later registration takes a full snapshot.
pub(super) unsafe fn topology(base: *mut u8, pid: u32) {
    if let Some(entry) = unsafe { find(base, pid) } {
        if !notify("changes", unsafe { load32(entry, SLOT_PID) }) {
            std::process::abort();
        }
    }
}

/// Called before exec rebinds a process object. Children also watch their
/// parent's exact object, so they must replace that subscription on exec.
pub(super) unsafe fn replacing(base: *mut u8, entry: *mut u8) {
    let pid = unsafe { load32(entry, SLOT_NAMESPACE_PID) };
    unsafe {
        topology(base, load32(entry, SLOT_PPID));
        topology(base, pid);
        for index in 0..CAPACITY {
            let child = slot(base, index);
            if load32(child, SLOT_PID) != 0 && load32(child, SLOT_PPID) == pid {
                topology(base, load32(child, SLOT_NAMESPACE_PID));
            }
        }
    }
}

/// Keep guest delivery's empty hot path entirely in userspace.
pub fn has_pending(pid: u32) -> bool {
    let Some(base) = map() else { return false };
    let Some(entry) = (unsafe { find_stable(base, pid) }) else {
        return false;
    };
    // Only the owner reads its own stable row on this fast path.
    unsafe { load64(entry, SLOT_PENDING) != 0 }
}

/// Reset and drain under the same mutex used by pre-commit publishers.
pub fn take_pending(pid: u32, event: usize) -> Result<u64, ()> {
    with_table(|base| {
        let entry = unsafe { find(base, pid) }.ok_or(())?;
        if unsafe { ResetEvent(event as HANDLE) } == 0 {
            return Err(());
        }
        Ok(unsafe { swap64(entry, SLOT_PENDING, 0) })
    })
    .ok_or(())?
}

fn pin(entry: Entry) -> Option<OwnedHandle> {
    let (pid, expected) = (entry.pid, entry.token);
    let raw = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if raw.is_null() {
        return None;
    }
    let owned = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut times = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    let valid = unsafe {
        GetProcessTimes(
            raw,
            &raw mut times[0],
            &raw mut times[1],
            &raw mut times[2],
            &raw mut times[3],
        )
    } != 0;
    let observed = (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime);
    (valid && observed == expected).then_some(owned)
}

/// Pin both the current image and the coordinator's original exec wrapper.
/// Call without the shared table mutex: the coordinator owns a separate lock.
pub(super) fn completion_sources(entry: Entry) -> Result<Vec<OwnedHandle>, ()> {
    let mut handles = Vec::with_capacity(2);
    if let Some(wrapper) = crate::duplicate_child_wait_handle(entry.namespace_pid)? {
        handles.push(wrapper);
    }
    if let Some(current) = pin(entry) {
        handles.push(current);
    }
    Ok(handles)
}

pub(super) fn completed(handle: &OwnedHandle) -> Result<bool, ()> {
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        windows_sys::Win32::Foundation::WAIT_TIMEOUT => Ok(false),
        _ => Err(()),
    }
}

pub struct Sources {
    pub handles: Vec<OwnedHandle>,
    /// A child disappeared while taking the snapshot. Reap it before sleeping.
    pub rescan: bool,
}

/// Includes adopted children and CLONE_PARENT children, which need not have an
/// entry in this process's native fork registry. Handles pin creation identity.
pub fn sources(pid: u32, changed: usize) -> Result<Sources, ()> {
    let (children, parent) = with_table(|base| {
        if unsafe { ResetEvent(changed as HANDLE) } == 0 {
            return Err(());
        }
        unsafe { sweep(base) };
        let own = unsafe { find(base, pid) }.ok_or(())?;
        let mut children = Vec::new();
        for index in 0..CAPACITY {
            let child = unsafe { slot(base, index) };
            let interested = unsafe {
                load32(child, SLOT_PID) != 0
                    && load32(child, SLOT_PPID) == pid
                    && load32(child, SLOT_FLAGS) & FLAG_EXIT_NOTIFIED == 0
            };
            if interested {
                children.push(unsafe { read_slot(child) });
            }
        }
        let parent = unsafe { find(base, load32(own, SLOT_PPID)).map(|entry| read_slot(entry)) };
        Ok((children, parent))
    })
    .ok_or(())??;
    let mut sources = Sources {
        handles: Vec::new(),
        rescan: false,
    };
    for child in children {
        let mut waiting = false;
        for handle in completion_sources(child)? {
            if !completed(&handle)? {
                waiting = true;
                sources.handles.push(handle);
            }
        }
        // Completion raced the preceding reap. Reconcile before sleeping.
        // A completed replacement plus a live wrapper is NOT this race: wait
        // on the wrapper instead of spinning on the replacement's exit handle.
        sources.rescan |= !waiting;
    }
    if let Some(handle) = parent.and_then(pin) {
        if !completed(&handle)? {
            sources.handles.push(handle);
        }
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use windows_sys::Win32::System::Threading::GetProcessId;

    fn own() -> Entry {
        lookup_host(current_host_pid())
            .unwrap_or_else(|| register(current_host_pid(), 0, 0, 0, 0).unwrap())
    }

    #[test]
    fn native_child_helper() {
        if std::env::var_os("KINAKAZE_NOTIFICATION_TEST_CHILD").is_some() {
            let _ = std::io::stdin().read(&mut [0u8]);
        }
    }

    struct TestChild {
        process: Child,
        pid: u32,
    }
    impl TestChild {
        fn new(parent: Entry) -> Self {
            let process = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "job::notifications::tests::native_child_helper",
                    "--nocapture",
                ])
                .env("KINAKAZE_NOTIFICATION_TEST_CHILD", "1")
                .creation_flags(0x08000000)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = register(
                process.id(),
                parent.pgid,
                parent.sid,
                parent.namespace_pid,
                0,
            )
            .unwrap()
            .namespace_pid;
            Self { process, pid }
        }
    }
    impl Drop for TestChild {
        fn drop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
            release_slot(self.pid);
        }
    }

    #[test]
    fn wake_before_commit_cannot_be_drained_before_the_pending_bit() {
        let own = own();
        let wake = event("wake", own.pid).unwrap();
        let raw = wake.as_raw_handle() as usize;
        let _ = take_pending(own.namespace_pid, raw).unwrap();
        let (awake_tx, awake_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            assert_eq!(
                unsafe { WaitForSingleObject(raw as HANDLE, 2_000) },
                WAIT_OBJECT_0
            );
            awake_tx.send(()).unwrap();
            done_tx.send(take_pending(own.namespace_pid, raw)).unwrap();
        });
        with_table(|base| {
            let entry = unsafe { find(base, own.namespace_pid) }.unwrap();
            assert!(unsafe { signal(entry) });
            awake_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_millis(30))
                    .is_err()
            );
            unsafe { or64(entry, SLOT_PENDING, 1 << 27) };
        })
        .unwrap();
        assert_eq!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            Ok(1 << 27)
        );
        reader.join().unwrap();
        assert_ne!(
            unsafe { WaitForSingleObject(wake.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn a_child_without_a_local_fork_handle_wakes_on_exact_kernel_exit() {
        let own = own();
        let changes = event("changes", own.pid).unwrap();
        let mut child = TestChild::new(own);
        assert_eq!(
            unsafe { WaitForSingleObject(changes.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        let snapshot = sources(own.namespace_pid, changes.as_raw_handle() as usize).unwrap();
        let handle = snapshot
            .handles
            .iter()
            .find(|h| unsafe { GetProcessId(h.as_raw_handle()) } == child.process.id())
            .unwrap();
        assert_ne!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        // An early Linux exit report must not be mistaken for native teardown.
        post_report(child.pid, StateChange::Exited(0));
        assert!(!reap_dead_children(own.namespace_pid).unwrap());
        drop(child.process.stdin.take());
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        assert!(reap_dead_children(own.namespace_pid).unwrap());
        let after = sources(own.namespace_pid, changes.as_raw_handle() as usize).unwrap();
        assert!(
            !after
                .handles
                .iter()
                .any(|h| unsafe { GetProcessId(h.as_raw_handle()) } == child.process.id())
        );
    }

    #[test]
    fn adoption_wakes_the_new_reaper_and_adds_the_orphan_process_handle() {
        let own = own();
        update_flags(own.namespace_pid, FLAG_SUBREAPER, 0);
        let changes = event("changes", own.pid).unwrap();
        let parent = TestChild::new(own);
        let orphan = TestChild::new(lookup(parent.pid).unwrap());
        let before = sources(own.namespace_pid, changes.as_raw_handle() as usize).unwrap();
        assert!(
            !before
                .handles
                .iter()
                .any(|h| unsafe { GetProcessId(h.as_raw_handle()) } == orphan.process.id())
        );
        post_report(parent.pid, StateChange::Exited(0));
        assert_eq!(
            unsafe { WaitForSingleObject(changes.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(lookup(orphan.pid).unwrap().ppid, own.namespace_pid);
        let after = sources(own.namespace_pid, changes.as_raw_handle() as usize).unwrap();
        assert!(
            after
                .handles
                .iter()
                .any(|h| unsafe { GetProcessId(h.as_raw_handle()) } == orphan.process.id())
        );
        if own.flags & FLAG_SUBREAPER == 0 {
            update_flags(own.namespace_pid, 0, FLAG_SUBREAPER);
        }
    }

    #[test]
    fn exec_exit_waits_for_original_wrapper_cleanup_without_polling() {
        let own = own();
        let changes = event("changes", own.pid).unwrap();
        let mut wrapper = TestChild::new(own);
        let mut replacement = TestChild::new(own);
        assert_eq!(
            crate::windows::register_child_handle(
                wrapper.pid,
                wrapper.process.as_raw_handle() as usize
            ),
            1
        );
        struct Registration(u32);
        impl Drop for Registration {
            fn drop(&mut self) {
                if let Some(handle) = crate::child_processes().lock().unwrap().remove(self.0) {
                    unsafe { CloseHandle(handle as HANDLE) };
                }
            }
        }
        let _registered = Registration(wrapper.pid);
        assert!(replace_host(wrapper.pid, replacement.process.id()));
        post_report(wrapper.pid, StateChange::Exited(0));
        drop(replacement.process.stdin.take());
        assert_eq!(
            unsafe { WaitForSingleObject(replacement.process.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        // The Linux report and replacement exit are both ready, but the old
        // exec wrapper still owns inherited descriptors and wait(2) waits on it.
        assert!(!reap_dead_children(own.namespace_pid).unwrap());
        let pending = sources(own.namespace_pid, changes.as_raw_handle() as usize).unwrap();
        assert!(
            !pending.rescan,
            "must sleep on the wrapper, not spin on the exited image"
        );
        let retained = pending
            .handles
            .iter()
            .find(|h| unsafe { GetProcessId(h.as_raw_handle()) } == wrapper.process.id())
            .unwrap();
        assert!(
            !pending
                .handles
                .iter()
                .any(|h| unsafe { GetProcessId(h.as_raw_handle()) } == replacement.process.id())
        );
        drop(wrapper.process.stdin.take());
        assert_eq!(
            unsafe { WaitForSingleObject(retained.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        assert!(reap_dead_children(own.namespace_pid).unwrap());
        assert!(!reap_dead_children(own.namespace_pid).unwrap());
        assert_eq!(peek_report(wrapper.pid), Some(StateChange::Exited(0)));
    }
}
