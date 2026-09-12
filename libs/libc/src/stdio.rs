//! Buffered streams and the `printf` family.
//!
//! A `FILE` is an opaque handle wrapping a Linux descriptor plus a buffer. The
//! three standard streams are pre-created; `stdout` is line-buffered when it
//! refers to a terminal and fully buffered otherwise, which is the behaviour C
//! programs depend on for interactive output.

use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use std::sync::{Mutex, OnceLock};

use crate::format::{BufferSink, Sink, VaList};
pub(crate) mod cookie;
mod lifecycle;
pub(crate) mod line;
mod memory;
mod position;
mod wide;

/// `EOF`.
pub const EOF: c_int = -1;
/// Default buffer size, matching the common `BUFSIZ`.
pub const BUFSIZ: usize = 8192;

const SEEK_SET: c_int = 0;

/// Buffering discipline.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Buffering {
    /// `_IONBF`: every write reaches the descriptor immediately.
    None,
    /// `_IOLBF`: flushed when a newline is written.
    Line,
    /// `_IOFBF`: flushed when the buffer fills.
    Full,
    /// Not yet chosen: decided on first use from what the descriptor refers to.
    ///
    /// `stdout` is line-buffered on a terminal and fully buffered otherwise, but
    /// the standard streams are now compile-time constants, so the question
    /// cannot be answered where they are defined. It is answered on the first
    /// write instead.
    Undecided,
}

/// The state behind a `FILE`.
struct Stream {
    fd: c_int,
    /// The FILE mode can be narrower than the underlying descriptor's access.
    access: u8,
    /// Allocated only for callback streams. Ordinary descriptor streams keep
    /// the existing direct I/O path and have no callback allocation.
    cookie: Option<Box<cookie::Cookie>>,
    buffer: Vec<u8>,
    input: Vec<u8>,
    input_pos: usize,
    input_end: usize,
    buffering: Buffering,
    orientation: i32,
    /// Encoding is fixed when the stream first becomes wide oriented.
    wide_utf8: bool,
    /// Sticky end-of-file indicator, cleared by `clearerr`.
    eof: bool,
    /// Sticky error indicator, cleared by `clearerr`.
    error: bool,
    /// Bytes pushed back by `ungetc`, consumed most recent first.
    pushback: Vec<u8>,
}

impl Stream {
    const fn new(fd: c_int, buffering: Buffering) -> Self {
        Self {
            fd,
            access: kinakaze_vfs::fs::O_RDWR as u8,
            cookie: None,
            buffer: Vec::new(),
            input: Vec::new(),
            input_pos: 0,
            input_end: 0,
            buffering,
            orientation: 0,
            wide_utf8: false,
            eof: false,
            error: false,
            pushback: Vec::new(),
        }
    }

    const fn with_access(mut self, flags: c_int) -> Self {
        self.access = (flags & 3) as u8;
        self
    }

    const fn access_flags(&self) -> i32 {
        match self.access as c_int {
            kinakaze_vfs::fs::O_RDONLY => 0x08, // _IO_NO_WRITES
            kinakaze_vfs::fs::O_WRONLY => 0x04, // _IO_NO_READS
            _ => 0,
        }
    }

    /// Settles a deferred buffering choice against the live descriptor.
    ///
    /// C requires a stream on a terminal to be line buffered, and "a terminal"
    /// now includes a pseudo-terminal. Leaving a pty on full buffering would
    /// make a prompt written without a newline never reach the reader — which is
    /// exactly the case `sshpass` and `expect`-style programs are built around.
    fn resolve_buffering(&mut self) {
        if self.buffering != Buffering::Undecided {
            return;
        }
        self.buffering = match kinakaze_vfs::get(self.fd) {
            Ok(entry)
                if matches!(
                    entry.kind,
                    kinakaze_vfs::FdKind::Console
                        | kinakaze_vfs::FdKind::PtyMaster
                        | kinakaze_vfs::FdKind::PtySlave
                ) =>
            {
                Buffering::Line
            }
            _ => Buffering::Full,
        };
    }

