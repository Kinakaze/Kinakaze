//! Translation from the hosted Linux namespace into Windows paths.
//!
//! Hosted workers configure the namespace base explicitly. A process root may
//! change through chroot/pivot while retained objects keep namespace coordinates.
//! Unconfined native callers can address drives through `/c/tmp/file`.
//!
//! # Names Linux allows and Windows does not
//!
//! Linux permits every byte except `/` and NUL in a filename. Windows refuses
//! `< > : " | ? *`, the C0 controls, and a trailing dot or space -- the last two
//! silently, by stripping them, which is worse than an error because the name
//! then round-trips to something different.
//!
//! Rejecting those names outright is not an option: Debian's multiarch layout
//! puts a colon in ordinary filenames (`/var/lib/dpkg/info/liblz4-1:amd64.list`),
//! so dpkg cannot install a single package without them.
//!
//! So each offending character is folded into the Unicode private use area at
//! `U+F000 + code`, the convention Interix, Cygwin and WSL all settled on. The
//! mapping is total and reversible, the host filesystem sees a legal name, and
//! [`to_guest_path`] folds it back so the guest never learns it happened. The
//! cost is that a guest file whose name genuinely contains `U+F001..U+F07F`
//! would collide with an escape; those code points are private use and carry no
//! assigned meaning, so nothing real names a file with them.
//!
//! A literal backslash is still refused for the whole path rather than escaped.
//! It is the one character that could turn a guest-supplied relative path into
//! a Windows path, and guarding that is worth more than the vanishingly rare
//! Linux filename that contains one.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

/// Base of the private use area the escapes live in.
const ESCAPE_BASE: u32 = 0xF000;

/// The highest code point that is escaped, and so the highest that is restored.
const ESCAPE_MAX: u32 = 0x7F;

/// Linux stops pathname expansion after forty symbolic links.
const MAX_SYMLINKS: usize = 40;

/// Encodes a native filesystem path for Win32 path-taking APIs.
///
/// Absolute DOS and UNC paths use the extended-length namespace so every VFS
/// operation has the same 32,767-WCHAR limit instead of depending on the host
/// process's `longPathAware` manifest or the legacy `MAX_PATH` limit. Device
/// paths already select their own namespace and relative paths retain Win32's
/// normal current-directory semantics.
#[cfg(windows)]
pub fn wide_path(path: &Path) -> Result<Vec<u16>, i32> {
    const SEPARATOR: u16 = b'\\' as u16;
    const FORWARD_SLASH: u16 = b'/' as u16;
    const COLON: u16 = b':' as u16;
    const MAX_EXTENDED_PATH: usize = 32_767;

    let original: Vec<u16> = path.as_os_str().encode_wide().collect();
    if original.contains(&0) {
        return Err(crate::EINVAL);
    }
    let separator = |word: u16| word == SEPARATOR || word == FORWARD_SLASH;
    let namespaced = original.len() >= 4
        && separator(original[0])
        && separator(original[1])
        && matches!(original[2], word if word == b'?' as u16 || word == b'.' as u16)
        && separator(original[3]);

    let mut encoded = if namespaced {
        original
    } else if original.len() >= 2 && separator(original[0]) && separator(original[1]) {
        let mut encoded = r"\\?\UNC\".encode_utf16().collect::<Vec<_>>();
        encoded.extend_from_slice(&original[2..]);
        encoded
    } else if original.len() >= 3 && original[1] == COLON && separator(original[2]) {
        let mut encoded = r"\\?\".encode_utf16().collect::<Vec<_>>();
        encoded.extend_from_slice(&original);
        encoded
    } else {
        original
    };

    if encoded.starts_with(&[SEPARATOR, SEPARATOR, b'?' as u16, SEPARATOR]) {
        for word in &mut encoded[4..] {
            if *word == FORWARD_SLASH {
                *word = SEPARATOR;
            }
        }
    }
    if encoded.len() + 1 > MAX_EXTENDED_PATH {
        return Err(crate::ENAMETOOLONG);
    }
    encoded.push(0);
    Ok(encoded)
}

