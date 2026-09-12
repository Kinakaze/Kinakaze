//! inotify: Linux file-monitoring ABI on top of Windows `ReadDirectoryChangesW`.
//!
//! # Design
//!
//! `inotify_init1`    -> allocates a synthetic FD of kind `Inotify`.
//! `inotify_add_watch(fd, path, mask)` -> starts `ReadDirectoryChangesW` on the
//!                      directory in a background thread and returns a watch
//!                      descriptor (wd).
//! `inotify_rm_watch(fd, wd)` -> signals the background thread to stop.
//! `read(inotify_fd, buf, len)` -> drains the pending `inotify_event` queue.
//!
//! # inotify_event wire format (Linux x86_64 ABI)
//!
//! ```text
//! struct inotify_event {
//!     int32_t  wd;      // watch descriptor
//!     uint32_t mask;    // event type bits
//!     uint32_t cookie;  // rename-pair cookie (0 if not a rename)
//!     uint32_t len;     // byte count of name[], 0-padded to 4-byte alignment
//!     char     name[];  // filename within the watched directory (optional)
//! };
//! ```

use std::collections::{HashMap, VecDeque};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};

// ---------------------------------------------------------------------------
// inotify event mask bits (Linux ABI).
// ---------------------------------------------------------------------------

pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_ALL_EVENTS: u32 = 0x0000_0fff;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;

pub const IN_CLOEXEC: i32 = 0o2000000;
pub const IN_NONBLOCK: i32 = 0o0004000;

// Windows FILE_NOTIFY_INFORMATION action codes.
const FILE_ACTION_ADDED: u32 = 1;
const FILE_ACTION_REMOVED: u32 = 2;
const FILE_ACTION_MODIFIED: u32 = 3;
const FILE_ACTION_RENAMED_OLD_NAME: u32 = 4;
const FILE_ACTION_RENAMED_NEW_NAME: u32 = 5;

// ---------------------------------------------------------------------------
// Internal state.
// ---------------------------------------------------------------------------

struct Watch {
    path: PathBuf,
    mask: Arc<AtomicU32>,
    cancel_event: HANDLE,
    thread: Option<JoinHandle<()>>,
}

// SAFETY: HANDLE is just an integer; only mutated under the mutex.
unsafe impl Send for Watch {}
unsafe impl Sync for Watch {}

struct InotifyState {
    next_wd: i32,
    watches: HashMap<i32, Watch>,
    watches_by_path: HashMap<PathBuf, i32>,
    queue: VecDeque<Vec<u8>>,
    nonblock: bool,
}

struct InotifyFd {
    state: Mutex<InotifyState>,
    readable: Condvar,
}

static INOTIFY_MAP: OnceLock<Mutex<HashMap<i32, Arc<InotifyFd>>>> = OnceLock::new();

fn inotify_map() -> &'static Mutex<HashMap<i32, Arc<InotifyFd>>> {
    INOTIFY_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

pub fn create_inotify(flags: i32) -> Result<i32, i32> {
    if flags & !(IN_CLOEXEC | IN_NONBLOCK) != 0 {
        return Err(crate::EINVAL);
    }
    let nonblock = flags & IN_NONBLOCK != 0;
    let mut fd_flags = crate::FdFlags::NONE;
    if flags & IN_CLOEXEC != 0 {
        fd_flags.0 |= crate::FdFlags::CLOSE_ON_EXEC.0;
    }
    if nonblock {
        fd_flags.0 |= crate::FdFlags::NONBLOCK.0;
    }
    let fd = crate::install_handleless(crate::FdKind::Inotify, fd_flags)?;
    let ifd = Arc::new(InotifyFd {
        state: Mutex::new(InotifyState {
            next_wd: 1,
            watches: HashMap::new(),
            watches_by_path: HashMap::new(),
            queue: VecDeque::new(),
            nonblock,
        }),
        readable: Condvar::new(),
    });
    if let Ok(mut map) = inotify_map().lock() {
        map.insert(fd, ifd);
    }
    Ok(fd)
}

