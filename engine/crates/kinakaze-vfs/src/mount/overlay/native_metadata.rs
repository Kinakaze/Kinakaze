//! One operation's native metadata capability, selected while resolving links.
//! The open inode and mount writer live through the caller's mutation. No path
//! observation or mutable inode metadata survives the operation.
use super::native_open;
use crate::fs::object::Object;
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_READ_ATTRIBUTES, FILE_READ_EA, FileAttributeTagInfo, GetFileInformationByHandleEx,
};

pub struct NativeMetadata {
    object: Object,
    _writer: Option<Arc<Object>>,
}

impl NativeMetadata {
    pub(crate) fn object(&self) -> &Object {
        &self.object
    }
}

impl AsRawHandle for NativeMetadata {
    fn as_raw_handle(&self) -> RawHandle {
        self.object.raw()
    }
}

/// Only handles a native namespace path without mount or link crossings. An
/// unsuccessful speculative open makes no changes and leaves error selection,
/// copy-up and all other path rules to the ordinary resolver.
pub fn prepare(path: &str, follow: bool, access: u32) -> Result<Option<NativeMetadata>, i32> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| {
        std::env::var_os("KINAKAZE_NATIVE_METADATA").as_deref() != Some(std::ffi::OsStr::new("0"))
    }) || path.is_empty()
        || path.contains('\0')
        || crate::path::overlay_root().is_some()
        || crate::mount::api::tree_reference(path).is_some()
        || crate::tmpfs::owns(path)
        || crate::procfs::owns(path)
    {
        return Ok(None);
    }
    let Some(namespace) = crate::path::namespace_root_path()? else {
        return Ok(None);
    };
    let absolute = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{path}", crate::fs::getcwd())
    };
    let Some(object) = open(
        &crate::path::default_system_root(),
        &namespace,
        &absolute,
        follow,
        access,
    )?
    else {
        return Ok(None);
    };
    // This path has no dot components. Collapse repeated separators before
    // selecting its already-resolved mount policy, just as the native walk does.
    let canonical = format!(
        "/{}",
        absolute
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    let writer = crate::mount::native::write_canonical(&canonical)?;
    Ok(Some(NativeMetadata {
        object,
        _writer: writer,
    }))
}

fn open(
    root: &Path,
    namespace: &str,
    absolute: &str,
    follow: bool,
    access: u32,
) -> Result<Option<Object>, i32> {
    // Hosted links are regular-file placeholders: a successful native open
    // cannot have traversed one in an ancestor. Dot and trailing-slash rules
    // still need the component walker and its shared link limit.
    if !absolute.starts_with('/')
        || absolute.ends_with('/')
        || absolute.split('/').any(|part| part == "." || part == "..")
    {
        return Ok(None);
    }
    let mut guest = namespace.trim_end_matches('/').to_owned();
    let mut native = root.to_path_buf();
    for part in namespace.split('/').filter(|part| !part.is_empty()) {
        native.push(crate::path::escape_component(part).as_ref());
    }
    for part in absolute.split('/').filter(|part| !part.is_empty()) {
        guest.push('/');
        guest.push_str(part);
        native.push(crate::path::escape_component(part).as_ref());
    }
    if crate::mount::has_attachment(&guest)? {
        return Ok(None);
    }
    let object = match native_open::open(&native, access | FILE_READ_ATTRIBUTES | FILE_READ_EA) {
        Ok(object) => object,
        Err(_) => return Ok(None),
    };
    let mut attributes: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        trace_native!(
            "native.GetFileInformationByHandleEx",
            GetFileInformationByHandleEx(
                object.raw(),
                FileAttributeTagInfo,
                (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of_val(&attributes) as u32,
            )
        )
    } == 0
        || attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Ok(None);
    }
    if follow
        && attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        && crate::fs::inode::read_object(&object)?.symlink.is_some()
    {
        return Ok(None);
    }
    Ok(Some(object))
}

#[cfg(test)]
mod tests;
