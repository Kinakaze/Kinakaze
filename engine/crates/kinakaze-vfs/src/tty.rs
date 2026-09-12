//! Real pseudo-terminals, and the software line discipline the console runs on.
//!
//! The pty a guest sees here is not a ConPTY, and it cannot be one. A ConPTY
//! master is *two* pipe handles rather than one file, and its slave never exists
//! as a handle in this process at all — `CreatePseudoConsole` gives it to conhost
//! and [`crate::pty::open_pty`] closes this side's copy immediately. There is no
//! object to install as `/dev/pts/N`.
//!
//! So a terminal is assembled here out of parts Windows does provide:
//!
//! * A **named file mapping** holds the whole terminal: `termios`, both byte
//!   queues, the window size, the foreground process group. Shared memory rather
//!   than a process-local `Mutex` because a pty routinely spans processes —
//!   `posix_openpt` in a shell, `login_tty` in a forked child, then `execve`
//!   into a program that is a *different Windows process* and inherits only
//!   handles. A heap-allocated terminal would stop existing at that exec.
//! * A **named mutex** serialises access, so [`crate::ldisc`] runs in whichever
//!   process is doing the I/O and every process sees one terminal.
//! * Two **named manual-reset events** publish readiness, one per direction.
//!   Each descriptor's `raw` handle *is* one of those events, which gives `poll`
//!   a real object to wait on and — because a named object keeps its name
//!   through `DuplicateHandle` and process inheritance — lets an inherited
//!   descriptor be recognised as a pty end with no side table to keep in step.
//! * Two **named events** track liveness. Every process holding descriptors on a
//!   side owns one instance, and Windows releases it when that process exits
//!   however it exits. That is what makes "the last slave closed" observable
//!   after a crash, which a reference count in shared memory would not be.
//!
//! ## Where this still falls short of Linux
//!
//! * Explicit devpts mounts have independent index spaces. The built-in
//!   compatibility `/dev/pts` uses a separate bounded index space.
//! * `TIOCSTI` is implemented for the caller's own terminal only, because the
//!   check it needs — "is this the caller's controlling terminal, or is the
//!   caller privileged" — has no Windows counterpart for the second half.
//! * A process killed with `TerminateProcess` releases its liveness instance,
//!   so hangup is still observed; but it does not run [`release_console`], so a
//!   raw-mode console is left raw. `_exit` has the same hole.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, PIPE_ACCESS_DUPLEX, ReadFile, SYNCHRONIZE, WIN32_FIND_DATAW,
    WriteFile,
};
use windows_sys::Win32::System::Console::{
    ENABLE_EXTENDED_FLAGS, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, SetConsoleMode,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS,
    MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, ReleaseMutex, ResetEvent,
    SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::ldisc::{self, Ldisc, Queue, Read as LdiscRead, ReadPolicy};
use crate::pty::{ICANON, TOSTOP, Termios, WinSize};
use crate::{EAGAIN, EBUSY, EINTR, EINVAL, EIO, ENOENT, ENOTTY, FdFlags, FdKind};

/// Highest pseudo-terminal number this layer allocates.
///
/// Linux's default is 4096 and the number is policy rather than structure. It is
/// lower here because allocating one probes the name of every candidate, and a
/// terminal count in the hundreds is already far past what a hosted guest opens.
pub const MAX_PTYS: u32 = 256;

/// Identifies a mapped block as this layout at this version.
const SHARED_MAGIC: u64 = 0x5952_4300_5054_5932;

/// `TIOCSPTLCK`: the slave may not be opened until `unlockpt` clears this.
const FLAG_LOCKED: u32 = 1 << 0;
/// `TIOCEXCL`: further opens of the slave are refused.
const FLAG_EXCLUSIVE: u32 = 1 << 1;
/// `TIOCPKT`: master reads are prefixed with a status byte.
const FLAG_PACKET: u32 = 1 << 2;
/// The master has gone, so slaves see a hangup.
const FLAG_HUNGUP: u32 = 1 << 3;
/// A master descriptor has existed at some point.
const FLAG_MASTER_SEEN: u32 = 1 << 4;
/// A slave descriptor has existed at some point.
const FLAG_SLAVE_SEEN: u32 = 1 << 5;

// `TIOCPKT` status bits, as the master observes them.
pub const TIOCPKT_DATA: u8 = 0x00;
pub const TIOCPKT_FLUSHREAD: u8 = 0x01;
pub const TIOCPKT_FLUSHWRITE: u8 = 0x02;
pub const TIOCPKT_STOP: u8 = 0x04;
pub const TIOCPKT_START: u8 = 0x08;
pub const TIOCPKT_NOSTOP: u8 = 0x10;
pub const TIOCPKT_DOSTOP: u8 = 0x20;

/// `ESRCH`, which the crate's errno list does not otherwise name.
#[allow(dead_code)]
const ESRCH: i32 = crate::job::ESRCH;
/// `SIGTTIN`, raised when a background process reads its controlling terminal.
pub const SIGTTIN: i32 = crate::signal::SIGTTIN;
/// `SIGTTOU`, raised when a background process writes one under `TOSTOP`.
pub const SIGTTOU: i32 = crate::signal::SIGTTOU;

/// The complete state of one pseudo-terminal, as it lives in shared memory.
///
/// `repr(C)` is not cosmetic: two builds of this DLL could map the same block,
/// so the layout is a contract with itself. [`SHARED_MAGIC`] is the version
/// check that keeps a mismatched layout from being read as data.
#[repr(C)]
struct Shared {
    magic: u64,
    devpts: u64,
    inode: u64,
    mount_namespace: u64,
    mount_id: u64,
    mount_flags: u64,
    number: u32,
    uid: u32,
    gid: u32,
    mode: u32,
    flags: u32,
    packet_status: u32,
    foreground: i32,
    session: i32,
    ldisc_number: i32,
    column: u32,
    canon_column: u32,
    pending: u32,
    line_count: u32,
    input_len: u32,
    output_len: u32,
    stopped: u8,
    throttled: u8,
    literal: u8,
    erase_printing: u8,
    termios: Termios,
    winsize: WinSize,
    /// Lengths of the complete canonical lines, in order.
    ///
    /// Sized so it can never overflow: every line holds at least one byte, so
    /// there can never be more lines than the input buffer has room for.
    lines: [u16; ldisc::INPUT_LIMIT],
    input: [u8; ldisc::INPUT_LIMIT],
    output: [u8; ldisc::OUTPUT_LIMIT],
}

impl Shared {
    /// Whether the master has gone away, which slaves must report as `EIO`.
    fn hangup_pending(&self) -> bool {
        self.flags & FLAG_HUNGUP != 0
    }
}

/// Which end of a pseudo-terminal a descriptor refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Master,
    Slave,
}

impl Side {
    /// The one-letter suffix that distinguishes the two ends' named objects.
    fn suffix(self) -> &'static str {
        match self {
            Self::Master => "m",
            Self::Slave => "s",
        }
    }
}

/// A handle this module owns and must close.
struct Owned(HANDLE);

// SAFETY: every handle stored here is a process-wide kernel object — a section,
// a mutex, an event or a pipe instance — and Win32 accepts all of them from any
// thread. None is thread-affine.
unsafe impl Send for Owned {}
// SAFETY: as above; the handle is only ever read out of this wrapper.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: this type owns the handle it was constructed from.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// This process's attachment to one pseudo-terminal.
///
/// Attachments are cached per process and shared by every descriptor on either
/// end, which keeps one mapping and one mutex per terminal however many
/// descriptors point at it.
pub struct Attachment {
    index: u32,
    /// Kept so the mapping outlives every view of it.
    _section: Owned,
    view: *mut Shared,
    lock: Owned,
    master_event: Owned,
    slave_event: Owned,
    /// This process's liveness pipe instance for the master side, if any.
    ///
    /// A raw value rather than an `Owned` so it can be claimed and released
    /// without taking the attachment apart.
    master_liveness: AtomicUsize,
    slave_liveness: AtomicUsize,
}

// SAFETY: the view points into a file mapping that lives as long as the
// attachment, and every access to it is made under `lock`.
unsafe impl Send for Attachment {}
// SAFETY: as above.
unsafe impl Sync for Attachment {}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.release_liveness(Side::Master);
        self.release_liveness(Side::Slave);
        if !self.view.is_null() {
            // SAFETY: the view came from `MapViewOfFile` and is not aliased once
            // the attachment is being dropped.
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view.cast(),
                })
            };
        }
    }
}

/// Builds the name of one of a terminal's named objects.
///
/// `Local\` scopes the name to the logon session, which is the same boundary a
/// `/dev/pts` namespace has on Linux: two users on one machine get separate
/// terminals with the same number rather than sharing one.
fn object_name(index: u32, suffix: &str) -> Vec<u16> {
    let name = if suffix.is_empty() {
        format!(
            "Local\\kinakaze.pts.v2.{}.{index}",
            kinakaze_runtime::authority::domain_id()
        )
    } else {
        format!(
            "Local\\kinakaze.pts.v2.{}.{index}.{suffix}",
            kinakaze_runtime::authority::domain_id()
        )
    };
    name.encode_utf16().chain(Some(0)).collect()
}

/// Builds the name of a side's liveness event object.
fn liveness_name(index: u32, side: Side) -> Vec<u16> {
    format!(
        "Local\\kinakaze.pts.v2.{}.{index}.{}_live",
        kinakaze_runtime::authority::domain_id(),
        side.suffix()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect()
}

/// Auxiliary handles stay private so unrelated helper processes cannot pin
/// terminals or keep a closed side alive. Descriptor readiness handles alone
/// travel across fork/exec; restore attaches before the parent handoff completes.
fn private_security() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 0,
    }
}

fn last_errno() -> i32 {
    // SAFETY: GetLastError has no preconditions.
    crate::errno_from_win32(unsafe { GetLastError() })
}

impl Attachment {
    /// Creates the terminal numbered `index`, or reports that it exists.
    fn create(index: u32) -> Result<Self, i32> {
        let mut security = private_security();
        let name = object_name(index, "");
        // SAFETY: the name is a null-terminated wide string and the size is the
        // mapping's own; INVALID_HANDLE_VALUE selects a pagefile-backed section.
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &raw mut security,
                PAGE_READWRITE,
                0,
                size_of::<Shared>() as u32,
                name.as_ptr(),
            )
        };
        if section.is_null() {
            return Err(last_errno());
        }
        // SAFETY: GetLastError has no preconditions.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // The number is taken. Releasing rather than adopting is deliberate:
            // adopting would silently join someone else's terminal.
            // SAFETY: the handle was just created here and is not stored.
            unsafe { CloseHandle(section) };
            return Err(crate::EEXIST);
        }
        Self::adopt(index, section, true)
    }

    /// Attaches to an existing terminal.
    fn open(index: u32) -> Result<Self, i32> {
        let name = object_name(index, "");
        // SAFETY: the name is a null-terminated wide string.
        let section = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
        if section.is_null() {
            return Err(ENOENT);
        }
        Self::adopt(index, section, false)
    }

    /// Maps a section and opens the terminal's mutex and events.
    fn adopt(index: u32, section: HANDLE, fresh: bool) -> Result<Self, i32> {
        // SAFETY: the section was created or opened with full access and is at
        // least `size_of::<Shared>()` bytes.
        let mapped =
            unsafe { MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<Shared>()) };
        if mapped.Value.is_null() {
            // SAFETY: the section handle is owned here and not stored.
            unsafe { CloseHandle(section) };
            return Err(EIO);
        }
        let view = mapped.Value.cast::<Shared>();

        let mut security = private_security();
        let lock_name = object_name(index, "k");
        // SAFETY: the name is a null-terminated wide string; ownership is not
        // requested, so the mutex starts unheld.
        let lock = unsafe { CreateMutexW(&raw mut security, 0, lock_name.as_ptr()) };
        let master_name = object_name(index, "m");
        let slave_name = object_name(index, "s");
        // Manual reset, because readiness is a level and not an edge: a second
        // waiter must see the same state the first one did.
        // SAFETY: both names are null-terminated wide strings.
        let master_event = unsafe { CreateEventW(&raw mut security, 1, 0, master_name.as_ptr()) };
        // SAFETY: as above.
        let slave_event = unsafe { CreateEventW(&raw mut security, 1, 0, slave_name.as_ptr()) };
        if lock.is_null() || master_event.is_null() || slave_event.is_null() {
            // SAFETY: each handle is either null or owned here and unpublished.
            unsafe {
                UnmapViewOfFile(mapped);
                CloseHandle(section);
                CloseHandle(lock);
                CloseHandle(master_event);
                CloseHandle(slave_event);
            }
            return Err(EIO);
        }

        let attachment = Self {
            index,
            _section: Owned(section),
            view,
            lock: Owned(lock),
            master_event: Owned(master_event),
            slave_event: Owned(slave_event),
            master_liveness: AtomicUsize::new(0),
            slave_liveness: AtomicUsize::new(0),
        };
        if fresh {
            attachment.initialize();
        }
        Ok(attachment)
    }

    /// Writes the initial terminal state into a freshly created mapping.
    ///
    /// A new mapping is already zeroed by the kernel, so only the fields whose
    /// correct value is not zero are written.
    fn initialize(&self) {
        let _guard = self.acquire();
        // SAFETY: the view is a live mapping of at least `Shared` bytes and the
        // mutex is held for the duration of the borrow.
        let shared = unsafe { &mut *self.view };
        shared.magic = SHARED_MAGIC;
        shared.number = self.index;
        let c = crate::credentials::filesystem();
        shared.uid = c.uid;
        shared.gid = 5;
        shared.mode = 0o620;
        shared.termios = ldisc::default_termios();
        // Linux hands out a locked slave: `unlockpt` is what a caller must run
        // before opening it, and a terminal that skipped the lock would let a
        // racing open reach a slave the master has not finished configuring.
        shared.flags = FLAG_LOCKED;
        shared.ldisc_number = ldisc::N_TTY;
        // 24x80 is what a terminal with no better information reports; zero
        // would tell a full-screen program it has no screen.
        shared.winsize = WinSize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
    }

    /// Takes the terminal's mutex, returning a guard that releases it.
    fn acquire(&self) -> LockGuard<'_> {
        // SAFETY: the mutex handle is live for the attachment's lifetime.
        let waited = unsafe { WaitForSingleObject(self.lock.0, INFINITE) };
        // WAIT_ABANDONED means the previous owner died holding the mutex. The
        // state may be torn, but the alternative is refusing to touch the
        // terminal ever again; Windows hands ownership over and this proceeds,
        // which is how a kernel treats a task killed inside a driver.
        debug_assert!(waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED);
        LockGuard { attachment: self }
    }

    /// Runs `operation` against the terminal state under its lock.
    ///
    /// Readiness is republished on the way out, unconditionally: every path that
    /// changes a queue changes readiness, and recomputing it in one place is
    /// what keeps a `poll` waiter from sleeping through a wakeup.
    fn with<R>(&self, operation: impl FnOnce(&mut Shared) -> R) -> R {
        let _guard = self.acquire();
        // SAFETY: the view is live and the mutex is held.
        let shared = unsafe { &mut *self.view };
        let result = operation(shared);
        self.publish(shared);
        result
    }

    /// Signals or clears the two readiness events to match the current state.
    ///
    /// Hangup is derived here rather than recorded by whoever closed the master,
    /// because a process that is killed never gets to record anything. The
    /// liveness pipes answer the question the way a kernel would: the object is
    /// gone, therefore the side is gone.
    fn publish(&self, shared: &mut Shared) {
        if shared.flags & FLAG_MASTER_SEEN != 0 && !self.side_alive(Side::Master) {
            shared.flags |= FLAG_HUNGUP;
        }
        let slave_ready = shared.hangup_pending()
            || if shared.termios.c_lflag & ICANON != 0 {
                shared.line_count > 0
            } else {
                shared.input_len > 0
            };
        // A master whose slaves have all gone is readable-at-end-of-file, which
        // is how a supervising program notices its child exited.
        let master_ready =
            shared.output_len > 0 || shared.packet_status != 0 || self.slaves_gone(shared);
        set_event(self.slave_event.0, slave_ready);
        set_event(self.master_event.0, master_ready);
    }

    /// Whether every slave descriptor has gone, which a master reads as EOF.
    fn slaves_gone(&self, shared: &Shared) -> bool {
        shared.flags & FLAG_SLAVE_SEEN != 0 && !self.side_alive(Side::Slave)
    }

    /// Whether any process still holds a descriptor on `side`.
    fn side_alive(&self, side: Side) -> bool {
        let name = liveness_name(self.index, side);
        let probe = unsafe {
            windows_sys::Win32::System::Threading::OpenEventW(SYNCHRONIZE, 0, name.as_ptr())
        };
        if probe.is_null() || probe == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return false;
        }
        unsafe { windows_sys::Win32::Foundation::CloseHandle(probe) };
        true
    }

    /// Creates this process's liveness instance for `side` if it has none.
    fn claim_liveness(&self, side: Side) {
        let slot = match side {
            Side::Master => &self.master_liveness,
            Side::Slave => &self.slave_liveness,
        };
        if slot.load(Ordering::Acquire) != 0 {
            return;
        }
        let name = liveness_name(self.index, side);
        let mut security = private_security();
        let event = unsafe { CreateEventW(&raw mut security, 1, 0, name.as_ptr()) };
        if event.is_null() || event == INVALID_HANDLE_VALUE {
            return;
        }
        if slot
            .compare_exchange(0, event as usize, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            unsafe { CloseHandle(event) };
        }
    }

    /// Drops this process's liveness instance for `side`.
    fn release_liveness(&self, side: Side) {
        let slot = match side {
            Side::Master => &self.master_liveness,
            Side::Slave => &self.slave_liveness,
        };
        let handle = slot.swap(0, Ordering::AcqRel);
        if handle != 0 {
            // SAFETY: the handle was created by `claim_liveness` and removed
            // from the slot before being closed, so it is closed once.
            unsafe { CloseHandle(handle as HANDLE) };
        }
    }

    /// The `/dev/pts` number of this terminal.
    pub fn number(&self) -> u32 {
        self.index
    }
}