/// Missing inode records identify unmanaged host entries. Corrupt metadata and
/// access/I/O failures are errors, not permission guesses or ordinary files.
#[cfg(windows)]
pub fn emulated_symlink_target(path: &Path) -> Result<Option<String>, i32> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, GetFileAttributesW,
        INVALID_FILE_ATTRIBUTES,
    };
    // Hosted symlinks are regular-file placeholders, including links whose
    // targets are directories. A native directory cannot be one. Query its
    // attributes without opening an EA handle for every ancestor of every
    // stat/open; no result is cached across rename or replacement.
    let attributes = unsafe { GetFileAttributesW(wide_path(path)?.as_ptr()) };
    if attributes != INVALID_FILE_ATTRIBUTES
        && attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            == FILE_ATTRIBUTE_DIRECTORY
    {
        return Ok(None);
    }
    Ok(crate::fs::inode::read_path(path)?.symlink)
}

#[cfg(not(windows))]
pub fn emulated_symlink_target(_path: &Path) -> Result<Option<String>, i32> {
    Ok(None)
}

/// Create a privilege-free Linux symlink with inode-owned metadata. Its target
/// is independent of a Windows pathname and remains available after unlink.
#[cfg(windows)]
pub fn create_emulated_symlink(path: &Path, target: &str) -> Result<(), i32> {
    use std::os::windows::io::AsRawHandle;
    if target.is_empty() || target.contains('\0') {
        return Err(crate::EINVAL);
    }
    let placeholder = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            e.raw_os_error()
                .map_or(crate::EIO, |n| crate::errno_from_win32(n as u32))
        })?;
    let record = crate::fs::inode::Record {
        mode: Some(crate::fs::S_IFLNK | 0o777),
        device: None,
        symlink: Some(target.to_owned()),
        ..crate::fs::inode::Record::default()
    };
    let result = crate::fs::inode::replace(placeholder.as_raw_handle(), &record).and_then(|()| {
        crate::fs::initialize_created_handle(
            placeholder.as_raw_handle(),
            path,
            crate::fs::S_IFLNK | 0o777,
        )
    });
    if result.is_err() {
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
            FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx, SetFileInformationByHandle,
        };
        let object = crate::fs::object::Object::reopen(placeholder.as_raw_handle(), DELETE)?;
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        };
        if unsafe {
            SetFileInformationByHandle(
                object.raw(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                std::mem::size_of_val(&disposition) as u32,
            )
        } == 0
        {
            return Err(crate::errno_from_win32(unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
    }
    result
}

#[cfg(not(windows))]
pub fn create_emulated_symlink(_path: &Path, _target: &str) -> Result<(), i32> {
    Err(95)
}

/// True for a character Windows refuses anywhere in a name.
fn is_reserved(character: char) -> bool {
    matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (character as u32) < 0x20
}

/// True for a character Windows strips when it ends a name.
fn is_reserved_at_end(character: char) -> bool {
    character == '.' || character == ' '
}

/// Folds a single guest name into one the host filesystem will store verbatim.
///
/// Borrowing on the common path keeps this off the hot path of every resolve:
/// almost no name needs escaping, and those that do not are not copied.
pub(crate) fn escape_component(component: &str) -> Cow<'_, str> {
    let trailing = component
        .chars()
        .next_back()
        .is_some_and(is_reserved_at_end);
    if !trailing && !component.contains(is_reserved) {
        return Cow::Borrowed(component);
    }
    let mut escaped = String::with_capacity(component.len());
    let mut characters = component.chars().peekable();
    while let Some(character) = characters.next() {
        // A dot or space is legal mid-name and only has to be escaped when it
        // is the last character, so the lookahead decides rather than the class.
        let last = characters.peek().is_none();
        if is_reserved(character) || (last && is_reserved_at_end(character)) {
            // SAFETY-free: the sum is inside the private use area by
            // construction, since `character` is below `ESCAPE_MAX`.
            if let Some(mapped) = char::from_u32(ESCAPE_BASE + character as u32) {
                escaped.push(mapped);
                continue;
            }
        }
        escaped.push(character);
    }
    Cow::Owned(escaped)
}

/// Restores the guest's original name from a stored host name.
///
/// The inverse of [`escape_component`], applied to whole paths on the way out
/// so that `readdir` and `getcwd` report what the guest asked to create.
pub fn unescape_path(path: &str) -> Cow<'_, str> {
    if !path
        .chars()
        .any(|character| matches!(character as u32, code if (ESCAPE_BASE + 1..=ESCAPE_BASE + ESCAPE_MAX).contains(&code)))
    {
        return Cow::Borrowed(path);
    }
    Cow::Owned(
        path.chars()
            .map(|character| {
                let code = character as u32;
                if (ESCAPE_BASE + 1..=ESCAPE_BASE + ESCAPE_MAX).contains(&code) {
                    char::from_u32(code - ESCAPE_BASE).unwrap_or(character)
                } else {
                    character
                }
            })
            .collect::<String>(),
    )
}

