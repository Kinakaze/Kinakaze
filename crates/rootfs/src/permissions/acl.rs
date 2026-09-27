//! Minimal NTFS permission needed on newly created installer directories.
use std::{ffi::c_void, io, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{GENERIC_ALL, LocalFree},
    Security::{
        ACE_HEADER, ACL, Authorization::*, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        GetAce, OWNER_SECURITY_INFORMATION, PSID,
    },
    Storage::FileSystem::FILE_DELETE_CHILD,
};

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: Win32 allocated this buffer with LocalAlloc; this is its owner.
        unsafe { LocalFree(self.0) };
    }
}

fn checked(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

struct Security {
    name: Vec<u16>,
    owner: PSID,
    acl: *mut ACL,
    _descriptor: Local,
}
impl Security {
    fn read(path: &Path) -> io::Result<Self> {
        let name: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut owner, mut acl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        // SAFETY: path is terminated; all output pointers refer to live variables.
        checked(unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        })?;
        let result = Self {
            name,
            owner,
            acl,
            _descriptor: Local(descriptor),
        };
        if owner.is_null() || acl.is_null() {
            return Err(io::Error::other(
                "directory has no owner or discretionary ACL",
            ));
        }
        Ok(result)
    }

    fn entry(&self, rights: u32, mode: ACCESS_MODE) -> EXPLICIT_ACCESS_W {
        EXPLICIT_ACCESS_W {
            grfAccessPermissions: rights,
            grfAccessMode: mode,
            grfInheritance: CONTAINER_INHERIT_ACE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: self.owner.cast(),
            },
        }
    }

    fn apply(&self, entries: &[EXPLICIT_ACCESS_W], old: *const ACL, flags: u32) -> io::Result<()> {
        let mut merged = null_mut();
        // SAFETY: entries and owner SID remain live; old ACL is null or owned by self.
        checked(unsafe {
            SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), old, &mut merged)
        })?;
        let _merged = Local(merged.cast());
        // SAFETY: path and ACL remain live for the synchronous call. Only the DACL
        // of this newly created directory changes, not its parent or existing roots.
        checked(unsafe {
            SetNamedSecurityInfoW(
                self.name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | flags,
                null_mut(),
                null_mut(),
                merged,
                null_mut(),
            )
        })
    }
}

pub(super) fn grant_owner_delete_children(path: &Path) -> io::Result<()> {
    let security = Security::read(path)?;
    // Explicit allows can take precedence over inherited denies. Refuse to
    // augment any ACL that denies this right, preserving administrator policy.
    // SAFETY: GetNamedSecurityInfo owns a valid ACL; GetAce returns an ACE within
    // that live allocation. Every denied-ACE variant starts with header + mask.
    unsafe {
        for index in 0..u32::from((*security.acl).AceCount) {
            let mut ace = null_mut();
            if GetAce(security.acl, index, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = &*ace.cast::<ACE_HEADER>();
            // ACCESS_DENIED, ACCESS_DENIED_OBJECT, ACCESS_DENIED_CALLBACK,
            // and ACCESS_DENIED_CALLBACK_OBJECT ACE types from winnt.h.
            if matches!(header.AceType, 1 | 6 | 10 | 12)
                && *ace.cast::<u32>().add(1) & (FILE_DELETE_CHILD | GENERIC_ALL) != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "directory ACL denies the delete-child permission needed for case sensitivity",
                ));
            }
        }
    }
    security.apply(
        &[security.entry(FILE_DELETE_CHILD, GRANT_ACCESS)],
        security.acl,
        0,
    )
}

#[cfg(test)]
mod tests;