/// Resolves a guest pathname with the same cwd/root/symlink rules as the rest
/// of the VFS, then installs a watch on the resolved object.
pub fn add_watch_linux(fd: i32, path: &str, mask: u32) -> Result<i32, i32> {
    let resolved = crate::fs::resolve(path)?;
    add_watch(fd, resolved, mask)
}

pub fn add_watch(fd: i32, path: PathBuf, mask: u32) -> Result<i32, i32> {
    if mask & IN_ALL_EVENTS == 0 {
        return Err(crate::EINVAL);
    }
    if mask & IN_MASK_ADD != 0 && mask & IN_MASK_CREATE != 0 {
        return Err(crate::EINVAL);
    }
    let ifd = {
        let map = inotify_map().lock().map_err(|_| crate::EIO)?;
        map.get(&fd).cloned().ok_or(crate::EBADF)?
    };
    let mut state = ifd.state.lock().map_err(|_| crate::EIO)?;
    if let Some(&existing_wd) = state.watches_by_path.get(&path) {
        if mask & IN_MASK_CREATE != 0 {
            return Err(crate::EEXIST);
        }
        let watch = state.watches.get(&existing_wd).ok_or(crate::EIO)?;
        let event_mask = mask & IN_ALL_EVENTS;
        if mask & IN_MASK_ADD != 0 {
            watch.mask.fetch_or(event_mask, Ordering::Release);
        } else {
            watch.mask.store(event_mask, Ordering::Release);
        }
        return Ok(existing_wd);
    }

    let metadata = std::fs::metadata(&path).map_err(errno_from_io)?;
    if mask & IN_ONLYDIR != 0 && !metadata.is_dir() {
        return Err(crate::ENOTDIR);
    }
    let (watch_root, filter_name) = if metadata.is_dir() {
        (path.clone(), None)
    } else {
        let parent = path.parent().ok_or(crate::ENOENT)?.to_path_buf();
        let name = path
            .file_name()
            .ok_or(crate::ENOENT)?
            .encode_wide()
            .collect::<Vec<_>>();
        (parent, Some(name))
    };

    let dir_handle = open_watch_root(&watch_root)?;
    let wd = state.next_wd;
    state.next_wd += 1;

    // SAFETY: CreateEventW params are all valid (null security/name, manual-reset=1, initial=0).
    let cancel_event = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
    if cancel_event.is_null() || cancel_event == INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError observes the failed CreateEventW immediately.
        let error = unsafe { GetLastError() };
        // SAFETY: the directory handle was opened successfully and has not
        // been transferred to a worker thread.
        unsafe { CloseHandle(dir_handle) };
        return Err(crate::errno_from_win32(error));
    }

    let ifd_clone = Arc::clone(&ifd);
    let effective_mask = Arc::new(AtomicU32::new(mask & IN_ALL_EVENTS));
    let thread = match spawn_watch_thread(
        wd,
        dir_handle,
        filter_name,
        Arc::clone(&effective_mask),
        ifd_clone,
        cancel_event,
    ) {
        Ok(thread) => thread,
        Err(error) => {
            // SAFETY: neither handle was transferred when thread creation
            // failed.
            unsafe {
                CloseHandle(cancel_event);
                CloseHandle(dir_handle);
            }
            return Err(error);
        }
    };

    state.watches.insert(
        wd,
        Watch {
            path: path.clone(),
            mask: effective_mask,
            cancel_event,
            thread: Some(thread),
        },
    );
    state.watches_by_path.insert(path, wd);
    Ok(wd)
}

pub fn rm_watch(fd: i32, wd: i32) -> Result<(), i32> {
    let ifd = {
        let map = inotify_map().lock().map_err(|_| crate::EIO)?;
        map.get(&fd).cloned().ok_or(crate::EBADF)?
    };
    let mut state = ifd.state.lock().map_err(|_| crate::EIO)?;
    let Some(mut watch) = state.watches.remove(&wd) else {
        return Err(crate::EINVAL);
    };
    state.watches_by_path.remove(&watch.path);
    // SAFETY: cancel_event is valid and owned by this watch.
    unsafe { SetEvent(watch.cancel_event) };
    drop(state);
    if let Some(thread) = watch.thread.take() {
        let _ = thread.join();
    }
    // SAFETY: thread no longer running; safe to close.
    unsafe { CloseHandle(watch.cancel_event) };
    let mut state = ifd.state.lock().map_err(|_| crate::EIO)?;
    let event = encode_event(wd, IN_IGNORED, 0, &[]);
    state.queue.push_back(event);
    ifd.readable.notify_all();
    Ok(())
}