    /// Writes buffered bytes to the descriptor.
    ///
    /// A short write is retried, because a signal-interrupted write may have
    /// transferred only part of the buffer.
    fn flush(&mut self) -> Result<(), i32> {
        if let Some(cookie) = &self.cookie {
            if self.buffer.is_empty() {
                return Ok(());
            }
            let length = self.buffer.len();
            let result = cookie.write(&self.buffer);
            return match result {
                Ok(written) => {
                    self.buffer.drain(..written);
                    if written == length {
                        Ok(())
                    } else {
                        self.error = true;
                        Err(kinakaze_tls::errno())
                    }
                }
                Err(error) => {
                    self.error = true;
                    Err(error)
                }
            };
        }
        let mut position = 0;
        while position < self.buffer.len() {
            match self.write_backend(&self.buffer[position..]) {
                Ok(0) => {
                    self.error = true;
                    return Err(kinakaze_vfs::EIO);
                }
                Ok(written) => position += written,
                Err(error) => {
                    // Drop what was already written so a retry does not
                    // duplicate it.
                    self.buffer.drain(..position);
                    self.error = true;
                    return Err(error);
                }
            }
        }
        self.buffer.clear();
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize, i32> {
        if self.access == kinakaze_vfs::fs::O_RDONLY as u8 {
            self.error = true;
            return Err(kinakaze_vfs::EBADF);
        }
        self.sync_input()?;
        self.resolve_buffering();
        if crate::fork_trace_enabled() {
            eprintln!(
                "kinakaze stdio: pid {} fd={} buffered write {} bytes mode={}",
                std::process::id(),
                self.fd,
                bytes.len(),
                self.buffering as u8
            );
        }
        if self.buffering == Buffering::None {
            if let Some(cookie) = &self.cookie {
                return match cookie.write(bytes) {
                    Ok(written) => {
                        if written != bytes.len() {
                            self.error = true;
                        }
                        Ok(written)
                    }
                    Err(error) => {
                        self.error = true;
                        Err(error)
                    }
                };
            }
            let mut position = 0;
            while position < bytes.len() {
                match self.write_backend(&bytes[position..]) {
                    Ok(0) => {
                        self.error = true;
                        return Err(kinakaze_vfs::EIO);
                    }
                    Ok(written) => position += written,
                    Err(error) => {
                        self.error = true;
                        return Err(error);
                    }
                }
            }
            return Ok(bytes.len());
        }

        self.buffer.extend_from_slice(bytes);
        let should_flush = match self.buffering {
            Buffering::Line => bytes.contains(&b'\n'),
            _ => self.buffer.len() >= BUFSIZ,
        };
        if should_flush {
            self.flush()?;
        }
        Ok(bytes.len())
    }

    fn read(&mut self, out: &mut [u8]) -> Result<usize, i32> {
        if self.access == kinakaze_vfs::fs::O_WRONLY as u8 {
            self.error = true;
            return Err(kinakaze_vfs::EBADF);
        }
        let mut filled = 0;
        // Pushed-back bytes are consumed before touching the descriptor.
        while filled < out.len() {
            match self.pushback.pop() {
                Some(byte) => {
                    out[filled] = byte;
                    filled += 1;
                }
                None => break,
            }
        }
        if filled == out.len() {
            return Ok(filled);
        }
        let available = self.input_end - self.input_pos;
        let copied = available.min(out.len() - filled);
        out[filled..filled + copied]
            .copy_from_slice(&self.input[self.input_pos..self.input_pos + copied]);
        self.input_pos += copied;
        filled += copied;
        if filled > 0 || self.eof {
            return Ok(filled);
        }
        self.flush()?;
        self.resolve_buffering();
        // Large fread calls already provide useful storage. Small callers such
        // as fgets/fgetc refill once per block instead of reopening/validating
        // an overlay inode for every character in a configuration file.
        let buffered = self.buffering != Buffering::None && out.len() < BUFSIZ;
        let result = if buffered {
            self.input.resize(BUFSIZ, 0);
            self.input_pos = 0;
            self.input_end = 0;
            match &self.cookie {
                Some(cookie) => cookie.read(&mut self.input),
                None => kinakaze_vfs::read(self.fd, &mut self.input),
            }
        } else {
            match &self.cookie {
                Some(cookie) => cookie.read(out),
                None => kinakaze_vfs::read(self.fd, out),
            }
        };
        match result {
            Ok(0) => {
                self.eof = true;
                Ok(0)
            }
            Ok(count) if buffered => {
                self.input_end = count;
                let copied = out.len().min(count);
                out[..copied].copy_from_slice(&self.input[..copied]);
                self.input_pos = copied;
                Ok(copied)
            }
            Ok(count) => Ok(count),
            Err(error) => {
                self.error = true;
                Err(error)
            }
        }
    }

    fn unread(&self) -> usize {
        self.input_end - self.input_pos + self.pushback.len()
    }

    fn discard_input(&mut self) {
        self.input_pos = 0;
        self.input_end = 0;
        self.pushback.clear();
    }

    /// POSIX fflush on seekable input restores the logical position before a
    /// caller accesses fileno(). A pipe cannot rewind: keep its prefetched data.
    fn sync_input(&mut self) -> Result<(), i32> {
        let unread = self.unread();
        if unread == 0 {
            return Ok(());
        }
        match self.seek_backend(-(unread as i64), 1) {
            Ok(_) => {
                self.discard_input();
                Ok(())
            }
            Err(kinakaze_vfs::ESPIPE) => Ok(()),
            Err(error) => {
                self.error = true;
                Err(error)
            }
        }
    }

    fn write_backend(&self, bytes: &[u8]) -> Result<usize, i32> {
        match &self.cookie {
            Some(cookie) => cookie.write(bytes),
            None => kinakaze_vfs::write(self.fd, bytes),
        }
    }

    fn seek_backend(&self, offset: i64, whence: i32) -> Result<u64, i32> {
        match &self.cookie {
            Some(cookie) => cookie.seek(offset, whence),
            None => kinakaze_vfs::fs::lseek(self.fd, offset, whence),
        }
    }

    fn close_backend(&mut self) -> Result<(), i32> {
        match self.cookie.take() {
            Some(cookie) => cookie.close(),
            None => kinakaze_vfs::close(self.fd),
        }
    }

    fn retire(&mut self) {
        self.fd = -1;
        self.cookie = None;
        // FILE controls stay allocated for existing mutex waiters. Their input
        // and output buffers must not be retained for the process lifetime.
        self.buffer = Vec::new();
        self.input = Vec::new();
        self.pushback = Vec::new();
        self.discard_input();
        self.eof = true;
    }
}

/// A `FILE` as the guest sees it, matching glibc's `_IO_FILE` ABI layout on x86_64.
///
/// Python and glibc inline buffer checks (`_IO_read_ptr < _IO_read_end`) expect
/// the standard field offsets. When both pointers are null/equal, inline macros
/// correctly call into `fgetc` / `getc_unlocked` / `__underflow`.
#[repr(C)]
pub struct File {
    pub _flags: core::sync::atomic::AtomicI32,
    pub _IO_read_ptr: *mut c_char,
    pub _IO_read_end: *mut c_char,
    pub _IO_read_base: *mut c_char,
    pub _IO_write_base: *mut c_char,
    pub _IO_write_ptr: *mut c_char,
    pub _IO_write_end: *mut c_char,
    pub _IO_buf_base: *mut c_char,
    pub _IO_buf_end: *mut c_char,
    pub _IO_save_base: *mut c_char,
    pub _IO_backup_base: *mut c_char,
    pub _IO_save_end: *mut c_char,
    pub _markers: *mut c_void,
    pub _chain: *mut c_void,
    pub _fileno: c_int,
    pub _flags2: c_int,
    pub _old_offset: i64,
    pub _cur_column: u16,
    pub _vtable_offset: i8,
    pub _shortbuf: [c_char; 1],
    pub _lock: *mut c_void,
    pub _offset: i64,
    pub _codecvt: *mut c_void,
    pub _wide_data: *mut c_void,
    pub _freeres_list: *mut c_void,
    pub _freeres_buf: *mut c_void,
    pub _pad5: usize,
    pub _mode: core::sync::atomic::AtomicI32,
    pub _unused2: [c_char; 20],
    pub stream: Mutex<Stream>,
}

// File contains raw pointers to match glibc layout, but accesses to the inner stream
// are synchronized by the Mutex.
unsafe impl Sync for File {}
unsafe impl Send for File {}

impl File {
    fn publish_indicators(&self, stream: &Stream) {
        let indicators =
            (if stream.eof { 0x10 } else { 0 }) | (if stream.error { 0x20 } else { 0 });
        let flags = self._flags.load(core::sync::atomic::Ordering::Relaxed);
        self._flags.store(
            (flags & !0x3c) | stream.access_flags() | indicators,
            core::sync::atomic::Ordering::Relaxed,
        );
    }

    pub const fn new_static(fileno: c_int, stream: Stream) -> Self {
        Self {
            _flags: core::sync::atomic::AtomicI32::new(stream.access_flags()),
            _IO_read_ptr: ptr::null_mut(),
            _IO_read_end: ptr::null_mut(),
            _IO_read_base: ptr::null_mut(),
            _IO_write_base: ptr::null_mut(),
            _IO_write_ptr: ptr::null_mut(),
            _IO_write_end: ptr::null_mut(),
            _IO_buf_base: ptr::null_mut(),
            _IO_buf_end: ptr::null_mut(),
            _IO_save_base: ptr::null_mut(),
            _IO_backup_base: ptr::null_mut(),
            _IO_save_end: ptr::null_mut(),
            _markers: ptr::null_mut(),
            _chain: ptr::null_mut(),
            _fileno: fileno,
            _flags2: 0,
            _old_offset: 0,
            _cur_column: 0,
            _vtable_offset: 0,
            _shortbuf: [0; 1],
            _lock: ptr::null_mut(),
            _offset: 0,
            _codecvt: ptr::null_mut(),
            _wide_data: ptr::null_mut(),
            _freeres_list: ptr::null_mut(),
            _freeres_buf: ptr::null_mut(),
            _pad5: 0,
            _mode: core::sync::atomic::AtomicI32::new(0),
            _unused2: [0; 20],
            stream: Mutex::new(stream),
        }
    }
}

/// A `FILE *` as an exported variable.
///
/// This is what the guest's `extern FILE *stdout;` names. It is an
/// `AtomicPtr` rather than a plain pointer for two reasons: the storage has to
/// be writable, since a guest may assign to `stdout`, and a static holding a
/// bare `*mut` is not `Sync`. The representation is identical to a pointer, so
/// the guest sees exactly the eight bytes it expects.
#[repr(transparent)]
pub struct StreamPointer(core::sync::atomic::AtomicPtr<File>);

impl StreamPointer {
    const fn new(file: &'static File) -> Self {
        Self(core::sync::atomic::AtomicPtr::new(
            ptr::from_ref(file).cast_mut(),
        ))
    }