/// Releases a terminal's mutex when it goes out of scope.
struct LockGuard<'a> {
    attachment: &'a Attachment,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: the mutex is held by this thread for the guard's lifetime.
        unsafe { ReleaseMutex(self.attachment.lock.0) };
    }
}

/// Sets or resets an event to match a boolean state.
fn set_event(handle: HANDLE, signalled: bool) {
    // SAFETY: the handle is a live manual-reset event.
    unsafe {
        if signalled {
            SetEvent(handle);
        } else {
            ResetEvent(handle);
        }
    }
}

/// Attachments this process holds, by terminal number.
static ATTACHMENTS: OnceLock<Mutex<HashMap<u32, Arc<Attachment>>>> = OnceLock::new();

fn attachments() -> &'static Mutex<HashMap<u32, Arc<Attachment>>> {
    ATTACHMENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns this process's attachment to terminal `index`, creating it if needed.
fn attach(index: u32) -> Result<Arc<Attachment>, i32> {
    let mut cache = attachments().lock().map_err(|_| EIO)?;
    if let Some(existing) = cache.get(&index) {
        return Ok(existing.clone());
    }
    let attachment = Arc::new(Attachment::open(index)?);
    // SAFETY: the mapping is live. The magic is read outside the terminal lock
    // only to reject a block this build did not write, and it cannot tear: it is
    // written once, before anything else can find the terminal.
    if unsafe { (*attachment.view).magic } != SHARED_MAGIC {
        return Err(EIO);
    }
    cache.insert(index, attachment.clone());
    Ok(attachment)
}

/// Allocates the lowest free terminal number and creates it.
///
/// The search is a probe rather than a counter because the namespace is shared
/// with every other process in the logon session: a counter would hand out a
/// number another process already holds.
fn allocate() -> Result<Arc<Attachment>, i32> {
    for index in 0..MAX_PTYS {
        match Attachment::create(index) {
            Ok(attachment) => {
                let attachment = Arc::new(attachment);
                if let Ok(mut cache) = attachments().lock() {
                    cache.insert(index, attachment.clone());
                }
                return Ok(attachment);
            }
            Err(error) if error == crate::EEXIST => continue,
            Err(error) => return Err(error),
        }
    }
    // Linux reports ENOSPC when the pty limit is reached, not EMFILE: the
    // descriptor table has room, the terminal namespace does not.
    Err(crate::ENOSPC)
}

// ---------------------------------------------------------------------------
// Moving the line discipline between shared memory and working form.
// ---------------------------------------------------------------------------

/// Loads the terminal's discipline out of shared memory.
fn load_ldisc(shared: &Shared) -> Ldisc {
    let input_len = (shared.input_len as usize).min(ldisc::INPUT_LIMIT);
    let line_count = (shared.line_count as usize).min(ldisc::INPUT_LIMIT);
    let output_len = (shared.output_len as usize).min(ldisc::OUTPUT_LIMIT);
    Ldisc::load(ldisc::State {
        termios: shared.termios,
        input: shared.input[..input_len].to_vec(),
        pending: (shared.pending as usize).min(input_len),
        lines: shared.lines[..line_count]
            .iter()
            .map(|length| *length as usize)
            .collect(),
        output: shared.output[..output_len].to_vec(),
        column: shared.column as usize,
        canon_column: shared.canon_column as usize,
        stopped: shared.stopped != 0,
        throttled: shared.throttled != 0,
        literal: shared.literal != 0,
        erase_printing: shared.erase_printing != 0,
        number: shared.ldisc_number,
    })
}

/// Stores a discipline back into shared memory.
fn store_ldisc(shared: &mut Shared, discipline: &Ldisc) {
    let state = discipline.save();
    let input_len = state.input.len().min(ldisc::INPUT_LIMIT);
    shared.input[..input_len].copy_from_slice(&state.input[..input_len]);
    shared.input_len = input_len as u32;
    shared.pending = state.pending.min(input_len) as u32;

    let line_count = state.lines.len().min(ldisc::INPUT_LIMIT);
    for (slot, length) in shared.lines[..line_count]
        .iter_mut()
        .zip(state.lines.iter().take(line_count))
    {
        *slot = *length as u16;
    }
    shared.line_count = line_count as u32;

    let output_len = state.output.len().min(ldisc::OUTPUT_LIMIT);
    shared.output[..output_len].copy_from_slice(&state.output[..output_len]);
    shared.output_len = output_len as u32;

    shared.termios = state.termios;
    shared.column = state.column as u32;
    shared.canon_column = state.canon_column as u32;
    shared.stopped = u8::from(state.stopped);
    shared.throttled = u8::from(state.throttled);
    shared.literal = u8::from(state.literal);
    shared.erase_printing = u8::from(state.erase_printing);
    shared.ldisc_number = state.number;
}

// ---------------------------------------------------------------------------
// Identifying a descriptor as one end of a terminal.
// ---------------------------------------------------------------------------

/// The NT `UNICODE_STRING` header that precedes an object's name.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryObject(
        handle: HANDLE,
        class: u32,
        information: *mut core::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// `ObjectNameInformation`.
const OBJECT_NAME_INFORMATION: u32 = 1;

/// Reads a kernel object's name, or `None` when it has none.
fn handle_name(handle: HANDLE) -> Option<String> {
    // Room for the header plus a generous name; the names here are short and
    // entirely controlled by this module.
    let mut buffer = vec![0u8; size_of::<UnicodeString>() + 1024];
    let mut returned = 0u32;
    // SAFETY: the buffer is a writable local of the length being declared, and
    // the query only reads the handle.
    let status = unsafe {
        NtQueryObject(
            handle,
            OBJECT_NAME_INFORMATION,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &raw mut returned,
        )
    };
    if status < 0 {
        return None;
    }
    // SAFETY: a successful query wrote a UNICODE_STRING at the start of the
    // buffer whose `buffer` field points into the same allocation.
    let header = unsafe { &*buffer.as_ptr().cast::<UnicodeString>() };
    if header.buffer.is_null() || header.length == 0 {
        return None;
    }
    let count = header.length as usize / 2;
    // SAFETY: the object manager reported `length` bytes of name at `buffer`.
    let name = unsafe { std::slice::from_raw_parts(header.buffer, count) };
    Some(String::from_utf16_lossy(name))
}

/// Recovers the terminal number and side a handle names.
///
/// This is the whole reason a descriptor's handle is a *named* event. A pty
/// descriptor reaches a fresh process through `execve` as nothing but an
/// inherited handle: there is no side table to consult and no message to pass.
/// Asking the object manager for its name recovers both facts, and it keeps
/// working through `dup`, `dup2` and `fork` for free, because a duplicate names
/// the same object.
fn identify(handle: HANDLE) -> Option<(u32, Side)> {
    let name = handle_name(handle)?;
    // The full path is `\Sessions\<n>\BaseNamedObjects\kinakaze.pts.<n>.<side>`,
    // or without the session prefix in session zero.
    let leaf = name.rsplit('\\').next()?;
    let rest = leaf.strip_prefix("kinakaze.pts.v2.")?;
    let (domain, rest) = rest.split_once('.')?;
    if domain.parse::<u64>().ok()? != kinakaze_runtime::authority::domain_id() {
        return None;
    }
    let (number, suffix) = rest.rsplit_once('.')?;
    let index: u32 = number.parse().ok()?;
    let side = match suffix {
        "m" => Side::Master,
        "s" => Side::Slave,
        _ => return None,
    };
    Some((index, side))
}

/// Handles already identified, so the object manager is asked at most once each.
///
/// Keyed by handle value, which is sound because every entry is removed when the
/// descriptor owning the handle is closed: a recycled handle value cannot find a
/// stale answer.
static IDENTIFIED: OnceLock<Mutex<HashMap<usize, (u32, Side)>>> = OnceLock::new();

fn identified() -> &'static Mutex<HashMap<usize, (u32, Side)>> {
    IDENTIFIED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The "this side has been opened at least once" flag for a side.
///
/// Liveness only means something once a side has existed: without this a
/// terminal would report a hangup between the section being created and the
/// master descriptor being installed.
fn seen_flag(side: Side) -> u32 {
    match side {
        Side::Master => FLAG_MASTER_SEEN,
        Side::Slave => FLAG_SLAVE_SEEN,
    }
}

/// Resolves a descriptor to the terminal it names and the end it is.
///
/// Reports `ENOTTY` rather than `EBADF` for a live descriptor that is not a
/// terminal, which is the distinction every `ioctl` caller depends on.
pub fn resolve(fd: i32) -> Result<(Arc<Attachment>, Side), i32> {
    let entry = crate::get(fd)?;
    if !matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave) {
        return Err(ENOTTY);
    }
    resolve_handle(entry.raw)
}

/// Resolves a raw handle believed to belong to a pty descriptor.
fn resolve_handle(raw: usize) -> Result<(Arc<Attachment>, Side), i32> {
    let cached = identified()
        .lock()
        .ok()
        .and_then(|map| map.get(&raw).copied());
    if let Some((index, side)) = cached {
        return Ok((attach(index)?, side));
    }
    let (index, side) = identify(raw as HANDLE).ok_or(ENOTTY)?;
    let attachment = attach(index)?;
    if let Ok(mut map) = identified().lock() {
        map.insert(raw, (index, side));
    }
    // A handle this process has not seen before arrived by duplication or by
    // inheritance, and either way this process's claim on the side has just
    // become true. Claiming here is what keeps liveness correct without a hook
    // at every site that can install a descriptor.
    attachment.claim_liveness(side);
    attachment.with(|shared| shared.flags |= seen_flag(side));
    Ok((attachment, side))
}

/// Whether a descriptor is one end of a pseudo-terminal.
pub fn is_pty(fd: i32) -> bool {
    crate::get(fd)
        .map(|entry| matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave))
        .unwrap_or(false)
}

/// Classifies a bare handle that may be an inherited pty end.
///
/// `GetFileType` cannot answer this: a terminal descriptor's handle is an event,
/// which it reports as `FILE_TYPE_UNKNOWN`. This is the hook the standard-stream
/// setup uses so a program started by `login_tty` and `execve` — whose fd 0 is
/// nothing but an inherited handle — comes up with a real terminal on it rather
/// than an unusable unknown.
pub fn classify_handle(raw: usize) -> Option<FdKind> {
    if raw == 0 {
        return None;
    }
    let (index, side) = identify(raw as HANDLE)?;
    // Attaching here rather than lazily is what makes the liveness claim happen
    // before anything can observe the terminal as unoccupied.
    let attachment = attach(index).ok()?;
    if let Ok(mut map) = identified().lock() {
        map.insert(raw, (index, side));
    }
    attachment.claim_liveness(side);
    attachment.with(|shared| shared.flags |= seen_flag(side));
    Some(match side {
        Side::Master => FdKind::PtyMaster,
        Side::Slave => FdKind::PtySlave,
    })
}