#[derive(Debug)]
pub enum PathError {
    InteriorNul,
    Backslash,
    TooManySymlinks,
    MountNamespace,
    Filesystem(i32),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InteriorNul => write!(formatter, "Linux path contains an interior NUL"),
            Self::Backslash => write!(formatter, "Linux path contains a Windows path separator"),
            Self::TooManySymlinks => write!(formatter, "too many levels of symbolic links"),
            Self::MountNamespace => write!(formatter, "invalid or unavailable mount namespace"),
            Self::Filesystem(error) => write!(formatter, "filesystem error (Linux errno {error})"),
        }
    }
}

mod root;
pub(crate) use root::{
    OverlayRoot, default_system_root, namespace_root_path, overlay_root, set_overlay_root,
    to_namespace_path,
};
pub use root::{initialize_namespace_root, set_system_root, system_root};
#[cfg(windows)]
pub(crate) use root::{restore_root, serialize_root};

#[cfg(windows)]
fn translate_mount_path(path: &str) -> Result<String, PathError> {
    if path.contains('\0') {
        return Err(PathError::InteriorNul);
    }
    if path.contains('\\') {
        return Err(PathError::Backslash);
    }
    crate::mount::translate(path).map_err(|_| PathError::MountNamespace)
}

/// Resolves a Linux path using the current system root as `/`.
pub fn resolve_linux_path(linux_path: &str) -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    if let Some(resolved) =
        crate::mount::overlay::resolve(linux_path, true, false).map_err(PathError::Filesystem)?
    {
        return Ok(resolved.path);
    }
    #[cfg(windows)]
    let translated = translate_mount_path(linux_path)?;
    #[cfg(windows)]
    let linux_path = translated.as_str();
    resolve_linux_path_components(&system_root()?, linux_path, true)
}

/// Resolves every component except a symbolic link in the final component.
///
/// This is the pathname rule used by `lstat`, `readlink`, `unlink`, `rename`
/// and creation calls.  Intermediate links are still followed exactly as they
/// are by the Linux VFS.
pub fn resolve_linux_path_no_follow(linux_path: &str) -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    if let Some(resolved) =
        crate::mount::overlay::resolve(linux_path, false, false).map_err(PathError::Filesystem)?
    {
        return Ok(resolved.path);
    }
    #[cfg(windows)]
    let translated = translate_mount_path(linux_path)?;
    #[cfg(windows)]
    let linux_path = translated.as_str();
    resolve_linux_path_components(&system_root()?, linux_path, false)
}

/// Resolves a Linux path using an explicit virtual root.
///
/// Empty components and `.` are discarded. `..` cannot escape either the
/// selected drive root or `system_root`, matching the clamping behaviour of a
/// Unix root directory.
pub fn resolve_linux_path_from(system_root: &Path, linux_path: &str) -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    if let Some(resolved) =
        crate::mount::overlay::resolve(linux_path, true, false).map_err(PathError::Filesystem)?
    {
        return Ok(resolved.path);
    }
    #[cfg(windows)]
    let translated = translate_mount_path(linux_path)?;
    #[cfg(windows)]
    let linux_path = translated.as_str();
    resolve_linux_path_components(system_root, linux_path, true)
}