    /// Reads the current stream pointer.
    pub fn get(&self) -> *mut File {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// Every live stream, so `exit` can flush them all.
static OPEN_STREAMS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

fn open_streams() -> &'static Mutex<Vec<usize>> {
    OPEN_STREAMS.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn trace_event(arguments: std::fmt::Arguments<'_>) {
    if !trace_enabled() {
        return;
    }
    use std::io::Write;
    let pid = std::process::id();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("stdio-trace-{pid}.log"))
    {
        let _ = writeln!(log, "kinakaze stdio: pid={pid} {arguments}");
    }
}

pub(crate) fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_STDIO_TRACE").is_some())
}

pub(crate) fn is_published(file: *mut File) -> bool {
    open_streams()
        .lock()
        .is_ok_and(|streams| streams.contains(&(file as usize)))
}

/// Leaks a stream and returns the pointer the guest will hold.
fn publish(mut stream: Stream) -> *mut File {
    let fd = stream.fd;
    // Only the ABI control block is guest-owned. The module serializes its
    // private buffers and rebuilds them after fresh DLL loading in a child.
    let file =
        unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<File>()) }.cast::<File>();
    if file.is_null() {
        // fopencookie failure leaves ownership of the caller's cookie intact.
        if stream.cookie.is_none() {
            let _ = stream.close_backend();
        }
        crate::set_errno(kinakaze_vfs::ENOMEM);
        return ptr::null_mut();
    }
    unsafe {
        file.write(File::new_static(fd, stream));
    }
    trace_event(format_args!("publish file={file:p} fd={fd}"));
    if let Ok(mut streams) = open_streams().lock() {
        streams.push(file as usize);
    }
    file
}

// The three standard streams are statics rather than allocations made on first
// use, because a guest that references `stdout` from its main program gets an
// `R_X86_64_COPY` relocation: the loader copies the pointer out of this DLL
// before any of our code has run. A lazily created stream would still be null at
// that moment and the guest would keep the null forever. As statics, the
// exported pointers are constants fixed at link time and are already correct.

/// `stdin`: fully buffered.
static STDIN: File = File::new_static(
    0,
    Stream::new(0, Buffering::Full).with_access(kinakaze_vfs::fs::O_RDONLY),
);

/// `stdout`: line-buffered on a terminal, fully buffered otherwise.
static STDOUT: File = File::new_static(
    1,
    Stream::new(1, Buffering::Undecided).with_access(kinakaze_vfs::fs::O_WRONLY),
);

/// `stderr`: unbuffered, so diagnostics survive a crash.
static STDERR: File = File::new_static(
    2,
    Stream::new(2, Buffering::None).with_access(kinakaze_vfs::fs::O_WRONLY),
);

/// Returns one of the three standard streams, indexed by descriptor number.
fn standard(index: usize) -> *mut File {
    let file: &'static File = match index {
        0 => &STDIN,
        1 => &STDOUT,
        _ => &STDERR,
    };
    // The guest mutates the stream only through the mutex inside it, so handing
    // out a mutable pointer to a shared static is sound.
    ptr::from_ref(file).cast_mut()
}

/// Reports whether `file` is one of the standard streams.
///
/// Those live in static storage, so they must never be handed to `Box::from_raw`.
fn is_standard(file: *mut File) -> bool {
    (0..3).any(|index| standard(index) == file)
}

/// Runs `action` against a stream, or reports `EOF` for a null pointer.
fn with_stream<T>(file: *mut File, fallback: T, action: impl FnOnce(&mut Stream) -> T) -> T {
    if file.is_null() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return fallback;
    }
    // SAFETY: a non-null FILE always came from `publish`, which leaks the
    // allocation, so the reference stays valid for the life of the process.
    let file = unsafe { &*file };
    match file.stream.lock() {
        Ok(mut stream) => {
            let result = action(&mut stream);
            // GNU headers inline feof_unlocked/ferror_unlocked by reading FILE.
            // Publish the same sticky indicators exposed by the function ABI.
            file.publish_indicators(&stream);
            result
        }
        Err(_) => fallback,
    }
}

fn with_oriented<T: Copy>(
    file: *mut File,
    orientation: i32,
    failure: T,
    action: impl FnOnce(&mut Stream) -> T,
) -> T {
    with_stream(file, failure, |stream| {
        if stream.orientation == 0 {
            stream.orientation = orientation;
            stream.wide_utf8 = orientation > 0 && crate::locale::utf8();
            unsafe {
                (*file)
                    ._mode
                    .store(orientation, core::sync::atomic::Ordering::Relaxed);
            }
        }
        if stream.orientation != orientation {
            return failure;
        }
        action(stream)
    })
}

pub(crate) fn reset_after_reopen(file: *mut File, mode: &[u8]) {
    with_stream(file, (), |stream| {
        if let Some((flags, _)) = parse_mode(mode) {
            stream.access = (flags & 3) as u8;
        }
        stream.discard_input();
        stream.eof = false;
        stream.error = false;
        stream.orientation = 0;
        stream.wide_utf8 = false;
        unsafe {
            (*file)
                ._mode
                .store(0, core::sync::atomic::Ordering::Relaxed);
        }
    });
}

/// Flushes every open stream. Called from `exit`.
pub fn flush_all() -> Result<(), i32> {
    let mut first_error = None;
    let mut flush = |file: &File| {
        let result = file
            .stream
            .lock()
            .map_err(|_| kinakaze_vfs::EIO)
            .and_then(|mut stream| stream.flush());
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    };
    // The standard streams are statics and so are not in the published list.
    for index in 0..3 {
        // SAFETY: `standard` returns the address of a live static.
        let file = unsafe { &*standard(index) };
        flush(file);
    }
    let streams = open_streams().lock().map_err(|_| kinakaze_vfs::EIO)?;
    // Callbacks may open or close other streams. Controls remain pinned, so
    // release the registry before executing any callback under a stream lock.
    let handles = streams.clone();
    drop(streams);
    for handle in &handles {
        // SAFETY: handles in this list were leaked by `publish` and stay valid.
        let file = unsafe { &*(*handle as *const File) };
        flush(file);
    }
    first_error.map_or(Ok(()), Err)
}

/// Translates a `fopen` mode string into open flags.
fn parse_mode(mode: &[u8]) -> Option<(c_int, Buffering)> {
    use kinakaze_vfs::fs::{O_APPEND, O_CREAT, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
    let plus = mode.contains(&b'+');
    let flags = match mode.first()? {
        b'r' if plus => O_RDWR,
        b'r' => O_RDONLY,
        b'w' if plus => O_RDWR | O_CREAT | O_TRUNC,
        b'w' => O_WRONLY | O_CREAT | O_TRUNC,
        b'a' if plus => O_RDWR | O_CREAT | O_APPEND,
        b'a' => O_WRONLY | O_CREAT | O_APPEND,
        _ => return None,
    };
    Some((flags, Buffering::Full))
}

/// `fopen`.
///
/// # Safety
///
/// Both arguments must be null-terminated strings.
pub unsafe extern "sysv64" fn fopen(path: *const c_char, mode: *const c_char) -> *mut File {
    if path.is_null() || mode.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees null-terminated strings.
    let path_text = unsafe { core::ffi::CStr::from_ptr(path) };
    // SAFETY: the caller guarantees null-terminated strings.
    let mode_text = unsafe { core::ffi::CStr::from_ptr(mode) };
    let Ok(path_text) = path_text.to_str() else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    };
    let Some((flags, buffering)) = parse_mode(mode_text.to_bytes()) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    };
    match kinakaze_vfs::fs::open(path_text, flags, crate::fsextra::creation_mode(0o666)) {
        Ok(fd) => publish(Stream::new(fd, buffering).with_access(flags)),
        Err(error) => {
            crate::set_errno(error);
            ptr::null_mut()
        }
    }
}