/// The `/dev/pts` number behind a descriptor.
pub fn pty_number(fd: i32) -> Result<u32, i32> {
    resolve(fd).map(|(attachment, _)| attachment.with(|s| s.number))
}

/// Which end of a terminal a descriptor is.
pub fn pty_side(fd: i32) -> Result<Side, i32> {
    resolve(fd).map(|(_, side)| side)
}

// ---------------------------------------------------------------------------
// Opening and closing.
// ---------------------------------------------------------------------------

/// Opens a per-descriptor handle to a side's readiness event.
///
/// Each descriptor gets its own handle so closing one does not disturb another
/// and the descriptor table's ordinary close path works unchanged.
fn open_end_handle(index: u32, side: Side) -> Result<HANDLE, i32> {
    let name = object_name(index, side.suffix());
    // SAFETY: the name is a null-terminated wide string naming an event this
    // process created or is attached to.
    let handle = unsafe { OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(EIO);
    }
    Ok(handle)
}

/// Installs a descriptor for one end of a terminal.
fn install_end(attachment: &Arc<Attachment>, side: Side, flags: FdFlags) -> Result<i32, i32> {
    let handle = open_end_handle(attachment.index, side)?;
    let kind = match side {
        Side::Master => FdKind::PtyMaster,
        Side::Slave => FdKind::PtySlave,
    };
    // The liveness claim is taken *before* the descriptor becomes visible: a
    // concurrent close on the other side must not observe a moment where this
    // side looks unoccupied.
    attachment.claim_liveness(side);
    let fd = crate::install(handle as usize, kind, flags).inspect_err(|_| {
        // SAFETY: installation failed, so this function still owns the handle.
        unsafe { CloseHandle(handle) };
        if !side_still_held(-1, attachment.index, side) {
            attachment.release_liveness(side);
        }
    })?;
    if let Ok(mut map) = identified().lock() {
        map.insert(handle as usize, (attachment.index, side));
    }
    attachment.with(|shared| shared.flags |= seen_flag(side));
    Ok(fd)
}

/// Translates a device node's open flags into descriptor flags.
fn descriptor_flags(open_flags: i32) -> FdFlags {
    let mut flags = crate::fs::special_fd_flags(open_flags);
    if open_flags & crate::fs::O_CLOEXEC != 0 {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if open_flags & crate::fs::O_NONBLOCK != 0 {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    flags
}

/// Creates a new terminal and returns a descriptor on its master end.
///
/// This is `open("/dev/ptmx")` and `posix_openpt`. The slave is left locked, so
/// the caller must run `unlockpt` before opening it — the ordering Linux
/// enforces, and the reason `grantpt`/`unlockpt`/`ptsname` exist as a sequence.
pub fn open_master(open_flags: i32) -> Result<i32, i32> {
    registry_allocate(true, || {
        let attachment = allocate()?;
        let fd = install_end(&attachment, Side::Master, descriptor_flags(open_flags))?;
        Ok((fd, attachment.index))
    })
    .map(|p| p.0)
}

/// Allocate a private transport ID; the user-visible number belongs to devpts.
pub(crate) fn open_instance_master(
    volume: u64,
    inode: u64,
    number: u32,
    flags: i32,
    reserved: bool,
    policy: (u64, u64, u64),
) -> Result<(i32, u32), i32> {
    registry_allocate(reserved, || {
        let object = crate::mount::shared::new_object()?;
        let index = u32::try_from(
            object
                .id()
                .checked_add(MAX_PTYS as u64)
                .ok_or(crate::ENOSPC)?,
        )
        .map_err(|_| crate::ENOSPC)?;
        if index > i32::MAX as u32 {
            return Err(crate::ENOSPC);
        }
        let attachment = Arc::new(Attachment::create(index)?);
        attachment.with(|s| {
            s.devpts = volume;
            s.inode = inode;
            s.number = number;
            s.mount_namespace = policy.0;
            s.mount_id = policy.1;
            s.mount_flags = policy.2;
        });
        attachments()
            .lock()
            .map_err(|_| EIO)?
            .insert(index, attachment.clone());
        let fd = install_end(&attachment, Side::Master, descriptor_flags(flags))?;
        Ok((fd, index))
    })
}
static REGISTRY: Mutex<Option<Arc<crate::mount::shared::Store>>> = Mutex::new(None);
fn registry() -> Result<Arc<crate::mount::shared::Store>, i32> {
    let mut slot = REGISTRY.lock().map_err(|_| EIO)?;
    if let Some(s) = &*slot {
        return Ok(s.clone());
    }
    let s = Arc::new(crate::mount::shared::Store::user_object(
        u64::MAX - 23,
        true,
    )?);
    s.update(|bytes| {
        if bytes.is_empty() {
            let mut b = Vec::new();
            for n in [4096, 1024, 0] {
                crate::state_codec::word(&mut b, n);
            }
            Ok((b, ()))
        } else {
            Ok((bytes.to_vec(), ()))
        }
    })?;
    *slot = Some(s.clone());
    Ok(s)
}
fn registry_change<T>(
    f: impl FnOnce(&mut u32, &mut u32, &mut Vec<u32>) -> Result<T, i32>,
) -> Result<T, i32> {
    registry()?.update(|bytes| {
        let mut r = crate::state_codec::Reader(bytes);
        let mut max = r.word()? as u32;
        let mut reserve = r.word()? as u32;
        let count = r.word()?;
        if count > 1_048_576 {
            return Err(EIO);
        }
        let mut ids = Vec::new();
        for _ in 0..count {
            let id = r.word()? as u32;
            if instance_status(id).1 {
                ids.push(id);
            }
        }
        r.end()?;
        let out = f(&mut max, &mut reserve, &mut ids)?;
        let mut b = Vec::new();
        for v in [max as u64, reserve as u64, ids.len() as u64] {
            crate::state_codec::word(&mut b, v);
        }
        for id in ids {
            crate::state_codec::word(&mut b, id as u64);
        }
        Ok((b, out))
    })
}
fn registry_allocate(
    reserved: bool,
    f: impl FnOnce() -> Result<(i32, u32), i32>,
) -> Result<(i32, u32), i32> {
    registry_change(|max, reserve, ids| {
        let limit = if reserved {
            *max
        } else {
            max.saturating_sub(*reserve)
        };
        if ids.len() as u32 >= limit {
            return Err(crate::ENOSPC);
        }
        let pair = f()?;
        ids.push(pair.1);
        Ok(pair)
    })
}
fn registry_indices() -> Result<Vec<u32>, i32> {
    registry_change(|_, _, ids| Ok(ids.clone()))
}
pub fn pty_sysctl(name: &str, value: Option<u32>) -> Result<u32, i32> {
    if value.is_some() && !crate::user_namespace::capable(1, 21) {
        return Err(crate::EPERM);
    }
    registry_change(|max, reserve, ids| {
        let slot = match name {
            "max" => max,
            "reserve" => reserve,
            "nr" if value.is_none() => return Ok(ids.len() as u32),
            _ => return Err(crate::EACCES),
        };
        if let Some(value) = value {
            if value > i32::MAX as u32 {
                return Err(EINVAL);
            }
            *slot = value;
        }
        Ok(*slot)
    })
}

pub(crate) fn instance_policy(fd: i32) -> Result<(u64, u64, u64), i32> {
    let (a, _) = resolve(fd)?;
    Ok(a.with(|s| (s.mount_namespace, s.mount_id, s.mount_flags)))
}
pub(crate) fn serialize() -> Result<Vec<u8>, i32> {
    // Keep the quota catalog alive across exec even when no PTYs are open.
    let _ = registry()?;
    let mut b = Vec::new();
    crate::state_codec::word(&mut b, CONTROLLING.load(Ordering::Acquire) as u32 as u64);
    Ok(b)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    if bytes.len() != 8 || registry().is_err() {
        return false;
    }
    let index = u64::from_le_bytes(bytes.try_into().unwrap()) as u32 as i32;
    CONTROLLING.store(index, Ordering::Release);
    for fd in crate::list_open_fds() {
        if is_pty(fd) && resolve(fd).is_err() {
            return false;
        }
    }
    true
}

/// No attachment/cache is acquired while the devpts inode lock is held.
pub(crate) fn instance_status(index: u32) -> (bool, bool) {
    let alive = |side| {
        let name = liveness_name(index, side);
        let h = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
        if h.is_null() {
            false
        } else {
            unsafe {
                CloseHandle(h);
            }
            true
        }
    };
    let master = alive(Side::Master);
    (master, master || alive(Side::Slave))
}
pub(crate) fn instance(fd: i32) -> Result<Option<(u64, u64, u32)>, i32> {
    if !is_pty(fd) {
        return Ok(None);
    }
    let (a, _) = resolve(fd)?;
    Ok(a.with(|s| {
        if s.devpts == 0 {
            None
        } else {
            Some((s.devpts, s.inode, s.number))
        }
    }))
}

/// Opens `/dev/pts/N`.
///
/// Two refusals are real rather than decorative: a slave that `unlockpt` has not
/// released reports `EIO`, exactly as Linux does, and a terminal holding
/// `TIOCEXCL` reports `EBUSY`.
pub fn open_slave(index: u32, open_flags: i32) -> Result<i32, i32> {
    if index >= MAX_PTYS {
        return Err(ENOENT);
    }
    open_instance_slave(index, open_flags)
}
pub(crate) fn open_instance_slave(index: u32, open_flags: i32) -> Result<i32, i32> {
    let attachment = attach(index)?;
    let refusal = attachment.with(|shared| {
        if shared.flags & FLAG_LOCKED != 0 {
            return Some(EIO);
        }
        if shared.flags & FLAG_EXCLUSIVE != 0 {
            return Some(EBUSY);
        }
        None
    });
    if let Some(error) = refusal {
        return Err(error);
    }
    let fd = install_end(&attachment, Side::Slave, descriptor_flags(open_flags))?;
    // O_NOCTTY decides whether opening a terminal makes it this process's
    // controlling one. Honouring it is what keeps a program that opens a pty on
    // someone else's behalf from stealing its own session's terminal.
    if open_flags & crate::fs::O_NOCTTY == 0 && controlling_terminal().is_none() {
        set_controlling(Some(index));
    }
    Ok(fd)
}

/// The terminal this process has as its controlling terminal, if any.
///
/// Held as a number rather than a descriptor because a controlling terminal
/// outlives the descriptor that established it: a shell that closes the fd it
/// called `TIOCSCTTY` on still has a controlling terminal.
static CONTROLLING: AtomicI32 = AtomicI32::new(NO_CONTROLLING);

/// Sentinel for "this process has no controlling terminal".
const NO_CONTROLLING: i32 = -1;

/// Reports the controlling terminal's number.
///
/// Three answers, tried in the order a kernel would.
///
/// The recorded one is this process's own `TIOCSCTTY`. It is lost at `execve`,
/// because `execve` here replaces the process image by starting a *new Windows
/// process* and a static does not cross that boundary.
///
/// The session is what survives. A controlling terminal is a property of the
/// session rather than of a process, and the session id does cross an exec, so
/// asking which live terminal belongs to this session recovers the answer. That
/// is the case `sshpass` depends on and the one a descriptor scan cannot serve:
/// it opens the slave purely to claim the terminal, closes it again, and execs a
/// program whose standard descriptors were never the pty at all — and that
/// program then opens `/dev/tty` to prompt for a password.
///
/// The descriptor scan is the last resort, for a program started by `login_tty`
/// whose session bookkeeping did not survive but whose fd 0 plainly is a
/// terminal.
pub fn controlling_terminal() -> Option<u32> {
    let recorded = CONTROLLING.load(Ordering::Acquire);
    if recorded >= 0 {
        return Some(recorded as u32);
    }
    let session = jobs::current_sid();
    if session > 0
        && let Some(index) = terminal_of_session(session)
    {
        // Remembered so the search happens once per process rather than on
        // every `/dev/tty` open.
        set_controlling(Some(index));
        return Some(index);
    }
    for fd in 0..3 {
        if let Ok((attachment, Side::Slave)) = resolve(fd) {
            return Some(attachment.index);
        }
    }
    None
}

/// Finds the live terminal that session `session` controls, if any.
///
/// The search maps each candidate rather than consulting a table, because the
/// table would have to live somewhere and the terminals themselves already
/// record who owns them. Attachments made here are deliberately not cached: a
/// process should not end up holding a mapping of every terminal in its logon
/// session merely for having asked this question once.
fn terminal_of_session(session: i32) -> Option<u32> {
    for index in registry_indices()
        .unwrap_or_default()
        .into_iter()
        .chain(0..MAX_PTYS)
    {
        let Ok(attachment) = Attachment::open(index) else {
            continue;
        };
        // SAFETY: the mapping is live for the attachment's lifetime; the magic
        // is written once, before the terminal can be found by anyone else.
        if unsafe { (*attachment.view).magic } != SHARED_MAGIC {
            continue;
        }
        let owner = {
            let _guard = attachment.acquire();
            // SAFETY: the view is live and the terminal's mutex is held.
            unsafe { (*attachment.view).session }
        };
        if owner == session {
            return Some(index);
        }
    }
    None
}

/// Records or clears this process's controlling terminal.
pub fn set_controlling(index: Option<u32>) {
    CONTROLLING.store(
        index.map_or(NO_CONTROLLING, |index| index as i32),
        Ordering::Release,
    );
}

/// Opens `/dev/tty`, the caller's controlling terminal.
///
/// A process with no controlling terminal gets `ENXIO` on Linux. That errno has
/// no constant in this crate, and `ENOENT` sends every caller down the same "no
/// terminal here" branch, so that is what is reported.
pub fn open_controlling(open_flags: i32) -> Result<i32, i32> {
    let Some(index) = controlling_terminal() else {
        return Err(ENOENT);
    };
    let attachment = attach(index)?;
    // Opening /dev/tty never changes which terminal is controlling, so the lock
    // and exclusive checks that guard /dev/pts/N do not apply: this process
    // already owns the terminal.
    install_end(&attachment, Side::Slave, descriptor_flags(open_flags))
}

/// Terminal numbers that currently exist, for listing `/dev/pts`.
pub fn live_terminals() -> Vec<u32> {
    (0..MAX_PTYS)
        .filter(|index| terminal_exists(*index))
        .collect()
}

/// Whether a terminal number names a live pseudo-terminal.
pub fn terminal_exists(index: u32) -> bool {
    if index >= MAX_PTYS {
        return false;
    }
    let name = object_name(index, "");
    // SAFETY: the name is a null-terminated wide string.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: the handle was just opened here and is not stored.
    unsafe { CloseHandle(handle) };
    true
}

/// Returns (uid, gid, mode) for terminal `index`, or ENOENT if not present.
pub fn terminal_stat(index: u32) -> Result<(u32, u32, u32), i32> {
    if index >= MAX_PTYS {
        return Err(ENOENT);
    }
    if let Ok(cache) = attachments().lock() {
        if let Some(existing) = cache.get(&index) {
            return Ok(existing.with(|s| (s.uid, s.gid, s.mode)));
        }
    }
    let name = object_name(index, "");
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(ENOENT);
    }
    let mapped = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, size_of::<Shared>()) };
    if mapped.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(EIO);
    }
    let view = mapped.Value.cast::<Shared>();
    let (uid, gid, mode) = unsafe { ((*view).uid, (*view).gid, (*view).mode) };
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: mapped.Value,
        });
        CloseHandle(handle);
    }
    Ok((uid, gid, mode))
}

