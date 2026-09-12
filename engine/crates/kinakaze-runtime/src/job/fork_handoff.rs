//! One-shot fork restoration notification. Only the two processes participating
//! in this fork hold the event; its name includes the native creation identity.
use super::Entry;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CreateEventW, WaitForMultipleObjects, WaitForSingleObject,
};

pub(super) fn event(entry: &Entry) -> Result<OwnedHandle, u32> {
    let name: Vec<u16> = format!(
        "Local\\kinakaze.fork.restored.{}.{}.{}.{:016x}",
        crate::authority::domain_id(),
        entry.namespace_pid,
        entry.pid,
        entry.token,
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let raw = unsafe { CreateEventW(core::ptr::null(), 1, 0, name.as_ptr()) };
    if raw.is_null() {
        return Err(unsafe { GetLastError() });
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Completion {
    Restored(u32),
    Exited,
}

/// The process handle is a retained reference to this specific fork child.
/// A completion published before event creation is found by the state recheck;
/// later completion signals the event. No timer-based readiness scan is needed.
pub(crate) fn wait(pid: u32, process: HANDLE, timeout: u32) -> Result<Completion, u32> {
    let Some(entry) = super::lookup(pid) else {
        return if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
            Ok(Completion::Exited)
        } else {
            Err(5)
        };
    };
    let restored = event(&entry)?;
    if let Some(error) = super::namespaces::fork_result(pid) {
        return Ok(Completion::Restored(error));
    }
    // Put native exit first: a child dying during publication must wake its
    // parent even if it could not signal. The state remains the error authority.
    let handles = [process, restored.as_raw_handle()];
    let wake = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) };
    if let Some(error) = super::namespaces::fork_result(pid) {
        return Ok(Completion::Restored(error));
    }
    match wake {
        WAIT_OBJECT_0 => Ok(Completion::Exited),
        WAIT_TIMEOUT => Err(WAIT_TIMEOUT),
        value if value == WAIT_OBJECT_0 + 1 => Err(5), // Event without publication.
        _ => Err(unsafe { GetLastError() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use windows_sys::Win32::System::Threading::SetEvent;

    #[test]
    fn child_helper() {
        if std::env::var_os("KINAKAZE_FORK_ACK_TEST").is_none() {
            return;
        }
        let mut byte = [0];
        if std::io::stdin().read(&mut byte).unwrap() == 0 {
            return;
        }
        if byte[0] != 1 {
            super::super::namespaces::set_fork_error(u32::from(byte[0]));
        }
        assert!(super::super::namespaces::mark_fork_restored());
        println!("published");
        std::io::stdout().flush().unwrap();
        if byte[0] != 1 {
            std::process::exit(127);
        }
        let _ = std::io::stdin().read(&mut byte);
    }

    struct Fixture {
        child: Child,
        entry: Entry,
    }
    impl Fixture {
        fn new() -> Self {
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "job::fork_handoff::tests::child_helper",
                    "--nocapture",
                ])
                .env("KINAKAZE_FORK_ACK_TEST", "1")
                .creation_flags(0x08000000)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let entry = super::super::register(child.id(), 0, 0, 0, 0).unwrap();
            Self { child, entry }
        }
        fn publish_error(&mut self, code: u8) {
            self.child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&[code])
                .unwrap();
            let stdout = self.child.stdout.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let observed = std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                    .any(|line| line.trim_end().ends_with("published"));
                let _ = tx.send(observed);
            });
            assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        }
        fn publish(&mut self) {
            self.publish_error(1);
        }
        fn wait(&self, milliseconds: u32) -> Result<Completion, u32> {
            super::wait(
                self.entry.namespace_pid,
                self.child.as_raw_handle(),
                milliseconds,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            super::super::release_slot(self.entry.namespace_pid);
        }
    }

    #[test]
    fn exact_child_publication_signals_an_already_open_event() {
        let mut f = Fixture::new();
        let notification = event(&f.entry).unwrap();
        assert_eq!(f.wait(0), Err(WAIT_TIMEOUT));
        let pid = f.entry.namespace_pid;
        let process = f.child.as_raw_handle() as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            tx.send(super::wait(pid, process as HANDLE, 2000)).unwrap();
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(30))
                .is_err()
        );
        f.publish();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(),
            Ok(Completion::Restored(0))
        );
        waiter.join().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(notification.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(f.wait(0), Ok(Completion::Restored(0)));
        assert!(f.child.try_wait().unwrap().is_none());
    }

    #[test]
    fn publication_before_subscription_is_not_lost() {
        let mut f = Fixture::new();
        f.publish(); // The publisher's only event handle has already closed.
        assert_eq!(f.wait(0), Ok(Completion::Restored(0)));
    }

    #[test]
    fn initializer_error_is_preserved_with_its_completion() {
        let mut f = Fixture::new();
        f.publish_error(22);
        assert_eq!(f.wait(0), Ok(Completion::Restored(22)));
    }

    #[test]
    fn failed_creation_retires_its_callback_handle_and_shared_row() {
        use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut f = Fixture::new();
        let mut retained = core::ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    f.child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut retained,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        assert!(
            crate::child_processes()
                .lock()
                .unwrap()
                .insert(f.entry.namespace_pid, retained as usize)
        );
        f.publish_error(22);
        assert_eq!(
            crate::windows::wait_fork_handoff(f.entry.namespace_pid),
            Err(crate::ForkError {
                stage: crate::ForkStage::Unsupported,
                os_code: 22
            })
        );
        assert!(
            crate::child_processes()
                .lock()
                .unwrap()
                .get(f.entry.namespace_pid)
                .is_none()
        );
        assert!(super::super::lookup(f.entry.namespace_pid).is_none());
        assert_eq!(f.child.wait().unwrap().code(), Some(127));
    }

    #[test]
    fn expired_observations_release_their_event_handles() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        let f = Fixture::new();
        let count = || {
            let mut value = 0;
            assert_ne!(
                unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut value) },
                0
            );
            value
        };
        assert_eq!(f.wait(0), Err(WAIT_TIMEOUT));
        let before = count();
        for _ in 0..100 {
            assert_eq!(f.wait(0), Err(WAIT_TIMEOUT));
        }
        assert_eq!(count(), before);
    }

    #[test]
    fn child_exit_without_restoration_wakes_the_parent() {
        let mut f = Fixture::new();
        drop(f.child.stdin.take());
        assert_eq!(f.wait(2000), Ok(Completion::Exited));
    }

    #[test]
    fn replaced_native_creation_identity_has_a_distinct_event() {
        let f = Fixture::new();
        let first = event(&f.entry).unwrap();
        let mut replacement = f.entry;
        replacement.token += 1;
        let second = event(&replacement).unwrap();
        assert_ne!(unsafe { SetEvent(first.as_raw_handle()) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(second.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
    }
}