/// Resolves an already-bound underlying bridge path without looking it up in
/// the current mount tree again. Only stored mount backing paths may use this:
/// an ordinary guest pathname must still go through mount translation above.
pub(crate) fn resolve_bound_mount_source(
    system_root: &Path,
    backing_path: &str,
) -> Result<PathBuf, PathError> {
    resolve_linux_path_components(system_root, backing_path, true)
}

/// Explicit-root form of [`resolve_linux_path_no_follow`].
pub fn resolve_linux_path_from_no_follow(
    system_root: &Path,
    linux_path: &str,
) -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    if let Some(resolved) =
        crate::mount::overlay::resolve(linux_path, false, false).map_err(PathError::Filesystem)?
    {
        return Ok(resolved.path);
    }
    #[cfg(windows)]
    let translated = translate_mount_path(linux_path)?;
    #[cfg(windows)]
    let linux_path = translated.as_str();
    resolve_linux_path_components(system_root, linux_path, false)
}

pub(crate) fn resolve_unmounted(
    root: &Path,
    path: &str,
    follow: bool,
) -> Result<PathBuf, PathError> {
    resolve_linux_path_components(root, path, follow)
}

fn resolve_linux_path_components(
    system_root: &Path,
    linux_path: &str,
    follow_final: bool,
) -> Result<PathBuf, PathError> {
    if linux_path.contains('\0') {
        return Err(PathError::InteriorNul);
    }
    if linux_path.contains('\\') {
        return Err(PathError::Backslash);
    }

    let absolute = linux_path.starts_with('/');
    // Ordinary path components borrow from the caller. Owning every component
    // up front made each stat/open on a JVM class or shared library perform a
    // string allocation per slash; only components injected by a symlink need
    // independent storage.
    let mut pending: VecDeque<Cow<'_, str>> = linux_path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(Cow::Borrowed)
        .collect();
    let allow_drives = !crate::fs_context::read(|s| s.confined);
    let drive = if absolute && allow_drives {
        pending.front().and_then(|first| drive_letter(first))
    } else {
        None
    };
    if drive.is_some() {
        pending.pop_front();
    }
    let mut resolved = drive.map_or_else(
        || system_root.to_path_buf(),
        |letter| PathBuf::from(format!("{}:\\", letter.to_ascii_uppercase())),
    );
    // Count components appended beyond the selected virtual root instead of
    // cloning that root merely so `..` can compare against it.
    let mut depth = 0usize;
    let mut followed = 0usize;

    while let Some(component) = pending.pop_front() {
        push_component(&mut resolved, &mut depth, component.as_ref());
        if !follow_final && pending.is_empty() {
            continue;
        }
        let Some(target) = emulated_symlink_target(&resolved).map_err(PathError::Filesystem)?
        else {
            continue;
        };
        followed += 1;
        if followed > MAX_SYMLINKS {
            return Err(PathError::TooManySymlinks);
        }
        if target.contains('\0') {
            return Err(PathError::InteriorNul);
        }
        if target.contains('\\') {
            return Err(PathError::Backslash);
        }

        // A relative target is interpreted from the link's parent.  An
        // absolute target starts again at the guest root (or at an explicitly
        // selected `/c/...` drive), never at the host drive root by accident.
        resolved.pop();
        depth = depth.saturating_sub(1);
        let target_absolute = target.starts_with('/');
        let mut target_components: VecDeque<Cow<'_, str>> = target
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| Cow::Owned(part.to_owned()))
            .collect();
        if target_absolute {
            let target_drive = target_components.front().and_then(|first| {
                if allow_drives {
                    drive_letter(first)
                } else {
                    None
                }
            });
            if target_drive.is_some() {
                target_components.pop_front();
            }
            resolved = target_drive.map_or_else(
                || system_root.to_path_buf(),
                |letter| PathBuf::from(format!("{}:\\", letter.to_ascii_uppercase())),
            );
            depth = 0;
        }
        while let Some(component) = target_components.pop_back() {
            pending.push_front(component);
        }
    }
    Ok(resolved)
}