/// Changes the ownership of terminal `index`.
pub fn set_terminal_ownership(index: u32, uid: u32, gid: u32) -> Result<(), i32> {
    if index >= MAX_PTYS {
        return Err(ENOENT);
    }
    let attachment = attach(index)?;
    attachment.with(|s| {
        if uid != u32::MAX {
            s.uid = uid;
        }
        if gid != u32::MAX {
            s.gid = gid;
        }
    });
    Ok(())
}

/// Changes the file mode of terminal `index`.
pub fn set_terminal_mode(index: u32, mode: u32) -> Result<(), i32> {
    if index >= MAX_PTYS {
        return Err(ENOENT);
    }
    let attachment = attach(index)?;
    attachment.with(|s| {
        s.mode = mode & 0o7777;
    });
    Ok(())
}

/// Returns (uid, gid, mode) for a pty descriptor.
pub fn pty_stat(fd: i32) -> Result<(u32, u32, u32), i32> {
    let (attachment, _) = resolve(fd)?;
    Ok(attachment.with(|s| (s.uid, s.gid, s.mode)))
}

/// Changes the ownership of a pty descriptor.
pub fn set_pty_ownership(fd: i32, uid: u32, gid: u32) -> Result<(), i32> {
    let (attachment, _) = resolve(fd)?;
    attachment.with(|s| {
        if uid != u32::MAX {
            s.uid = uid;
        }
        if gid != u32::MAX {
            s.gid = gid;
        }
    });
    Ok(())
}

/// Changes the mode of a pty descriptor.
pub fn set_pty_mode(fd: i32, mode: u32) -> Result<(), i32> {
    let (attachment, _) = resolve(fd)?;
    attachment.with(|s| {
        s.mode = mode & 0o7777;
    });
    Ok(())
}

/// Releases a pty descriptor's handle and this process's claim on its side.
///
/// Called from the descriptor table's close path. The liveness instance is
/// dropped only once this process has no descriptors left on that side, because
/// it is the object that tells the *other* end whether anyone is still there.
pub fn close_descriptor(fd: i32, raw: usize) {
    let resolved = resolve_handle(raw).ok();
    if let Ok(mut map) = identified().lock() {
        map.remove(&raw);
    }
    // SAFETY: the descriptor table transferred ownership of this handle.
    unsafe { CloseHandle(raw as HANDLE) };
    let Some((attachment, side)) = resolved else {
        return;
    };
    if !side_still_held(fd, attachment.index, side) {
        attachment.release_liveness(side);
        if side == Side::Master && !attachment.side_alive(Side::Master) {
            // Losing the last master is a hangup for the session, which is why a
            // shell dies when its terminal window closes.
            hangup(&attachment);
        }
    }
    // The far end's readiness changes the moment this side goes away, so the
    // events are recomputed whether or not the side was released.
    attachment.with(|_| ());
    if !side_still_held(fd, attachment.index, Side::Master)
        && !side_still_held(fd, attachment.index, Side::Slave)
    {
        if let Ok(mut map) = attachments().lock() {
            map.remove(&attachment.index);
        }
    }
}

/// Whether any descriptor other than `closing` still holds `side` of `index`.
fn side_still_held(closing: i32, index: u32, side: Side) -> bool {
    let wanted = match side {
        Side::Master => FdKind::PtyMaster,
        Side::Slave => FdKind::PtySlave,
    };
    crate::list_open_fds().into_iter().any(|fd| {
        if fd == closing {
            return false;
        }
        let Ok(entry) = crate::get(fd) else {
            return false;
        };
        if entry.kind != wanted {
            return false;
        }
        identified()
            .lock()
            .ok()
            .and_then(|map| map.get(&entry.raw).copied())
            .or_else(|| identify(entry.raw as HANDLE))
            .map(|(found, s)| found == index && s == side)
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// Job control: who may use the terminal, and who hears about it.
// ---------------------------------------------------------------------------

/// Bridge to the process-group layer.
///
/// A terminal generates signals for a *process group*, which is a concept this
/// crate's signal module does not have — it delivers to threads of this process.
/// Everything job-control-shaped is funnelled through here, so the terminal code
/// reads as terminal code and there is exactly one place that knows how a
/// process group is reached.
mod jobs {
    pub use crate::job::{current_pgid, current_sid, group_in_session, signal_process_group};

    /// Delivers `signal` to every process in `pgid`.
    pub fn signal_group(pgid: i32, signal: i32) -> Result<(), i32> {
        if pgid <= 0 {
            return Err(super::EINVAL);
        }
        signal_process_group(pgid, signal)
    }
}

/// Refuses terminal access from a background process group.
///
/// This is the rule that stops a background job from stealing the keyboard, and
/// the reason a shell can put a job in the background at all. It applies only
/// when the terminal really is this process's controlling one: a program that
/// opened somebody else's pty is not in that session and is not restricted.
fn check_foreground(
    attachment: &Arc<Attachment>,
    shared: &Shared,
    reading: bool,
    controlling: Option<u32>,
) -> Result<(), i32> {
    if controlling != Some(attachment.index) {
        return Ok(());
    }
    let foreground = shared.foreground;
    if foreground <= 0 || foreground == jobs::current_pgid() {
        return Ok(());
    }
    if shared.session != 0 && !jobs::group_in_session(jobs::current_pgid(), shared.session) {
        return Ok(());
    }
    if !reading && shared.termios.c_lflag & TOSTOP == 0 {
        // Without TOSTOP a background write goes through, which is the default
        // on every Unix: output from a background job appears rather than
        // stopping it.
        return Ok(());
    }
    let signal = if reading { SIGTTIN } else { SIGTTOU };
    let _ = jobs::signal_group(jobs::current_pgid(), signal);
    // Linux stops the process here and the call is restarted or reported as
    // interrupted afterwards. `EINTR` is the honest outcome: the handler, if
    // there was one, has already run.
    Err(EINTR)
}

// ---------------------------------------------------------------------------
// Reading and writing a terminal.
// ---------------------------------------------------------------------------

/// Waits for a terminal event, staying interruptible by a signal.
///
/// This is the pattern the descriptor table's overlapped path uses, reproduced
/// here because a pty read has no overlapped request to cancel. The dual wait is
/// the point: a guest blocked reading a terminal must be interruptible by `^C`,
/// and a wait on the readiness event alone could not be.
fn wait_ready(event: HANDLE, timeout_ms: Option<u32>) -> Result<(), i32> {
    let timeout = timeout_ms.unwrap_or(INFINITE);
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        // Without an interrupt event the wait is simply uninterruptible.
        // SAFETY: the event is live for the duration of the wait.
        unsafe { WaitForMultipleObjects(1, &event, 0, timeout) };
        return Ok(());
    }
    let handles = [event, interrupt];
    crate::signal::register_waiter();
    // SAFETY: both handles are live for the duration of the wait.
    let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) };
    crate::signal::unregister_waiter();
    if waited == WAIT_OBJECT_0 + 1 {
        // SAFETY: the interrupt event belongs to this thread.
        unsafe { ResetEvent(interrupt) };
        return match crate::signal::deliver_pending() {
            crate::signal::Delivery::Interrupted => Err(EINTR),
            // A handler with SA_RESTART asked for the operation to resume.
            _ => Ok(()),
        };
    }
    Ok(())
}

/// How long a blocked writer sleeps before re-checking for queue space.
///
/// Neither queue has an event for "space became available"; a third named object
/// per terminal, for a case that only arises when the far end has stopped
/// reading, is not worth it. The wait is still interruptible, because it is
/// spent on the thread's interrupt event.
const SPACE_POLL_MS: u32 = 10;

/// Reads from a descriptor on either end of a terminal.
pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    let entry = crate::get(fd)?;
    if entry.flags.contains(FdFlags::WRITE_ACCESS) && !entry.flags.contains(FdFlags::READ_ACCESS) {
        return Err(crate::EBADF);
    }
    let (attachment, side) = resolve(fd)?;
    let nonblock = entry.flags.contains(FdFlags::NONBLOCK);
    match side {
        Side::Master => read_master(&attachment, buffer, nonblock),
        Side::Slave => read_slave(&attachment, buffer, nonblock),
    }
}

/// Reads the terminal's output: what the program on the slave side has written.
fn read_master(
    attachment: &Arc<Attachment>,
    buffer: &mut [u8],
    nonblock: bool,
) -> Result<usize, i32> {
    if buffer.is_empty() {
        return Ok(0);
    }
    loop {
        let outcome = attachment.with(|shared| {
            // Packet mode prefixes every read with a status byte, which is how
            // `rlogind` and its descendants learn that the slave changed its
            // termios or flushed a queue without inspecting the terminal.
            let packet = shared.flags & FLAG_PACKET != 0;
            let mut written = 0usize;
            if packet {
                let status = shared.packet_status as u8;
                if status != TIOCPKT_DATA {
                    shared.packet_status = 0;
                    buffer[0] = status;
                    // A control packet carries no data alongside it.
                    return Some(1);
                }
                buffer[0] = TIOCPKT_DATA;
                written = 1;
            }
            let mut discipline = load_ldisc(shared);
            let copied = discipline.read_output(&mut buffer[written..]);
            store_ldisc(shared, &discipline);
            if copied > 0 {
                return Some(written + copied);
            }
            // Nothing to say after all; do not hand back a lone packet header.
            None
        });
        if let Some(count) = outcome {
            if crate::fork_trace_enabled() {
                eprintln!(
                    "kinakaze tty: pid {} read_master returned count={}",
                    std::process::id(),
                    count
                );
            }
            return Ok(count);
        }
        let gone = attachment.with(|shared| attachment.slaves_gone(shared));
        if crate::fork_trace_enabled() {
            eprintln!(
                "kinakaze tty: pid {} read_master checking slaves_gone={gone}",
                std::process::id()
            );
        }
        if gone {
            return Ok(0);
        }
        if nonblock {
            return Err(EAGAIN);
        }
        wait_ready(attachment.master_event.0, Some(SPACE_POLL_MS))?;
    }
}

/// Reads cooked input on the slave side, honouring `VMIN` and `VTIME`.
fn read_slave(
    attachment: &Arc<Attachment>,
    buffer: &mut [u8],
    nonblock: bool,
) -> Result<usize, i32> {
    if buffer.is_empty() {
        return Ok(0);
    }
    let mut filled = 0usize;
    let mut deadline: Option<std::time::Instant> = None;

    loop {
        let mut outcome = LdiscRead::WouldBlock;
        let mut policy = ReadPolicy::Canonical;
        let mut hungup = false;
        // Discovery scans terminal session records under their own locks.
        // Never run it while holding this terminal: concurrent readers on two
        // different ptys could each wait for the other's terminal mutex.
        let controlling = controlling_terminal();
        let denied = attachment.with(|shared| {
            if let Err(error) = check_foreground(attachment, shared, true, controlling) {
                return Some(error);
            }
            let mut discipline = load_ldisc(shared);
            outcome = discipline.read(&mut buffer[filled..]);
            policy = discipline.read_policy();
            store_ldisc(shared, &discipline);
            hungup = shared.hangup_pending();
            None
        });
        if let Some(error) = denied {
            return Err(error);
        }

        match outcome {
            // A VEOF on an empty line ends the read wherever it stands, which is
            // how a shell sees end of input.
            LdiscRead::EndOfFile => return Ok(filled),
            LdiscRead::Data(count) => {
                filled += count;
                if let ReadPolicy::Interbyte { timeout_ms, .. } = policy {
                    // Every byte restarts the inter-byte timer.
                    deadline = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_millis(u64::from(timeout_ms)),
                    );
                }
            }
            LdiscRead::WouldBlock => {}
        }

        if filled == buffer.len() {
            return Ok(filled);
        }
        match policy {
            ReadPolicy::Canonical => {
                if filled > 0 {
                    return Ok(filled);
                }
            }
            ReadPolicy::Poll => return Ok(filled),
            ReadPolicy::Blocking { min } => {
                if filled >= min.min(buffer.len()).max(1) {
                    return Ok(filled);
                }
            }
            ReadPolicy::ReadTimer { timeout_ms } => {
                if filled > 0 {
                    return Ok(filled);
                }
                let expiry = *deadline.get_or_insert_with(|| {
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(u64::from(timeout_ms))
                });
                if std::time::Instant::now() >= expiry {
                    // The timer expired with nothing typed. A zero-length read
                    // is the answer, and it does not mean end of file: the
                    // caller asked for a timed read and got its timeout.
                    return Ok(0);
                }
            }
            ReadPolicy::Interbyte { min, .. } => {
                if filled >= min.min(buffer.len()).max(1) {
                    return Ok(filled);
                }
                if let Some(expiry) = deadline
                    && std::time::Instant::now() >= expiry
                {
                    return Ok(filled);
                }
            }
        }

        if hungup {
            // The master is gone. Bytes already cooked are delivered first and
            // the error after, which lets a shell read the last line typed
            // before the terminal was destroyed.
            return if filled > 0 { Ok(filled) } else { Err(EIO) };
        }
        if nonblock {
            return if filled > 0 { Ok(filled) } else { Err(EAGAIN) };
        }

        let wait = deadline.map(|expiry| {
            expiry
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        });
        wait_ready(attachment.slave_event.0, wait)?;
    }
}

/// Writes to a descriptor on either end of a terminal.
pub fn write(fd: i32, buffer: &[u8]) -> Result<usize, i32> {
    let entry = crate::get(fd)?;
    if entry.flags.contains(FdFlags::READ_ACCESS) && !entry.flags.contains(FdFlags::WRITE_ACCESS) {
        return Err(crate::EBADF);
    }
    let (attachment, side) = resolve(fd)?;
    let nonblock = entry.flags.contains(FdFlags::NONBLOCK);
    match side {
        Side::Master => write_master(&attachment, buffer, nonblock),
        Side::Slave => write_slave(&attachment, buffer, nonblock),
    }
}