/// `fdopen`, which wraps an existing descriptor.
///
/// # Safety
///
/// `mode` must be a null-terminated string.
pub unsafe extern "sysv64" fn fdopen(fd: c_int, mode: *const c_char) -> *mut File {
    if mode.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    let mode = unsafe { core::ffi::CStr::from_ptr(mode) }.to_bytes();
    let Some((flags, buffering)) = parse_mode(mode) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    };
    use kinakaze_vfs::fs::{O_APPEND, O_RDONLY, O_WRONLY};
    let descriptor_flags = unsafe { crate::fdio::kinakaze_abi_fcntl64(fd, 3, 0) };
    if descriptor_flags < 0 {
        return ptr::null_mut();
    }
    let access = flags & 3;
    if (descriptor_flags & 3 == O_RDONLY && access != O_RDONLY)
        || (descriptor_flags & 3 == O_WRONLY && access != O_WRONLY)
    {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    if flags & O_APPEND != 0 && descriptor_flags & O_APPEND == 0 {
        if unsafe {
            crate::fdio::kinakaze_abi_fcntl64(fd, 4, (descriptor_flags | O_APPEND) as usize)
        } < 0
        {
            return ptr::null_mut();
        }
        if access == O_WRONLY {
            if let Err(error) = kinakaze_vfs::fs::lseek(fd, 0, 2) {
                if error != kinakaze_vfs::ESPIPE {
                    crate::set_errno(error);
                    return ptr::null_mut();
                }
            }
        }
    }
    publish(Stream::new(fd, buffering).with_access(flags))
}

/// `fclose`, which flushes and then closes the descriptor.
///
/// # Safety
///
/// `file` must be a stream from this library that is not used again.
pub unsafe extern "sysv64" fn fclose(file: *mut File) -> c_int {
    trace_event(format_args!("fclose file={file:p}"));
    if file.is_null() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return EOF;
    }
    // Closing a standard stream flushes it and releases the descriptor, but the
    // `FILE` itself is static storage and must not be reclaimed.
    if is_standard(file) {
        return with_stream(file, EOF, |stream| {
            let flushed = stream.flush();
            let closed = stream.close_backend();
            stream.retire();
            match (flushed, closed) {
                (Ok(()), Ok(())) => 0,
                (Err(error), _) | (_, Err(error)) => {
                    crate::set_errno(error);
                    EOF
                }
            }
        });
    }
    // Remove the stream from the flush-all list before releasing it.
    if let Ok(mut streams) = open_streams().lock() {
        streams.retain(|handle| *handle != file as usize);
    }
    // Keep the FILE allocation alive after close. A stream operation can
    // already be queued in the mutex when another thread calls fclose. If the
    // box were reclaimed here, that waiter would wake against freed storage;
    // the Windows heap can reuse it before WaitOnAddress returns, turning the
    // mutex byte into an unrelated allocation and parking the waiter forever.
    //
    // POSIX makes use of a FILE after fclose invalid, so retaining this small
    // control block is invisible to conforming callers. It also makes racy
    // cleanup fail with EBADF instead of becoming a use-after-free.
    let file = unsafe { &*file };
    match file.stream.lock() {
        Ok(mut stream) => {
            // Flush before closing, but close the descriptor either way so a
            // failed flush cannot leak it.
            let flushed = stream.flush();
            let closed = stream.close_backend();
            stream.retire();
            match (flushed, closed) {
                (Ok(()), Ok(())) => 0,
                (Err(error), _) | (_, Err(error)) => {
                    crate::set_errno(error);
                    EOF
                }
            }
        }
        Err(_) => EOF,
    }
}

/// `fflush`. A null argument flushes every stream, as C specifies.
///
/// # Safety
///
/// `file` must be null or a stream from this library.
pub unsafe extern "sysv64" fn fflush(file: *mut File) -> c_int {
    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze stdio: pid {} fflush({file:p})",
            std::process::id()
        );
    }
    if file.is_null() {
        return match flush_all() {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                EOF
            }
        };
    }
    with_stream(file, EOF, |stream| {
        match stream.flush().and_then(|()| stream.sync_input()) {
            Ok(()) => 0,
            Err(error) => {
                crate::set_errno(error);
                EOF
            }
        }
    })
}

/// `fwrite`, which reports the number of complete items written.
///
/// # Safety
///
/// `data` must be readable for `size * count` bytes.
pub unsafe extern "sysv64" fn fwrite(
    data: *const c_void,
    size: usize,
    count: usize,
    file: *mut File,
) -> usize {
    let Some(total) = size.checked_mul(count) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    };
    if total == 0 || data.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees the region.
    let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), total) };
    with_oriented(file, -1, 0, |stream| match stream.write(bytes) {
        // Partial items do not count, per the C contract.
        // A zero item size yields zero items rather than dividing by zero.
        Ok(written) => written.checked_div(size).unwrap_or(0),
        Err(error) => {
            crate::set_errno(error);
            0
        }
    })
}

/// `fread`.
///
/// # Safety
///
/// `data` must be writable for `size * count` bytes.
pub unsafe extern "sysv64" fn fread(
    data: *mut c_void,
    size: usize,
    count: usize,
    file: *mut File,
) -> usize {
    let Some(total) = size.checked_mul(count) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    };
    if total == 0 || data.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees the region.
    let out = unsafe { core::slice::from_raw_parts_mut(data.cast::<u8>(), total) };
    with_oriented(file, -1, 0, |stream| {
        let mut filled = 0;
        // Loop so a short read still fills whole items where possible.
        while filled < total {
            match stream.read(&mut out[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) => {
                    crate::set_errno(error);
                    break;
                }
            }
        }
        filled.checked_div(size).unwrap_or(0)
    })
}

/// `fputc`.
pub extern "sysv64" fn fputc(character: c_int, file: *mut File) -> c_int {
    let byte = character as u8;
    with_oriented(file, -1, EOF, |stream| match stream.write(&[byte]) {
        Ok(1) => character & 0xff,
        Ok(_) => EOF,
        Err(error) => {
            crate::set_errno(error);
            EOF
        }
    })
}

/// `fgetc`.
pub extern "sysv64" fn fgetc(file: *mut File) -> c_int {
    with_oriented(file, -1, EOF, |stream| {
        let mut byte = [0u8; 1];
        match stream.read(&mut byte) {
            Ok(1) => byte[0] as c_int,
            Ok(_) => EOF,
            Err(error) => {
                crate::set_errno(error);
                EOF
            }
        }
    })
}

/// `ungetc`, which pushes one byte back for the next read.
pub extern "sysv64" fn ungetc(character: c_int, file: *mut File) -> c_int {
    if character == EOF {
        return EOF;
    }
    with_oriented(file, -1, EOF, |stream| {
        stream.pushback.push(character as u8);
        // Pushing back clears end-of-file, since a byte is now available.
        stream.eof = false;
        character & 0xff
    })
}

/// `fputs`.
///
/// # Safety
///
/// `text` must be null-terminated.
pub unsafe extern "sysv64" fn fputs(text: *const c_char, file: *mut File) -> c_int {
    if text.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return EOF;
    }
    // SAFETY: the caller guarantees a terminator.
    let length = unsafe { crate::string::strlen(text) };
    // SAFETY: `length` bytes precede the terminator.
    let bytes = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length) };
    with_oriented(file, -1, EOF, |stream| match stream.write(bytes) {
        Ok(written) if written == bytes.len() => 0,
        Ok(_) => EOF,
        Err(error) => {
            crate::set_errno(error);
            EOF
        }
    })
}