pub fn read_inotify(fd: i32, buf: &mut [u8], nonblock: bool) -> Result<usize, i32> {
    let ifd = {
        let map = inotify_map().lock().map_err(|_| crate::EIO)?;
        map.get(&fd).cloned().ok_or(crate::EBADF)?
    };
    let mut state = ifd.state.lock().map_err(|_| crate::EIO)?;
    loop {
        if !state.queue.is_empty() {
            let mut written = 0usize;
            while let Some(event) = state.queue.front() {
                if written + event.len() > buf.len() {
                    if written == 0 {
                        return Err(crate::EINVAL);
                    }
                    break;
                }
                let event = state.queue.pop_front().unwrap();
                buf[written..written + event.len()].copy_from_slice(&event);
                written += event.len();
            }
            return Ok(written);
        }
        if nonblock || state.nonblock {
            return Err(crate::EAGAIN);
        }
        state = ifd.readable.wait(state).map_err(|_| crate::EIO)?;
    }
}

pub fn poll_inotify(fd: i32) -> Result<bool, i32> {
    let ifd = {
        let map = inotify_map().lock().map_err(|_| crate::EIO)?;
        map.get(&fd).cloned().ok_or(crate::EBADF)?
    };
    let state = ifd.state.lock().map_err(|_| crate::EIO)?;
    Ok(!state.queue.is_empty())
}

pub fn close_inotify(fd: i32) {
    let ifd = {
        let Ok(mut map) = inotify_map().lock() else {
            return;
        };
        map.remove(&fd)
    };
    let Some(ifd) = ifd else { return };
    let Ok(mut state) = ifd.state.lock() else {
        return;
    };
    let watches: Vec<i32> = state.watches.keys().copied().collect();
    for wd in watches {
        if let Some(mut watch) = state.watches.remove(&wd) {
            // SAFETY: cancel_event is valid and owned.
            unsafe { SetEvent(watch.cancel_event) };
            drop(state);
            if let Some(thread) = watch.thread.take() {
                let _ = thread.join();
            }
            // SAFETY: thread exited.
            unsafe { CloseHandle(watch.cancel_event) };
            state = ifd.state.lock().unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn encode_event(wd: i32, mask: u32, cookie: u32, name: &[u8]) -> Vec<u8> {
    let name_len = if name.is_empty() {
        0u32
    } else {
        let with_nul = name.len() + 1;
        ((with_nul + 3) & !3) as u32
    };
    let total = 16 + name_len as usize;
    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&wd.to_ne_bytes());
    buf[4..8].copy_from_slice(&mask.to_ne_bytes());
    buf[8..12].copy_from_slice(&cookie.to_ne_bytes());
    buf[12..16].copy_from_slice(&name_len.to_ne_bytes());
    if !name.is_empty() {
        let copy_len = name.len().min(name_len as usize - 1);
        buf[16..16 + copy_len].copy_from_slice(&name[..copy_len]);
    }
    buf
}

fn action_to_mask(action: u32, watch_mask: u32) -> u32 {
    let candidate = match action {
        FILE_ACTION_ADDED => IN_CREATE,
        FILE_ACTION_REMOVED => IN_DELETE,
        FILE_ACTION_MODIFIED => IN_MODIFY | IN_ATTRIB,
        FILE_ACTION_RENAMED_OLD_NAME => IN_MOVED_FROM,
        FILE_ACTION_RENAMED_NEW_NAME => IN_MOVED_TO,
        _ => 0,
    };
    candidate & watch_mask
}

fn wide_to_utf8(wide: &[u8]) -> Vec<u8> {
    if wide.len() < 2 {
        return Vec::new();
    }
    let words: Vec<u16> = wide
        .chunks_exact(2)
        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&words).into_bytes()
}

fn errno_from_io(error: std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map(|code| crate::errno_from_win32(code as u32))
        .unwrap_or(crate::EIO)
}

fn open_watch_root(path: &PathBuf) -> Result<HANDLE, i32> {
    let wide = crate::path::wide_path(path)?;
    // Open synchronously so inotify_add_watch never reports success for an
    // object that Windows refused to monitor.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            core::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        Err(crate::errno_from_win32(unsafe { GetLastError() }))
    } else {
        Ok(handle)
    }
}

