use crate::{Result, download::Proxy};
use std::{
    ffi::c_void,
    io,
    ptr::{null, null_mut},
};
use windows_sys::Win32::Networking::WinHttp::*;

struct Handle(*mut c_void);
impl Handle {
    fn new(value: *mut c_void) -> io::Result<Self> {
        if value.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(value))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner of a live synchronous WinHTTP handle.
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

pub(crate) struct Client(Handle);
// SAFETY: WinHTTP sessions support concurrent requests. Each read owns separate
// connection/request handles, and the shared session outlives every read.
unsafe impl Send for Client {}
unsafe impl Sync for Client {}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn checked(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Client {
    pub(crate) fn new(proxy: &Proxy) -> Result<Self> {
        // AUTOMATIC_PROXY includes the Windows Settings/Internet Options user
        // proxy, PAC/WPAD, bypass rules and the machine configuration. Unlike
        // DEFAULT_PROXY, it does not require importing browser settings via netsh.
        match proxy {
            Proxy::System => Self::open(WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, None, None),
            Proxy::Direct => Self::open(WINHTTP_ACCESS_TYPE_NO_PROXY, None, None),
            Proxy::Http(authority) => {
                Self::open(WINHTTP_ACCESS_TYPE_NAMED_PROXY, Some(authority), None)
            }
        }
    }

    fn open(
        access: WINHTTP_ACCESS_TYPE,
        proxy: Option<&str>,
        bypass: Option<&str>,
    ) -> Result<Self> {
        let agent = wide(concat!("Kinakaze/", env!("CARGO_PKG_VERSION")));
        let proxy = proxy.map(wide);
        let bypass = bypass.map(wide);
        // SAFETY: the input strings are NUL terminated and remain live for the call.
        let session = Handle::new(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                access,
                proxy.as_ref().map_or(null(), |s| s.as_ptr()),
                bypass.as_ref().map_or(null(), |s| s.as_ptr()),
                0,
            )
        })?;
        // SAFETY: session is a valid synchronous session; timeouts are milliseconds.
        checked(unsafe { WinHttpSetTimeouts(session.0, 30_000, 30_000, 30_000, 60_000) })?;
        let redirect = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        // SAFETY: WinHTTP reads a DWORD from the live redirect value.
        checked(unsafe {
            WinHttpSetOption(
                session.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&redirect as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        })?;
        Ok(Self(session))
    }

    pub(crate) fn read(
        &self,
        url: &str,
        limit: u64,
        mut progress: impl FnMut(u64),
    ) -> Result<Vec<u8>> {
        let uri: http::Uri = url.parse()?;
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err("unsupported download URL scheme".into()),
        };
        let host = wide(
            uri.host()
                .ok_or("download URL has no host")?
                .trim_matches(['[', ']']),
        );
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let path = wide(uri.path_and_query().map_or("/", |path| path.as_str()));
        let method = wide("GET");
        // SAFETY: session is live; host is NUL terminated. Request and connection
        // are owned locally and close before the shared session can be dropped.
        let connection = Handle::new(unsafe { WinHttpConnect(self.0.0, host.as_ptr(), port, 0) })?;
        // SAFETY: connection is live; optional strings are null, other strings live.
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                if secure { WINHTTP_FLAG_SECURE } else { 0 },
            )
        })?;
        // SAFETY: synchronous GET has no headers/body or asynchronous context.
        checked(unsafe { WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) })?;
        // SAFETY: request was sent synchronously; no reserved pointer is supplied.
        checked(unsafe { WinHttpReceiveResponse(request.0, null_mut()) })?;
        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: status and size are writable DWORDs; NUMBER requests numeric data.
        checked(unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut size,
                null_mut(),
            )
        })?;
        if status != 200 {
            return Err(format!("download returned HTTP {status}").into());
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let mut received = 0;
            // SAFETY: buffer is writable for its full size, and received is a DWORD.
            checked(unsafe {
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut received,
                )
            })?;
            if received == 0 {
                return Ok(bytes);
            }
            if bytes.len() as u64 + u64::from(received) > limit {
                return Err("download exceeds the manifest archive size".into());
            }
            bytes.extend_from_slice(&buffer[..received as usize]);
            progress(bytes.len() as u64);
        }
    }
}

#[cfg(test)]
mod tests;
