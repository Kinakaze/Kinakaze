//! One whole-path query which refuses native reparses in every component.
//! A regular Win32 attribute query follows ancestor junctions, which would
//! bypass the Linux resolver's interpretation of their targets.
use std::{ffi::c_void, path::Path, ptr};
use windows_sys::Win32::Storage::FileSystem::FILE_BASIC_INFO;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: *mut c_void,
    name: *mut UnicodeString,
    attributes: u32,
    security: *mut c_void,
    qos: *mut c_void,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlDosPathNameToNtPathName_U_WithStatus(
        dos: *const u16,
        nt: *mut UnicodeString,
        file: *mut c_void,
        relative: *mut c_void,
    ) -> i32;
    fn RtlFreeUnicodeString(name: *mut UnicodeString);
    fn NtQueryAttributesFile(object: *const ObjectAttributes, basic: *mut FILE_BASIC_INFO) -> i32;
}
struct Name(UnicodeString);
impl Drop for Name {
    fn drop(&mut self) {
        unsafe { RtlFreeUnicodeString(&mut self.0) };
    }
}

pub(super) fn query(path: &Path) -> Result<u32, u32> {
    let wide = crate::path::wide_path(path).map_err(|_| 0xc000_000du32)?;
    let mut name = Name(unsafe { std::mem::zeroed() });
    let status = unsafe {
        RtlDosPathNameToNtPathName_U_WithStatus(
            wide.as_ptr(),
            &mut name.0,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(status as u32);
    }
    let object = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: ptr::null_mut(),
        name: &mut name.0,
        attributes: 0x1000, // OBJ_DONT_REPARSE, including ancestors.
        security: ptr::null_mut(),
        qos: ptr::null_mut(),
    };
    let mut basic = unsafe { std::mem::zeroed() };
    let status = unsafe { NtQueryAttributesFile(&object, &mut basic) };
    if status == 0 {
        Ok(basic.FileAttributes)
    } else {
        Err(status as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_path_query_refuses_ancestor_junctions() {
        use windows_sys::Win32::{
            Foundation::GENERIC_WRITE,
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
            System::IO::DeviceIoControl,
        };
        let root = std::env::temp_dir().join(format!(
            "kinakaze-no-reparse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("real")).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        std::fs::write(root.join("real/file"), b"data").unwrap();
        let link = root.join("link");
        std::fs::create_dir(&link).unwrap();
        let object = crate::fs::object::Object::owned(unsafe {
            CreateFileW(
                crate::path::wide_path(&link).unwrap().as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        })
        .unwrap();
        let target = format!(r"\??\{}", root.join("real").display());
        let bytes: Vec<u8> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut data = vec![0u8; 16];
        data[..4].copy_from_slice(&0xa000_0003u32.to_le_bytes());
        data[4..6].copy_from_slice(&((12 + bytes.len()) as u16).to_le_bytes());
        data[10..12].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
        data[12..14].copy_from_slice(&((bytes.len() + 2) as u16).to_le_bytes());
        data.extend(bytes);
        data.extend([0; 4]);
        let mut returned = 0;
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    object.raw(),
                    0x0009_00a4,
                    data.as_ptr().cast(),
                    data.len() as u32,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                    ptr::null_mut(),
                )
            },
            0
        );
        drop(object);
        assert!(query(&root.join("real/file")).is_ok());
        assert_eq!(std::fs::read(root.join("link/file")).unwrap(), b"data");
        assert_eq!(query(&root.join("link/file")), Err(0xc000_050b));
        assert!(
            super::super::resolve(&root, "/", "/link/file", true)
                .unwrap()
                .is_none()
        );
        assert!(
            super::super::resolve(&root, "/", "/link/new", true)
                .unwrap()
                .is_none()
        );
    }
}
