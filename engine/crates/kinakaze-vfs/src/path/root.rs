//! Immutable namespace base, mutable process root and versioned fork/exec state.
use super::{PathError, unescape_path};
use std::path::{Path, PathBuf};
#[cfg(all(test, windows))]
mod tests;

static DEFAULT_SYSTEM_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
thread_local! {
    static OPERATION_BASE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct OverlayRoot {
    pub namespace_path: String,
    pub source: String,
    pub relative: String,
    pub native_base: PathBuf,
}
pub(crate) fn overlay_root() -> Option<OverlayRoot> {
    crate::fs_context::read(|s| s.overlay.clone())
}

pub(crate) fn default_system_root() -> std::borrow::Cow<'static, Path> {
    if let Some(base) = OPERATION_BASE.with(|slot| slot.borrow().clone()) {
        return std::borrow::Cow::Owned(base);
    }
    std::borrow::Cow::Borrowed(
        DEFAULT_SYSTEM_ROOT
            .get_or_init(|| {
                // Standalone native callers retain their executable-based root.
                // Hosted workers must initialize an explicit base before reaching
                // this API; environment variables never choose a namespace base.
                std::env::current_exe()
                    .expect("cannot locate standalone VFS executable")
                    .parent()
                    .expect("standalone VFS executable has no parent")
                    .to_path_buf()
            })
            .as_path(),
    )
}

/// Set the immutable namespace base before starting guest pathname operations.
/// chroot only changes fs_context; fork/exec restore this same original base.
pub fn initialize_namespace_root(root: PathBuf) -> Result<(), i32> {
    if !root.is_absolute() {
        return Err(crate::EINVAL);
    }
    if DEFAULT_SYSTEM_ROOT.get() == Some(&root) {
        return Ok(());
    }
    let root = root.canonicalize().map_err(|_| crate::ENOENT)?;
    match DEFAULT_SYSTEM_ROOT.set(root) {
        Ok(()) => Ok(()),
        Err(root) if DEFAULT_SYSTEM_ROOT.get() == Some(&root) => Ok(()),
        Err(_) => Err(crate::EINVAL),
    }
}

/// Returns the directory exposed to the guest as `/`.
pub fn system_root() -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    if let Some(object) = crate::fs_context::read(|s| s.root_object.clone()) {
        return object.path().map_err(PathError::Filesystem);
    }
    Ok(crate::fs_context::read(|s| s.root.clone())
        .unwrap_or_else(|| default_system_root().to_path_buf()))
}

/// Namespace-relative location exposed to the caller as `/`.
pub(crate) fn namespace_root_path() -> Result<Option<String>, i32> {
    #[cfg(windows)]
    let namespace = crate::mount::namespace_root_path()?;
    #[cfg(not(windows))]
    let namespace: Option<String> = None;
    let contextual = if let Some(root) = overlay_root() {
        Some(root.namespace_path)
    } else if let Some(root) =
        crate::fs_context::read(|s| if s.confined { s.root.clone() } else { None })
    {
        match root.strip_prefix(default_system_root()) {
            Ok(relative) => {
                let text =
                    unescape_path(&relative.to_string_lossy().replace('\\', "/")).into_owned();
                Some(format!("/{}", text.trim_matches('/')))
            }
            Err(_) if namespace.is_some() => return Err(crate::EIO),
            Err(_) => None,
        }
    } else {
        None
    };
    if let (Some(root), Some(contextual)) = (&namespace, &contextual)
        && contextual != root
        && !contextual
            .strip_prefix(root)
            .is_some_and(|tail| root == "/" || tail.starts_with('/'))
    {
        return Err(crate::EIO);
    }
    Ok(contextual.or(namespace))
}

/// Native handles retain their position in the original namespace after chroot.
pub(crate) fn to_namespace_path(path: &Path) -> Option<String> {
    let root = default_system_root().canonicalize().ok()?;
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let relative = path.strip_prefix(root).ok()?;
    Some(format!(
        "/{}",
        unescape_path(&relative.to_string_lossy().replace('\\', "/")).trim_matches('/')
    ))
}

