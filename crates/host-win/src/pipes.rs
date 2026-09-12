use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    NMPWAIT_WAIT_FOREVER, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};

fn pipe_name(endpoint: &str) -> io::Result<Vec<u16>> {
    const PREFIX: &str = r"\\.\pipe\";
    let suffix = endpoint.strip_prefix(PREFIX).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "endpoint must be a local \\\\.\\pipe\\ name",
        )
    })?;
    if suffix.is_empty()
        || suffix.contains(['\\', '/', ':'])
        || endpoint.encode_utf16().count() > 256
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid local pipe name",
        ));
    }
    crate::wide(OsStr::new(endpoint))
}

/// Blocking, current-user-only local byte-stream listener.
///
/// The first instance excludes an existing server. Each accept creates the next
/// listening instance before transferring the connected one, so exclusivity is
/// continuous even when the returned connections are immediately dropped.
pub struct PipeListener {
    name: Vec<u16>,
    security: crate::security::UserSecurity,
    pending: OwnedHandle,
}

impl PipeListener {
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        let name = pipe_name(endpoint)?;
        let security = crate::security::UserSecurity::new()?;
        let pending = Self::instance(&name, &security, true)?;
        Ok(Self {
            name,
            security,
            pending,
        })
    }

    fn instance(
        name: &[u16],
        security: &crate::security::UserSecurity,
        first: bool,
    ) -> io::Result<OwnedHandle> {
        let attributes = security.attributes();
        let first_flag = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
        // SAFETY: Name and descriptor remain alive; synchronous pipe has no overlapped state.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | first_flag,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                5000,
                &attributes,
            )
        };
        // SAFETY: CreateNamedPipeW transfers the newly created handle.
        unsafe { crate::owned(handle) }
    }

    /// Wait for one client. To stop an accept loop, set its external shutdown
    /// flag and connect once to this listener's private endpoint to wake it.
    pub fn accept(&mut self) -> io::Result<PipeConnection> {
        loop {
            // SAFETY: The listener owns a synchronous, unconnected server handle.
            if unsafe { ConnectNamedPipe(self.pending.as_raw_handle(), null_mut()) } == 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(code) if code == ERROR_PIPE_CONNECTED as i32 => {}
                    // A client may connect then disappear before accept starts.
                    // Replace the spent instance without surrendering the name.
                    Some(code)
                        if code == ERROR_NO_DATA as i32
                            || code == ERROR_BROKEN_PIPE as i32
                            || code == ERROR_PIPE_NOT_CONNECTED as i32 =>
                    {
                        self.pending = Self::instance(&self.name, &self.security, false)?;
                        continue;
                    }
                    _ => return Err(error),
                }
            }
            let replacement = Self::instance(&self.name, &self.security, false)?;
            let handle = std::mem::replace(&mut self.pending, replacement);
            return Ok(PipeConnection {
                handle,
                server: true,
            });
        }
    }
}

/// A non-inheritable synchronous pipe stream. Drop closes without blocking on
/// FlushFileBuffers; protocol replies determine when an exchange is complete.
pub struct PipeConnection {
    handle: OwnedHandle,
    server: bool,
}

impl PipeConnection {
    pub fn connect(endpoint: &str) -> io::Result<Self> {
        let name = pipe_name(endpoint)?;
        loop {
            // SECURITY_IDENTIFICATION prevents a server impersonating the client
            // at a higher impersonation level if an endpoint was misconfigured.
            // SAFETY: Null security attributes make this handle non-inheritable.
            let raw = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    null_mut(),
                )
            };
            // SAFETY: Successful CreateFileW transfers this handle to the caller.
            match unsafe { crate::owned(raw) } {
                Ok(handle) => {
                    return Ok(Self {
                        handle,
                        server: false,
                    });
                }
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    // SAFETY: Name remains live; the kernel wait avoids busy polling.
                    if unsafe { WaitNamedPipeW(name.as_ptr(), NMPWAIT_WAIT_FOREVER) } == 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Query the OS for the opposite endpoint's process ID. Never trusts wire data.
    pub fn peer_pid(&self) -> io::Result<u32> {
        let mut pid = 0;
        // SAFETY: Connected pipe handle and output storage are valid.
        let success = unsafe {
            if self.server {
                GetNamedPipeClientProcessId(self.handle.as_raw_handle(), &mut pid)
            } else {
                GetNamedPipeServerProcessId(self.handle.as_raw_handle(), &mut pid)
            }
        };
        if success == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(pid)
        }
    }
}

impl Read for PipeConnection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let len = buffer.len().min(u32::MAX as usize) as u32;
        let mut received = 0;
        // SAFETY: Buffer is writable for len, and this is a synchronous pipe.
        if unsafe {
            ReadFile(
                self.handle.as_raw_handle(),
                buffer.as_mut_ptr(),
                len,
                &mut received,
                null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(code) if code == ERROR_BROKEN_PIPE as i32
                || code == ERROR_PIPE_NOT_CONNECTED as i32 || code == ERROR_NO_DATA as i32)
            {
                return Ok(0);
            }
            return Err(error);
        }
        Ok(received as usize)
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let len = buffer.len().min(u32::MAX as usize) as u32;
        let mut sent = 0;
        // SAFETY: Buffer is readable for len, and this is a synchronous pipe.
        if unsafe {
            WriteFile(
                self.handle.as_raw_handle(),
                buffer.as_ptr(),
                len,
                &mut sent,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(sent as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        // No user-space buffering; FlushFileBuffers would wait for a peer read.
        Ok(())
    }
}