fn spawn_watch_thread(
    wd: i32,
    dir_handle: HANDLE,
    filter_name: Option<Vec<u16>>,
    mask: Arc<AtomicU32>,
    ifd: Arc<InotifyFd>,
    cancel_event: HANDLE,
) -> Result<JoinHandle<()>, i32> {
    let dir_handle = dir_handle as usize;
    let cancel_event = cancel_event as usize;
    std::thread::Builder::new()
        .name(format!("kinakaze-inotify-wd{wd}"))
        .spawn(move || {
            run_watch(
                wd,
                dir_handle as HANDLE,
                filter_name,
                mask,
                ifd,
                cancel_event as HANDLE,
            )
        })
        .map_err(errno_from_io)
}

fn run_watch(
    wd: i32,
    dir_handle: HANDLE,
    filter_name: Option<Vec<u16>>,
    mask: Arc<AtomicU32>,
    ifd: Arc<InotifyFd>,
    cancel_event: HANDLE,
) {
    let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION;

    let mut buf = vec![0u8; 65536];
    let mut rename_cookie: u32 = 0;

    loop {
        let io_event = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
        if io_event.is_null() || io_event == INVALID_HANDLE_VALUE {
            break;
        }

        let mut overlapped: OVERLAPPED = unsafe { core::mem::zeroed() };
        overlapped.hEvent = io_event;
        let mut bytes_returned: u32 = 0;

        unsafe {
            ReadDirectoryChangesW(
                dir_handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                0,
                notify_filter,
                &mut bytes_returned,
                &mut overlapped,
                None,
            )
        };

        let wait_handles = [io_event, cancel_event];
        let wr = unsafe { WaitForMultipleObjects(2, wait_handles.as_ptr(), 0, INFINITE) };

        if wr != WAIT_OBJECT_0 {
            unsafe { CloseHandle(io_event) };
            break;
        }

        let mut transferred: u32 = 0;
        let success = unsafe { GetOverlappedResult(dir_handle, &overlapped, &mut transferred, 0) };
        unsafe { CloseHandle(io_event) };

        if success == 0 || transferred == 0 {
            if mask.load(Ordering::Acquire) & (IN_DELETE_SELF | IN_MOVE_SELF) != 0 {
                let event = encode_event(wd, IN_DELETE_SELF | IN_ISDIR, 0, &[]);
                push_event(&ifd, event);
            }
            break;
        }

        let mut offset = 0usize;
        loop {
            if offset + 12 > transferred as usize {
                break;
            }
            let action = u32::from_ne_bytes(buf[offset..offset + 4].try_into().unwrap());
            let next_offset = u32::from_ne_bytes(buf[offset + 4..offset + 8].try_into().unwrap());
            let name_len_bytes =
                u32::from_ne_bytes(buf[offset + 8..offset + 12].try_into().unwrap()) as usize;

            let name_start = offset + 12;
            let name_end = name_start + name_len_bytes;
            let name_bytes = if name_end <= buf.len() {
                &buf[name_start..name_end]
            } else {
                &[]
            };
            let name_words = name_bytes
                .chunks_exact(2)
                .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                .collect::<Vec<_>>();
            if filter_name
                .as_ref()
                .is_some_and(|filter| filter != &name_words)
            {
                if next_offset == 0 {
                    break;
                }
                offset += next_offset as usize;
                continue;
            }
            let name_utf8 = wide_to_utf8(name_bytes);

            let event_mask = action_to_mask(action, mask.load(Ordering::Acquire));
            if event_mask != 0 {
                if action == FILE_ACTION_RENAMED_OLD_NAME {
                    rename_cookie = rename_cookie.wrapping_add(1);
                    if rename_cookie == 0 {
                        rename_cookie = 1;
                    }
                }
                let cookie = if matches!(
                    action,
                    FILE_ACTION_RENAMED_OLD_NAME | FILE_ACTION_RENAMED_NEW_NAME
                ) {
                    rename_cookie
                } else {
                    0
                };
                let event = encode_event(wd, event_mask, cookie, &name_utf8);
                push_event(&ifd, event);
            }

            if next_offset == 0 {
                break;
            }
            offset += next_offset as usize;
        }
    }

    unsafe { CloseHandle(dir_handle) };
}

