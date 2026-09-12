//! Read native link records from a pinned inode. Neither the link's current
//! pathname nor its target is opened. Native filesystem-control requests use a
//! private asynchronous handle and retire before their buffers are released.

use super::{NativeIoStatus, complete_native_status, object::Object};
use crate::{EIO, EOPNOTSUPP, errno_from_win32};
use std::{ffi::c_void, path::Path, ptr};
use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::FILE_READ_ATTRIBUTES};

const GET_REPARSE_POINT: u32 = 0x0009_00a8;
const TAG_SYMLINK: u32 = 0xa000_000c;
const TAG_MOUNT_POINT: u32 = 0xa000_0003;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtFsControlFile(
        file: HANDLE,
        event: HANDLE,
        apc: *const c_void,
        context: *const c_void,
        io: *mut NativeIoStatus,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Target {
    path: String,
    relative: bool,
}

fn word(bytes: &[u8], offset: usize) -> Result<usize, i32> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(EIO)?
            .try_into()
            .unwrap(),
    ) as usize)
}

fn wide_string(path: &[u8], offset: usize, length: usize) -> Result<String, i32> {
    if offset % 2 != 0 || length % 2 != 0 {
        return Err(EIO);
    }
    let bytes = path
        .get(offset..offset.checked_add(length).ok_or(EIO)?)
        .ok_or(EIO)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text = String::from_utf16(&units).map_err(|_| EIO)?;
    if text.contains('\0') {
        return Err(EIO);
    }
    Ok(text)
}

fn decode(bytes: &[u8]) -> Result<Option<Target>, i32> {
    if bytes.len() < 8 {
        return Err(EIO);
    }
    let tag = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let length = word(bytes, 4)?;
    let data = bytes.get(8..8 + length).ok_or(EIO)?;
    // Reparse points such as cloud placeholders are not path redirections.
    // Do not report them as symbolic links merely because REPARSE_POINT is set.
    if tag & 0x2000_0000 == 0 {
        return Ok(None);
    }
    let (base, relative) = match tag {
        TAG_SYMLINK => {
            let flags = u32::from_le_bytes(data.get(8..12).ok_or(EIO)?.try_into().unwrap());
            if flags & !1 != 0 {
                return Err(EIO);
            }
            (12, flags == 1)
        }
        TAG_MOUNT_POINT => (8, false),
        _ => return Err(EOPNOTSUPP),
    };
    let path = data.get(base..).ok_or(EIO)?;
    let target = wide_string(path, word(data, 0)?, word(data, 2)?)?;
    // Both counted names must fit the record even though the display name is
    // not authoritative for target lookup or readlink output.
    wide_string(path, word(data, 4)?, word(data, 6)?)?;
    if target.is_empty() {
        return Err(EIO);
    }
    Ok(Some(Target {
        path: target,
        relative,
    }))
}

pub(super) fn read(file: HANDLE) -> Result<Option<Target>, i32> {
    let query = unsafe { Object::reopen(file, FILE_READ_ATTRIBUTES)? };
    let mut buffer = vec![0u64; 16_384 / 8];
    let mut io = NativeIoStatus::default();
    let status = unsafe {
        NtFsControlFile(
            query.raw(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            &mut io,
            GET_REPARSE_POINT,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    };
    let status = unsafe { complete_native_status(query.raw(), &mut io, status)? };
    if status as u32 == 0xc000_0275 {
        return Ok(None);
    } // STATUS_NOT_A_REPARSE_POINT
    if status < 0 {
        return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
    }
    if io.information > buffer.len() * 8 {
        return Err(EIO);
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), io.information) };
    decode(bytes)
}

