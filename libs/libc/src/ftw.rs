//! Iterative file-tree walks over the guest VFS. No native heap state crosses callbacks.
pub(crate) mod storage;
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use kinakaze_alloc::guest;
use kinakaze_vfs::{
    EACCES, EINVAL, ENOENT, ENOMEM, EOVERFLOW,
    fs::{self, Stat},
};
use storage::{Bytes, Seen};

const PHYS: i32 = 1;
const MOUNT: i32 = 2;
const CHDIR: i32 = 4;
const DEPTH: i32 = 8;
const ACTION: i32 = 16;
const FILE: i32 = 0;
const DIRECTORY: i32 = 1;
const UNREADABLE: i32 = 2;
const NO_STAT: i32 = 3;
const LINK: i32 = 4;
const POST_DIRECTORY: i32 = 5;
const DANGLING: i32 = 6;

#[repr(C)]
pub struct Ftw {
    base: c_int,
    level: c_int,
}
pub type Simple = unsafe extern "sysv64" fn(*const c_char, *const Stat, c_int) -> c_int;
pub type Detailed = unsafe extern "sysv64" fn(*const c_char, *const Stat, c_int, *mut Ftw) -> c_int;
#[derive(Clone, Copy)]
enum Callback {
    Simple(Simple),
    Detailed(Detailed),
}

struct Frame {
    parent: *mut Frame,
    display: Bytes,
    absolute: Bytes,
    stat: Stat,
    names: Option<Bytes>,
    next: usize,
    level: i32,
    entered: bool,
}
struct Stack(*mut Frame);
impl Stack {
    fn push(&mut self, display: Bytes, absolute: Bytes, level: i32) -> Result<(), i32> {
        let frame = unsafe { guest::malloc(size_of::<Frame>()).cast::<Frame>() };
        if frame.is_null() {
            return Err(ENOMEM);
        }
        unsafe {
            frame.write(Frame {
                parent: self.0,
                display,
                absolute,
                stat: Stat::default(),
                names: None,
                next: 0,
                level,
                entered: false,
            })
        };
        self.0 = frame;
        Ok(())
    }
    fn pop(&mut self) {
        let old = self.0;
        if !old.is_null() {
            unsafe {
                self.0 = (*old).parent;
                ptr::drop_in_place(old);
                guest::free(old.cast());
            }
        }
    }
    fn skip_siblings(&mut self) {
        if let Some(parent) = unsafe { self.0.as_mut() } {
            parent.next = parent.names.as_ref().map_or(0, |n| n.length);
        }
    }
}
impl Drop for Stack {
    fn drop(&mut self) {
        while !self.0.is_null() {
            self.pop();
        }
    }
}

fn classify(path: &str, physical: bool, root: bool) -> Result<(Stat, i32), i32> {
    match if physical {
        fs::lstat(path)
    } else {
        fs::stat(path)
    } {
        Ok(stat) => {
            let kind = match stat.st_mode & fs::S_IFMT {
                fs::S_IFDIR => DIRECTORY,
                fs::S_IFLNK => LINK,
                _ => FILE,
            };
            Ok((stat, kind))
        }
        Err(error) => {
            if !physical
                && matches!(error, ENOENT | EACCES)
                && let Ok(stat) = fs::lstat(path)
                && stat.st_mode & fs::S_IFMT == fs::S_IFLNK
            {
                return Ok((stat, DANGLING));
            }
            if !root && matches!(error, ENOENT | EACCES) {
                crate::set_errno(error);
                Ok((Stat::default(), NO_STAT))
            } else {
                Err(error)
            }
        }
    }
}

unsafe fn invoke(frame: &Frame, kind: i32, flags: i32, callback: Callback) -> Result<i32, i32> {
    if flags & CHDIR != 0 {
        let path = frame.absolute.as_str();
        let directory = if kind == POST_DIRECTORY {
            path
        } else {
            path.rsplit_once('/').map_or(
                "/",
                |(parent, _)| if parent.is_empty() { "/" } else { parent },
            )
        };
        fs::chdir(directory)?;
    }
    let mut info = Ftw {
        base: i32::try_from(frame.display.as_str().rfind('/').map_or(0, |i| i + 1))
            .map_err(|_| EOVERFLOW)?,
        level: frame.level,
    };
    let kind = if matches!(callback, Callback::Simple(_)) {
        match kind {
            LINK => FILE,
            POST_DIRECTORY => DIRECTORY,
            DANGLING => NO_STAT,
            _ => kind,
        }
    } else {
        kind
    };
    Ok(unsafe {
        match callback {
            Callback::Simple(f) => f(frame.display.pointer.cast(), &frame.stat, kind),
            Callback::Detailed(f) => f(frame.display.pointer.cast(), &frame.stat, kind, &mut info),
        }
    })
}