fn drive_letter(component: &str) -> Option<char> {
    let mut chars = component.chars();
    let letter = chars.next()?;
    (chars.next().is_none() && letter.is_ascii_alphabetic()).then_some(letter)
}

fn push_component(path: &mut PathBuf, depth: &mut usize, component: &str) {
    match component {
        "." => {}
        ".." => {
            if *depth != 0 {
                path.pop();
                *depth -= 1;
            }
        }
        component => {
            path.push(escape_component(component).as_ref());
            *depth += 1;
        }
    }
}

/// Converts a Windows host path into its guest Linux path equivalent.
/// If `path` is inside `system_root()`, it produces `/<relative>`.
/// Otherwise, `X:\foo\bar` becomes `/x/foo/bar`.
///
/// Private use escapes are folded back here, so a name the host had to store as
/// `liblz4-1\u{f03a}amd64.list` is reported to the guest as it was created.
pub fn to_guest_path(path: &Path) -> String {
    unescape_path(&to_host_shaped_guest_path(path)).into_owned()
}

fn strip_native_root<'a>(path: &'a Path, root: &Path) -> Option<&'a Path> {
    use std::path::{Component, Prefix};
    fn prefix(value: Prefix<'_>) -> Prefix<'_> {
        match value {
            Prefix::VerbatimDisk(drive) => Prefix::Disk(drive),
            Prefix::VerbatimUNC(server, share) => Prefix::UNC(server, share),
            other => other,
        }
    }
    // An existing root canonicalizes to a verbatim path; a not-yet-created
    // child cannot canonicalize. Compare their native prefixes equivalently.
    let mut parts = path.components();
    for expected in root.components() {
        let actual = parts.next()?;
        let equal = match (actual, expected) {
            (Component::Prefix(a), Component::Prefix(b)) => prefix(a.kind()) == prefix(b.kind()),
            _ => actual == expected,
        };
        if !equal {
            return None;
        }
    }
    Some(parts.as_path())
}