fn push_event(ifd: &Arc<InotifyFd>, event: Vec<u8>) {
    if let Ok(mut state) = ifd.state.lock() {
        state.queue.push_back(event);
        ifd.readable.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inotify_init1_flags_and_descriptor_allocation() {
        let fd = create_inotify(IN_CLOEXEC | IN_NONBLOCK).expect("create_inotify");
        assert!(fd >= 0);

        let mut buf = [0u8; 64];
        // In nonblocking mode with empty queue, read returns EAGAIN
        assert_eq!(read_inotify(fd, &mut buf, true), Err(crate::EAGAIN));

        crate::close(fd).expect("close");
    }

    #[test]
    fn inotify_init1_rejects_unknown_flags() {
        assert_eq!(create_inotify(1), Err(crate::EINVAL));
    }

    #[test]
    fn inotify_encode_event_wire_format() {
        let encoded = encode_event(1, IN_CREATE, 0, b"test.txt");
        // inotify_event header is 16 bytes. "test.txt" is 8 bytes + 1 nul = 9 -> aligned to 12 bytes.
        // total size = 28 bytes.
        assert_eq!(encoded.len(), 28);

        let wd = i32::from_ne_bytes(encoded[0..4].try_into().unwrap());
        let mask = u32::from_ne_bytes(encoded[4..8].try_into().unwrap());
        let cookie = u32::from_ne_bytes(encoded[8..12].try_into().unwrap());
        let len = u32::from_ne_bytes(encoded[12..16].try_into().unwrap());

        assert_eq!(wd, 1);
        assert_eq!(mask, IN_CREATE);
        assert_eq!(cookie, 0);
        assert_eq!(len, 12);
        assert_eq!(&encoded[16..24], b"test.txt");
        assert_eq!(encoded[24], 0); // nul terminator
    }

    #[test]
    fn inotify_watch_lifecycle_and_rm_watch() {
        let temp_dir =
            std::env::temp_dir().join(format!("kinakaze-inotify-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let fd = create_inotify(IN_NONBLOCK).expect("create_inotify");
        let wd =
            add_watch(fd, temp_dir.clone(), IN_CREATE | IN_DELETE | IN_MODIFY).expect("add_watch");
        assert!(wd >= 1);

        // Removing the watch should generate an IN_IGNORED event
        assert_eq!(rm_watch(fd, wd), Ok(()));

        let mut buf = [0u8; 128];
        let n = read_inotify(fd, &mut buf, false).expect("read IN_IGNORED event");
        assert!(n >= 16);

        let r_wd = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
        let r_mask = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        assert_eq!(r_wd, wd);
        assert_ne!(r_mask & IN_IGNORED, 0);

        crate::close(fd).expect("close");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn inotify_rejects_invalid_watch_operations() {
        let temp_dir = std::env::temp_dir().join(format!(
            "kinakaze-inotify-invalid-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).expect("create temp directory");

        let fd = create_inotify(IN_NONBLOCK).expect("create_inotify");
        assert_eq!(add_watch(fd, temp_dir.clone(), 0), Err(crate::EINVAL));
        let wd = add_watch(fd, temp_dir.clone(), IN_CREATE).expect("add_watch");
        assert_eq!(
            add_watch(fd, temp_dir.clone(), IN_CREATE | IN_MASK_CREATE),
            Err(crate::EEXIST)
        );
        assert_eq!(rm_watch(fd, wd + 1), Err(crate::EINVAL));

        crate::close(fd).expect("close");
        std::fs::remove_dir_all(&temp_dir).expect("remove temp directory");
    }
}