unsafe fn walk(stack: &mut Stack, flags: i32, callback: Callback) -> Result<i32, i32> {
    let mut seen = Seen::default();
    let mut device = 0;
    while let Some(frame) = unsafe { stack.0.as_mut() } {
        let (kind, result) = if !frame.entered {
            let (stat, mut kind) =
                classify(frame.absolute.as_str(), flags & PHYS != 0, frame.level == 0)?;
            frame.stat = stat;
            if frame.level == 0 {
                device = stat.st_dev;
            }
            if kind != NO_STAT && flags & MOUNT != 0 && stat.st_dev != device {
                stack.pop();
                continue;
            }
            if kind == DIRECTORY {
                if flags & PHYS == 0 && !seen.insert(stat.st_dev, stat.st_ino)? {
                    stack.pop();
                    continue;
                }
                match Bytes::directory(frame.absolute.as_str()) {
                    Ok(names) => frame.names = Some(names),
                    Err(EACCES) => {
                        crate::set_errno(EACCES);
                        kind = UNREADABLE;
                    }
                    Err(error) => return Err(error),
                }
            }
            frame.entered = true;
            if kind == DIRECTORY && flags & DEPTH != 0 {
                continue;
            }
            (kind, unsafe { invoke(frame, kind, flags, callback) }?)
        } else if let Some(names) = &frame.names {
            if frame.next < names.length {
                let name =
                    unsafe { CStr::from_ptr(names.pointer.add(frame.next).cast()) }.to_bytes();
                frame.next += name.len() + 1;
                let display = frame.display.join(name)?;
                let absolute = frame.absolute.join(name)?;
                let level = frame.level.checked_add(1).ok_or(EOVERFLOW)?;
                stack.push(display, absolute, level)?;
                continue;
            }
            (
                POST_DIRECTORY,
                if flags & DEPTH != 0 {
                    unsafe { invoke(frame, POST_DIRECTORY, flags, callback) }?
                } else {
                    0
                },
            )
        } else {
            unreachable!()
        };
        if result != 0 {
            if flags & ACTION != 0 && matches!(result, 2 | 3) {
                if result == 3 {
                    stack.pop();
                    stack.skip_siblings();
                    continue;
                }
                if kind == DIRECTORY {
                    stack.pop();
                    continue;
                }
            } else {
                return Ok(result);
            }
        }
        if kind != DIRECTORY {
            stack.pop();
        }
    }
    Ok(0)
}

unsafe fn start(path: *const c_char, callback: Callback, flags: i32) -> i32 {
    let result = (|| -> Result<i32, i32> {
        if path.is_null() || flags & !31 != 0 {
            return Err(EINVAL);
        }
        let text = unsafe { CStr::from_ptr(path) }.to_str().map_err(|_| 84)?;
        if text.is_empty() {
            return Err(ENOENT);
        }
        let path = if text.trim_end_matches('/').is_empty() {
            "/"
        } else {
            text.trim_end_matches('/')
        };
        let display = Bytes::text(path)?;
        let absolute = Bytes::text(&fs::absolute_linux(path))?;
        let mut stack = Stack(ptr::null_mut());
        stack.push(display, absolute, 0)?;
        // Directory snapshots close before callbacks, using at most one traversal
        // descriptor regardless of the requested budget; CHDIR preserves cwd by FD.
        let saved = if flags & CHDIR != 0 {
            Some(fs::open(
                ".",
                fs::O_PATH | fs::O_DIRECTORY | fs::O_CLOEXEC,
                0,
            )?)
        } else {
            None
        };
        let result = unsafe { walk(&mut stack, flags, callback) };
        let errno = kinakaze_tls::errno();
        if let Some(fd) = saved {
            let restored = fs::fchdir(fd);
            let _ = kinakaze_vfs::close(fd);
            if result == Ok(0) {
                restored?;
            }
        }
        crate::set_errno(errno);
        result
    })();
    match result {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ftw(
    path: *const c_char,
    callback: Option<Simple>,
    _descriptors: c_int,
) -> c_int {
    match callback {
        Some(f) => unsafe { start(path, Callback::Simple(f), 0) },
        None => {
            crate::set_errno(EINVAL);
            -1
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_nftw(
    path: *const c_char,
    callback: Option<Detailed>,
    _descriptors: c_int,
    flags: c_int,
) -> c_int {
    match callback {
        Some(f) => unsafe { start(path, Callback::Detailed(f), flags) },
        None => {
            crate::set_errno(EINVAL);
            -1
        }
    }
}
