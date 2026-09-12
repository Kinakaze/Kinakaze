use std::ffi::{OsStr, c_void};
use std::io;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// LocalAlloc memory returned by security conversion functions.
struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: This is the unique owner of memory allocated by LocalAlloc.
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// A protected DACL granting access only to the current process token's user.
pub(crate) struct UserSecurity(LocalAllocation);

impl UserSecurity {
    pub(crate) fn new() -> io::Result<Self> {
        let mut raw_token = null_mut();
        // SAFETY: Output points to live storage; pseudo handle is borrowed only.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken transferred ownership of this handle.
        let token = unsafe { crate::owned(raw_token)? };
        let mut needed = 0;
        // SAFETY: A null, zero-length output queries the required buffer size.
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed);
        }
        if needed < size_of::<TOKEN_USER>() as u32 {
            return Err(io::Error::last_os_error());
        }
        // Use usize storage so TOKEN_USER and its pointer field are aligned.
        let words = (needed as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; words];
        // SAFETY: Storage is sufficiently large and aligned for TOKEN_USER.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                storage.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Successful TokenUser query initialized this aligned structure.
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        let mut sid_text = null_mut();
        // SAFETY: The token buffer keeps User.Sid alive through this call.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid_allocation = LocalAllocation(sid_text.cast());
        let mut length = 0;
        // SAFETY: ConvertSidToStringSidW guarantees a NUL-terminated UTF-16 string.
        while unsafe { *sid_text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: The string has been measured and remains owned above.
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
            .map_err(|_| io::Error::other("invalid user SID encoding"))?;
        drop(sid_allocation);
        let sddl = crate::wide(OsStr::new(&format!("D:P(A;;GA;;;{sid})")))?;
        let mut descriptor = null_mut();
        // SAFETY: SDDL is NUL terminated; the output descriptor is independently allocated.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(LocalAllocation(descriptor)))
    }

    pub(crate) fn attributes(&self) -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: 0,
        }
    }
}

// SAFETY: The descriptor is immutable LocalAlloc memory, held until all uses end.
unsafe impl Send for UserSecurity {}
// SAFETY: Shared access only produces a descriptor borrowed by CreateNamedPipeW.
unsafe impl Sync for UserSecurity {}