/// Feeds bytes into the terminal as if they had been typed.
fn write_master(attachment: &Arc<Attachment>, buffer: &[u8], nonblock: bool) -> Result<usize, i32> {
    if buffer.is_empty() {
        return Ok(0);
    }
    loop {
        let (accepted, signals) = attachment.with(|shared| {
            let space = ldisc::INPUT_LIMIT.saturating_sub(shared.input_len as usize);
            if space == 0 {
                return (0usize, Vec::new());
            }
            let take = buffer.len().min(space);
            let mut discipline = load_ldisc(shared);
            let signals = discipline.receive(&buffer[..take]);
            store_ldisc(shared, &discipline);
            (take, signals)
        });
        if accepted > 0 {
            deliver_terminal_signals(attachment, &signals);
            return Ok(accepted);
        }
        if nonblock {
            return Err(EAGAIN);
        }
        // The input queue is full because nothing is reading the slave.
        wait_ready(attachment.slave_event.0, Some(SPACE_POLL_MS))?;
    }
}

/// Writes program output through `OPOST` toward the master.
fn write_slave(attachment: &Arc<Attachment>, buffer: &[u8], nonblock: bool) -> Result<usize, i32> {
    if buffer.is_empty() {
        return Ok(0);
    }
    let mut written = 0usize;
    loop {
        let controlling = controlling_terminal();
        let outcome = attachment.with(|shared| {
            check_foreground(attachment, shared, false, controlling)?;
            if shared.hangup_pending() {
                // Writing to a terminal whose master has gone is an I/O error,
                // and Linux raises SIGHUP for it as well.
                return Err(EIO);
            }
            let mut discipline = load_ldisc(shared);
            let count = discipline.write_output(&buffer[written..]);
            store_ldisc(shared, &discipline);
            Ok(count)
        });
        match outcome {
            Err(error) => return if written > 0 { Ok(written) } else { Err(error) },
            Ok(count) => written += count,
        }
        if written == buffer.len() {
            return Ok(written);
        }
        if nonblock {
            return if written > 0 {
                Ok(written)
            } else {
                Err(EAGAIN)
            };
        }
        wait_ready(attachment.master_event.0, Some(SPACE_POLL_MS))?;
    }
}

/// Delivers the signals a line discipline generated to the foreground group.
fn deliver_terminal_signals(attachment: &Arc<Attachment>, signals: &[i32]) {
    if signals.is_empty() {
        return;
    }
    let foreground = attachment.with(|shared| shared.foreground);
    // No foreground group has been claimed yet — the state a pty is in before a
    // shell runs `tcsetpgrp`. Linux drops the signal rather than picking a
    // victim, and so does this.
    if foreground <= 0 {
        return;
    }
    for signal in signals {
        let _ = jobs::signal_group(foreground, *signal);
    }
}

/// Raises `SIGHUP` for the session that owns a terminal whose master closed.
///
/// Linux does this from the master's release path, which is why a shell running
/// under a terminal emulator dies when the window closes. The signal goes to the
/// session leader's group, not to whoever happened to be in the foreground.
pub fn hangup(attachment: &Arc<Attachment>) {
    let session = attachment.with(|shared| {
        shared.flags |= FLAG_HUNGUP;
        shared.session
    });
    if session > 0 {
        let _ = jobs::signal_group(session, crate::signal::SIGHUP);
    }
}

// ---------------------------------------------------------------------------
// Readiness, for poll, select and epoll.
// ---------------------------------------------------------------------------

/// A terminal descriptor's current readiness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Readiness {
    pub readable: bool,
    pub writable: bool,
    /// The other end has gone: `POLLHUP`.
    pub hangup: bool,
}

/// Reports whether a terminal descriptor would block.
pub fn readiness(fd: i32) -> Result<Readiness, i32> {
    let (attachment, side) = resolve(fd)?;
    Ok(attachment.with(|shared| match side {
        Side::Master => Readiness {
            readable: shared.output_len > 0
                || shared.packet_status != 0
                || attachment.slaves_gone(shared),
            writable: (shared.input_len as usize) < ldisc::INPUT_LIMIT,
            hangup: attachment.slaves_gone(shared),
        },
        Side::Slave => {
            let readable = shared.hangup_pending()
                || if shared.termios.c_lflag & ICANON != 0 {
                    shared.line_count > 0
                } else {
                    shared.input_len > 0
                };
            Readiness {
                readable,
                writable: (shared.output_len as usize) < ldisc::OUTPUT_LIMIT,
                hangup: shared.hangup_pending(),
            }
        }
    }))
}

/// The waitable object a descriptor becomes readable on.
///
/// This is the same handle the descriptor table stores as the entry's `raw`,
/// which is why a pty needs no separate readiness plumbing: `poll` can sleep on
/// it directly rather than spinning.
pub fn readable_event(fd: i32) -> Result<HANDLE, i32> {
    let entry = crate::get(fd)?;
    if !matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave) {
        return Err(ENOTTY);
    }
    Ok(entry.raw as HANDLE)
}

/// Bytes a read on this descriptor could return now, for `FIONREAD`.
pub fn input_queued(fd: i32) -> Result<usize, i32> {
    let (attachment, side) = resolve(fd)?;
    Ok(attachment.with(|shared| {
        let discipline = load_ldisc(shared);
        match side {
            Side::Master => discipline.pending_output(),
            Side::Slave => discipline.readable(),
        }
    }))
}

/// Bytes written that the other end has not read, for `TIOCOUTQ`.
pub fn output_queued(fd: i32) -> Result<usize, i32> {
    let (attachment, side) = resolve(fd)?;
    Ok(attachment.with(|shared| match side {
        // What the master has written is the terminal's *input* queue.
        Side::Master => shared.input_len as usize,
        Side::Slave => shared.output_len as usize,
    }))
}

// ===========================================================================
// The Windows console, driven through the same line discipline.
//
// conhost has its own line editor, and the previous version of this layer
// delegated `ECHO`, `ICANON` and `ISIG` to it. That is a much smaller vocabulary
// than `termios`: `VERASE` cannot be remapped, `VINTR` is fixed at `^C`,
// `VMIN`/`VTIME` do not exist, and `ICRNL` is not applied. A program that set
// `VINTR` to `q` was told it had worked and then saw `^C` keep interrupting.
//
// So conhost is put in raw mode — VT input, no line editing, no echo, no
// processed input — and the cooking is done here by the same [`crate::ldisc`]
// that drives a pty. Every `termios` bit then means the same thing on the real
// terminal as on a pseudo-terminal, which is the property that makes a program
// behave identically under both.
//
// `ENABLE_VIRTUAL_TERMINAL_PROCESSING` stays on for output regardless of
// `termios`: escape sequences reaching the terminal intact is the premise of
// this whole layer, and a program clearing `OPOST` is asking for *less*
// processing, not for its escape sequences to be printed literally.
// ===========================================================================

/// The process's one console, cooked by this layer rather than by conhost.
struct Console {
    input: HANDLE,
    output: HANDLE,
    /// The console modes as they were found, so they can be put back.
    original_input: u32,
    original_output: u32,
    discipline: Mutex<Ldisc>,
    /// Signalled while cooked input is waiting for a reader.
    ready: Owned,
    /// Whether the reader thread has been started.
    pumping: std::sync::atomic::AtomicBool,
    /// The foreground process group, for `TIOCGPGRP` on the console.
    foreground: AtomicI32,
    session: AtomicI32,
}

// SAFETY: both console handles are process-wide kernel objects usable from any
// thread, and the discipline is behind a mutex.
unsafe impl Send for Console {}
// SAFETY: as above.
unsafe impl Sync for Console {}

static CONSOLE: OnceLock<Option<Console>> = OnceLock::new();

/// Opens the process's console, or reports that it has none.
///
/// `CONIN$`/`CONOUT$` rather than the standard handles: a redirected stdin is a
/// pipe even when the process is still attached to a console, and the terminal
/// this layer drives is the console, not whatever fd 0 happens to be.
fn console() -> Option<&'static Console> {
    CONSOLE
        .get_or_init(|| {
            use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };

            let open = |name: &str| {
                let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                // Both accesses are required: the console device refuses a
                // read-only open of CONOUT$ for mode changes.
                // SAFETY: the name is a null-terminated wide string and the rest
                // is the documented console-device pattern.
                let handle = unsafe {
                    CreateFileW(
                        wide.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null_mut(),
                    )
                };
                (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(handle)
            };

            let input = open("CONIN$")?;
            let Some(output) = open("CONOUT$") else {
                // SAFETY: the input handle was just opened here and is unstored.
                unsafe { CloseHandle(input) };
                return None;
            };

            let mut original_input = 0u32;
            let mut original_output = 0u32;
            // SAFETY: both handles are live and the outputs are writable locals.
            let usable = unsafe {
                GetConsoleMode(input, &raw mut original_input) != 0
                    && GetConsoleMode(output, &raw mut original_output) != 0
            };
            if !usable {
                // SAFETY: both handles were opened here and are unstored.
                unsafe {
                    CloseHandle(input);
                    CloseHandle(output);
                }
                return None;
            }

            // SAFETY: null security and name request an unnamed manual-reset
            // event, initially unsignalled.
            let ready = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if ready.is_null() {
                // SAFETY: both handles were opened here and are unstored.
                unsafe {
                    CloseHandle(input);
                    CloseHandle(output);
                }
                return None;
            }

            Some(Console {
                input,
                output,
                original_input,
                original_output,
                discipline: Mutex::new(Ldisc::new(ldisc::default_termios())),
                ready: Owned(ready),
                pumping: std::sync::atomic::AtomicBool::new(false),
                foreground: AtomicI32::new(0),
                session: AtomicI32::new(0),
            })
        })
        .as_ref()
}

impl Console {
    /// Puts conhost into raw mode so this layer can do the cooking.
    fn take_over(&self) {
        // Everything conhost would interpret is turned off. VT input is turned
        // on so arrow keys and function keys arrive as escape sequences, which
        // is what a Unix program expects from a terminal; conhost's key records
        // have no equivalent a byte stream could carry.
        let input = ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS;
        // SAFETY: the handle is a live console input handle and only input-side
        // bits are being written to it.
        unsafe { SetConsoleMode(self.input, input) };
        // Processed output and VT processing are the terminal's own rendering,
        // not `termios` translation, and stay on regardless.
        let output =
            self.original_output | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        // SAFETY: the handle is a live console output handle.
        unsafe { SetConsoleMode(self.output, output) };
    }

    /// Puts the console modes back the way they were found.
    fn restore(&self) {
        // SAFETY: both handles are live and the modes are the ones read from
        // them at open time.
        unsafe {
            SetConsoleMode(self.input, self.original_input);
            SetConsoleMode(self.output, self.original_output);
        }
    }

    /// Writes whatever the discipline has produced to the real console.
    ///
    /// Called with the discipline locked, so echo cannot interleave with a
    /// program's own write halfway through an escape sequence.
    fn flush_output(&self, discipline: &mut Ldisc) {
        let pending = discipline.pending_output();
        if pending == 0 {
            return;
        }
        let mut bytes = vec![0u8; pending];
        let count = discipline.read_output(&mut bytes);
        let mut written = 0u32;
        // SAFETY: the buffer is readable for `count` bytes and the handle is a
        // live console output handle.
        unsafe {
            WriteFile(
                self.output,
                bytes.as_ptr(),
                count as u32,
                &raw mut written,
                std::ptr::null_mut(),
            )
        };
    }

    /// Starts the thread that reads raw bytes and cooks them.
    ///
    /// Deferred until the first read rather than done at startup: a program that
    /// never reads the terminal should not have its console taken over, and the
    /// unit tests in this crate open `CONIN$` to exercise `termios` without ever
    /// wanting the developer's keystrokes consumed.
    fn start_pump(&'static self) {
        if self.pumping.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        self.take_over();
        std::thread::Builder::new()
            .name(String::from("kinakaze-console"))
            .spawn(move || self.pump())
            .ok();
    }

    /// Reads the console forever, feeding the line discipline.
    fn pump(&'static self) {
        let mut buffer = [0u8; 256];
        loop {
            let mut read = 0u32;
            // A blocking console read. It is on its own thread precisely so the
            // guest's read can wait on an event and stay interruptible while
            // this one cannot be.
            // SAFETY: the buffer is writable for its length and the handle is a
            // live console input handle.
            let ok = unsafe {
                ReadFile(
                    self.input,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &raw mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                // The console has gone. Nothing more will ever arrive, so the
                // thread ends rather than spinning on a dead handle.
                return;
            }
            let signals = {
                let Ok(mut discipline) = self.discipline.lock() else {
                    return;
                };
                let signals = discipline.receive(&buffer[..read as usize]);
                self.flush_output(&mut discipline);
                set_event(self.ready.0, discipline.read_ready());
                signals
            };
            let foreground = self.foreground.load(Ordering::Acquire);
            for signal in signals {
                if foreground > 0 {
                    let _ = jobs::signal_group(foreground, signal);
                } else {
                    // With no foreground group claimed, the console's signals go
                    // to this process — which *is* the foreground job when
                    // nobody has said otherwise.
                    let _ = crate::signal::raise_signal(signal);
                }
            }
        }
    }
}

/// Restores the console when the process exits normally.
///
/// The C runtime walks `.CRT$XTU` during `exit`, which covers a return from
/// `main` and an explicit `exit()`. It does not cover `_exit` or
/// `TerminateProcess`; a guest killed that way leaves the console raw, which is
/// the same hole every terminal program has and the reason `reset` exists.
mod console_teardown {
    extern "C" fn restore() {
        if let Some(console) = super::console() {
            console.restore();
        }
    }

