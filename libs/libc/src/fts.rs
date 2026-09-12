//! Incremental BSD file-tree streams over the guest VFS.
//!
//! Entries, paths and sort storage live in guest memory. A comparator may fork;
//! no native heap collection or open directory iterator crosses that callback.
use crate::ftw::storage::Bytes;
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};
use kinakaze_alloc::guest;
use kinakaze_vfs::{
    EINVAL, ENOENT, ENOMEM, EOVERFLOW,
    fs::{self, Stat},
};

const COMFOLLOW: i32 = 1;
const LOGICAL: i32 = 2;
const NOCHDIR: i32 = 4;
const NOSTAT: i32 = 8;
const SEEDOT: i32 = 32;
const XDEV: i32 = 64;
const NAMEONLY: i32 = 256;
const STOP: i32 = 512;
const D: u16 = 1;
const DC: u16 = 2;
const DNR: u16 = 4;
const DOT: u16 = 5;
const DP: u16 = 6;
const INIT: u16 = 9;
const NS: u16 = 10;
const NSOK: u16 = 11;
const SL: u16 = 12;
const SLNONE: u16 = 13;
const AGAIN: u16 = 1;
const FOLLOW: u16 = 2;
const NOINSTR: u16 = 3;
const SKIP: u16 = 4;

pub type Compare = unsafe extern "sysv64" fn(*const *const FtsEnt, *const *const FtsEnt) -> c_int;

#[repr(C)]
pub struct Fts {
    cur: *mut FtsEnt,
    child: *mut FtsEnt,
    array: *mut *mut FtsEnt,
    dev: u64,
    path: *mut c_char,
    rfd: i32,
    pathlen: i32,
    nitems: i32,
    compar: Option<Compare>,
    options: i32,
}

#[repr(C)]
pub struct FtsEnt {
    cycle: *mut FtsEnt,
    parent: *mut FtsEnt,
    link: *mut FtsEnt,
    number: i64,
    pointer: *mut c_void,
    accpath: *mut c_char,
    path: *mut c_char,
    errno: i32,
    symfd: i32,
    pathlen: u16,
    namelen: u16,
    ino: u64,
    dev: u64,
    nlink: u64,
    level: i16,
    info: u16,
    flags: u16,
    instr: u16,
    statp: *mut Stat,
    name: [c_char; 1],
}

#[repr(C)]
struct Node {
    stat: Stat,
    absolute: Bytes,
    display: Bytes,
    entry: FtsEnt, // Flexible name bytes continue beyond this final field.
}

unsafe fn node<'a>(entry: *mut FtsEnt) -> &'a mut Node {
    unsafe {
        &mut *entry
            .cast::<u8>()
            .sub(core::mem::offset_of!(Node, entry))
            .cast::<Node>()
    }
}

unsafe fn allocate(
    display: Bytes,
    absolute: Bytes,
    name: &str,
    parent: *mut FtsEnt,
    level: i16,
) -> Result<*mut FtsEnt, i32> {
    let pathlen = u16::try_from(display.length - 1).map_err(|_| EOVERFLOW)?;
    let namelen = u16::try_from(name.len()).map_err(|_| EOVERFLOW)?;
    let allocation = unsafe { guest::malloc(size_of::<Node>() + name.len()).cast::<Node>() };
    if allocation.is_null() {
        return Err(ENOMEM);
    }
    unsafe {
        allocation.write(Node {
            stat: Stat::default(),
            absolute,
            display,
            entry: core::mem::zeroed(),
        });
        let n = &mut *allocation;
        let e = &mut n.entry;
        e.parent = parent;
        e.level = level;
        e.path = n.display.pointer.cast();
        e.accpath = n.absolute.pointer.cast();
        e.pathlen = pathlen;
        e.namelen = namelen;
        e.symfd = -1;
        e.instr = NOINSTR;
        e.statp = &raw mut n.stat;
        ptr::copy_nonoverlapping(name.as_ptr(), e.name.as_mut_ptr().cast(), name.len());
        e.name.as_mut_ptr().add(name.len()).write(0);
        Ok(e)
    }
}

unsafe fn release(e: *mut FtsEnt) {
    unsafe {
        let n = node(e) as *mut Node;
        ptr::drop_in_place(n);
        guest::free(n.cast());
    }
}
unsafe fn release_list(mut e: *mut FtsEnt) {
    while !e.is_null() {
        unsafe {
            let next = (*e).link;
            release(e);
            e = next;
        }
    }
}