/// Sets the directory exposed to the guest as `/`.
pub fn set_system_root(new_root: PathBuf) {
    set_root(new_root, None);
}
pub(crate) fn set_overlay_root(new_root: PathBuf, overlay: OverlayRoot) {
    set_root(new_root, Some(overlay));
}
fn set_root(new_root: PathBuf, overlay: Option<OverlayRoot>) {
    crate::fs_context::update(|s| {
        s.root = Some(new_root);
        #[cfg(windows)]
        {
            s.root_object = None;
        }
        s.overlay = overlay;
        s.confined = true;
    });
}

/// Root state is restored before mounts, cwd and descriptor attachments. The
/// immutable namespace base is distinct from a chrooted process's visible root.
#[cfg(windows)]
const STATE_MAGIC: &[u8; 8] = b"CYPATH02";

#[cfg(windows)]
fn write_path(bytes: &mut Vec<u8>, path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let length = path.as_os_str().encode_wide().count();
    crate::state_codec::word(bytes, length as u64);
    for word in path.as_os_str().encode_wide() {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
}

#[cfg(windows)]
fn read_path(reader: &mut crate::state_codec::Reader<'_>) -> Result<PathBuf, i32> {
    use std::os::windows::ffi::OsStringExt;
    let length = usize::try_from(reader.word()?).map_err(|_| crate::EIO)?;
    let size = length.checked_mul(2).ok_or(crate::EIO)?;
    let bytes = reader.0.get(..size).ok_or(crate::EIO)?;
    reader.0 = &reader.0[size..];
    let words: Vec<_> = bytes
        .chunks_exact(2)
        .map(|w| u16::from_le_bytes([w[0], w[1]]))
        .collect();
    if words.contains(&0) {
        return Err(crate::EINVAL);
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&words));
    if !path.is_absolute() {
        return Err(crate::EINVAL);
    }
    Ok(path)
}

#[cfg(windows)]
pub(crate) fn serialize_root() -> Result<Vec<u8>, i32> {
    let context = crate::fs_context::read(Clone::clone);
    let mut bytes = STATE_MAGIC.to_vec();
    let base = default_system_root();
    write_path(&mut bytes, &base);
    write_path(&mut bytes, context.root.as_deref().unwrap_or(&base));
    crate::state_codec::word(&mut bytes, u64::from(context.confined));
    crate::state_codec::word(&mut bytes, u64::from(context.overlay.is_some()));
    if let Some(overlay) = context.overlay {
        for value in [&overlay.namespace_path, &overlay.source, &overlay.relative] {
            crate::state_codec::bytes(&mut bytes, value.as_bytes());
        }
        write_path(&mut bytes, &overlay.native_base);
    }
    Ok(bytes)
}

#[cfg(windows)]
pub(crate) fn decode_root(bytes: &[u8]) -> Result<(PathBuf, crate::fs_context::State), i32> {
    let mut reader = crate::state_codec::Reader(bytes.strip_prefix(STATE_MAGIC).ok_or(crate::EIO)?);
    let base = read_path(&mut reader)?;
    let root = read_path(&mut reader)?;
    let confined = match reader.word()? {
        0 => false,
        1 => true,
        _ => return Err(crate::EINVAL),
    };
    let overlay = match reader.word()? {
        0 => None,
        1 => {
            let namespace_path = reader.text()?;
            let source = reader.text()?;
            let relative = reader.text()?;
            if !namespace_path.starts_with('/')
                || [&namespace_path, &source, &relative]
                    .iter()
                    .any(|s| s.contains('\0'))
            {
                return Err(crate::EINVAL);
            }
            Some(OverlayRoot {
                namespace_path,
                source,
                relative,
                native_base: read_path(&mut reader)?,
            })
        }
        _ => return Err(crate::EINVAL),
    };
    reader.end()?;
    Ok((
        base,
        crate::fs_context::State {
            root: Some(root),
            overlay,
            confined,
            ..Default::default()
        },
    ))
}

#[cfg(windows)]
pub(crate) fn restore_root(bytes: &[u8]) -> bool {
    fn restore(bytes: &[u8]) -> Result<(), i32> {
        let (base, restored) = decode_root(bytes)?;
        // Validate the complete record before publishing any of its state.
        initialize_namespace_root(base)?;
        crate::fs_context::update(|state| {
            state.root = restored.root;
            state.root_object = None;
            state.overlay = restored.overlay;
            state.confined = restored.confined;
        });
        Ok(())
    }
    restore(bytes).is_ok()
}