    #[used]
    #[unsafe(link_section = ".CRT$XTU")]
    static TERMINATOR: extern "C" fn() = restore;
}

/// Releases the console back to conhost's own line editor.
///
/// Exposed so a caller that is about to hand the terminal to a child process can
/// put it back deliberately rather than relying on process exit.
pub fn release_console() {
    if let Some(console) = console() {
        console.restore();
    }
}

/// Whether a descriptor refers to the process's console.
///
/// Two checks, because `GetFileType` answers `FILE_TYPE_CHAR` for `NUL` and for
/// printers as well as for consoles: the table's classification is necessary but
/// not sufficient, and `GetConsoleMode` succeeding is the real test.
pub fn is_console(fd: i32) -> bool {
    let Ok(entry) = crate::get(fd) else {
        return false;
    };
    if entry.kind != FdKind::Console && entry.kind != FdKind::Unknown {
        return false;
    }
    let mut mode = 0u32;
    // SAFETY: the handle is live while the descriptor is installed and `mode` is
    // a writable local.
    unsafe { GetConsoleMode(entry.raw as HANDLE, &raw mut mode) != 0 }
}

/// Reads from the console through the line discipline.
pub fn console_read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    let Some(console) = console() else {
        return Err(EIO);
    };
    if buffer.is_empty() {
        return Ok(0);
    }
    console.start_pump();
    let nonblock = crate::get(fd)
        .map(|entry| entry.flags.contains(FdFlags::NONBLOCK))
        .unwrap_or(false);

    let mut filled = 0usize;
    let mut deadline: Option<std::time::Instant> = None;
    loop {
        let (outcome, policy) = {
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            let outcome = discipline.read(&mut buffer[filled..]);
            let policy = discipline.read_policy();
            set_event(console.ready.0, discipline.read_ready());
            (outcome, policy)
        };
        match outcome {
            LdiscRead::EndOfFile => return Ok(filled),
            LdiscRead::Data(count) => {
                filled += count;
                if let ReadPolicy::Interbyte { timeout_ms, .. } = policy {
                    deadline = Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_millis(u64::from(timeout_ms)),
                    );
                }
            }
            LdiscRead::WouldBlock => {}
        }
        if filled == buffer.len() {
            return Ok(filled);
        }
        match policy {
            ReadPolicy::Canonical => {
                if filled > 0 {
                    return Ok(filled);
                }
            }
            ReadPolicy::Poll => return Ok(filled),
            ReadPolicy::Blocking { min } => {
                if filled >= min.min(buffer.len()).max(1) {
                    return Ok(filled);
                }
            }
            ReadPolicy::ReadTimer { timeout_ms } => {
                if filled > 0 {
                    return Ok(filled);
                }
                let expiry = *deadline.get_or_insert_with(|| {
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(u64::from(timeout_ms))
                });
                if std::time::Instant::now() >= expiry {
                    return Ok(0);
                }
            }
            ReadPolicy::Interbyte { min, .. } => {
                if filled >= min.min(buffer.len()).max(1) {
                    return Ok(filled);
                }
                if let Some(expiry) = deadline
                    && std::time::Instant::now() >= expiry
                {
                    return Ok(filled);
                }
            }
        }
        if nonblock {
            return if filled > 0 { Ok(filled) } else { Err(EAGAIN) };
        }
        let wait = deadline.map(|expiry| {
            expiry
                .saturating_duration_since(std::time::Instant::now())
                .as_millis()
                .min(u128::from(u32::MAX)) as u32
        });
        wait_ready(console.ready.0, wait)?;
    }
}

/// Writes to the console through `OPOST`.
pub fn console_write(buffer: &[u8]) -> Result<usize, i32> {
    let Some(console) = console() else {
        return Err(EIO);
    };
    let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
    let mut written = 0usize;
    // The queue is only a staging area here — it is drained to the console
    // immediately — so a short write just means going round again rather than
    // blocking on a reader.
    while written < buffer.len() {
        let count = discipline.write_output(&buffer[written..]);
        console.flush_output(&mut discipline);
        if count == 0 {
            break;
        }
        written += count;
    }
    Ok(written.max(if buffer.is_empty() { 0 } else { written }))
}

/// Whether the console has cooked input waiting.
pub fn console_readable() -> bool {
    console()
        .and_then(|console| console.discipline.lock().ok().map(|d| d.read_ready()))
        .unwrap_or(false)
}

/// The console's readiness event, for `poll` to wait on.
pub fn console_event() -> Option<HANDLE> {
    let console = console()?;
    console.start_pump();
    Some(console.ready.0)
}

/// Cooked bytes the console could return right now.
pub fn console_queued() -> usize {
    console()
        .and_then(|console| console.discipline.lock().ok().map(|d| d.readable()))
        .unwrap_or(0)
}

// ===========================================================================
// The terminal interface: everything `ioctl` and the `tc*` family reach.
//
// A pty and the console are the same kind of object as far as `termios` goes,
// so every entry point below resolves the descriptor to one of the two and then
// speaks to the line discipline. That is the whole point of doing the cooking
// in software: `VMIN` means the same thing on both, and there is no table of
// "bits Windows happens to implement" left to document.
// ===========================================================================

/// Which terminal a descriptor names.
enum Terminal {
    Pty(Arc<Attachment>, Side),
    /// The process's own console, which has no second end.
    Console,
}

/// Resolves a descriptor to a terminal, or reports `ENOTTY`.
///
/// `EBADF` outranks `ENOTTY`: a descriptor has to exist before it can be asked
/// whether it is a terminal.
fn terminal(fd: i32) -> Result<Terminal, i32> {
    let entry = crate::get(fd)?;
    match entry.kind {
        FdKind::PtyMaster | FdKind::PtySlave => {
            let (attachment, side) = resolve_handle(entry.raw)?;
            Ok(Terminal::Pty(attachment, side))
        }
        FdKind::Console | FdKind::Unknown if is_console(fd) => Ok(Terminal::Console),
        _ => Err(ENOTTY),
    }
}

/// Reports whether a descriptor is a terminal, as `isatty` would.
pub fn isatty(fd: i32) -> bool {
    terminal(fd).is_ok()
}

/// Reads a terminal's settings.
pub fn tcgetattr(fd: i32) -> Result<Termios, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => Ok(attachment.with(|shared| shared.termios)),
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let discipline = console.discipline.lock().map_err(|_| EIO)?;
            Ok(discipline.termios)
        }
    }
}

/// Applies a terminal's settings.
///
/// The three actions differ in *when* they take effect, and collapsing them was
/// the previous version's worst simplification: `TCSADRAIN` exists so a program
/// switching to raw mode does not reinterpret bytes it already wrote under the
/// old settings, and `TCSAFLUSH` exists so a password prompt does not read the
/// keystrokes typed while echo was still on.
pub fn tcsetattr(fd: i32, actions: i32, requested: &Termios) -> Result<(), i32> {
    if !matches!(
        actions,
        crate::pty::TCSANOW | crate::pty::TCSADRAIN | crate::pty::TCSAFLUSH
    ) {
        return Err(EINVAL);
    }
    match terminal(fd)? {
        Terminal::Pty(attachment, side) => {
            if actions != crate::pty::TCSANOW {
                drain_pty(&attachment, side)?;
            }
            attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                if actions == crate::pty::TCSAFLUSH {
                    discipline.flush(Queue::Input);
                }
                discipline.retune(*requested);
                store_ldisc(shared, &discipline);
                // Packet mode reports every settings change to the master, which
                // is how a remote login server keeps its own terminal in step.
                if shared.flags & FLAG_PACKET != 0 {
                    shared.packet_status |= u32::from(TIOCPKT_NOSTOP);
                }
            });
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            if actions == crate::pty::TCSAFLUSH {
                discipline.flush(Queue::Input);
            }
            discipline.retune(*requested);
            console.flush_output(&mut discipline);
            set_event(console.ready.0, discipline.read_ready());
            Ok(())
        }
    }
}

/// Waits until a pty's pending output has been consumed by the other end.
///
/// This is a real wait, not a no-op: the queue is finite and a `tcdrain` before
/// a mode change has to mean the far side has actually seen the bytes.
fn drain_pty(attachment: &Arc<Attachment>, side: Side) -> Result<(), i32> {
    loop {
        let outstanding = attachment.with(|shared| match side {
            Side::Master => shared.input_len,
            Side::Slave => shared.output_len,
        });
        if outstanding == 0 {
            return Ok(());
        }
        let event = match side {
            Side::Master => attachment.slave_event.0,
            Side::Slave => attachment.master_event.0,
        };
        wait_ready(event, Some(SPACE_POLL_MS))?;
    }
}

/// Waits for pending output to leave, as `tcdrain` asks.
pub fn tcdrain(fd: i32) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, side) => drain_pty(&attachment, side),
        // Console writes go straight to conhost inside `console_write`, so the
        // post-condition tcdrain promises already holds when it returns.
        Terminal::Console => Ok(()),
    }
}

/// Discards queued input, output, or both.
pub fn tcflush(fd: i32, queue: i32) -> Result<(), i32> {
    let selector = match queue {
        crate::pty::TCIFLUSH => Queue::Input,
        crate::pty::TCOFLUSH => Queue::Output,
        crate::pty::TCIOFLUSH => Queue::Both,
        _ => return Err(EINVAL),
    };
    match terminal(fd)? {
        Terminal::Pty(attachment, side) => {
            attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                // "Input" and "output" are named from the *caller's* point of
                // view, so which queue a flush clears depends on which end asked.
                let target = match (side, selector) {
                    (Side::Master, Queue::Input) => Queue::Output,
                    (Side::Master, Queue::Output) => Queue::Input,
                    (_, other) => other,
                };
                discipline.flush(target);
                store_ldisc(shared, &discipline);
                if shared.flags & FLAG_PACKET != 0 {
                    let bit = match target {
                        Queue::Input => TIOCPKT_FLUSHREAD,
                        Queue::Output => TIOCPKT_FLUSHWRITE,
                        Queue::Both => TIOCPKT_FLUSHREAD | TIOCPKT_FLUSHWRITE,
                    };
                    shared.packet_status |= u32::from(bit);
                }
            });
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            discipline.flush(selector);
            set_event(console.ready.0, discipline.read_ready());
            Ok(())
        }
    }
}

// `tcflow` actions.
pub const TCOOFF: i32 = 0;
pub const TCOON: i32 = 1;
pub const TCIOFF: i32 = 2;
pub const TCION: i32 = 3;

/// Suspends or resumes transmission, as `tcflow` asks.
///
/// Previously this returned zero without doing anything. It now moves the same
/// flag `^S` and `^Q` move, so a program that suspends its own output really
/// stops producing it.
pub fn tcflow(fd: i32, action: i32) -> Result<(), i32> {
    let stop = match action {
        TCOOFF => Some(true),
        TCOON => Some(false),
        // TCIOFF and TCION transmit a STOP or START character to the far end
        // rather than changing local state.
        TCIOFF | TCION => None,
        _ => return Err(EINVAL),
    };
    match terminal(fd)? {
        Terminal::Pty(attachment, side) => {
            attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                match stop {
                    Some(stopped) => discipline.set_output_stopped(stopped),
                    None => {
                        let character = if action == TCIOFF {
                            discipline.termios.c_cc[crate::pty::VSTOP]
                        } else {
                            discipline.termios.c_cc[crate::pty::VSTART]
                        };
                        if character != ldisc::DISABLED {
                            match side {
                                Side::Slave => {
                                    discipline.write_output(&[character]);
                                }
                                // A master sending flow control is feeding the
                                // discipline, which interprets it.
                                Side::Master => {
                                    discipline.receive(&[character]);
                                }
                            }
                        }
                    }
                }
                store_ldisc(shared, &discipline);
                if let Some(stopped) = stop
                    && shared.flags & FLAG_PACKET != 0
                {
                    shared.packet_status |=
                        u32::from(if stopped { TIOCPKT_STOP } else { TIOCPKT_START });
                }
            });
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            if let Some(stopped) = stop {
                discipline.set_output_stopped(stopped);
            }
            Ok(())
        }
    }
}

/// Sends a break condition.
///
/// A pty has no line to break, so on Linux this is a no-op for the *slave* and a
/// real event for the master: writing a break from the master is what a serial
/// concentrator emulates. That is what happens here — the discipline sees a
/// break and applies `IGNBRK`/`BRKINT` to it.
pub fn tcsendbreak(fd: i32, _duration: i32) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, Side::Master) => {
            let signals = attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                let signals = discipline.receive_break();
                store_ldisc(shared, &discipline);
                signals
            });
            deliver_terminal_signals(&attachment, &signals);
            Ok(())
        }
        // A slave asking for a break is asking to signal a modem that is not
        // there. Linux succeeds without anything reaching the master, and
        // programs like `ssh` treat a failure as fatal, so this succeeds too.
        Terminal::Pty(_, Side::Slave) | Terminal::Console => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Window size.
// ---------------------------------------------------------------------------

/// Reports the terminal's size, as `TIOCGWINSZ` would.
pub fn get_window_size(fd: i32) -> Result<WinSize, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => Ok(attachment.with(|shared| shared.winsize)),
        // The console's size is a property of the screen buffer, so it is asked
        // rather than remembered: the user can resize the window at any moment.
        Terminal::Console => crate::pty::get_window_size(fd),
    }
}