unsafe fn classify(stream: &Fts, e: *mut FtsEnt, follow: bool) {
    unsafe {
        let n = node(e);
        let path = n.absolute.as_str();
        let follow = follow
            || stream.options & LOGICAL != 0
            || n.entry.level == 0 && stream.options & COMFOLLOW != 0;
        n.entry.errno = 0;
        n.entry.cycle = ptr::null_mut();
        let mut dangling = false;
        let result = if follow {
            fs::stat(path)
        } else {
            fs::lstat(path)
        };
        let result = result.or_else(|error| {
            if follow
                && let Ok(st) = fs::lstat(path)
                && st.st_mode & fs::S_IFMT == fs::S_IFLNK
            {
                dangling = true;
                return Ok(st);
            }
            Err(error)
        });
        match result {
            Ok(st) => {
                n.stat = st;
                n.entry.ino = st.st_ino;
                n.entry.dev = st.st_dev;
                n.entry.nlink = st.st_nlink;
                n.entry.info = if dangling {
                    SLNONE
                } else {
                    match st.st_mode & fs::S_IFMT {
                        fs::S_IFDIR => D,
                        fs::S_IFLNK => SL,
                        fs::S_IFREG => 8,
                        _ => 3,
                    }
                };
                if n.entry.info == D {
                    let name = CStr::from_ptr(n.entry.name.as_ptr()).to_bytes();
                    if n.entry.level > 0 && (name == b"." || name == b"..") {
                        n.entry.info = DOT;
                    } else {
                        let mut parent = n.entry.parent;
                        while !parent.is_null() && (*parent).level >= 0 {
                            if (*parent).dev == st.st_dev && (*parent).ino == st.st_ino {
                                n.entry.cycle = parent;
                                n.entry.info = DC;
                                break;
                            }
                            parent = (*parent).parent;
                        }
                    }
                }
            }
            Err(error) => {
                n.stat = Stat::default();
                n.entry.errno = error;
                n.entry.info = NS;
            }
        }
        n.entry.statp = if stream.options & NOSTAT != 0 {
            ptr::null_mut()
        } else {
            &raw mut n.stat
        };
    }
}

unsafe fn sort(head: *mut FtsEnt, compare: Option<Compare>) -> Result<*mut FtsEnt, i32> {
    let Some(compare) = compare else {
        return Ok(head);
    };
    let mut count = 0usize;
    let mut e = head;
    unsafe {
        while !e.is_null() {
            count += 1;
            e = (*e).link;
        }
    }
    if count < 2 {
        return Ok(head);
    }
    let storage = Bytes::zeroed(
        count
            .checked_mul(size_of::<*mut FtsEnt>())
            .ok_or(EOVERFLOW)?,
    )?;
    let entries =
        unsafe { core::slice::from_raw_parts_mut(storage.pointer.cast::<*mut FtsEnt>(), count) };
    e = head;
    for slot in entries.iter_mut() {
        unsafe {
            *slot = e;
            e = (*e).link;
        }
    }
    entries.sort_unstable_by(|a, b| unsafe {
        compare(
            (a as *const *mut FtsEnt).cast(),
            (b as *const *mut FtsEnt).cast(),
        )
        .cmp(&0)
    });
    for index in 0..count {
        unsafe {
            (*entries[index]).link = entries.get(index + 1).copied().unwrap_or(ptr::null_mut());
        }
    }
    Ok(entries[0])
}

unsafe fn children(stream: &mut Fts, names_only: bool) -> Result<*mut FtsEnt, i32> {
    unsafe {
        let parent = stream.cur;
        let n = node(parent);
        let fd = fs::open(
            n.absolute.as_str(),
            fs::O_RDONLY | fs::O_DIRECTORY | fs::O_CLOEXEC,
            0,
        )?;
        let result = fs::read_directory_fd(fd);
        let _ = kinakaze_vfs::close(fd);
        let mut head: *mut FtsEnt = ptr::null_mut();
        let mut tail: *mut FtsEnt = ptr::null_mut();
        // Drop the host-owned directory snapshot before invoking a comparator.
        let result = (|| {
            for entry in result? {
                if stream.options & SEEDOT == 0 && (entry.name == "." || entry.name == "..") {
                    continue;
                }
                let level = (*parent).level.checked_add(1).ok_or(EOVERFLOW)?;
                let e = allocate(
                    n.display.join(entry.name.as_bytes())?,
                    n.absolute.join(entry.name.as_bytes())?,
                    &entry.name,
                    parent,
                    level,
                )?;
                if head.is_null() {
                    head = e;
                } else {
                    (*tail).link = e;
                }
                tail = e;
                if names_only
                    || stream.options & NOSTAT != 0
                        && stream.options & LOGICAL == 0
                        && !entry.is_directory
                        && !entry.is_symlink
                {
                    (*e).info = NSOK;
                    (*e).statp = ptr::null_mut();
                } else {
                    classify(stream, e, false);
                }
            }
            Ok::<_, i32>(())
        })();
        if let Err(error) = result {
            release_list(head);
            return Err(error);
        }
        match sort(head, stream.compar) {
            Ok(head) => Ok(head),
            Err(error) => {
                release_list(head);
                Err(error)
            }
        }
    }
}

