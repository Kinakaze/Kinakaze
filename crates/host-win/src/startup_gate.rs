//! One-shot readiness gate for overlapping manager and worker bootstrap.
use std::ffi::OsStr;
use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};

/// A non-inherited, current-user event. Keep the owner alive until the child
/// has opened it; signaling remains observable by children that open later.
pub struct StartupGate {
    handle: OwnedHandle,
    name: String,
}

impl StartupGate {
    pub fn new() -> io::Result<Self> {
        let name = format!(r"Local\kinakaze-startup-{}", crate::random_token()?);
        let wide = crate::wide(OsStr::new(&name))?;
        let security = crate::security::UserSecurity::new()?;
        let attributes = security.attributes();
        // SAFETY: Inputs live through the call; the returned handle has one owner.
        let raw = unsafe { CreateEventW(&attributes, 1, 0, wide.as_ptr()) };
        let error = unsafe { GetLastError() };
        let handle = unsafe { crate::owned(raw)? };
        if error == ERROR_ALREADY_EXISTS {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "startup gate collision",
            ));
        }
        Ok(Self { handle, name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn release(&self) -> io::Result<()> {
        // SAFETY: The event handle remains owned throughout this call.
        if unsafe { SetEvent(self.handle.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Open with wait-only access, then close the temporary handle before
    /// returning. A dead supervisor cannot strand a child indefinitely.
    pub fn wait(name: &OsStr, timeout: Duration) -> io::Result<()> {
        let name = crate::wide(name)?;
        // SAFETY: The name is terminated and the handle is not inherited.
        let event =
            unsafe { crate::owned(OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()))? };
        let timeout = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: The event remains live until the wait completes.
        match unsafe { WaitForSingleObject(event.as_raw_handle(), timeout) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "manager readiness timed out",
            )),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            _ => Err(io::Error::other("unexpected startup gate wait result")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_blocks_until_release_and_retains_readiness() {
        let gate = StartupGate::new().unwrap();
        assert_eq!(
            StartupGate::wait(OsStr::new(gate.name()), Duration::ZERO)
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let name = gate.name().to_owned();
        let child = std::thread::spawn(move || {
            StartupGate::wait(OsStr::new(&name), Duration::from_secs(5))
        });
        gate.release().unwrap();
        child.join().unwrap().unwrap();
        StartupGate::wait(OsStr::new(gate.name()), Duration::ZERO).unwrap();
        let name = gate.name().to_owned();
        drop(gate);
        assert!(StartupGate::wait(OsStr::new(&name), Duration::ZERO).is_err());
    }
}