/// `fgets`, which stops after a newline and always terminates.
///
/// # Safety
///
/// `buffer` must be writable for `size` bytes.
pub unsafe extern "sysv64" fn fgets(
    buffer: *mut c_char,
    size: c_int,
    file: *mut File,
) -> *mut c_char {
    if buffer.is_null() || size <= 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    let capacity = size as usize;
    let filled = with_oriented(file, -1, None, |stream| {
        let mut written = 0;
        // Consume characters from the input buffer; read-ahead after a newline
        // remains in this FILE for the next stdio operation.
        while written + 1 < capacity {
            let mut byte = [0u8; 1];
            match stream.read(&mut byte) {
                Ok(1) => {
                    // SAFETY: `written` is below `capacity`.
                    unsafe { *buffer.add(written) = byte[0] as c_char };
                    written += 1;
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                Ok(_) => break,
                Err(error) => {
                    crate::set_errno(error);
                    return None;
                }
            }
        }
        Some(written)
    });
    match filled {
        // End of file with nothing read returns null.
        Some(0) | None => ptr::null_mut(),
        Some(written) => {
            // SAFETY: `written` is strictly below `capacity`.
            unsafe { *buffer.add(written) = 0 };
            buffer
        }
    }
}

/// `feof`.
pub extern "sysv64" fn feof(file: *mut File) -> c_int {
    with_stream(file, 0, |stream| c_int::from(stream.eof))
}

/// `ferror`.
pub extern "sysv64" fn ferror(file: *mut File) -> c_int {
    with_stream(file, 0, |stream| c_int::from(stream.error))
}

/// `clearerr`.
pub extern "sysv64" fn clearerr(file: *mut File) {
    with_stream(file, (), |stream| {
        stream.eof = false;
        stream.error = false;
    });
}

/// `fileno`.
pub extern "sysv64" fn fileno(file: *mut File) -> c_int {
    with_stream(file, EOF, |stream| {
        if stream.fd < 0 {
            crate::set_errno(kinakaze_vfs::EBADF);
        }
        stream.fd
    })
}

/// `fseek`, which discards buffered output and pushback first.
pub extern "sysv64" fn fseek(file: *mut File, offset: i64, whence: c_int) -> c_int {
    with_stream(file, EOF, |stream| {
        if let Err(error) = stream.flush() {
            crate::set_errno(error);
            return EOF;
        }
        let offset = if whence == 1 {
            match offset.checked_sub(stream.unread() as i64) {
                Some(offset) => offset,
                None => {
                    crate::set_errno(kinakaze_vfs::EOVERFLOW);
                    return EOF;
                }
            }
        } else {
            offset
        };
        match stream.seek_backend(offset, whence) {
            Ok(_) => {
                stream.discard_input();
                // A successful seek clears end-of-file.
                stream.eof = false;
                0
            }
            Err(error) => {
                crate::set_errno(error);
                EOF
            }
        }
    })
}

/// `ftell`, reporting the position the next read or write would use.
pub extern "sysv64" fn ftell(file: *mut File) -> i64 {
    with_stream(file, -1, |stream| {
        match stream.seek_backend(0, 1) {
            // Buffered output has not reached the descriptor yet, and pushed-back
            // bytes have not been re-read, so both shift the logical position.
            Ok(position) => position as i64 + stream.buffer.len() as i64 - stream.unread() as i64,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        }
    })
}

/// `rewind`.
pub extern "sysv64" fn rewind(file: *mut File) {
    fseek(file, 0, SEEK_SET);
    with_stream(file, (), |stream| {
        stream.eof = false;
        stream.error = false;
    });
}

/// `setvbuf`.
///
/// The caller's buffer is ignored: this implementation owns its buffering, so
/// only the mode is honoured.
pub extern "sysv64" fn setvbuf(
    file: *mut File,
    _buffer: *mut c_char,
    mode: c_int,
    _size: usize,
) -> c_int {
    let buffering = match mode {
        0 => Buffering::Full,
        1 => Buffering::Line,
        2 => Buffering::None,
        _ => return EOF,
    };
    with_stream(file, EOF, |stream| {
        // Switching discipline must not leave bytes stranded in the buffer.
        if let Err(error) = stream.flush().and_then(|()| stream.sync_input()) {
            crate::set_errno(error);
            return EOF;
        }
        stream.buffering = buffering;
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setlinebuf(file: *mut File) {
    let _ = setvbuf(file, core::ptr::null_mut(), 1, 0);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setbuffer(
    file: *mut File,
    buffer: *mut c_char,
    size: usize,
) {
    let _ = setvbuf(file, buffer, if buffer.is_null() { 2 } else { 0 }, size);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn setlinebuf(file: *mut File) {
    unsafe { kinakaze_abi_setlinebuf(file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_open_memstream(
    bufloc: *mut *mut c_char,
    sizeloc: *mut usize,
) -> *mut File {
    unsafe { memory::open(bufloc, sizeloc) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn open_memstream(
    bufloc: *mut *mut c_char,
    sizeloc: *mut usize,
) -> *mut File {
    unsafe { kinakaze_abi_open_memstream(bufloc, sizeloc) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_parse_printf_format(
    template: *const c_char,
    n: usize,
    argtypes: *mut c_int,
) -> usize {
    unsafe { crate::format::parse_types(template, n, argtypes) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn parse_printf_format(
    template: *const c_char,
    n: usize,
    argtypes: *mut c_int,
) -> usize {
    unsafe { kinakaze_abi_parse_printf_format(template, n, argtypes) }
}

/// Buffer ordinary conversions for one stream write; hooks receive the real
/// FILE after preceding bytes are flushed. Guest backing survives callback fork.
struct FileSink {
    file: *mut File,
    data: *mut u8,
    length: usize,
    capacity: usize,
    written: usize,
    error: bool,
}
impl FileSink {
    fn flush(&mut self) {
        if self.error || self.length == 0 {
            return;
        }
        let count = unsafe { fwrite(self.data.cast(), 1, self.length, self.file) };
        if count != self.length {
            self.error = true;
        }
        self.length = 0;
    }
}
impl Drop for FileSink {
    fn drop(&mut self) {
        unsafe { kinakaze_alloc::guest::free(self.data) };
    }
}
impl Sink for FileSink {
    fn write(&mut self, bytes: &[u8]) {
        if self.error || bytes.is_empty() {
            return;
        }
        let Some(end) = self.length.checked_add(bytes.len()) else {
            self.error = true;
            crate::set_errno(kinakaze_vfs::EOVERFLOW);
            return;
        };
        if end > self.capacity {
            let capacity = end.max(self.capacity.saturating_mul(2));
            let data = unsafe { kinakaze_alloc::guest::reallocate(self.data, 16, capacity) };
            if data.is_null() {
                self.error = true;
                crate::set_errno(kinakaze_vfs::ENOMEM);
                return;
            }
            self.data = data;
            self.capacity = capacity;
        }
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.add(self.length), bytes.len())
        };
        self.length = end;
        self.written += bytes.len();
    }
    fn written(&self) -> usize {
        self.written
    }
    fn stream(&mut self) -> Option<*mut File> {
        self.flush();
        Some(self.file)
    }
    fn account(&mut self, count: usize) {
        self.written += count;
    }
}
unsafe fn stream_format(file: *mut File, format: *const c_char, arguments: &mut VaList) -> c_int {
    if !with_oriented(file, -1, false, |_| true) {
        return EOF;
    }
    let mut sink = FileSink {
        file,
        data: ptr::null_mut(),
        length: 0,
        capacity: 0,
        written: 0,
        error: false,
    };
    let result = unsafe { crate::format::format(&mut sink, format, arguments) };
    sink.flush();
    if sink.error { EOF } else { result }
}

/// `vfprintf`.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
pub unsafe extern "sysv64" fn vfprintf(
    file: *mut File,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    if arguments.is_null() {
        return EOF;
    }
    // SAFETY: the caller guarantees a valid va_list.
    unsafe { stream_format(file, format, &mut *arguments) }
}

/// `vsnprintf`.
///
/// # Safety
///
/// `buffer` must be writable for `size` bytes and `arguments` must match
/// `format`.
pub unsafe extern "sysv64" fn vsnprintf(
    buffer: *mut c_char,
    size: usize,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    if arguments.is_null() {
        return EOF;
    }
    // SAFETY: the caller guarantees the buffer and va_list.
    let mut sink = unsafe { BufferSink::new(buffer.cast(), size) };
    // SAFETY: the caller guarantees the format matches the arguments.
    let length = unsafe { crate::format::format(&mut sink, format, &mut *arguments) };
    sink.finish();
    length
}

/// `vsprintf`, which has no bound and is therefore only as safe as its caller.
///
/// # Safety
///
/// `buffer` must be large enough for the entire formatted result.
pub unsafe extern "sysv64" fn vsprintf(
    buffer: *mut c_char,
    format: *const c_char,
    arguments: *mut VaList,
) -> c_int {
    // SAFETY: the caller promises the buffer is unbounded in practice.
    unsafe { vsnprintf(buffer, usize::MAX, format, arguments) }
}

/// Unprefixed System V exports, including the variadic entry points.
pub mod exports {
    use super::*;

    #[unsafe(no_mangle)]
    /// The `stdin` stream.
    pub extern "sysv64" fn kinakaze_stdin() -> *mut File {
        standard(0)
    }

    #[unsafe(no_mangle)]
    /// The `stdout` stream.
    pub extern "sysv64" fn kinakaze_stdout() -> *mut File {
        standard(1)
    }

    #[unsafe(no_mangle)]
    /// The `stderr` stream.
    pub extern "sysv64" fn kinakaze_stderr() -> *mut File {
        standard(2)
    }

    // glibc declares the standard streams as `extern FILE *stdout;`, so compiled
    // guest code loads the pointer out of a data symbol and never calls an
    // accessor. These are that data symbol. They hold the addresses of the same
    // statics `kinakaze_stdout` and friends return: two independent `FILE`s on
    // one descriptor would each buffer part of the output and interleave it.
    //
    // The type is an atomic pointer so the storage lands in a writable section,
    // which a guest reassigning `stdout` requires. Its layout is a bare pointer,
    // which is what the guest sees.

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    /// The `stdin` data symbol.
    pub static kinakaze_abi_stdin: StreamPointer = StreamPointer::new(&STDIN);

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    pub static stdin: StreamPointer = StreamPointer::new(&STDIN);

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    /// The `stdout` data symbol.
    pub static kinakaze_abi_stdout: StreamPointer = StreamPointer::new(&STDOUT);

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    pub static stdout: StreamPointer = StreamPointer::new(&STDOUT);

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    /// The `stderr` data symbol.
    pub static kinakaze_abi_stderr: StreamPointer = StreamPointer::new(&STDERR);

    #[unsafe(no_mangle)]
    #[allow(non_upper_case_globals)]
    pub static stderr: StreamPointer = StreamPointer::new(&STDERR);

    // Rust cannot declare `extern "sysv64"` variadic functions, so `printf` and
    // friends are assembly thunks in `crate::variadic` that build the System V
    // `va_list` and tail-call these implementations.

    #[unsafe(no_mangle)]
    /// Implementation behind the `printf` assembly thunk.
    ///
    /// # Safety
    ///
    /// `format` must be null-terminated and `arguments` must match it.
    pub unsafe extern "sysv64" fn kinakaze_printf_impl(
        format: *const c_char,
        arguments: *mut VaList,
    ) -> c_int {
        if arguments.is_null() {
            return EOF;
        }
        // SAFETY: the thunk passes a live va_list it just constructed.
        unsafe { stream_format(standard(1), format, &mut *arguments) }
    }

    #[unsafe(no_mangle)]
    /// Implementation behind the `fprintf` assembly thunk.
    ///
    /// # Safety
    ///
    /// `format` must be null-terminated and `arguments` must match it.
    pub unsafe extern "sysv64" fn kinakaze_fprintf_impl(
        file: *mut File,
        format: *const c_char,
        arguments: *mut VaList,
    ) -> c_int {
        if arguments.is_null() {
            return EOF;
        }
        // SAFETY: the thunk passes a live va_list it just constructed.
        unsafe { stream_format(file, format, &mut *arguments) }
    }

    #[unsafe(no_mangle)]
    /// Implementation behind the `snprintf` assembly thunk.
    ///
    /// # Safety
    ///
    /// `buffer` must be writable for `size` bytes and `arguments` must match
    /// `format`.
    pub unsafe extern "sysv64" fn kinakaze_snprintf_impl(
        buffer: *mut c_char,
        size: usize,
        format: *const c_char,
        arguments: *mut VaList,
    ) -> c_int {
        // SAFETY: the thunk passes a live va_list it just constructed.
        unsafe { vsnprintf(buffer, size, format, arguments) }
    }

    #[unsafe(no_mangle)]
    /// Implementation behind the `sprintf` assembly thunk.
    ///
    /// # Safety
    ///
    /// `buffer` must be large enough for the entire formatted result.
    pub unsafe extern "sysv64" fn kinakaze_sprintf_impl(
        buffer: *mut c_char,
        format: *const c_char,
        arguments: *mut VaList,
    ) -> c_int {
        // SAFETY: the thunk passes a live va_list it just constructed.
        unsafe { vsprintf(buffer, format, arguments) }
    }

    macro_rules! stdio_export {
        ($alias:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            stdio_export!($alias, $name, $name ( $($argument: $type),* ) -> $result);
        };
        ($alias:ident, $bare:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            #[unsafe(no_mangle)]
            /// System V ABI export. See the wrapped function for the contract.
            ///
            /// # Safety
            ///
            /// Pointer arguments must satisfy the wrapped function's contract.
            pub unsafe extern "sysv64" fn $alias($($argument: $type),*) -> $result {
                // SAFETY: forwarded from this shim's own contract.
                unsafe { super::$name($($argument),*) }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "sysv64" fn $bare($($argument: $type),*) -> $result {
                unsafe { super::$name($($argument),*) }
            }
        };
    }

    macro_rules! stdio_export_safe {
        ($alias:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            stdio_export_safe!($alias, $name, $name ( $($argument: $type),* ) -> $result);
        };
        ($alias:ident, $bare:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            #[unsafe(no_mangle)]
            /// System V ABI export. See the wrapped function for behaviour.
            pub extern "sysv64" fn $alias($($argument: $type),*) -> $result {
                super::$name($($argument),*)
            }

            #[unsafe(no_mangle)]
            pub extern "sysv64" fn $bare($($argument: $type),*) -> $result {
                super::$name($($argument),*)
            }
        };
    }

    stdio_export!(kinakaze_abi_fopen, fopen(path: *const c_char, mode: *const c_char) -> *mut File);
    stdio_export!(kinakaze_abi_fdopen, fdopen(fd: c_int, mode: *const c_char) -> *mut File);
    stdio_export!(kinakaze_abi_fclose, fclose(file: *mut File) -> c_int);
    stdio_export!(kinakaze_abi_fflush, fflush(file: *mut File) -> c_int);
    stdio_export!(kinakaze_abi_fwrite, fwrite(data: *const c_void, size: usize, count: usize, file: *mut File) -> usize);
    stdio_export!(kinakaze_abi_fread, fread(data: *mut c_void, size: usize, count: usize, file: *mut File) -> usize);
    stdio_export!(kinakaze_abi_fputs, fputs(text: *const c_char, file: *mut File) -> c_int);
    stdio_export!(kinakaze_abi_fgets, fgets(buffer: *mut c_char, size: c_int, file: *mut File) -> *mut c_char);
    stdio_export!(kinakaze_abi_vfprintf, vfprintf(file: *mut File, format: *const c_char, arguments: *mut VaList) -> c_int);
    stdio_export!(kinakaze_abi_vsnprintf, vsnprintf(buffer: *mut c_char, size: usize, format: *const c_char, arguments: *mut VaList) -> c_int);
    stdio_export!(kinakaze_abi_vsprintf, vsprintf(buffer: *mut c_char, format: *const c_char, arguments: *mut VaList) -> c_int);
    stdio_export_safe!(kinakaze_abi_fputc, fputc(character: c_int, file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_fgetc, fgetc(file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_getc, getc, fgetc(file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_ungetc, ungetc(character: c_int, file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_feof, feof(file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_ferror, ferror(file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_clearerr, clearerr(file: *mut File) -> ());
    stdio_export_safe!(kinakaze_abi_fileno, fileno(file: *mut File) -> c_int);
    stdio_export_safe!(kinakaze_abi_fseek, fseek(file: *mut File, offset: i64, whence: c_int) -> c_int);
    stdio_export_safe!(kinakaze_abi_fseeko, fseeko, fseek(file: *mut File, offset: i64, whence: c_int) -> c_int);
    stdio_export_safe!(kinakaze_abi_ftell, ftell(file: *mut File) -> i64);
    stdio_export_safe!(kinakaze_abi_ftello, ftello, ftell(file: *mut File) -> i64);
    stdio_export_safe!(kinakaze_abi_rewind, rewind(file: *mut File) -> ());
    stdio_export_safe!(kinakaze_abi_setvbuf, setvbuf(file: *mut File, buffer: *mut c_char, mode: c_int, size: usize) -> c_int);

    #[unsafe(no_mangle)]
    /// `putchar`, defined as `fputc` on stdout.
    pub extern "sysv64" fn kinakaze_abi_putchar(character: c_int) -> c_int {
        super::fputc(character, standard(1))
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn putchar(character: c_int) -> c_int {
        super::fputc(character, standard(1))
    }

    #[unsafe(no_mangle)]
    /// `puts`, which appends a newline unlike `fputs`.
    ///
    /// # Safety
    ///
    /// `text` must be null-terminated.
    pub unsafe extern "sysv64" fn kinakaze_abi_puts(text: *const c_char) -> c_int {
        // SAFETY: forwarded from this function's contract.
        if unsafe { super::fputs(text, standard(1)) } == EOF {
            return EOF;
        }
        super::fputc(b'\n' as c_int, standard(1))
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn puts(text: *const c_char) -> c_int {
        kinakaze_abi_puts(text)
    }

    #[unsafe(no_mangle)]
    /// `getchar`.
    pub extern "sysv64" fn kinakaze_abi_getchar() -> c_int {
        super::fgetc(standard(0))
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn getchar() -> c_int {
        super::fgetc(standard(0))
    }

    #[unsafe(no_mangle)]
    /// `perror`, which writes a message and the current errno text to stderr.
    ///
    /// # Safety
    ///
    /// `prefix` must be null or null-terminated.
    pub unsafe extern "sysv64" fn kinakaze_abi_perror(prefix: *const c_char) {
        let err_stream = standard(2);
        if !prefix.is_null() {
            // SAFETY: forwarded from this function's contract.
            unsafe { super::fputs(prefix, err_stream) };
            // SAFETY: a literal with a terminator.
            unsafe { super::fputs(c": ".as_ptr(), err_stream) };
        }
        let message = crate::string::strerror(kinakaze_tls::errno());
        // SAFETY: `strerror` returns a null-terminated static string.
        unsafe { super::fputs(message, err_stream) };
        super::fputc(b'\n' as c_int, err_stream);
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn perror(prefix: *const c_char) {
        unsafe { kinakaze_abi_perror(prefix) };
    }

    #[unsafe(no_mangle)]
    /// `remove`, which deletes a file or an empty directory.
    ///
    /// # Safety
    ///
    /// `path` must be null-terminated.
    pub unsafe extern "sysv64" fn kinakaze_abi_remove(path: *const c_char) -> c_int {
        // SAFETY: forwarded from this function's contract.
        let result = unsafe { crate::fs::unlink(path) };
        if result == 0 {
            return 0;
        }
        // C's `remove` also removes directories.
        // SAFETY: forwarded from this function's contract.
        unsafe { crate::fs::rmdir(path) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn remove(path: *const c_char) -> c_int {
        kinakaze_abi_remove(path)
    }

    /// `putc` — alias for `fputc`.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_putc(character: c_int, file: *mut File) -> c_int {
        super::fputc(character, file)
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn putc(character: c_int, file: *mut File) -> c_int {
        super::fputc(character, file)
    }

    /// `getwc` — the same wide input operation as `fgetwc`.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_getwc(file: *mut File) -> i32 {
        wide::kinakaze_abi_fgetwc(file) as i32
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn getwc(file: *mut File) -> i32 {
        kinakaze_abi_getwc(file)
    }

    /// `putwc` — the same wide output operation as `fputwc`.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_putwc(wc: i32, file: *mut File) -> i32 {
        wide::kinakaze_abi_fputwc(wc as u32, file) as i32
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn putwc(wc: i32, file: *mut File) -> i32 {
        kinakaze_abi_putwc(wc, file)
    }

    /// `ungetwc` — pushes one complete encoded scalar back onto the stream.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_ungetwc(wc: i32, file: *mut File) -> i32 {
        wide::ungetwc(wc as u32, file) as i32
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn ungetwc(wc: i32, file: *mut File) -> i32 {
        kinakaze_abi_ungetwc(wc, file)
    }

    /// `fwprintf` — wide char fprintf stub, returns -1 always.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_fwprintf(_file: *mut File, _fmt: *const i32) -> c_int {
        -1
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn fwprintf(_file: *mut File, _fmt: *const i32) -> c_int {
        -1
    }

    /// `tmpfile` — creates a temporary file opened for reading and writing.
    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_tmpfile() -> *mut File {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = format!("/tmp/tmpfile_{}_{}", std::process::id(), id);
        let flags = kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_CREAT | kinakaze_vfs::fs::O_EXCL;
        match kinakaze_vfs::fs::open(&path, flags, 0o600) {
            Ok(fd) => {
                let _ = kinakaze_vfs::fs::unlink(&path);
                publish(Stream::new(fd, Buffering::Full))
            }
            Err(e) => {
                crate::set_errno(e);
                core::ptr::null_mut()
            }
        }
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn tmpfile() -> *mut File {
        kinakaze_abi_tmpfile()
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn kinakaze_abi_tmpfile64() -> *mut File {
        kinakaze_abi_tmpfile()
    }

    #[unsafe(no_mangle)]
    pub extern "sysv64" fn tmpfile64() -> *mut File {
        kinakaze_abi_tmpfile()
    }

    /// `mkstemp` — creates a temp file with a pattern, returns fd.
    ///
    /// # Safety
    /// `pattern` must be writable and end in `XXXXXX`.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_mkstemp(pattern: *mut c_char) -> c_int {
        unsafe { crate::fdio::kinakaze_abi_mkstemp64(pattern) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn mkstemp(pattern: *mut c_char) -> c_int {
        kinakaze_abi_mkstemp(pattern)
    }

    /// `__fpending` — returns pending buffered bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __fpending(file: *mut File) -> usize {
        kinakaze_abi___fpending(file)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___fpending(file: *mut File) -> usize {
        if file.is_null() {
            return 0;
        }
        let Ok(stream) = (unsafe { (*file).stream.lock() }) else {
            return 0;
        };
        stream.buffer.len()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_fread_unlocked(
        data: *mut c_void,
        size: usize,
        count: usize,
        file: *mut File,
    ) -> usize {
        unsafe { super::fread(data, size, count, file) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_vwarn(format: *const c_char, args: *mut VaList) {
        let error = kinakaze_tls::errno();
        unsafe {
            super::gnu_error::warn(error, format, args);
        }
        crate::set_errno(error);
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_vwarnx(format: *const c_char, args: *mut VaList) {
        let error = kinakaze_tls::errno();
        unsafe {
            super::gnu_error::warn(0, format, args);
        }
        crate::set_errno(error);
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_verr(
        status: c_int,
        format: *const c_char,
        args: *mut VaList,
    ) -> ! {
        unsafe {
            kinakaze_abi_vwarn(format, args);
        }
        crate::process::kinakaze_abi_exit(status)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_verrx(
        status: c_int,
        format: *const c_char,
        args: *mut VaList,
    ) -> ! {
        unsafe {
            kinakaze_abi_vwarnx(format, args);
        }
        crate::process::kinakaze_abi_exit(status)
    }

    /// `fwrite_unlocked`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn fwrite_unlocked(
        data: *const c_void,
        size: usize,
        count: usize,
        file: *mut File,
    ) -> usize {
        unsafe { super::fwrite(data, size, count, file) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_fwrite_unlocked(
        data: *const c_void,
        size: usize,
        count: usize,
        file: *mut File,
    ) -> usize {
        unsafe { super::fwrite(data, size, count, file) }
    }

    /// `fflush_unlocked`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn fflush_unlocked(file: *mut File) -> c_int {
        super::fflush(file)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi_fflush_unlocked(file: *mut File) -> c_int {
        super::fflush(file)
    }

    /// `__overflow`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __overflow(file: *mut File, ch: c_int) -> c_int {
        super::fputc(ch, file)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___overflow(file: *mut File, ch: c_int) -> c_int {
        super::fputc(ch, file)
    }

    /// `__freading`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __freading(file: *mut File) -> c_int {
        unsafe { kinakaze_abi___freading(file) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___freading(_file: *mut File) -> c_int {
        1
    }

    /// `__fwriting`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __fwriting(file: *mut File) -> c_int {
        unsafe { kinakaze_abi___fwriting(file) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___fwriting(_file: *mut File) -> c_int {
        0
    }

    /// `__fpurge`
    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __fpurge(file: *mut File) {
        unsafe { kinakaze_abi___fpurge(file) };
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___fpurge(file: *mut File) {
        if file.is_null() {
            return;
        }
        if let Ok(mut stream) = unsafe { (*file).stream.lock() } {
            stream.buffer.clear();
            stream.discard_input();
        }
    }
}

#[cfg(test)]
#[path = "stdio_input_tests.rs"]
mod input_tests;

#[cfg(test)]
mod tests {
    use super::exports::{
        kinakaze_abi_stderr, kinakaze_abi_stdin, kinakaze_abi_stdout, kinakaze_stderr,
        kinakaze_stdin, kinakaze_stdout,
    };
    use super::*;

    /// The data symbols and the accessors must name one object per descriptor.
    ///
    /// Two `FILE`s on one descriptor would each hold half of the program's output
    /// in their own buffer and flush them in whatever order the buffers filled,
    /// which interleaves the output without any error being reported.
    #[test]
    fn stream_data_symbols_match_the_accessors() {
        assert_eq!(kinakaze_abi_stdin.get(), kinakaze_stdin());
        assert_eq!(kinakaze_abi_stdout.get(), kinakaze_stdout());
        assert_eq!(kinakaze_abi_stderr.get(), kinakaze_stderr());
    }

    /// The three streams must be distinct objects on their own descriptors.
    #[test]
    fn the_standard_streams_are_distinct() {
        let streams = [kinakaze_stdin(), kinakaze_stdout(), kinakaze_stderr()];
        assert_ne!(streams[0], streams[1]);
        assert_ne!(streams[1], streams[2]);
        assert_ne!(streams[0], streams[2]);
        for (expected, stream) in streams.iter().enumerate() {
            // SAFETY: the accessors return the addresses of live statics.
            let file = unsafe { &**stream };
            let fd = file.stream.lock().expect("stream lock").fd;
            assert_eq!(fd, expected as c_int);
        }
    }

    /// A standard stream is static storage and must survive being closed.
    #[test]
    fn is_standard_recognizes_the_static_streams() {
        assert!(is_standard(kinakaze_stdout()));
        assert!(!is_standard(ptr::null_mut()));
    }

    /// Closing a dynamic stream retires its descriptor without reclaiming the
    /// mutex storage that an already queued stream operation may still use.
    #[test]
    fn fclose_keeps_the_control_block_alive_for_waiters() {
        let mut state = Stream::new(-1, Buffering::Full);
        state.input = vec![0; BUFSIZ];
        state.buffer = vec![0; BUFSIZ];
        let file = publish(state);
        assert_eq!(unsafe { fclose(file) }, EOF);

        // SAFETY: fclose deliberately retains dynamic FILE control blocks.
        let stream = unsafe { &*file }.stream.lock().expect("closed stream lock");
        assert_eq!(stream.fd, -1);
        assert!(stream.eof);
        assert_eq!(stream.input.capacity(), 0);
        assert_eq!(stream.buffer.capacity(), 0);
    }

    /// Verifies that `File` matches glibc's `_IO_FILE` structure offsets on x86_64.
    /// Inlined glibc reader macros rely on these exact field offsets to determine
    /// whether a fast buffered read is possible or if `fgetc` / `getc_unlocked` must be called.
    #[test]
    fn glibc_io_file_layout_matches_x86_64_abi() {
        use std::mem::offset_of;
        assert_eq!(offset_of!(File, _flags), 0);
        assert_eq!(offset_of!(File, _IO_read_ptr), 8);
        assert_eq!(offset_of!(File, _IO_read_end), 16);
        assert_eq!(offset_of!(File, _IO_read_base), 24);
        assert_eq!(offset_of!(File, _IO_write_base), 32);
        assert_eq!(offset_of!(File, _IO_write_ptr), 40);
        assert_eq!(offset_of!(File, _IO_write_end), 48);
        assert_eq!(offset_of!(File, _IO_buf_base), 56);
        assert_eq!(offset_of!(File, _IO_buf_end), 64);
        assert_eq!(offset_of!(File, _fileno), 112);
        assert_eq!(offset_of!(File, stream), 216);

        // Verify standard streams have empty read buffers so inlined glibc checks fall back to fgetc
        for stream in [kinakaze_stdin(), kinakaze_stdout(), kinakaze_stderr()] {
            let file = unsafe { &*stream };
            assert_eq!(file._IO_read_ptr, std::ptr::null_mut());
            assert_eq!(file._IO_read_end, std::ptr::null_mut());
            // _IO_read_ptr >= _IO_read_end is true (null >= null), triggering underflow / fgetc fallback
            assert!(file._IO_read_ptr >= file._IO_read_end);
        }
    }
}

pub(crate) mod gnu_error;