fn to_host_shaped_guest_path(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Ok(root) = system_root() {
        let canonical_root = root.canonicalize().unwrap_or(root);
        if let Some(relative) = strip_native_root(&canonical, &canonical_root) {
            let rel_str = relative.to_string_lossy().replace('\\', "/");
            let clean = rel_str.trim_matches('/');
            return if clean.is_empty() {
                "/".to_string()
            } else {
                format!("/{clean}")
            };
        }
    }
    let s = canonical.to_string_lossy();
    let s = s.trim_start_matches(r"\\?\");
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = s[2..].replace('\\', "/");
        let rest = rest.trim_matches('/');
        if rest.is_empty() {
            format!("/{drive}")
        } else {
            format!("/{drive}/{rest}")
        }
    } else {
        let clean = s.replace('\\', "/");
        if clean.starts_with('/') {
            clean
        } else {
            format!("/{clean}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_root_matches_verbatim_and_dos_prefixes_without_existing_children() {
        for (path, root) in [
            (r"C:\root\missing", r"\\?\C:\root"),
            (r"\\?\C:\root\missing", r"C:\root"),
            (r"\\server\share\root\missing", r"\\?\UNC\server\share\root"),
        ] {
            assert_eq!(
                strip_native_root(Path::new(path), Path::new(root)),
                Some(Path::new("missing"))
            );
        }
        assert!(
            strip_native_root(Path::new(r"C:\root2\missing"), Path::new(r"\\?\C:\root")).is_none()
        );
        assert!(
            strip_native_root(Path::new(r"D:\root\missing"), Path::new(r"\\?\C:\root")).is_none()
        );
    }

    #[cfg(windows)]
    fn decoded_path(encoded: &[u16]) -> String {
        String::from_utf16(&encoded[..encoded.len() - 1]).unwrap()
    }

    #[cfg(windows)]
    fn temporary_root(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kinakaze-path-{tag}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[cfg(windows)]
    #[test]
    fn win32_paths_use_the_extended_length_namespace() {
        assert_eq!(
            decoded_path(&wide_path(Path::new(r"C:\root/deep/file")).unwrap()),
            r"\\?\C:\root\deep\file"
        );
        assert_eq!(
            decoded_path(&wide_path(Path::new(r"\\server\share/deep/file")).unwrap()),
            r"\\?\UNC\server\share\deep\file"
        );
        assert_eq!(
            decoded_path(&wide_path(Path::new(r"\\?\C:\already\extended")).unwrap()),
            r"\\?\C:\already\extended"
        );
        assert_eq!(
            decoded_path(&wide_path(Path::new(r"\\.\pipe\kernel-object")).unwrap()),
            r"\\.\pipe\kernel-object"
        );
        assert_eq!(
            decoded_path(&wide_path(Path::new(r"relative\file")).unwrap()),
            r"relative\file"
        );
    }

    #[cfg(windows)]
    #[test]
    fn extended_length_path_creates_a_real_file_past_max_path() {
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };

        let root = temporary_root("long-win32-path");
        let mut parent = root.clone();
        while parent.as_os_str().encode_wide().count() < 275 {
            parent.push("0123456789abcdef0123456789abcdef");
        }
        std::fs::create_dir_all(&parent).unwrap();
        let file = parent.join("created-by-createfilew");
        assert!(file.as_os_str().encode_wide().count() > 260);
        let encoded = wide_path(&file).unwrap();
        let handle = unsafe {
            CreateFileW(
                encoded.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        assert!(!handle.is_null() && handle != INVALID_HANDLE_VALUE);
        unsafe { CloseHandle(handle) };
        assert!(file.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn guest_root_is_the_executable_directory() {
        let root = Path::new(r"C:\host\app");
        assert_eq!(
            resolve_linux_path_from(root, "/etc/hosts").unwrap(),
            root.join("etc").join("hosts")
        );
        assert_eq!(resolve_linux_path_from(root, "/").unwrap(), root);
        assert_eq!(
            resolve_linux_path_from(root, "var/log").unwrap(),
            root.join("var").join("log")
        );
    }

    #[test]
    fn one_letter_root_component_selects_a_drive() {
        let root = Path::new(r"E:\host");
        assert_eq!(
            resolve_linux_path_from(root, "/c/Windows/System32").unwrap(),
            PathBuf::from(r"C:\Windows\System32")
        );
        assert_eq!(
            resolve_linux_path_from(root, "/d/data").unwrap(),
            PathBuf::from(r"D:\data")
        );
    }

    #[test]
    fn parent_components_are_clamped_at_the_virtual_root() {
        let root = Path::new(r"E:\host\app");
        assert_eq!(
            resolve_linux_path_from(root, "/../../safe").unwrap(),
            root.join("safe")
        );
        assert_eq!(
            resolve_linux_path_from(root, "/c/../../safe").unwrap(),
            PathBuf::from(r"C:\safe")
        );
    }

    #[test]
    fn windows_escape_syntax_is_rejected() {
        let root = Path::new(r"E:\host");
        assert!(matches!(
            resolve_linux_path_from(root, r"/tmp\escape"),
            Err(PathError::Backslash)
        ));
    }

    /// Debian's multiarch layout names files `<package>:<arch>.list`, so a colon
    /// has to survive the round trip or dpkg cannot install anything.
    #[test]
    fn reserved_characters_are_escaped_into_the_private_use_area() {
        let root = Path::new(r"E:\host");
        let resolved =
            resolve_linux_path_from(root, "/var/lib/dpkg/info/liblz4-1:amd64.list").unwrap();
        assert_eq!(
            resolved,
            root.join("var")
                .join("lib")
                .join("dpkg")
                .join("info")
                .join("liblz4-1\u{f03a}amd64.list")
        );
        // The stored name is legal on NTFS: no bare colon survives.
        assert!(
            !resolved
                .to_string_lossy()
                .trim_start_matches("E:")
                .contains(':')
        );
    }

    #[test]
    fn every_reserved_character_round_trips() {
        for reserved in ['<', '>', ':', '"', '|', '?', '*'] {
            let name = format!("a{reserved}b");
            let escaped = escape_component(&name);
            assert_ne!(escaped.as_ref(), name, "{reserved:?} should be escaped");
            assert_eq!(unescape_path(escaped.as_ref()), name);
        }
    }

    /// Windows silently strips a trailing dot or space, which would make the
    /// name round-trip to something else -- worse than refusing it.
    #[test]
    fn a_trailing_dot_or_space_is_escaped_but_an_interior_one_is_not() {
        assert_eq!(escape_component("file."), "file\u{f02e}");
        assert_eq!(escape_component("file "), "file\u{f020}");
        assert_eq!(escape_component("a.b.c"), "a.b.c");
        assert_eq!(escape_component("a b c"), "a b c");
        assert_eq!(unescape_path("file\u{f02e}"), "file.");
    }

    #[test]
    fn ordinary_names_are_not_copied() {
        assert!(matches!(
            escape_component("liblz4-1.so.1"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            unescape_path("/usr/lib/liblz4-1.so.1"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    /// `.` and `..` keep their navigation meaning and are never escaped, even
    /// though `.` ends with a reserved-at-end character. The components are
    /// multi-letter because a single-letter leading component selects a drive.
    #[test]
    fn dot_components_still_navigate() {
        let root = Path::new(r"E:\host");
        assert_eq!(
            resolve_linux_path_from(root, "/aa/./bb/../cc").unwrap(),
            root.join("aa").join("cc")
        );
    }

    #[test]
    fn system_root_can_be_customized() {
        let state = serialize_root().unwrap();
        let original = system_root().unwrap();
        let custom = std::env::temp_dir().join("kinakaze-custom-root-test");
        set_system_root(custom.clone());
        assert_eq!(system_root().unwrap(), custom);
        assert_eq!(
            resolve_linux_path("/etc/passwd").unwrap(),
            custom.join("etc").join("passwd")
        );
        // Restore original
        assert!(restore_root(&state));
        assert_eq!(system_root().unwrap(), original);
    }

    #[cfg(windows)]
    #[test]
    fn hosted_symlinks_preserve_relative_targets_and_final_no_follow() {
        let root = temporary_root("relative-link");
        let bin = root.join("usr").join("bin");
        let jdk_bin = root
            .join("usr")
            .join("lib")
            .join("jvm")
            .join("jdk")
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&jdk_bin).unwrap();
        let target = jdk_bin.join("java");
        std::fs::write(&target, b"launcher").unwrap();
        let link = bin.join("java");
        create_emulated_symlink(&link, "../lib/jvm/jdk/bin/java").unwrap();

        assert_eq!(
            resolve_linux_path_from(&root, "/usr/bin/java").unwrap(),
            target
        );
        assert_eq!(
            resolve_linux_path_from_no_follow(&root, "/usr/bin/java").unwrap(),
            link
        );
        assert_eq!(
            emulated_symlink_target(&link).unwrap().as_deref(),
            Some("../lib/jvm/jdk/bin/java")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn hosted_symlinks_follow_absolute_targets_and_intermediate_directories() {
        let root = temporary_root("absolute-link");
        let real = root.join("real");
        let aliases = root.join("aliases");
        std::fs::create_dir_all(real.join("bin")).unwrap();
        std::fs::create_dir_all(&aliases).unwrap();
        std::fs::write(real.join("bin").join("tool"), b"tool").unwrap();
        create_emulated_symlink(&aliases.join("current"), "/real").unwrap();

        assert_eq!(
            resolve_linux_path_from(&root, "/aliases/current/bin/tool").unwrap(),
            real.join("bin").join("tool")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn hosted_symlink_loops_report_eloop_after_linux_limit() {
        let root = temporary_root("link-loop");
        std::fs::create_dir_all(&root).unwrap();
        create_emulated_symlink(&root.join("loop"), "loop").unwrap();
        assert!(matches!(
            resolve_linux_path_from(&root, "/loop"),
            Err(PathError::TooManySymlinks)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