/// Sets the terminal's size, as `TIOCSWINSZ` would.
///
/// A change raises `SIGWINCH` for the foreground process group, which is how a
/// full-screen program learns to redraw. A *non*-change does not, because Linux
/// suppresses the signal when the size is unchanged and a program that redrew on
/// every `TIOCSWINSZ` would flicker.
pub fn set_window_size(fd: i32, size: &WinSize) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            let (changed, foreground) = attachment.with(|shared| {
                let changed = shared.winsize != *size;
                shared.winsize = *size;
                (changed, shared.foreground)
            });
            if changed && foreground > 0 {
                let _ = jobs::signal_group(foreground, crate::signal::SIGWINCH);
            }
            Ok(())
        }
        Terminal::Console => {
            crate::pty::set_window_size(fd, size)?;
            if let Some(console) = console() {
                let foreground = console.foreground.load(Ordering::Acquire);
                if foreground > 0 {
                    let _ = jobs::signal_group(foreground, crate::signal::SIGWINCH);
                } else {
                    let _ = crate::signal::raise_signal(crate::signal::SIGWINCH);
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Sessions and foreground process groups.
// ---------------------------------------------------------------------------

/// The terminal's foreground process group, as `tcgetpgrp` reports it.
///
/// A terminal nobody has claimed reports the caller's own group. That is not a
/// fabrication: a process alone in its session holding a terminal *is* that
/// terminal's foreground group, and it is the answer `getpgrp` already gives.
pub fn foreground_pgrp(fd: i32) -> Result<i32, i32> {
    let recorded = match terminal(fd)? {
        Terminal::Pty(attachment, _) => attachment.with(|shared| shared.foreground),
        Terminal::Console => console().ok_or(ENOTTY)?.foreground.load(Ordering::Acquire),
    };
    Ok(if recorded > 0 {
        recorded
    } else {
        jobs::current_pgid()
    })
}

/// Hands the terminal to a process group, as `tcsetpgrp` asks.
///
/// The session check is the real one: Linux refuses `EPERM` when the group is
/// not in the terminal's session, and a shell relies on that refusal to detect
/// that it has lost the terminal.
pub fn set_foreground_pgrp(fd: i32, pgid: i32) -> Result<(), i32> {
    if pgid <= 0 {
        return Err(EINVAL);
    }
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => attachment.with(|shared| {
            if shared.session != 0 && !jobs::group_in_session(pgid, shared.session) {
                return Err(crate::EPERM);
            }
            shared.foreground = pgid;
            Ok(())
        }),
        Terminal::Console => {
            console()
                .ok_or(ENOTTY)?
                .foreground
                .store(pgid, Ordering::Release);
            Ok(())
        }
    }
}

/// The session that owns the terminal, as `tcgetsid` reports it.
pub fn session_id(fd: i32) -> Result<i32, i32> {
    let recorded = match terminal(fd)? {
        Terminal::Pty(attachment, _) => attachment.with(|shared| shared.session),
        Terminal::Console => console().ok_or(ENOTTY)?.session.load(Ordering::Acquire),
    };
    Ok(if recorded > 0 {
        recorded
    } else {
        jobs::current_sid()
    })
}

/// Makes a terminal the caller's controlling terminal, as `TIOCSCTTY` asks.
///
/// This is what `login_tty` does after `setsid`, and it is the step that turns a
/// pty slave into "the terminal" for a whole session — the reason a program
/// started that way can open `/dev/tty` and find the right thing.
///
/// `steal` is the ioctl's argument: only a caller with it set may take a
/// terminal that already belongs to another session, and Linux additionally
/// requires privilege for that. There is no Windows counterpart to the privilege
/// check, so the argument alone decides, which is a documented widening.
pub fn set_controlling_terminal(fd: i32, steal: bool) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            let session = jobs::current_sid();
            let group = jobs::current_pgid();
            attachment.with(|shared| {
                if shared.session != 0 && shared.session != session && !steal {
                    return Err(crate::EPERM);
                }
                shared.session = session;
                // The claiming process's group becomes the foreground one, which
                // is what makes its own reads legal immediately afterwards.
                shared.foreground = group;
                Ok(())
            })?;
            set_controlling(Some(attachment.index));
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            console
                .session
                .store(jobs::current_sid(), Ordering::Release);
            console
                .foreground
                .store(jobs::current_pgid(), Ordering::Release);
            set_controlling(None);
            Ok(())
        }
    }
}

/// Gives up the controlling terminal, as `TIOCNOTTY` asks.
pub fn drop_controlling_terminal(fd: i32) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            let session = jobs::current_sid();
            attachment.with(|shared| {
                if shared.session == session {
                    shared.session = 0;
                    shared.foreground = 0;
                }
            });
            if controlling_terminal() == Some(attachment.index) {
                set_controlling(None);
            }
            Ok(())
        }
        Terminal::Console => {
            if let Some(console) = console() {
                console.session.store(0, Ordering::Release);
                console.foreground.store(0, Ordering::Release);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// The pty-specific ioctls.
// ---------------------------------------------------------------------------

/// Requires a master descriptor, which several ioctls are only defined on.
fn master(fd: i32) -> Result<Arc<Attachment>, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, Side::Master) => Ok(attachment),
        _ => Err(ENOTTY),
    }
}

/// `TIOCSPTLCK`: locks or unlocks the slave side.
pub fn set_slave_lock(fd: i32, locked: bool) -> Result<(), i32> {
    let attachment = master(fd)?;
    attachment.with(|shared| {
        if locked {
            shared.flags |= FLAG_LOCKED;
        } else {
            shared.flags &= !FLAG_LOCKED;
        }
    });
    Ok(())
}

/// Reads the slave lock, which Linux also exposes through `TIOCGPTLCK`.
pub fn slave_locked(fd: i32) -> Result<bool, i32> {
    let attachment = master(fd)?;
    Ok(attachment.with(|shared| shared.flags & FLAG_LOCKED != 0))
}

/// `TIOCGPTPEER`: opens the slave of a master without going through a name.
///
/// The point of this ioctl on Linux is that it cannot be raced or spoofed by a
/// `/dev/pts` that has been remounted; here it is simply the honest way to reach
/// the peer, and it works even when the guest's filesystem has no `/dev` at all.
pub fn open_peer(fd: i32, open_flags: i32) -> Result<i32, i32> {
    let attachment = master(fd)?;
    if open_flags
        & !(crate::fs::O_RDWR | crate::fs::O_NOCTTY | crate::fs::O_CLOEXEC | crate::fs::O_NONBLOCK)
        != 0
    {
        return Err(EINVAL);
    }
    open_instance_slave(attachment.index, open_flags)
}

/// `TIOCEXCL` / `TIOCNXCL`: exclusive-use state, which really is enforced.
pub fn set_exclusive(fd: i32, exclusive: bool) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            attachment.with(|shared| {
                if exclusive {
                    shared.flags |= FLAG_EXCLUSIVE;
                } else {
                    shared.flags &= !FLAG_EXCLUSIVE;
                }
            });
            Ok(())
        }
        // A console cannot be opened a second time by name, so exclusivity has
        // nothing to exclude and the request is accepted as already satisfied.
        Terminal::Console => Ok(()),
    }
}

/// Whether a terminal is marked exclusive-use.
pub fn is_exclusive(fd: i32) -> Result<bool, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            Ok(attachment.with(|shared| shared.flags & FLAG_EXCLUSIVE != 0))
        }
        Terminal::Console => Ok(false),
    }
}

/// `TIOCPKT`: turns the master's packet mode on or off.
pub fn set_packet_mode(fd: i32, enabled: bool) -> Result<(), i32> {
    let attachment = master(fd)?;
    attachment.with(|shared| {
        if enabled {
            shared.flags |= FLAG_PACKET;
            shared.packet_status = 0;
        } else {
            shared.flags &= !FLAG_PACKET;
        }
    });
    Ok(())
}

/// `TIOCSTI`: pushes a byte into the terminal as if it had been typed.
///
/// Linux restricts this to the caller's own controlling terminal unless the
/// caller is privileged, because it is otherwise a way to type commands into
/// another user's shell. The controlling-terminal half of that check is enforced
/// here; the privileged half has no Windows counterpart, so there is no way to
/// pass it — which makes this strictly *more* restrictive than Linux rather than
/// less, and that is the safe direction.
pub fn push_input(fd: i32, byte: u8) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            if controlling_terminal() != Some(attachment.index) {
                return Err(crate::EPERM);
            }
            let signals = attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                let signals = discipline.receive(&[byte]);
                store_ldisc(shared, &discipline);
                signals
            });
            deliver_terminal_signals(&attachment, &signals);
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            let signals = discipline.receive(&[byte]);
            console.flush_output(&mut discipline);
            set_event(console.ready.0, discipline.read_ready());
            drop(discipline);
            for signal in signals {
                let _ = crate::signal::raise_signal(signal);
            }
            Ok(())
        }
    }
}

/// `TIOCGETD`: the line discipline number in effect.
pub fn line_discipline(fd: i32) -> Result<i32, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => Ok(attachment.with(|shared| shared.ldisc_number)),
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let discipline = console.discipline.lock().map_err(|_| EIO)?;
            Ok(discipline.number)
        }
    }
}

/// `TIOCSETD`: selects a line discipline.
///
/// Recognised Linux disciplines (including `N_TTY`, `N_NULL`, `N_SLIP`, `N_PPP`,
/// `N_HDLC`, etc.) are accepted. Unrecognised disciplines return `EINVAL`.
pub fn set_line_discipline(fd: i32, number: i32) -> Result<(), i32> {
    if !ldisc::is_valid_ldisc(number) {
        return Err(EINVAL);
    }
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            attachment.with(|shared| {
                let mut discipline = load_ldisc(shared);
                discipline.set_number(number);
                store_ldisc(shared, &discipline);
            });
            Ok(())
        }
        Terminal::Console => {
            let console = console().ok_or(ENOTTY)?;
            let mut discipline = console.discipline.lock().map_err(|_| EIO)?;
            discipline.set_number(number);
            set_event(console.ready.0, discipline.read_ready());
            Ok(())
        }
    }
}

/// `TIOCVHANGUP`: simulates the terminal's carrier dropping.
///
/// Every process in the session sees the hangup and every further slave read
/// reports `EIO`, which is what a session manager uses to evict whatever is
/// still attached to a terminal it is taking back.
pub fn vhangup(fd: i32) -> Result<(), i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, _) => {
            hangup(&attachment);
            Ok(())
        }
        // A console has no carrier to drop and nothing to hang up on. Reporting
        // success would claim an eviction that did not happen.
        Terminal::Console => Err(crate::ENOSYS),
    }
}