unsafe fn present(stream: &mut Fts, e: *mut FtsEnt) -> Result<*mut FtsEnt, i32> {
    unsafe {
        stream.cur = e;
        if e.is_null() {
            stream.path = ptr::null_mut();
            if stream.options & NOCHDIR == 0 {
                fs::fchdir(stream.rfd)?;
            }
            crate::set_errno(0);
            return Ok(e);
        }
        stream.path = (*e).path;
        stream.pathlen = (*e).pathlen as i32 + 1;
        if (*e).level == 0 {
            stream.dev = (*e).dev;
        }
        if stream.options & NOCHDIR == 0 {
            if (*e).level == 0 {
                fs::fchdir(stream.rfd)?;
            } else {
                fs::chdir(node((*e).parent).absolute.as_str())?;
            }
        }
        Ok(e)
    }
}

unsafe fn read(stream: &mut Fts) -> Result<*mut FtsEnt, i32> {
    unsafe {
        let mut e = stream.cur;
        if e.is_null() || stream.options & STOP != 0 {
            return Ok(ptr::null_mut());
        }
        let instruction = (*e).instr;
        (*e).instr = NOINSTR;
        if instruction == AGAIN || instruction == FOLLOW && matches!((*e).info, SL | SLNONE) {
            release_list(stream.child);
            stream.child = ptr::null_mut();
            stream.options &= !NAMEONLY;
            classify(stream, e, instruction == FOLLOW);
            return present(stream, e);
        }
        if (*e).info == D {
            if instruction == SKIP || stream.options & XDEV != 0 && (*e).dev != stream.dev {
                release_list(stream.child);
                stream.child = ptr::null_mut();
                (*e).info = DP;
                return present(stream, e);
            }
            if stream.options & NAMEONLY != 0 {
                release_list(stream.child);
                stream.child = ptr::null_mut();
                stream.options &= !NAMEONLY;
            }
            if stream.child.is_null() {
                match children(stream, false) {
                    Ok(child) => stream.child = child,
                    Err(error) if error == ENOMEM || error == EOVERFLOW => return Err(error),
                    Err(error) => {
                        (*e).info = DNR;
                        (*e).errno = error;
                        return present(stream, e);
                    }
                }
            }
            let child = core::mem::replace(&mut stream.child, ptr::null_mut());
            if child.is_null() {
                (*e).info = DP;
                return present(stream, e);
            }
            // Keep the preorder parent alive until the final child is consumed.
            e = child;
        } else {
            let next = (*e).link;
            let parent = (*e).parent;
            release(e);
            if next.is_null() {
                if (*parent).level < 0 {
                    release(parent);
                    return present(stream, ptr::null_mut());
                }
                (*parent).info = DP;
                return present(stream, parent);
            }
            e = next;
        }
        loop {
            if (*e).instr != SKIP {
                break;
            }
            let next = (*e).link;
            let parent = (*e).parent;
            release(e);
            if next.is_null() {
                if (*parent).level < 0 {
                    release(parent);
                    return present(stream, ptr::null_mut());
                }
                (*parent).info = DP;
                return present(stream, parent);
            }
            e = next;
        }
        if (*e).instr == FOLLOW {
            classify(stream, e, true);
        }
        (*e).instr = NOINSTR;
        present(stream, e)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fts_open(
    paths: *const *const c_char,
    options: c_int,
    compare: Option<Compare>,
) -> *mut Fts {
    let result = (|| {
        if paths.is_null() || options & !255 != 0 {
            return Err(EINVAL);
        }
        let stream = unsafe { guest::malloc(size_of::<Fts>()).cast::<Fts>() };
        if stream.is_null() {
            return Err(ENOMEM);
        }
        unsafe {
            stream.write(Fts {
                cur: ptr::null_mut(),
                child: ptr::null_mut(),
                array: ptr::null_mut(),
                dev: 0,
                path: ptr::null_mut(),
                rfd: -1,
                pathlen: 0,
                nitems: 0,
                compar: compare,
                options: options | if options & LOGICAL != 0 { NOCHDIR } else { 0 },
            });
            let result = (|| {
                let parent = allocate(Bytes::text("")?, Bytes::text("")?, "", ptr::null_mut(), -1)?;
                (*stream).cur = parent;
                let initial = allocate(Bytes::text("")?, Bytes::text("")?, "", parent, 0)?;
                (*initial).info = INIT;
                (*stream).cur = initial;
                let mut tail = initial;
                let mut arg = paths;
                while !(*arg).is_null() {
                    let path = CStr::from_ptr(*arg).to_str().map_err(|_| 84)?;
                    if path.is_empty() {
                        return Err(ENOENT);
                    }
                    let path = if path.trim_end_matches('/').is_empty() {
                        "/"
                    } else {
                        path.trim_end_matches('/')
                    };
                    let name = path
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("/");
                    let e = allocate(
                        Bytes::text(path)?,
                        Bytes::text(&fs::absolute_linux(path))?,
                        name,
                        parent,
                        0,
                    )?;
                    (*tail).link = e;
                    tail = e;
                    classify(&*stream, e, false);
                    arg = arg.add(1);
                }
                (*initial).link = sort((*initial).link, compare)?;
                if (*stream).options & NOCHDIR == 0 {
                    (*stream).rfd = fs::open(".", fs::O_PATH | fs::O_DIRECTORY | fs::O_CLOEXEC, 0)?;
                }
                Ok(stream)
            })();
            if result.is_err() {
                kinakaze_abi_fts_close(stream);
            }
            result
        }
    })();
    match result {
        Ok(stream) => stream,
        Err(error) => {
            crate::set_errno(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fts_read(stream: *mut Fts) -> *mut FtsEnt {
    let Some(stream) = (unsafe { stream.as_mut() }) else {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    };
    match unsafe { read(stream) } {
        Ok(entry) => entry,
        Err(error) => {
            stream.options |= STOP;
            crate::set_errno(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fts_children(
    stream: *mut Fts,
    instruction: c_int,
) -> *mut FtsEnt {
    if stream.is_null() || !matches!(instruction, 0 | NAMEONLY) {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    }
    unsafe {
        let stream = &mut *stream;
        crate::set_errno(0);
        if stream.cur.is_null() || stream.options & STOP != 0 {
            return ptr::null_mut();
        }
        if (*stream.cur).info == INIT {
            return (*stream.cur).link;
        }
        if (*stream.cur).info != D {
            return ptr::null_mut();
        }
        release_list(stream.child);
        stream.child = ptr::null_mut();
        stream.options = (stream.options & !NAMEONLY) | instruction;
        match children(stream, instruction == NAMEONLY) {
            Ok(list) => {
                stream.child = list;
                list
            }
            Err(error) => {
                crate::set_errno(error);
                ptr::null_mut()
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fts_set(
    _stream: *mut Fts,
    entry: *mut FtsEnt,
    instruction: c_int,
) -> c_int {
    if entry.is_null() || !(0..=4).contains(&instruction) {
        crate::set_errno(EINVAL);
        return 1;
    }
    unsafe {
        (*entry).instr = instruction as u16;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fts_close(stream: *mut Fts) -> c_int {
    if stream.is_null() {
        crate::set_errno(EINVAL);
        return -1;
    }
    unsafe {
        let mut e = (*stream).cur;
        while !e.is_null() {
            let parent = (*e).parent;
            release_list(e);
            e = parent;
        }
        release_list((*stream).child);
        let fd = (*stream).rfd;
        guest::free(stream.cast());
        if fd >= 0 {
            let result = fs::fchdir(fd);
            let _ = kinakaze_vfs::close(fd);
            if let Err(error) = result {
                crate::set_errno(error);
                return -1;
            }
        }
    }
    0
}

// Linux x86-64 has identical FTS/FTS64 structures and stat layouts.
macro_rules! alias {
    ($name:ident, $target:ident, ($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $name($($arg: $ty),*) -> $ret {
            unsafe { $target($($arg),*) }
        }
    };
}
alias!(kinakaze_abi_fts64_open, kinakaze_abi_fts_open, (paths: *const *const c_char, options: c_int, compare: Option<Compare>) -> *mut Fts);
alias!(kinakaze_abi_fts64_read, kinakaze_abi_fts_read, (stream: *mut Fts) -> *mut FtsEnt);
alias!(kinakaze_abi_fts64_children, kinakaze_abi_fts_children, (stream: *mut Fts, instruction: c_int) -> *mut FtsEnt);
alias!(kinakaze_abi_fts64_set, kinakaze_abi_fts_set, (stream: *mut Fts, entry: *mut FtsEnt, instruction: c_int) -> c_int);
alias!(kinakaze_abi_fts64_close, kinakaze_abi_fts_close, (stream: *mut Fts) -> c_int);