fn dos_name(name: &str) -> Result<&str, i32> {
    let name = name
        .strip_prefix(r"\??\")
        .or_else(|| name.strip_prefix(r"\\?\"))
        .unwrap_or(name);
    let bytes = name.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        // UNC/volume/device namespaces do not have a mount in the current guest
        // path model. Do not invent a /server or /Volume{...} target for them.
        return Err(EOPNOTSUPP);
    }
    Ok(name)
}

impl Target {
    /// Translate namespace syntax only. Canonicalizing would follow the target,
    /// erase dot components, and change readlink results when the target moves.
    pub(super) fn guest(&self, root: &Path) -> Result<String, i32> {
        if self.relative {
            let path = Path::new(&self.path);
            if path.has_root()
                || path
                    .components()
                    .any(|part| matches!(part, std::path::Component::Prefix(_)))
            {
                return Err(EIO);
            }
            return Ok(crate::path::unescape_path(&self.path.replace('\\', "/")).into_owned());
        }
        let target = dos_name(&self.path)?.replace('\\', "/");
        let root = dos_name(root.to_str().ok_or(EIO)?)?.replace('\\', "/");
        let root = root.trim_end_matches('/');
        let same_drive = target.as_bytes()[0].eq_ignore_ascii_case(&root.as_bytes()[0]);
        let relative = if same_drive && target.get(1..root.len()) == root.get(1..) {
            target
                .get(root.len()..)
                .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        } else {
            None
        };
        let guest = match relative {
            Some("") => "/".to_owned(),
            Some(rest) => rest.to_owned(),
            None => format!(
                "/{}{}",
                target.as_bytes()[0].to_ascii_lowercase() as char,
                &target[2..]
            ),
        };
        Ok(crate::path::unescape_path(&guest).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("kinakaze-reparse-{}-{stamp}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(tag: u32, substitute: &str, print: &str, flags: u32) -> Vec<u8> {
        let a: Vec<u8> = substitute
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let b: Vec<u8> = print.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let base = if tag == TAG_SYMLINK { 12 } else { 8 };
        let mut data = vec![0u8; 8 + base];
        data[..4].copy_from_slice(&tag.to_le_bytes());
        data[4..6].copy_from_slice(&((base + a.len() + b.len() + 4) as u16).to_le_bytes());
        data[10..12].copy_from_slice(&(a.len() as u16).to_le_bytes());
        data[12..14].copy_from_slice(&((a.len() + 2) as u16).to_le_bytes());
        data[14..16].copy_from_slice(&(b.len() as u16).to_le_bytes());
        if tag == TAG_SYMLINK {
            data[16..20].copy_from_slice(&flags.to_le_bytes());
        }
        data.extend_from_slice(&a);
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&b);
        data.extend_from_slice(&[0, 0]);
        data
    }

    fn junction(link: &Path, target: &Path) -> Object {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows_sys::Win32::Storage::FileSystem::DELETE;
        std::fs::create_dir(link).unwrap();
        let object = Object::open(link, GENERIC_READ | GENERIC_WRITE | DELETE).unwrap();
        assert_eq!(read(object.raw()).unwrap(), None);
        let target_name = format!(r"\??\{}", target.to_str().unwrap());
        let data = record(TAG_MOUNT_POINT, &target_name, target.to_str().unwrap(), 0);
        let mut io = NativeIoStatus::default();
        let status = unsafe {
            NtFsControlFile(
                object.raw(),
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                &mut io,
                0x0009_00a4,
                data.as_ptr().cast(),
                data.len() as u32,
                ptr::null_mut(),
                0,
            )
        };
        assert_eq!(
            unsafe { complete_native_status(object.raw(), &mut io, status) }.unwrap(),
            0
        );
        object
    }

    #[test]
    fn link_target_translation_does_not_resolve_or_normalize_the_target() {
        let root = Path::new(r"C:\guest");
        let relative = decode(&record(TAG_SYMLINK, r"..\目标\missing", "display only", 1))
            .unwrap()
            .unwrap();
        assert_eq!(relative.guest(root).unwrap(), "../目标/missing");
        let absolute = decode(&record(
            TAG_SYMLINK,
            r"\??\C:\guest\dir\..\missing",
            "wrong",
            0,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(absolute.guest(root).unwrap(), "/dir/../missing");
        let outside = decode(&record(TAG_MOUNT_POINT, r"\??\D:\outside", "", 0))
            .unwrap()
            .unwrap();
        assert_eq!(outside.guest(root).unwrap(), "/d/outside");
        let sibling = decode(&record(TAG_MOUNT_POINT, r"\??\C:\guest-other\file", "", 0))
            .unwrap()
            .unwrap();
        assert_eq!(sibling.guest(root).unwrap(), "/c/guest-other/file");
        let unc = decode(&record(TAG_SYMLINK, r"\??\UNC\server\share", "", 0))
            .unwrap()
            .unwrap();
        assert_eq!(unc.guest(root), Err(EOPNOTSUPP));
        for name in [r"\rooted", r"C:drive-relative", r"C:\absolute"] {
            let invalid = decode(&record(TAG_SYMLINK, name, "", 1)).unwrap().unwrap();
            assert_eq!(invalid.guest(root), Err(EIO));
        }
    }

    #[test]
    fn corrupt_reparse_offsets_lengths_flags_and_tags_are_rejected() {
        let valid = record(TAG_SYMLINK, "../target", "", 1);
        for length in 0..valid.len() {
            assert!(decode(&valid[..length]).is_err());
        }
        for offset in [8, 10, 12, 14, 19] {
            let mut bad = valid.clone();
            bad[offset] = 0xff;
            assert!(decode(&bad).is_err());
        }
        let mut bad = valid.clone();
        bad[0..4].copy_from_slice(&0xa000_ffffu32.to_le_bytes());
        assert_eq!(decode(&bad), Err(EOPNOTSUPP));
        assert!(decode(&record(TAG_SYMLINK, "bad\0target", "", 1)).is_err());
    }

    #[test]
    fn non_name_surrogate_reparse_points_are_not_links() {
        for tag in [0x9000_001au32, 0x8000_0017, 0x8000_001b] {
            let mut bytes = vec![0u8; 8];
            bytes[..4].copy_from_slice(&tag.to_le_bytes());
            assert_eq!(decode(&bytes), Ok(None));
        }
    }

    #[test]
    fn native_junction_target_survives_rename_replacement_and_posix_unlink() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
            FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx, SetFileInformationByHandle,
        };
        let f = Fixture::new();
        let target = f.0.join("target");
        let link = f.0.join("link");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"untouched").unwrap();
        let object = junction(&link, &target);
        let target_name = format!(r"\??\{}", target.to_str().unwrap());
        let expected = Target {
            path: target_name,
            relative: false,
        };
        let guest_target = expected
            .guest(&crate::path::system_root().unwrap())
            .unwrap();
        assert_eq!(
            read(object.raw()).unwrap(),
            Some(Target {
                path: expected.path.clone(),
                relative: false
            })
        );
        let original = super::super::stat_handle(object.raw(), false).unwrap();
        assert_eq!(original.st_mode, super::super::S_IFLNK | 0o777);
        assert_eq!(original.st_size, guest_target.len() as i64);
        assert_eq!(
            super::super::read_link_host_path(&link).unwrap(),
            guest_target
        );
        std::fs::rename(&link, f.0.join("renamed-link")).unwrap();
        std::fs::create_dir(&link).unwrap();
        assert_eq!(read(object.raw()).unwrap().as_ref(), Some(&expected));
        assert_eq!(
            read(Object::open(&link, FILE_READ_ATTRIBUTES).unwrap().raw()).unwrap(),
            None
        );
        let deletion = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        };
        assert_ne!(
            unsafe {
                SetFileInformationByHandle(
                    object.raw(),
                    FileDispositionInfoEx,
                    (&deletion as *const FILE_DISPOSITION_INFO_EX).cast(),
                    std::mem::size_of_val(&deletion) as u32,
                )
            },
            0
        );
        assert_eq!(read(object.raw()).unwrap().as_ref(), Some(&expected));
        let unlinked = super::super::stat_handle(object.raw(), false).unwrap();
        assert_eq!(unlinked.st_ino, original.st_ino);
        assert_eq!(unlinked.st_mode, original.st_mode);
        assert_eq!(unlinked.st_size, original.st_size);
        assert_eq!(
            super::super::readlink_handle(object.raw()).unwrap(),
            guest_target
        );
        assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"untouched");
        assert!(!f.0.join("renamed-link").exists());
    }

    #[test]
    fn native_link_copy_up_reads_the_pinned_source_and_never_follows_the_target() {
        use crate::mount::overlay::{Node, XattrNamespace, copy_up::Staged};
        let f = Fixture::new();
        for name in ["lower", "upper", "work", "目标"] {
            std::fs::create_dir(f.0.join(name)).unwrap();
        }
        let target = f.0.join("目标");
        let link = f.0.join("lower/link");
        let _link = junction(&link, &target);
        let root = Node::root([f.0.join("lower")], XattrNamespace::Trusted).unwrap();
        let source = root.child("link").unwrap();
        let expected = super::super::readlink_handle(source.backing_object().raw()).unwrap();
        let original = *source.backing_metadata();
        std::fs::rename(&link, f.0.join("lower/renamed")).unwrap();
        crate::path::create_emulated_symlink(&link, "replacement").unwrap();
        // A dangling native target must still be copied as a link record.
        std::fs::remove_dir(&target).unwrap();
        let work = Object::open(&f.0.join("work"), FILE_READ_ATTRIBUTES).unwrap();
        let upper = Object::open(&f.0.join("upper"), FILE_READ_ATTRIBUTES).unwrap();
        Staged::prepare(&source, &work, false)
            .unwrap()
            .publish(&upper, "copy")
            .unwrap();
        let copied = Object::open(&f.0.join("upper/copy"), FILE_READ_ATTRIBUTES).unwrap();
        let metadata = super::super::stat_handle(copied.raw(), false).unwrap();
        assert_ne!(metadata.st_ino, original.st_ino);
        assert_eq!(metadata.st_mode, original.st_mode);
        assert_eq!(metadata.st_size, expected.len() as i64);
        assert_eq!(
            super::super::readlink_handle(copied.raw()).unwrap(),
            expected
        );
        assert_eq!(
            super::super::symlink_target_handle(copied.raw()).unwrap(),
            Some(expected)
        );
        assert_eq!(read(copied.raw()).unwrap(), None);
        assert_eq!(std::fs::read_dir(f.0.join("work")).unwrap().count(), 0);
    }
}