/// The device path a descriptor's terminal has, for `ttyname`.
///
/// A pty slave is `/dev/pts/N` and a master is `/dev/ptmx`, exactly as Linux
/// names them — and unlike the previous fixed `/dev/tty`, the path returned here
/// can be opened again and reaches the same terminal.
pub fn terminal_name(fd: i32) -> Result<String, i32> {
    match terminal(fd)? {
        Terminal::Pty(attachment, side) => {
            let (id, number) = attachment.with(|s| (s.devpts, s.number));
            if id != 0 {
                if let Some(path) = crate::devpts::terminal_path(id, number, side)? {
                    return Ok(path);
                }
            }
            Ok(if side == Side::Master {
                "/dev/ptmx".into()
            } else {
                format!("/dev/pts/{number}")
            })
        }
        // The console has no device node in the guest's namespace other than the
        // one that means "my controlling terminal".
        Terminal::Console => Ok(String::from("/dev/tty")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{O_NOCTTY, O_NONBLOCK, O_RDWR};
    use crate::pty::{ECHO, ICANON, ICRNL, ONLCR, OPOST, VMIN, VTIME};

    /// A pseudo-terminal pair that closes both ends when the test ends.
    ///
    /// The order matters: the slave goes first, so the master observes the
    /// end-of-file every teardown should produce rather than a hangup racing it.
    struct Pair {
        master: i32,
        slave: i32,
    }

    impl Pair {
        /// Runs the POSIX allocation sequence, exactly as a guest would.
        fn open() -> Self {
            let master = open_master(O_RDWR | O_NOCTTY).expect("posix_openpt failed");
            // The slave must be locked until unlockpt, which is a real refusal.
            let number = pty_number(master).unwrap();
            assert!(
                matches!(open_slave(number, O_RDWR | O_NOCTTY), Err(EIO)),
                "a locked slave must refuse to open"
            );
            set_slave_lock(master, false).unwrap();
            let slave = open_slave(number, O_RDWR | O_NOCTTY).expect("opening the slave failed");
            Self { master, slave }
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            let _ = crate::close(self.slave);
            let _ = crate::close(self.master);
        }
    }

    /// Reads until `count` bytes have arrived or the terminal ends.
    fn read_exact(fd: i32, count: usize) -> Vec<u8> {
        let mut collected = Vec::new();
        while collected.len() < count {
            let mut chunk = [0u8; 256];
            match read(fd, &mut chunk[..count - collected.len()]) {
                Ok(0) => break,
                Ok(read) => collected.extend_from_slice(&chunk[..read]),
                Err(error) => panic!("read failed with {error}"),
            }
        }
        collected
    }

    /// Switches a terminal out of canonical mode so reads are byte-granular.
    fn make_raw_pair(fd: i32) {
        let mut termios = tcgetattr(fd).unwrap();
        ldisc::make_raw(&mut termios);
        tcsetattr(fd, crate::pty::TCSANOW, &termios).unwrap();
    }

    #[test]
    fn the_posix_allocation_sequence_produces_a_working_pair() {
        let pair = Pair::open();
        let number = pty_number(pair.master).unwrap();

        // Both ends name the same terminal and know which end they are.
        assert_eq!(pty_number(pair.slave).unwrap(), number);
        assert_eq!(pty_side(pair.master).unwrap(), Side::Master);
        assert_eq!(pty_side(pair.slave).unwrap(), Side::Slave);
        assert_eq!(
            terminal_name(pair.slave).unwrap(),
            format!("/dev/pts/{number}")
        );
        assert_eq!(terminal_name(pair.master).unwrap(), "/dev/ptmx");
        assert!(isatty(pair.master) && isatty(pair.slave));

        // The terminal exists in the namespace `/dev/pts` lists.
        assert!(terminal_exists(number));
        assert!(live_terminals().contains(&number));
    }

    #[test]
    fn a_line_typed_at_the_master_is_read_as_a_line_by_the_slave() {
        let pair = Pair::open();
        // Enter arrives as a carriage return from a real terminal, and ICRNL is
        // what turns it into the newline that terminates the line.
        assert_eq!(write(pair.master, b"hello\r").unwrap(), 6);
        assert_eq!(read_exact(pair.slave, 6), b"hello\n");
    }

    #[test]
    fn concurrent_slave_io_does_not_nest_terminal_mutexes() {
        let ready = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let ready = ready.clone();
                std::thread::spawn(move || {
                    let pair = Pair::open();
                    make_raw_pair(pair.slave);
                    ready.wait();
                    for _ in 0..12 {
                        assert_eq!(write(pair.master, b"input").unwrap(), 5);
                        assert_eq!(read_exact(pair.slave, 5), b"input");
                        assert_eq!(write(pair.slave, b"output").unwrap(), 6);
                        assert_eq!(read_exact(pair.master, 6), b"output");
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_half_typed_line_is_queued_but_not_readable() {
        let pair = Pair::open();
        write(pair.master, b"partial").unwrap();
        // Canonical mode: the bytes are queued, but a read must not see them and
        // poll must not claim they are there.
        assert_eq!(input_queued(pair.slave).unwrap(), 0);
        assert!(!readiness(pair.slave).unwrap().readable);
        // A non-blocking read is the observable version of the same fact.
        let mut buffer = [0u8; 16];
        let flags = crate::get(pair.slave).unwrap().flags;
        assert!(!flags.contains(FdFlags::NONBLOCK));
        write(pair.master, b"\r").unwrap();
        assert_eq!(input_queued(pair.slave).unwrap(), 8);
        assert!(readiness(pair.slave).unwrap().readable);
        assert_eq!(read(pair.slave, &mut buffer).unwrap(), 8);
        assert_eq!(&buffer[..8], b"partial\n");
    }

    #[test]
    fn echo_from_the_master_comes_back_out_of_the_master() {
        let pair = Pair::open();
        write(pair.master, b"ab").unwrap();
        // The default terminal echoes, and the echo is the master's to read: it
        // is what a terminal emulator prints on the screen.
        assert_eq!(read_exact(pair.master, 2), b"ab");
        // A newline is echoed through OPOST, so ONLCR makes it CR LF.
        write(pair.master, b"\r").unwrap();
        assert_eq!(read_exact(pair.master, 2), b"\r\n");
    }

    #[test]
    fn slave_output_reaches_the_master_through_opost() {
        let pair = Pair::open();
        assert_eq!(write(pair.slave, b"line\n").unwrap(), 5);
        // ONLCR is on by default, so the bare newline arrives as CR LF.
        assert_eq!(read_exact(pair.master, 6), b"line\r\n");

        // Clearing OPOST stops the rewriting, which is what a full-screen
        // program depends on for its escape sequences.
        let mut termios = tcgetattr(pair.slave).unwrap();
        termios.c_oflag &= !OPOST;
        tcsetattr(pair.slave, crate::pty::TCSANOW, &termios).unwrap();
        write(pair.slave, b"raw\n").unwrap();
        assert_eq!(read_exact(pair.master, 4), b"raw\n");
    }

    #[test]
    fn termios_belongs_to_the_terminal_and_not_to_a_descriptor() {
        let pair = Pair::open();
        let mut termios = tcgetattr(pair.slave).unwrap();
        assert_ne!(termios.c_lflag & ICANON, 0);
        termios.c_lflag &= !(ICANON | ECHO);
        termios.c_iflag &= !ICRNL;
        tcsetattr(pair.slave, crate::pty::TCSANOW, &termios).unwrap();

        // Set through the slave, visible through the master: one terminal, one
        // set of settings.
        let seen = tcgetattr(pair.master).unwrap();
        assert_eq!(seen.c_lflag & (ICANON | ECHO), 0);
        assert_eq!(seen.c_iflag & ICRNL, 0);

        // And the behaviour really changed: no echo, and a read returns without
        // waiting for a line.
        write(pair.master, b"xy").unwrap();
        assert_eq!(read_exact(pair.slave, 2), b"xy");
        assert_eq!(readiness(pair.master).unwrap().readable, false);
    }

    #[test]
    fn vmin_and_vtime_are_honoured_on_a_pty() {
        let pair = Pair::open();
        make_raw_pair(pair.slave);
        let mut termios = tcgetattr(pair.slave).unwrap();
        // VMIN 0, VTIME 1: wait up to a tenth of a second, then return nothing.
        termios.c_cc[VMIN] = 0;
        termios.c_cc[VTIME] = 1;
        tcsetattr(pair.slave, crate::pty::TCSANOW, &termios).unwrap();

        let started = std::time::Instant::now();
        let mut buffer = [0u8; 16];
        assert_eq!(read(pair.slave, &mut buffer).unwrap(), 0);
        let waited = started.elapsed();
        // The timer really elapsed rather than the read returning immediately,
        // and it did not block forever either.
        assert!(
            waited >= std::time::Duration::from_millis(60),
            "VTIME did not delay the read: {waited:?}"
        );
        assert!(waited < std::time::Duration::from_secs(3));

        // VMIN 3: a read waits for three bytes rather than returning one.
        termios.c_cc[VMIN] = 3;
        termios.c_cc[VTIME] = 0;
        tcsetattr(pair.slave, crate::pty::TCSANOW, &termios).unwrap();
        write(pair.master, b"abc").unwrap();
        assert_eq!(read(pair.slave, &mut buffer).unwrap(), 3);
    }

    #[test]
    fn a_nonblocking_master_reports_eagain_instead_of_waiting() {
        let master = open_master(O_RDWR | O_NOCTTY | O_NONBLOCK).unwrap();
        set_slave_lock(master, false).unwrap();
        let slave =
            open_slave(pty_number(master).unwrap(), O_RDWR | O_NOCTTY | O_NONBLOCK).unwrap();
        let mut buffer = [0u8; 16];
        assert!(matches!(read(master, &mut buffer), Err(EAGAIN)));
        assert!(matches!(read(slave, &mut buffer), Err(EAGAIN)));
        crate::close(slave).unwrap();
        crate::close(master).unwrap();
    }

    #[test]
    fn closing_the_last_slave_makes_the_master_read_end_of_file() {
        let master = open_master(O_RDWR | O_NOCTTY).unwrap();
        set_slave_lock(master, false).unwrap();
        let slave = open_slave(pty_number(master).unwrap(), O_RDWR | O_NOCTTY).unwrap();
        write(slave, b"bye\n").unwrap();
        crate::close(slave).unwrap();

        // Buffered output is delivered first — losing the child's last words
        // would be the classic terminal-emulator bug — and only then EOF.
        assert_eq!(read_exact(master, 5), b"bye\r\n");
        let mut buffer = [0u8; 16];
        assert_eq!(read(master, &mut buffer).unwrap(), 0);
        assert!(readiness(master).unwrap().hangup);
        crate::close(master).unwrap();
    }

    #[test]
    fn closing_the_master_makes_slave_reads_report_eio() {
        let master = open_master(O_RDWR | O_NOCTTY).unwrap();
        set_slave_lock(master, false).unwrap();
        let slave = open_slave(pty_number(master).unwrap(), O_RDWR | O_NOCTTY).unwrap();
        write(master, b"typed\r").unwrap();
        crate::close(master).unwrap();

        // The line typed before the hangup is still readable; the error follows.
        assert_eq!(read_exact(slave, 6), b"typed\n");
        let mut buffer = [0u8; 16];
        assert!(matches!(read(slave, &mut buffer), Err(EIO)));
        assert!(readiness(slave).unwrap().hangup);
        crate::close(slave).unwrap();
    }

    #[test]
    fn a_duplicate_descriptor_names_the_same_terminal() {
        let pair = Pair::open();
        // Duplication goes through the descriptor table, which copies the handle
        // — and because the handle is a *named* object, the copy resolves to the
        // same terminal with no side table to keep in step.
        let entry = crate::get(pair.slave).unwrap();
        let mut copy: HANDLE = std::ptr::null_mut();
        // SAFETY: the source handle is live and the target is a writable local.
        let ok = unsafe {
            windows_sys::Win32::Foundation::DuplicateHandle(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                entry.raw as HANDLE,
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &raw mut copy,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        };
        assert_ne!(ok, 0);
        let duplicate = crate::install(copy as usize, FdKind::PtySlave, FdFlags::NONE).unwrap();
        assert_eq!(
            pty_number(duplicate).unwrap(),
            pty_number(pair.slave).unwrap()
        );

        // Closing the duplicate must not hang the terminal up, because the
        // original still holds the slave side.
        crate::close(duplicate).unwrap();
        assert!(!readiness(pair.slave).unwrap().hangup);
        write(pair.master, b"still here\r").unwrap();
        assert_eq!(read_exact(pair.slave, 11), b"still here\n");
    }

    #[test]
    fn the_window_size_round_trips_and_defaults_to_a_usable_screen() {
        let pair = Pair::open();
        let initial = get_window_size(pair.slave).unwrap();
        assert_eq!(initial.ws_row, 24);
        assert_eq!(initial.ws_col, 80);

        let requested = WinSize {
            ws_row: 40,
            ws_col: 132,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        set_window_size(pair.master, &requested).unwrap();
        // Set on the master, seen by the slave: it is one terminal.
        assert_eq!(get_window_size(pair.slave).unwrap(), requested);
    }

    #[test]
    fn exclusive_use_refuses_a_second_open_and_can_be_released() {
        let pair = Pair::open();
        let number = pty_number(pair.master).unwrap();
        set_exclusive(pair.master, true).unwrap();
        assert!(is_exclusive(pair.slave).unwrap());
        assert!(matches!(open_slave(number, O_RDWR | O_NOCTTY), Err(EBUSY)));

        set_exclusive(pair.master, false).unwrap();
        let second = open_slave(number, O_RDWR | O_NOCTTY).unwrap();
        crate::close(second).unwrap();
    }

    #[test]
    fn the_peer_ioctl_opens_the_slave_without_naming_it() {
        let master = open_master(O_RDWR | O_NOCTTY).unwrap();
        set_slave_lock(master, false).unwrap();
        let peer = open_peer(master, O_RDWR | O_NOCTTY).unwrap();
        assert_eq!(pty_side(peer).unwrap(), Side::Slave);
        assert_eq!(pty_number(peer).unwrap(), pty_number(master).unwrap());
        crate::close(peer).unwrap();
        crate::close(master).unwrap();
    }

    #[test]
    fn packet_mode_prefixes_reads_and_reports_a_flush() {
        let pair = Pair::open();
        set_packet_mode(pair.master, true).unwrap();
        write(pair.slave, b"hi").unwrap();
        // Ordinary data carries a leading zero byte.
        let framed = read_exact(pair.master, 3);
        assert_eq!(framed, [TIOCPKT_DATA, b'h', b'i']);

        // A flush on the slave is reported to the master as a control packet
        // with no data, which is how a remote login server keeps in step.
        tcflush(pair.slave, crate::pty::TCIFLUSH).unwrap();
        let mut buffer = [0u8; 8];
        assert_eq!(read(pair.master, &mut buffer).unwrap(), 1);
        assert_ne!(buffer[0] & TIOCPKT_FLUSHREAD, 0);
    }

    #[test]
    fn line_disciplines_can_be_set_and_queried() {
        let pair = Pair::open();
        assert_eq!(line_discipline(pair.slave).unwrap(), ldisc::N_TTY);

        // Setting standard disciplines succeeds and round-trips through TIOCGETD
        for disc in [ldisc::N_NULL, ldisc::N_SLIP, ldisc::N_PPP, ldisc::N_HDLC] {
            set_line_discipline(pair.slave, disc).unwrap();
            assert_eq!(line_discipline(pair.slave).unwrap(), disc);
            assert_eq!(line_discipline(pair.master).unwrap(), disc);
        }

        // Raw discipline passthrough: control chars (^C) are transferred as raw data
        write(pair.master, &[0x03, b'x', b'\n']).unwrap();
        let mut buffer = [0u8; 8];
        assert_eq!(read(pair.slave, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], &[0x03, b'x', b'\n']);

        // An invalid discipline is rejected with EINVAL
        assert!(matches!(set_line_discipline(pair.slave, 99), Err(EINVAL)));
        assert!(matches!(set_line_discipline(pair.slave, -1), Err(EINVAL)));

        // Resetting back to N_TTY works
        set_line_discipline(pair.slave, ldisc::N_TTY).unwrap();
        assert_eq!(line_discipline(pair.slave).unwrap(), ldisc::N_TTY);
    }

    #[test]
    fn a_non_terminal_descriptor_is_refused_by_every_terminal_call() {
        // A pipe reads and writes perfectly and is not a terminal, which is
        // exactly the case ENOTTY exists to distinguish.
        let mut read_end = std::ptr::null_mut();
        let mut write_end = std::ptr::null_mut();
        // SAFETY: both outputs are writable locals and a null descriptor selects
        // the defaults.
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Pipes::CreatePipe(
                    &raw mut read_end,
                    &raw mut write_end,
                    std::ptr::null(),
                    0,
                )
            },
            0
        );
        let fd = crate::install(read_end as usize, FdKind::Pipe, FdFlags::PIPE_READ_END).unwrap();

        assert!(matches!(tcgetattr(fd), Err(ENOTTY)));
        assert!(matches!(get_window_size(fd), Err(ENOTTY)));
        assert!(matches!(foreground_pgrp(fd), Err(ENOTTY)));
        assert!(matches!(pty_number(fd), Err(ENOTTY)));
        assert!(matches!(terminal_name(fd), Err(ENOTTY)));
        assert!(!isatty(fd));
        // EBADF outranks ENOTTY: a descriptor has to exist first.
        assert!(matches!(tcgetattr(-1), Err(crate::EBADF)));

        crate::close(fd).unwrap();
        // SAFETY: the write end was never installed, so it is still owned here.
        unsafe { CloseHandle(write_end) };
    }

    #[test]
    fn tcflush_clears_the_queue_the_calling_end_reads_from() {
        let pair = Pair::open();
        write(pair.master, b"discarded\r").unwrap();
        write(pair.slave, b"also discarded").unwrap();

        // The slave's "input" is what the master typed; the master's "input" is
        // what the slave wrote. Flushing from each end must clear its own.
        tcflush(pair.slave, crate::pty::TCIFLUSH).unwrap();
        assert_eq!(input_queued(pair.slave).unwrap(), 0);
        tcflush(pair.master, crate::pty::TCIFLUSH).unwrap();
        assert_eq!(input_queued(pair.master).unwrap(), 0);
    }

    #[test]
    fn interrupt_flushes_the_terminal_and_reaches_the_foreground_group() {
        let _serialized = crate::signal::test_lock();
        let pair = Pair::open();
        // Claim the terminal so there is a foreground group to signal.
        set_controlling_terminal(pair.slave, true).unwrap();

        static SEEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
        unsafe extern "sysv64" fn record(signal: i32) {
            SEEN.store(signal, Ordering::SeqCst);
        }
        crate::signal::sigaction(
            crate::signal::SIGINT,
            Some(crate::signal::Action {
                disposition: crate::signal::Disposition::Handle(record, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();

        write(pair.master, b"half typed").unwrap();
        write(pair.master, &[0x03]).unwrap();
        // The signal is pending for this process; running the handlers is what a
        // blocking call would have done on the way out.
        crate::signal::deliver_pending();
        assert_eq!(SEEN.load(Ordering::SeqCst), crate::signal::SIGINT);
        // ^C flushed the half-typed line, and echoed itself.
        assert_eq!(input_queued(pair.slave).unwrap(), 0);

        crate::signal::sigaction(
            crate::signal::SIGINT,
            Some(crate::signal::Action::default()),
        )
        .unwrap();
        drop_controlling_terminal(pair.slave).unwrap();
    }

    #[test]
    fn a_terminal_number_is_reused_once_its_terminal_is_gone() {
        let first = open_master(O_RDWR | O_NOCTTY).unwrap();
        let number = pty_number(first).unwrap();
        crate::close(first).unwrap();
        // Dropping the attachment is what releases the section, and the number
        // with it; without that the namespace would fill up over a long run.
        if let Ok(mut cache) = attachments().lock() {
            cache.remove(&number);
        }
        assert!(
            !terminal_exists(number),
            "terminal {number} outlived its master"
        );
    }
}
