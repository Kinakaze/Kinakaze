//! Unified Linux-style descriptor table for files, consoles, pipes and sockets.

use std::sync::{OnceLock, RwLock};

#[cfg(windows)]
mod io_event;
#[cfg(windows)]
mod native_pin;
#[cfg(windows)]
pub use native_pin::pin_native_fd;
#[cfg(windows)]
pub mod positional;

#[cfg(windows)]
pub mod bpf;
#[cfg(windows)]
pub mod cgroup;
pub mod credentials;
#[cfg(windows)]
pub mod keyring;
pub mod limits;
#[cfg(windows)]
mod state_codec;
#[cfg(windows)]
pub mod tmpfs;
pub use tmpfs::{devpts, mqueue, sysfs};
#[cfg(windows)]
pub mod deadline_wait;
#[cfg(windows)]
pub mod epoll;
pub mod eventfd;
#[cfg(windows)]
pub mod fifo;
pub mod image_io;
#[cfg(windows)]
pub mod inotify;
pub mod namespaces;
#[cfg(windows)]
mod ofd;
pub mod time_namespace;
pub mod timerfd;
pub mod user_namespace;

#[cfg(windows)]
pub mod fs;
pub mod fs_context;
#[cfg(windows)]
mod hostnet;
#[cfg(windows)]
pub mod interrupt;
#[cfg(windows)]
pub mod iouring;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod ldisc;
#[cfg(windows)]
pub mod memory;
#[cfg(windows)]
pub mod mount;
#[cfg(windows)]
pub mod netlink;
mod nftables;
pub mod path;
#[cfg(windows)]
pub mod pipe_inode;
#[cfg(windows)]
pub mod procfs;
#[cfg(windows)]
pub mod pty;
#[cfg(windows)]
pub mod record_lock;
mod route_state;
#[cfg(windows)]
pub mod signal;
#[cfg(windows)]
pub mod socket;
#[cfg(windows)]
pub mod tty;
#[cfg(windows)]
pub mod unix;
#[cfg(windows)]
pub mod usernet;
#[cfg(windows)]
pub mod usernet_broker;
#[cfg(windows)]
pub mod xattr;

pub use image_io::{open_guest_image, read_guest_image, snapshot_guest_image};
pub use path::{
    PathError, create_emulated_symlink, emulated_symlink_target, resolve_linux_path,
    resolve_linux_path_from, resolve_linux_path_from_no_follow, resolve_linux_path_no_follow,
    set_system_root, system_root, to_guest_path,
};

// Linux's default fs.nr_open ceiling; storage grows only for occupied pages.
pub const MAX_FDS: usize = 1024 * 1024;
mod fd_slots;

#[cfg(windows)]
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        algorithm: *mut core::ffi::c_void,
        buffer: *mut u8,
        length: u32,
        flags: u32,
    ) -> i32;
}

#[cfg(windows)]
pub(crate) fn fork_trace_elapsed_ms() -> u128 {
    static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
    STARTED
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
}

/// Enables a diagnostic for its original host process only.
///
/// A Linux fork child inherits the environment and DLL state, but diagnostics
/// must not become bytes on the executed program's stdout/stderr. The owner PID
/// is copied by fork and therefore stops the child before it emits anything.
#[cfg(windows)]
pub(crate) fn diagnostic_target_enabled(name: &str) -> bool {
    static OWNER_PID: OnceLock<u32> = OnceLock::new();
    let Some(target) = std::env::var_os(name) else {
        return false;
    };
    let target = target.to_string_lossy();
    let matches = target == "1"
        || std::env::args_os().any(|argument| argument.to_string_lossy().contains(target.as_ref()));
    matches && *OWNER_PID.get_or_init(std::process::id) == std::process::id()
}

// Linux x86_64 errno values. Guest code compares against these numbers
// directly, so they are ABI rather than an internal detail.
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const EBADF: i32 = 9;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const ESTALE: i32 = 116;
pub const EDOM: i32 = 33;
pub const EPIPE: i32 = 32;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;
pub const ELOOP: i32 = 40;
pub const ERANGE: i32 = 34;
pub const EOVERFLOW: i32 = 75;

// Linux x86_64 socket errno values.
pub const ENOTSOCK: i32 = 88;
pub const EDESTADDRREQ: i32 = 89;
pub const EMSGSIZE: i32 = 90;
pub const EPROTOTYPE: i32 = 91;
pub const ENOPROTOOPT: i32 = 92;
pub const EPROTONOSUPPORT: i32 = 93;
pub const ESOCKTNOSUPPORT: i32 = 94;
pub const EOPNOTSUPP: i32 = 95;
pub const EAFNOSUPPORT: i32 = 97;
pub const EADDRINUSE: i32 = 98;
pub const EADDRNOTAVAIL: i32 = 99;
pub const ENETDOWN: i32 = 100;
pub const ENETUNREACH: i32 = 101;
pub const ENETRESET: i32 = 102;
pub const ECONNABORTED: i32 = 103;
pub const ECONNRESET: i32 = 104;
pub const ENOBUFS: i32 = 105;
pub const EISCONN: i32 = 106;
pub const ENOTCONN: i32 = 107;
pub const ETIMEDOUT: i32 = 110;
pub const ECONNREFUSED: i32 = 111;
pub const EHOSTDOWN: i32 = 112;
pub const EHOSTUNREACH: i32 = 113;
pub const EALREADY: i32 = 114;
pub const EINPROGRESS: i32 = 115;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FdKind {
    File,
    Directory,
    Console,
    Pipe,
    Socket,
    Event,
    EventFd,
    /// An `AF_UNIX` socket emulated on a named pipe. Its handle appears only
    /// after `bind` or `connect`, so the descriptor can exist with `raw == 0`.
    UnixSocket,
    /// A file whose contents are generated rather than backed by a handle.
    Synthetic,
    /// A directory whose listing is generated rather than read from the host.
    SyntheticDirectory,
    /// `/dev/null`: writes discard, reads return EOF.
    Null,
    /// `/dev/zero`: writes discard, reads return zeros.
    Zero,
    /// `/dev/urandom` & `/dev/random`: cryptographically secure random bytes.
    Random,
    /// `/dev/full`: reads return zeros, writes return ENOSPC.
    Full,
    /// An inotify instance backed by `ReadDirectoryChangesW`.
    Inotify,
    /// The master end of a pseudo-terminal. See [`tty`].
    PtyMaster,
    /// The slave end of a pseudo-terminal: what `/dev/pts/N` opens.
    PtySlave,
    /// A cgroup v2 control file that can be read and written.
    CgroupFile,
    /// An `AF_NETLINK` endpoint with Linux message framing and an explicit
    /// host-state translator. It is not a Winsock handle.
    NetlinkSocket,
    /// A verified Linux eBPF program descriptor backed by the shared object registry.
    BpfProgram,
    /// A writable `/proc/sys` control backed by process-shared kernel state.
    ProcSysctl,
    /// A filesystem FIFO endpoint backed by a shared inode queue and a native
    /// last-handle-lifetime token; distinct from a Windows pipe endpoint.
    Fifo,
    Unknown,
    /// A native IoRing. Its placeholder handle alone cannot reconstruct queue
    /// state; preserve this classification across close/dup and wire snapshots.
    IoRing,
    /// A pinned mount namespace section, with namespace lifetime owned by fd.
    MountNamespace,
    TimeNamespace,
    Namespace,
    UserNamespace,
    TimerFd,
    /// A shared Linux filesystem configuration context.
    FsContext,
    /// A mount tree retained independently of a namespace attachment.
    MountTree,
    TmpfsFile,
    TmpfsDirectory,
    MessageQueue,
    SysfsFile,
}

#[cfg(windows)]
impl FdKind {
    /// Stable descriptor-kind code used by the fork handoff wire format.
    ///
    /// Keep this mapping explicit: serializing the Rust enum discriminant while
    /// decoding with a separate numeric match made every kind after a newly
    /// inserted variant shift by one.  In particular, `UnixSocket` was restored
    /// as `Synthetic`, which broke BusyBox's `wget` -> `ssl_client` socketpair.
    fn fork_code(self) -> u32 {
        match self {
            Self::File => 0,
            Self::Directory => 1,
            Self::Console => 2,
            Self::Pipe => 3,
            Self::Socket => 4,
            Self::Event => 5,
            Self::EventFd => 6,
            Self::UnixSocket => 7,
            Self::Synthetic => 8,
            Self::SyntheticDirectory => 9,
            Self::Null => 10,
            Self::Zero => 11,
            Self::Random => 12,
            Self::Full => 13,
            Self::Inotify => 15,
            // Appended, never inserted: the code is the wire format, and giving
            // a new variant a number in the middle renumbers every kind after it
            // and silently restores a forked child's descriptors as the wrong
            // thing. That has already happened here once.
            Self::PtyMaster => 16,
            Self::PtySlave => 17,
            Self::CgroupFile => 18,
            Self::NetlinkSocket => 19,
            Self::BpfProgram => 20,
            Self::ProcSysctl => 21,
            Self::Fifo => 22,
            Self::IoRing => 23,
            Self::MountNamespace => 24,
            Self::TimeNamespace => 27,
            Self::Namespace => 30,
            Self::UserNamespace => 29,
            Self::TimerFd => 28,
            Self::FsContext => 25,
            Self::MountTree => 26,
            Self::TmpfsFile => 31,
            Self::TmpfsDirectory => 32,
            Self::MessageQueue => 33,
            Self::SysfsFile => 34,
            Self::Unknown => 14,
        }
    }

    fn from_fork_code(code: u32) -> Self {
        match code {
            0 => Self::File,
            1 => Self::Directory,
            2 => Self::Console,
            3 => Self::Pipe,
            4 => Self::Socket,
            5 => Self::Event,
            6 => Self::EventFd,
            7 => Self::UnixSocket,
            8 => Self::Synthetic,
            9 => Self::SyntheticDirectory,
            10 => Self::Null,
            11 => Self::Zero,
            12 => Self::Random,
            13 => Self::Full,
            15 => Self::Inotify,
            16 => Self::PtyMaster,
            17 => Self::PtySlave,
            18 => Self::CgroupFile,
            19 => Self::NetlinkSocket,
            20 => Self::BpfProgram,
            21 => Self::ProcSysctl,
            22 => Self::Fifo,
            23 => Self::IoRing,
            24 => Self::MountNamespace,
            27 => Self::TimeNamespace,
            30 => Self::Namespace,
            29 => Self::UserNamespace,
            28 => Self::TimerFd,
            25 => Self::FsContext,
            26 => Self::MountTree,
            31 => Self::TmpfsFile,
            32 => Self::TmpfsDirectory,
            33 => Self::MessageQueue,
            34 => Self::SysfsFile,
            14 => Self::Unknown,
            _ => Self::Unknown,
        }
    }

    /// Whether this descriptor's complete open-file-description state lives in
    /// a VFS side table rather than a Windows handle.
    ///
    /// Keep this predicate centralized: installation and fork restoration must
    /// accept exactly the same handleless kinds. Diverging lists previously
    /// made a valid cgroup or eventfd entry serialize successfully and then
    /// reject its own fork frame in the child.
    fn allows_missing_host_handle(self) -> bool {
        matches!(
            self,
            Self::EventFd
                | Self::Inotify
                | Self::UnixSocket
                | Self::Synthetic
                | Self::SyntheticDirectory
                | Self::Null
                | Self::Zero
                | Self::Random
                | Self::Full
                | Self::CgroupFile
                | Self::NetlinkSocket
                | Self::BpfProgram
                | Self::ProcSysctl
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FdFlags(pub u32);

impl FdFlags {
    pub const NONE: Self = Self(0);
    pub const CLOSE_ON_EXEC: Self = Self(1 << 0);
    pub const NONBLOCK: Self = Self(1 << 1);
    pub const BORROWED: Self = Self(1 << 2);
    /// The host handle was opened with `FILE_FLAG_OVERLAPPED`.
    pub const OVERLAPPED: Self = Self(1 << 3);
    /// The descriptor has a file position the table must maintain.
    pub const SEEKABLE: Self = Self(1 << 4);
    /// Opened with `O_APPEND`: every write goes to the current end of file.
    pub const APPEND: Self = Self(1 << 5);
    /// This pipe descriptor owns a readable endpoint.
    ///
    /// Windows does not expose an anonymous pipe endpoint's access mode through
    /// a non-blocking readiness API.  Recording it when the descriptor is
    /// created makes the Linux open-description property survive dup, fork and
    /// exec without probing the handle in an epoll hot path.
    pub const PIPE_READ_END: Self = Self(1 << 6);
    /// This pipe descriptor owns a writable endpoint.
    pub const PIPE_WRITE_END: Self = Self(1 << 7);
    /// The descriptor names an object but carries no read or write access.
    ///
    /// Linux `O_PATH` descriptors may be used for metadata, as `*at` dirfds,
    /// and through `/proc/self/fd`, but ordinary I/O on them fails with
    /// `EBADF`.
    pub const PATH_ONLY: Self = Self(1 << 8);
    /// Open file description permits read operations.
    pub const READ_ACCESS: Self = Self(1 << 9);
    /// Open file description permits write operations.
    pub const WRITE_ACCESS: Self = Self(1 << 10);
    pub const NOATIME: Self = Self(1 << 11);
    /// O_PATH names the proc symlink itself; data contains its retained target.
    pub const PROC_SYMLINK: Self = Self(1 << 12);
    /// The Winsock handle carries IP frames; readiness and data belong to the
    /// shared user TCP/IP engine, not the native UDP transport.
    pub const PACKET_SOCKET: Self = Self(1 << 13);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn has_pipe_access(self) -> bool {
        self.contains(Self::PIPE_READ_END) || self.contains(Self::PIPE_WRITE_END)
    }
}

#[inline]
fn validate_descriptor_flags(kind: FdKind, flags: FdFlags) -> Result<(), i32> {
    if kind == FdKind::Pipe && !flags.has_pipe_access() {
        return Err(EINVAL);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct FdEntry {
    pub raw: usize,
    pub kind: FdKind,
    pub flags: FdFlags,
    pub generation: u32,
    /// Stable identity of the Linux open file description.
    ///
    /// Descriptor numbers and table generations identify one table slot.  A
    /// `dup` family call creates another slot for the *same* open description,
    /// so subsystems such as epoll must use this identity instead.  The value is
    /// preserved across fork and shared by every duplicate.
    pub description_id: u64,
    /// Current file position for seekable descriptors.
    ///
    /// Overlapped handles have no kernel-maintained file pointer, so every
    /// request carries an explicit offset that the table advances on success.
    pub offset: u64,
}

struct FdTable {
    slots: fd_slots::Slots,
    next_generation: u32,
    next_description_id: u64,
}

impl FdTable {
    fn with_standard_streams() -> Self {
        let mut table = Self {
            slots: fd_slots::Slots::default(),
            next_generation: 1,
            next_description_id: 1,
        };
        platform::install_standard_streams(&mut table);
        table
    }

    fn insert_at(&mut self, fd: i32, raw: usize, kind: FdKind, flags: FdFlags) {
        let description_id = self.next_description_id;
        self.next_description_id = self.next_description_id.wrapping_add(1).max(1);
        self.insert_at_description(fd, raw, kind, flags, 0, description_id);
    }

    fn insert_at_description(
        &mut self,
        fd: i32,
        raw: usize,
        kind: FdKind,
        flags: FdFlags,
        offset: u64,
        description_id: u64,
    ) {
        if (0..MAX_FDS as i32).contains(&fd) {
            let generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1).max(1);
            self.slots.insert(
                fd as usize,
                FdEntry {
                    raw,
                    kind,
                    flags,
                    generation,
                    description_id,
                    offset,
                },
            );
        }
    }

    #[cfg(test)]
    /// Returns the lowest unused descriptor at or above `floor`.
    ///
    /// Callers hold the table's write lock while both selecting and installing
    /// the entry. That makes the choice atomic with respect to every other open
    /// or duplicate, as Linux requires for `F_DUPFD`.
    fn first_free_from(&self, floor: usize) -> Option<usize> {
        self.first_free_between(floor, MAX_FDS)
    }

    fn first_free_between(&self, floor: usize, ceiling: usize) -> Option<usize> {
        self.slots.first_free_between(floor, ceiling)
    }
}

static TABLE: OnceLock<RwLock<FdTable>> = OnceLock::new();

fn table() -> &'static RwLock<FdTable> {
    TABLE.get_or_init(|| RwLock::new(FdTable::with_standard_streams()))
}

/// Registers the descriptor-table handoff during process startup.
///
/// A forked child is a fresh process that never runs the parent's setup code, so
/// the hooks cannot be installed from ordinary program flow. Placing the
/// registrar in `.CRT$XCU` makes the C runtime call it before `main` in every
/// process, parent and child alike, which is early enough for the child to
/// adopt the table when it resumes inside `fork`.
#[cfg(windows)]
mod fork_handoff {
    #[cfg(test)]
    mod tests;

    const KEY: u64 = 0x5646_535f_4644_5332; // "VFS_FDS2"

    thread_local! {
        // Length query and buffer copy belong to one coordinator call on this
        // thread. Consume before native fork so no cached Vec crosses restore.
        static SNAPSHOT: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    }

    fn discard_snapshot() {
        SNAPSHOT.with_borrow_mut(|frame| *frame = None);
    }

    pub(super) fn is_owner() -> bool {
        kinakaze_runtime::fork_participant_is_owner(KEY, snapshot)
    }

    pub(super) fn watch_owner(event: usize) -> bool {
        kinakaze_runtime::watch_fork_owner(KEY, snapshot, event)
    }

    unsafe extern "system" fn prepare() -> i32 {
        discard_snapshot();
        if let Err(error) = super::publish_proc_fd_snapshot() {
            kinakaze_runtime::fork_diagnostic(format_args!(
                "kinakaze: VFS fork prepare failed phase=proc-fd-snapshot error={error}"
            ));
            return error;
        }
        if let Err(error) = super::unix::prepare_process_handoff() {
            return error;
        }
        match super::socket::prepare_process_fork() {
            Ok(()) => 0,
            Err(error) => {
                super::unix::finish_process_handoff(0);
                error
            }
        }
    }

    unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
        if super::fork_trace_enabled() && buffer.is_null() {
            super::trace_pipe_table("snapshot");
        }
        if buffer.is_null() {
            discard_snapshot();
            let payload = match super::serialize_fork_state() {
                Ok(payload) => payload,
                Err(error) => return -(error as isize),
            };
            let length = payload.len() as isize;
            SNAPSHOT.with_borrow_mut(|frame| *frame = Some(payload));
            return length;
        }
        let Some(payload) = SNAPSHOT.with_borrow_mut(Option::take) else {
            return -(super::EINVAL as isize);
        };
        if capacity < payload.len() {
            return -(super::ENOMEM as isize);
        }
        // SAFETY: the runtime provided the queried writable capacity.
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len()) };
        payload.len() as isize
    }

    unsafe extern "system" fn child(payload: *const u8, len: usize) -> i32 {
        if payload.is_null() && len != 0 {
            return super::EFAULT;
        }
        let bytes = if len == 0 {
            &[]
        } else {
            // SAFETY: the runtime owns this readable record for the call.
            unsafe { std::slice::from_raw_parts(payload, len) }
        };
        if super::restore_fork_state(bytes) {
            if super::fork_trace_enabled() {
                super::trace_pipe_table("restored");
            }
            0
        } else {
            super::EINVAL
        }
    }

    unsafe extern "system" fn parent(result: i32) {
        discard_snapshot();
        super::socket::finish_process_fork(result);
        super::unix::finish_process_handoff(result);
    }

    /// Installs the hooks with the executable's process coordinator.
    fn register() {
        use windows_sys::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW, GetProcAddress,
        };
        let mut module = std::ptr::null_mut();
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                register as *const () as *const u16,
                &mut module,
            )
        };
        let is_libc = if found != 0 && !module.is_null() {
            unsafe { GetProcAddress(module, c"kinakaze_abi_open".as_ptr().cast()).is_some() }
        } else {
            false
        };
        let priority = if is_libc { 50 } else { 100 };
        let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority,
            key: KEY,
            prepare: Some(prepare),
            snapshot: Some(snapshot),
            parent: Some(parent),
            child: Some(child),
        });
    }

    /// Runs during C runtime initialization.
    extern "C" fn initializer() {
        register();
    }

    // The linker collects `.CRT$XCU` entries into the initializer array the C
    // runtime walks before `main`.
    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static INITIALIZER: extern "C" fn() = initializer;
}

/// Publishes the current descriptor table as exact logical procfs magic-link
/// metadata. Fork treats this snapshot as mandatory state: the runtime copies
/// it to the registered child PID before allowing that PID to escape.
#[cfg(windows)]
pub fn publish_proc_fd_snapshot() -> Result<(), i32> {
    job::ensure_registered();
    let pid = job::process_id();
    let links = procfs::local_fd_links()?;
    kinakaze_runtime::job::replace_fd_links(pid, &links).map_err(procfs::fd_link_error)
}

#[cfg(windows)]
fn trace_pipe_table(phase: &str) {
    let Ok(table) = table().read() else { return };
    let pipes: Vec<String> = table
        .slots
        .enumerated()
        .filter_map(|(fd, entry)| {
            let entry = entry.as_ref()?;
            (entry.kind == FdKind::Pipe)
                .then(|| format!("{fd}={:#x}/flags={:#x}", entry.raw, entry.flags.0))
        })
        .collect();
    eprintln!(
        "kinakaze vfs: {phase} pipe table in pid {} [{}]",
        std::process::id(),
        pipes.join(", ")
    );
}

/// Contents of a synthetic descriptor, keyed by descriptor number.
///
/// `/proc` files have no host handle, so their bytes live here and the table
/// entry only records the position. The generation is stored alongside so a
/// recycled descriptor number cannot read a previous file's text.
/// Synthetic descriptor contents, paired with the generation that owns them.
#[cfg(windows)]
type SyntheticTable = std::collections::HashMap<i32, (u32, Vec<u8>)>;

#[cfg(windows)]
static SYNTHETIC: OnceLock<RwLock<SyntheticTable>> = OnceLock::new();

#[cfg(windows)]
fn synthetic() -> &'static RwLock<SyntheticTable> {
    SYNTHETIC.get_or_init(|| RwLock::new(SyntheticTable::new()))
}

/// Installs a memory-backed descriptor holding `contents` and its opened identity.
#[cfg(windows)]
pub fn install_procfs_file(path: &str, contents: Vec<u8>, flags: FdFlags) -> Result<i32, i32> {
    if path.is_empty() || path.contains('\0') {
        return Err(EINVAL);
    }
    let mut descriptions = synthetic().write().map_err(|_| EIO)?;
    let mut paths = proc_sysctl_paths().lock().map_err(|_| EIO)?;
    let fd = install(0, FdKind::Synthetic, flags.union(FdFlags::SEEKABLE))?;
    let entry = get(fd)?;
    descriptions.insert(fd, (entry.generation, contents));
    paths.insert(fd, path.to_owned());
    Ok(fd)
}

/// Identity retained when a read-only synthetic file was opened. Keeping this
/// alongside its generation and contents lets proc-fd publication, dup, fork
/// and exec preserve it without consulting a subsequently reused fd number.
#[cfg(windows)]
pub(crate) fn synthetic_file_path(fd: i32) -> Result<String, i32> {
    let entry = get(fd)?;
    if entry.kind != FdKind::Synthetic {
        return Err(EBADF);
    }
    let descriptions = synthetic().read().map_err(|_| EIO)?;
    if !descriptions
        .get(&fd)
        .is_some_and(|(generation, _)| *generation == entry.generation)
    {
        return Err(EBADF);
    }
    proc_sysctl_paths()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)
}

/// Installs a descriptor representing a synthetic directory.
///
/// The listing is generated at `getdents` time from the stored path, so the
/// contents slot holds the path rather than file bytes.
#[cfg(windows)]
pub fn install_procfs_directory(path: String, flags: FdFlags) -> Result<i32, i32> {
    let fd = install(0, FdKind::SyntheticDirectory, flags)?;
    let entry = get(fd)?;
    synthetic()
        .write()
        .map_err(|_| EIO)?
        .insert(fd, (entry.generation, path.into_bytes()));
    Ok(fd)
}

static CGROUP_PATHS: OnceLock<std::sync::Mutex<std::collections::HashMap<i32, String>>> =
    OnceLock::new();

// Logical identities for both read-only synthetic files and writable sysctls.
// The synthetic publication lock protects lifetime/generation for both kinds.
static PROC_SYSCTL_PATHS: OnceLock<std::sync::Mutex<std::collections::HashMap<i32, String>>> =
    OnceLock::new();

fn cgroup_paths() -> &'static std::sync::Mutex<std::collections::HashMap<i32, String>> {
    CGROUP_PATHS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn proc_sysctl_paths() -> &'static std::sync::Mutex<std::collections::HashMap<i32, String>> {
    PROC_SYSCTL_PATHS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub(crate) fn cgroup_file_path(fd: i32) -> Result<String, i32> {
    let entry = get(fd)?;
    if entry.kind != FdKind::CgroupFile {
        return Err(EBADF);
    }
    cgroup_paths()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)
}

/// Installs a cgroup v2 control file descriptor.
#[cfg(windows)]
pub fn install_cgroup_file(path: String, flags: FdFlags) -> Result<i32, i32> {
    let contents = cgroup::read_file(&path)?;
    let mut synthetic = synthetic().write().map_err(|_| EIO)?;
    let mut paths = cgroup_paths().lock().map_err(|_| EIO)?;
    let fd = install(0, FdKind::CgroupFile, flags.union(FdFlags::SEEKABLE))?;
    let entry = get(fd)?;
    synthetic.insert(fd, (entry.generation, contents));
    paths.insert(fd, path);
    Ok(fd)
}

pub(crate) fn proc_sysctl_file_path(fd: i32) -> Result<String, i32> {
    let entry = get(fd)?;
    if entry.kind != FdKind::ProcSysctl {
        return Err(EBADF);
    }
    proc_sysctl_paths()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)
}

/// Copies the side-table portion of a synthetic open file description.
///
/// `dup`, `dup2`, and `F_DUPFD` create another descriptor for the same Linux
/// file object.  These objects have no Windows handle, so duplicating only the
/// main descriptor-table slot would leave the new fd without its contents or
/// control-file identity.  The generation check prevents a concurrently reused
/// source fd from donating stale state to the copy.
#[cfg(windows)]
pub fn duplicate_synthetic_description(oldfd: i32, newfd: i32) -> Result<(), i32> {
    let source = get(oldfd)?;
    let target = get(newfd)?;
    if source.kind != target.kind
        || !matches!(
            source.kind,
            FdKind::Synthetic
                | FdKind::SyntheticDirectory
                | FdKind::CgroupFile
                | FdKind::ProcSysctl
        )
    {
        return Err(EINVAL);
    }

    // Match the serialization lock order so fork/exec and dup cannot deadlock.
    let mut descriptions = synthetic().write().map_err(|_| EIO)?;
    let contents = descriptions
        .get(&oldfd)
        .filter(|(generation, _)| *generation == source.generation)
        .map(|(_, contents)| contents.clone())
        .ok_or(EBADF)?;
    let mut cgroup = cgroup_paths().lock().map_err(|_| EIO)?;
    let mut sysctls = proc_sysctl_paths().lock().map_err(|_| EIO)?;

    match source.kind {
        FdKind::SyntheticDirectory => {}
        FdKind::CgroupFile => {
            let path = cgroup.get(&oldfd).cloned().ok_or(EBADF)?;
            cgroup.insert(newfd, path);
        }
        FdKind::Synthetic | FdKind::ProcSysctl => {
            let path = sysctls.get(&oldfd).cloned().ok_or(EBADF)?;
            sysctls.insert(newfd, path);
        }
        _ => unreachable!(),
    }
    descriptions.insert(newfd, (target.generation, contents));
    Ok(())
}

/// Installs one writable proc-sysctl open file description.
#[cfg(windows)]
pub fn install_proc_sysctl_file(path: String, flags: FdFlags) -> Result<i32, i32> {
    let contents = procfs::pinned(|| procfs::read_file(&path))?;
    let mut synthetic = synthetic().write().map_err(|_| EIO)?;
    let mut paths = proc_sysctl_paths().lock().map_err(|_| EIO)?;
    let fd = install(0, FdKind::ProcSysctl, flags.union(FdFlags::SEEKABLE))?;
    let entry = get(fd)?;
    synthetic.insert(fd, (entry.generation, contents));
    paths.insert(fd, path);
    Ok(fd)
}

/// Installs a special character device descriptor (/dev/null, /dev/zero, /dev/urandom, etc.).
pub fn install_dev_special(kind: FdKind, flags: FdFlags) -> Result<i32, i32> {
    install(0, kind, flags)
}

/// Returns the list of currently open file descriptor numbers.
pub fn list_open_fds() -> Vec<i32> {
    let Ok(table) = table().read() else {
        return vec![0, 1, 2];
    };
    table
        .slots
        .enumerated()
        .filter_map(|(i, slot)| if slot.is_some() { Some(i as i32) } else { None })
        .collect()
}

/// Returns the path a synthetic directory descriptor refers to.
#[cfg(windows)]
pub fn synthetic_directory_path(fd: i32) -> Result<String, i32> {
    let entry = get(fd)?;
    if entry.kind != FdKind::SyntheticDirectory {
        return Err(ENOTDIR);
    }
    let synthetic = synthetic().read().map_err(|_| EIO)?;
    let (generation, bytes) = synthetic.get(&fd).ok_or(EBADF)?;
    if *generation != entry.generation {
        return Err(EBADF);
    }
    String::from_utf8(bytes.clone()).map_err(|_| EIO)
}

/// Reads from a synthetic descriptor at its current offset.
#[cfg(windows)]
fn read_synthetic(fd: i32, entry: FdEntry, buffer: &mut [u8]) -> Result<usize, i32> {
    let synthetic = synthetic().read().map_err(|_| EIO)?;
    let (generation, contents) = synthetic.get(&fd).ok_or(EBADF)?;
    if *generation != entry.generation {
        return Err(EBADF);
    }
    let start = (entry.offset as usize).min(contents.len());
    let available = &contents[start..];
    let count = available.len().min(buffer.len());
    buffer[..count].copy_from_slice(&available[..count]);
    Ok(count)
}

#[cfg(windows)]
fn read_proc_sysctl(fd: i32, entry: FdEntry, buffer: &mut [u8]) -> Result<usize, i32> {
    if !entry.flags.contains(FdFlags::READ_ACCESS) {
        return Err(EBADF);
    }
    let path = proc_sysctl_file_path(fd)?;
    let contents = procfs::pinned(|| procfs::read_file(&path))?;
    let start = usize::try_from(entry.offset)
        .unwrap_or(usize::MAX)
        .min(contents.len());
    let count = contents.len().saturating_sub(start).min(buffer.len());
    buffer[..count].copy_from_slice(&contents[start..start + count]);
    Ok(count)
}

/// Reports the size of a synthetic descriptor's contents.
#[cfg(windows)]
pub fn synthetic_size(fd: i32) -> Result<u64, i32> {
    let entry = get(fd)?;
    let synthetic = synthetic().read().map_err(|_| EIO)?;
    let (generation, contents) = synthetic.get(&fd).ok_or(EBADF)?;
    if *generation != entry.generation {
        return Err(EBADF);
    }
    Ok(contents.len() as u64)
}

/// Releases a synthetic descriptor's contents.
#[cfg(windows)]
fn forget_synthetic(fd: i32, generation: u32) {
    let Ok(mut descriptions) = synthetic().write() else {
        return;
    };
    if !descriptions
        .get(&fd)
        .is_some_and(|(stored, _)| *stored == generation)
    {
        return;
    }
    descriptions.remove(&fd);
    // Keep the publication lock through every side-table removal. Control-file
    // creators hold it while installing their fd and associated identities.
    if let Ok(mut map) = cgroup_paths().lock() {
        map.remove(&fd);
    }
    if let Ok(mut map) = proc_sysctl_paths().lock() {
        map.remove(&fd);
    }
}

/// Serializes the descriptor table so a forked child can restore it.
///
/// DLL globals are not copied by the process-cloning backend, but the managed
/// arena is, so the table travels as a byte payload staged in the arena. Ordinary
/// handle values stay valid because the child inherits them. Socket values are
/// placeholders until the Winsock handoff section reconstructs them for the
/// target process.
///
/// The format is a 4-byte count followed by one 36-byte record per descriptor:
/// fd (4), raw handle (8), kind (4), flags (4), offset (8), and the stable open
/// description identity (8).
#[cfg(windows)]
pub fn serialize_table() -> Result<Vec<u8>, i32> {
    let table = table().read().map_err(|_| EIO)?;
    let mut payload = Vec::new();
    let mut count = 0u32;
    payload.extend_from_slice(&count.to_le_bytes());
    for (fd, slot) in table.slots.enumerated() {
        let Some(entry) = slot else { continue };
        // A Windows IoRing is neither an inheritable HANDLE nor a reconstructible
        // userspace queue.  Reinstalling its placeholder fd would claim a ring
        // exists when every operation would address missing state.
        if entry.kind == FdKind::IoRing {
            return Err(EOPNOTSUPP);
        }
        // Borrowed standard streams are re-created by the child's own
        // initialization, so re-installing them would leak a duplicate.
        if entry.flags.contains(FdFlags::BORROWED) {
            continue;
        }
        payload.extend_from_slice(&(fd as u32).to_le_bytes());
        payload.extend_from_slice(&(entry.raw as u64).to_le_bytes());
        payload.extend_from_slice(&entry.kind.fork_code().to_le_bytes());
        payload.extend_from_slice(&entry.flags.0.to_le_bytes());
        payload.extend_from_slice(&entry.offset.to_le_bytes());
        payload.extend_from_slice(&entry.description_id.to_le_bytes());
        count += 1;
    }
    payload[..4].copy_from_slice(&count.to_le_bytes());
    if fork_trace_enabled() {
        let entries: Vec<String> = table
            .slots
            .enumerated()
            .filter_map(|(fd, slot)| {
                slot.as_ref().map(|e| {
                    format!(
                        "{fd}:raw={:#x},kind={:?},flags={:#x}",
                        e.raw, e.kind, e.flags.0
                    )
                })
            })
            .collect();
        eprintln!(
            "kinakaze vfs: serialize_table in pid {} slots=[{}] serialized_count={count}",
            std::process::id(),
            entries.join(", ")
        );
    }
    Ok(payload)
}

/// Restores descriptors serialized by [`serialize_table`].
///
/// Existing standard streams are left alone; only the inherited descriptors are
/// re-installed at their original numbers so the child sees the same fds.
#[cfg(windows)]
pub fn restore_table(payload: &[u8]) -> Result<usize, ()> {
    const RECORD: usize = 36;
    if payload.len() < 4 {
        return Err(());
    }
    let count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let expected = count
        .checked_mul(RECORD)
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(())?;
    if payload.len() != expected {
        return Err(());
    }

    let mut records = Vec::with_capacity(count);
    let mut seen = std::collections::HashSet::with_capacity(count);
    for index in 0..count {
        let start = 4 + index * RECORD;
        let record = &payload[start..start + RECORD];
        let read_u32 = |at: usize| {
            u32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]])
        };
        let fd = read_u32(0) as usize;
        let raw = u64::from_le_bytes(record[4..12].try_into().unwrap_or_default()) as usize;
        let kind = FdKind::from_fork_code(read_u32(12));
        let flags = FdFlags(read_u32(16));
        let offset = u64::from_le_bytes(record[20..28].try_into().unwrap_or_default());
        let description_id = u64::from_le_bytes(record[28..36].try_into().unwrap_or_default());
        if fd >= MAX_FDS
            || !seen.insert(fd)
            || description_id == 0
            || kind == FdKind::IoRing
            || (raw == 0 && !kind.allows_missing_host_handle())
            || validate_descriptor_flags(kind, flags).is_err()
        {
            return Err(());
        }
        records.push((fd, raw, kind, flags, offset, description_id));
    }

    let mut table = table().write().map_err(|_| ())?;
    // In-flight parent syscalls do not become child descriptors.
    table.slots.clear_reservations();
    for (fd, raw, kind, flags, offset, description_id) in records {
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze: restore_table fd={} raw={:#x} kind={:?} flags={:#x}",
                fd, raw, kind, flags.0
            );
        }
        if kind == FdKind::Pipe && std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
            eprintln!(
                "kinakaze pipe: pid={} restore fd={fd} raw={raw:#x} flags={:#x}",
                std::process::id(),
                flags.0
            );
        }
        table.next_description_id = table
            .next_description_id
            .max(description_id.wrapping_add(1).max(1));
        table.insert_at_description(fd as i32, raw, kind, flags, offset, description_id);
    }
    Ok(count)
}

/// Returns the live Winsock descriptors in descriptor-number order.
///
/// Windows handle inheritance does not establish an independent Winsock
/// provider reference in the child. The socket fork handoff uses this stable
/// list to exchange one `WSAPROTOCOL_INFO` record per descriptor.
#[cfg(windows)]
pub(crate) fn fork_socket_entries() -> Result<Vec<(i32, usize)>, i32> {
    let table = table().read().map_err(|_| EIO)?;
    Ok(table
        .slots
        .enumerated()
        .filter_map(|(fd, slot)| {
            let entry = slot.as_ref()?;
            (entry.kind == FdKind::Socket).then_some((fd as i32, entry.raw))
        })
        .collect())
}

#[cfg(windows)]
pub(crate) fn exec_socket_entries() -> Result<Vec<(i32, usize)>, i32> {
    let table = table().read().map_err(|_| EIO)?;
    Ok(table
        .slots
        .enumerated()
        .filter_map(|(fd, slot)| {
            let entry = slot.as_ref()?;
            (entry.kind == FdKind::Socket
                && !entry.flags.contains(FdFlags::CLOSE_ON_EXEC)
                && !entry.flags.contains(FdFlags::BORROWED))
            .then_some((fd as i32, entry.raw))
        })
        .collect())
}

/// Replaces the inherited socket handle after `WSASocketW` reconstructs the
/// child process's own provider reference.
#[cfg(windows)]
pub(crate) fn replace_fork_socket(
    fd: i32,
    inherited: usize,
    replacement: usize,
) -> Result<(), i32> {
    let mut table = table().write().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get_mut(fd as usize)
        .and_then(Option::as_mut)
        .ok_or(EBADF)?;
    if entry.kind != FdKind::Socket || entry.raw != inherited {
        return Err(EBADF);
    }
    entry.raw = replacement;
    // Sockets cross process boundaries only through WSADuplicateSocketW. Raw
    // handle inheritance would leave the child without its own provider ref.
    platform::set_inheritable(replacement, false);
    Ok(())
}

#[cfg(windows)]
// F3 requires an opened identity for every read-only synthetic file.
const FORK_STATE_MAGIC: u64 = 0x4352_5956_4653_4730; // "CRYVFSG0"
#[cfg(windows)]
const EXEC_IMAGE_SECTION: u32 = 9;

#[cfg(windows)]
fn append_fork_section(frame: &mut Vec<u8>, tag: u32, payload: &[u8]) -> Result<(), i32> {
    frame.extend_from_slice(&tag.to_le_bytes());
    let length = u32::try_from(payload.len()).map_err(|_| EIO)?;
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(payload);
    while !frame.len().is_multiple_of(8) {
        frame.push(0);
    }
    Ok(())
}

#[cfg(windows)]
fn fork_section_range(frame: &[u8], wanted: u32) -> Option<std::ops::Range<usize>> {
    if frame.len() < 16 || u64::from_le_bytes(frame[0..8].try_into().ok()?) != FORK_STATE_MAGIC {
        return None;
    }
    let count = u32::from_le_bytes(frame[8..12].try_into().ok()?) as usize;
    let mut cursor = 16usize;
    for _ in 0..count {
        let header_end = cursor.checked_add(8)?;
        let header = frame.get(cursor..header_end)?;
        let tag = u32::from_le_bytes(header[0..4].try_into().ok()?);
        let len = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
        let start = header_end;
        let end = start.checked_add(len)?;
        frame.get(start..end)?;
        if tag == wanted {
            return Some(start..end);
        }
        cursor = end.checked_next_multiple_of(8)?;
    }
    None
}

/// Complete VFS process state, excluding host objects whose API offers no fork
/// or duplication contract (currently IoRing and ConPTY controller objects).
#[cfg(windows)]
pub fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    let sections = [
        (1, serialize_table()?),
        (2, serialize_synthetic()?),
        (3, fs::serialize_cwd()?),
        (4, signal::serialize_fork_state()?),
        (5, unix::serialize_fork_state()?),
        (14, fifo::serialize_matching(|_| true)?),
        (6, epoll::serialize_fork_state()?),
        (7, socket::serialize_process_fork()?),
        (12, mount::serialize_fork_state()?),
        (15, path::serialize_root()?),
        (16, mount::overlay::serialize(|_| true)?),
        (28, mount::native::serialize(|_| true)?),
        (29, ofd::serialize(|_| true)?),
        (17, mount::api::serialize(|_| true)?),
        (18, mount::policy::serialize()?),
        (20, pipe_inode::serialize(|_| true)?),
        (21, time_namespace::serialize(true)?),
        (23, namespaces::serialize()?),
        (24, usernet::serialize(|_| true)?),
        (25, keyring::serialize(true)?),
        (26, tmpfs::serialize()?),
        (27, tty::serialize()?),
        (22, user_namespace::serialize()?),
        (10, netlink::serialize_matching(|_| true)?),
        (11, bpf::serialize_matching(|_| true)?),
        (13, Vec::new()),
    ];
    let mut frame = Vec::new();
    frame.extend_from_slice(&FORK_STATE_MAGIC.to_le_bytes());
    frame.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    for (tag, payload) in sections {
        append_fork_section(&mut frame, tag, &payload)?;
    }
    Ok(frame)
}

#[cfg(windows)]
pub fn restore_fork_state(frame: &[u8]) -> bool {
    if frame.len() < 16
        || u64::from_le_bytes(frame[0..8].try_into().unwrap_or_default()) != FORK_STATE_MAGIC
    {
        return false;
    }
    // Paths used by cwd and descriptor restoration must already see the shared
    // mount state. No copied parent-local view/handle may be used first.
    if let Some(range) = fork_section_range(frame, 19)
        && !credentials::restore_exec(&frame[range])
    {
        return false;
    }
    let Some(root_range) = fork_section_range(frame, 15) else {
        return false;
    };
    if !path::restore_root(&frame[root_range]) {
        return false;
    }
    let Some(mount_range) = fork_section_range(frame, 12) else {
        return false;
    };
    if !mount::restore_fork_state(&frame[mount_range]) {
        return false;
    }
    let count = u32::from_le_bytes(frame[8..12].try_into().unwrap_or_default()) as usize;
    let mut cursor = 16usize;
    for _ in 0..count {
        if cursor.checked_add(8).is_none_or(|end| end > frame.len()) {
            return false;
        }
        let tag = u32::from_le_bytes(frame[cursor..cursor + 4].try_into().unwrap_or_default());
        let len = u32::from_le_bytes(frame[cursor + 4..cursor + 8].try_into().unwrap_or_default())
            as usize;
        let start = cursor + 8;
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        let Some(payload) = frame.get(start..end) else {
            return false;
        };
        let ok = match tag {
            1 => restore_table(payload).is_ok(),
            2 => restore_synthetic(payload),
            3 => fs::restore_cwd(payload),
            4 => signal::restore_fork_state(payload),
            5 => unix::restore_fork_state(payload),
            6 => epoll::restore_fork_state(payload),
            7 => socket::restore_process_fork(payload),
            12 => true, // restored before any path-dependent section above
            15 => true,
            16 => mount::overlay::restore(payload),
            28 => mount::native::restore(payload),
            29 => ofd::restore(payload),
            17 => mount::api::restore(payload),
            18 => mount::policy::restore(payload),
            19 => true, // credentials restored before path-dependent sections
            20 => pipe_inode::restore(payload),
            21 => time_namespace::restore(payload),
            23 => namespaces::restore(payload),
            24 => usernet::restore(payload),
            25 => keyring::restore(payload),
            26 => tmpfs::restore(payload),
            27 => tty::restore(payload),
            22 => user_namespace::restore(payload),
            10 => netlink::restore(payload),
            11 => bpf::restore(payload),
            13 => record_lock::restore(payload),
            14 => fifo::restore_fork_state(payload),
            8 => restore_environment(payload),
            // The executable copy of kinakaze-vfs reads this section before the
            // guest libc is mapped. Reaching it here means the stable image was
            // structurally valid; libc itself does not need a second copy.
            EXEC_IMAGE_SECTION => true,
            _ => false,
        };
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze: restore_fork_state section tag={} len={} ok={} cursor={}",
                tag, len, ok, end
            );
        }
        if !ok && let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
            use std::io::Write;
            let path = std::path::PathBuf::from(directory)
                .join(format!("exec-restore-{}.log", std::process::id()));
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "failed section tag={tag} len={len} cursor={end}");
            }
        }
        if !ok {
            kinakaze_runtime::fork_diagnostic(format_args!(
                "kinakaze: VFS fork restore failed section={tag} length={len}"
            ));
            if fork_trace_enabled() {
                eprintln!("kinakaze: restore_fork_state FAILED at tag={}", tag);
            }
            return false;
        }
        cursor = end.next_multiple_of(8);
    }
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze: restore_fork_state cursor={} frame.len()={} match={}",
            cursor,
            frame.len(),
            cursor == frame.len()
        );
    }
    cursor == frame.len()
}

#[cfg(windows)]
fn serialize_synthetic() -> Result<Vec<u8>, i32> {
    serialize_synthetic_matching(|_| true)
}

#[cfg(windows)]
fn serialize_synthetic_matching(mut keep: impl FnMut(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = synthetic().read().map_err(|_| EIO)?;
    let cgroup_paths = cgroup_paths().lock().map_err(|_| EIO)?;
    let proc_sysctl_paths = proc_sysctl_paths().lock().map_err(|_| EIO)?;
    let mut payload = Vec::new();
    let mut count = 0u32;
    payload.extend_from_slice(&count.to_le_bytes());
    for (fd, (_, bytes)) in table.iter() {
        if !keep(*fd) {
            continue;
        }
        let path = cgroup_paths
            .get(fd)
            .or_else(|| proc_sysctl_paths.get(fd))
            .map_or(&[][..], |path| path.as_bytes());
        payload.extend_from_slice(&fd.to_le_bytes());
        payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
        payload.extend_from_slice(bytes);
        payload.extend_from_slice(path);
        while !payload.len().is_multiple_of(8) {
            payload.push(0);
        }
        count += 1;
    }
    payload[..4].copy_from_slice(&count.to_le_bytes());
    Ok(payload)
}

#[cfg(windows)]
fn restore_synthetic(payload: &[u8]) -> bool {
    if payload.len() < 4 {
        return false;
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    let mut cursor = 4usize;
    let mut restored = SyntheticTable::new();
    let mut restored_cgroup_paths = std::collections::HashMap::new();
    let mut restored_proc_sysctl_paths = std::collections::HashMap::new();
    for _ in 0..count {
        if cursor + 12 > payload.len() {
            return false;
        }
        let fd = i32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap_or_default());
        let len = u32::from_le_bytes(
            payload[cursor + 4..cursor + 8]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let path_len = u32::from_le_bytes(
            payload[cursor + 8..cursor + 12]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let start = cursor + 12;
        let Some(contents_end) = start.checked_add(len) else {
            return false;
        };
        let Some(path_end) = contents_end.checked_add(path_len) else {
            return false;
        };
        let Some(bytes) = payload.get(start..contents_end) else {
            return false;
        };
        let Some(path_bytes) = payload.get(contents_end..path_end) else {
            return false;
        };
        let Ok(entry) = get(fd) else { return false };
        match entry.kind {
            FdKind::SyntheticDirectory if path_bytes.is_empty() => {}
            FdKind::Synthetic if !path_bytes.is_empty() => {
                let Ok(path) = std::str::from_utf8(path_bytes) else {
                    return false;
                };
                if path.contains('\0') {
                    return false;
                }
                restored_proc_sysctl_paths.insert(fd, path.to_owned());
            }
            FdKind::CgroupFile if !path_bytes.is_empty() => {
                let Ok(path) = std::str::from_utf8(path_bytes) else {
                    return false;
                };
                restored_cgroup_paths.insert(fd, path.to_owned());
            }
            FdKind::ProcSysctl if !path_bytes.is_empty() => {
                let Ok(path) = std::str::from_utf8(path_bytes) else {
                    return false;
                };
                if !procfs::pinned(|| procfs::writable(path)) {
                    return false;
                }
                restored_proc_sysctl_paths.insert(fd, path.to_owned());
            }
            _ => return false,
        }
        restored.insert(fd, (entry.generation, bytes.to_vec()));
        cursor = path_end.next_multiple_of(8);
    }
    if cursor != payload.len() {
        return false;
    }
    let Ok(mut table) = synthetic().write() else {
        return false;
    };
    *table = restored;
    let Ok(mut paths) = cgroup_paths().lock() else {
        return false;
    };
    *paths = restored_cgroup_paths;
    let Ok(mut paths) = proc_sysctl_paths().lock() else {
        return false;
    };
    *paths = restored_proc_sysctl_paths;
    true
}

/// Maps a Win32 error code onto the closest Linux errno.
///
/// Shared by the descriptor I/O path and the file operations so a given Windows
/// failure always reaches the guest as the same errno.
#[cfg(windows)]
pub fn errno_from_win32(error: u32) -> i32 {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_BUFFER_OVERFLOW,
        ERROR_DIR_NOT_EMPTY, ERROR_DIRECTORY, ERROR_DISK_FULL, ERROR_FILE_EXISTS,
        ERROR_FILE_NOT_FOUND, ERROR_FILENAME_EXCED_RANGE, ERROR_HANDLE_DISK_FULL,
        ERROR_INVALID_HANDLE, ERROR_INVALID_NAME, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
        ERROR_NEGATIVE_SEEK, ERROR_NO_DATA, ERROR_NO_MORE_FILES, ERROR_NOT_SAME_DEVICE,
        ERROR_OPERATION_ABORTED, ERROR_PATH_NOT_FOUND, ERROR_PIPE_NOT_CONNECTED,
        ERROR_SHARING_VIOLATION, ERROR_TOO_MANY_OPEN_FILES, ERROR_WRITE_PROTECT,
    };
    match error {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_NO_MORE_FILES => ENOENT,
        ERROR_ACCESS_DENIED | ERROR_WRITE_PROTECT => EACCES,
        ERROR_SHARING_VIOLATION => EBUSY,
        ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS => EEXIST,
        ERROR_INVALID_HANDLE => EBADF,
        ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED => EPIPE,
        // A message-mode pipe read that did not fit the buffer. Only datagram
        // sockets can produce this, and they translate it into truncation.
        ERROR_MORE_DATA => EMSGSIZE,
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL => ENOSPC,
        ERROR_DIR_NOT_EMPTY => ENOTEMPTY,
        ERROR_DIRECTORY => ENOTDIR,
        ERROR_NOT_SAME_DEVICE => EXDEV,
        ERROR_TOO_MANY_OPEN_FILES => EMFILE,
        ERROR_FILENAME_EXCED_RANGE | ERROR_BUFFER_OVERFLOW => ENAMETOOLONG,
        ERROR_INVALID_NAME | ERROR_INVALID_PARAMETER | ERROR_NEGATIVE_SEEK => EINVAL,
        ERROR_OPERATION_ABORTED => EINTR,
        _ => EIO,
    }
}

pub fn get(fd: i32) -> Result<FdEntry, i32> {
    if fd < 0 {
        return Err(EBADF);
    }
    let table = table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    drop(table);
    #[cfg(windows)]
    return ofd::refresh(entry);
    #[cfg(not(windows))]
    Ok(entry)
}

/// Finds any live descriptor for one Linux open file description.
///
/// The descriptor number returned is an alias suitable for side tables after
/// the originally registered number has been closed.  Selection is stable only
/// for the returned snapshot; callers which keep it across a blocking operation
/// must tolerate a concurrent close just as they would for an ordinary fd.
pub fn get_by_description_id(description_id: u64) -> Option<(i32, FdEntry)> {
    if description_id == 0 {
        return None;
    }
    let table = table().read().ok()?;
    table.slots.enumerated().find_map(|(fd, slot)| {
        let entry = slot.as_ref()?;
        (entry.description_id == description_id).then_some((fd as i32, *entry))
    })
}

/// Installs a descriptor that has no host handle yet.
///
/// An `AF_UNIX` socket is usable before it is bound or connected, at which point
/// there is nothing to put in `raw`; the handle arrives later through
/// [`set_raw`].
pub fn install_handleless(kind: FdKind, flags: FdFlags) -> Result<i32, i32> {
    install_handleless_with(kind, flags, |_| Ok(()))
}

/// Installs a handleless descriptor and initializes its side table under the same
/// lock.
///
/// `register` runs while the descriptor table is still held, so the fd cannot
/// become visible to another thread before its side-table state exists. Doing
/// these separately leaves a window where a concurrent operation sees a
/// descriptor of the right kind with nothing behind it.
pub fn install_handleless_with(
    kind: FdKind,
    flags: FdFlags,
    register: impl FnOnce(i32) -> Result<(), i32>,
) -> Result<i32, i32> {
    if !kind.allows_missing_host_handle() {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;
    let limit = job::current_nofile_limit()?;
    let mut table = table().write().map_err(|_| EIO)?;
    let fd = table.first_free_between(0, limit).ok_or(EMFILE)? as i32;
    table.insert_at(fd, 0, kind, flags);
    if let Err(error) = register(fd) {
        table.slots.remove(fd as usize);
        return Err(error);
    }
    Ok(fd)
}

/// Stores a host handle for a descriptor that was installed without one.
///
/// The descriptor table is what `epoll` and the fork handoff read, so a handle
/// that stayed in a side table would be invisible to both.
pub fn set_raw(fd: i32, raw: usize) -> Result<(), i32> {
    if fd < 0 {
        return Err(EBADF);
    }
    let mut table = table().write().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get_mut(fd as usize)
        .and_then(|slot| slot.as_mut())
        .ok_or(EBADF)?;
    #[cfg(windows)]
    let inheritance =
        exec_inheritance::DescriptorInheritance::prepare(raw, entry.kind, entry.flags)?;
    entry.raw = raw;
    if entry.kind == FdKind::Pipe && std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
        eprintln!(
            "kinakaze pipe: pid={} set fd={fd} raw={raw:#x} flags={:#x}",
            std::process::id(),
            entry.flags.0
        );
    }
    #[cfg(windows)]
    inheritance.commit();
    Ok(())
}

/// Updates `FD_CLOEXEC` without changing fork inheritance.
///
/// Linux applies this flag only when an image is replaced by `exec`; it does not
/// remove the descriptor from a `fork` child. Windows' handle-inherit bit cannot
/// represent both operations, so handles stay inheritable for the fork backend
/// and exec filters the descriptor-table handles during process creation.
pub fn set_close_on_exec(fd: i32, enabled: bool) -> Result<(), i32> {
    if fd < 0 {
        return Err(EBADF);
    }
    let mut table = table().write().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get_mut(fd as usize)
        .and_then(|slot| slot.as_mut())
        .ok_or(EBADF)?;
    if enabled {
        entry.flags = entry.flags.union(FdFlags::CLOSE_ON_EXEC);
    } else {
        entry.flags = FdFlags(entry.flags.0 & !FdFlags::CLOSE_ON_EXEC.0);
    }
    Ok(())
}

/// Runs a Windows process creation with guest descriptors excluded from the
/// ambient Win32 handle-inheritance set.
///
/// `CreateProcessW` inherits every handle whose inherit bit is set, whereas an
/// `exec` replacement must receive only the descriptors the runtime explicitly
/// reconstructs.  The fork backend needs those bits set at rest, so exec holds
/// the descriptor-table write lock, clears them for the one process-creation
/// transaction, and restores them before allowing descriptor operations again.
/// Handles supplied explicitly as the new process's standard streams are
/// duplicates owned by `std::process::Command` and are therefore not in this
/// table.
#[cfg(windows)]
mod exec_inheritance;

#[cfg(windows)]
pub fn with_exec_handle_filter<T>(operation: impl FnOnce() -> T) -> std::io::Result<T> {
    exec_inheritance::with_filter(false, None, operation)
}

/// Retain exactly the guest descriptors without FD_CLOEXEC across exec.
#[cfg(windows)]
pub fn with_execve_handle_filter<T>(operation: impl FnOnce() -> T) -> std::io::Result<T> {
    exec_inheritance::with_filter(true, None, operation)
}

/// A descriptor generation fence for a fresh-image spawn. Validate under the
/// process-creation table lock so concurrent close/dup cannot change inheritance
/// between serialization and CreateProcessW. A mismatch takes the full fork path.
#[cfg(windows)]
pub struct ExecFdSnapshot(Vec<(usize, FdEntry)>);
#[cfg(windows)]
pub fn exec_descriptor_snapshot() -> Result<ExecFdSnapshot, i32> {
    let table = table().read().map_err(|_| EIO)?;
    Ok(ExecFdSnapshot(
        table
            .slots
            .enumerated()
            .filter_map(|(fd, entry)| entry.map(|entry| (fd, entry)))
            .collect(),
    ))
}
#[cfg(windows)]
pub fn with_checked_exec_handle_filter<T>(
    snapshot: &ExecFdSnapshot,
    operation: impl FnOnce() -> T,
) -> std::io::Result<T> {
    exec_inheritance::with_filter(true, Some(snapshot), operation)
}

static EXEC_ENVIRONMENT: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

#[cfg(windows)]
static RETAINED_EXEC_HANDOFF: std::sync::Mutex<Option<std::os::windows::io::OwnedHandle>> =
    std::sync::Mutex::new(None);

/// Retrieves and consumes the guest environment passed via `exec_handoff`, if any.
pub fn take_exec_environment() -> Option<Vec<String>> {
    EXEC_ENVIRONMENT.lock().ok()?.take()
}

pub fn serialize_environment(env: &[String]) -> Result<Vec<u8>, i32> {
    let mut payload = Vec::new();
    let count = u32::try_from(env.len()).map_err(|_| EIO)?;
    payload.extend_from_slice(&count.to_le_bytes());
    for item in env {
        let bytes = item.as_bytes();
        let length = u32::try_from(bytes.len()).map_err(|_| EIO)?;
        payload.extend_from_slice(&length.to_le_bytes());
        payload.extend_from_slice(bytes);
    }
    Ok(payload)
}

fn deserialize_environment(payload: &[u8]) -> Option<Vec<String>> {
    if payload.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    let mut cursor = 4usize;
    let mut env = Vec::with_capacity(count);
    for _ in 0..count {
        if cursor + 4 > payload.len() {
            return None;
        }
        let len =
            u32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap_or_default()) as usize;
        cursor += 4;
        if cursor + len > payload.len() {
            return None;
        }
        let item = match std::str::from_utf8(&payload[cursor..cursor + len]) {
            Ok(s) => s.to_string(),
            Err(_) => return None,
        };
        cursor += len;
        env.push(item);
    }
    (cursor == payload.len()).then_some(env)
}

fn restore_environment(payload: &[u8]) -> bool {
    let Some(env) = deserialize_environment(payload) else {
        return false;
    };
    if let Ok(mut guard) = EXEC_ENVIRONMENT.lock() {
        *guard = Some(env);
        return true;
    }
    false
}

/// Serializes VFS state across `execve`, excluding descriptors with `CLOSE_ON_EXEC`.
#[cfg(windows)]
fn serialize_exec_state_with_image(env: &[String], image: Option<&[u8]>) -> Result<Vec<u8>, i32> {
    serialize_launch_state(env, image, None)
}

#[cfg(windows)]
fn serialize_launch_state(
    env: &[String],
    image: Option<&[u8]>,
    spawn: Option<(Option<u64>, u64)>,
) -> Result<Vec<u8>, i32> {
    let table = table().read().map_err(|_| EIO)?;
    let lock_closes = if spawn.is_some() {
        Vec::new()
    } else {
        record_lock::serialize_exec_closed(
            &table.slots.iter().flatten().copied().collect::<Vec<_>>(),
        )?
    };
    let mut payload = Vec::new();
    let mut retained_fds = std::collections::HashSet::new();
    let mut count = 0u32;
    payload.extend_from_slice(&count.to_le_bytes());
    for (fd, slot) in table.slots.enumerated() {
        let Some(entry) = slot else { continue };
        if entry.flags.contains(FdFlags::CLOSE_ON_EXEC) {
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze: serialize_exec_state SKIP cloexec fd={} raw={:#x}",
                    fd, entry.raw
                );
            }
            continue;
        }
        if entry.kind == FdKind::IoRing {
            // CLOEXEC was handled above. A retained ring cannot be silently
            // dropped merely because the native backend lacks a handoff ABI.
            return Err(EOPNOTSUPP);
        }
        if entry.flags.contains(FdFlags::BORROWED) {
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze: serialize_exec_state SKIP borrowed fd={} raw={:#x}",
                    fd, entry.raw
                );
            }
            continue;
        }
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze: serialize_exec_state INCLUDE fd={} raw={:#x} kind={:?} flags={:#x}",
                fd, entry.raw, entry.kind, entry.flags.0
            );
        }
        payload.extend_from_slice(&(fd as u32).to_le_bytes());
        payload.extend_from_slice(&(entry.raw as u64).to_le_bytes());
        payload.extend_from_slice(&entry.kind.fork_code().to_le_bytes());
        payload.extend_from_slice(&entry.flags.0.to_le_bytes());
        payload.extend_from_slice(&entry.offset.to_le_bytes());
        payload.extend_from_slice(&entry.description_id.to_le_bytes());
        retained_fds.insert(fd as i32);
        count += 1;
    }
    payload[..4].copy_from_slice(&count.to_le_bytes());
    drop(table);

    let mut sections = vec![
        (1, payload),
        (
            2,
            serialize_synthetic_matching(|fd| retained_fds.contains(&fd))?,
        ),
        (3, fs::serialize_cwd()?),
        (
            4,
            match spawn {
                Some((mask, defaults)) => signal::serialize_spawn_state(mask, defaults)?,
                None => signal::serialize_exec_state()?,
            },
        ),
        (13, lock_closes),
        (5, unix::serialize_fork_state()?),
        (
            14,
            fifo::serialize_matching(|fd| retained_fds.contains(&fd))?,
        ),
        (6, epoll::serialize_fork_state()?),
        (7, socket::serialize_process_fork()?),
        (12, mount::serialize_fork_state()?),
        (15, path::serialize_root()?),
        (
            16,
            mount::overlay::serialize(|fd| retained_fds.contains(&fd))?,
        ),
        (
            28,
            mount::native::serialize(|fd| retained_fds.contains(&fd))?,
        ),
        (29, ofd::serialize(|fd| retained_fds.contains(&fd))?),
        (17, mount::api::serialize(|fd| retained_fds.contains(&fd))?),
        (18, mount::policy::serialize()?),
        (19, credentials::serialize_exec()),
        (20, pipe_inode::serialize(|fd| retained_fds.contains(&fd))?),
        (21, time_namespace::serialize(spawn.is_some())?),
        (23, namespaces::serialize()?),
        (24, usernet::serialize(|fd| retained_fds.contains(&fd))?),
        (25, keyring::serialize(spawn.is_some())?),
        (26, tmpfs::serialize()?),
        (27, tty::serialize()?),
        (22, user_namespace::serialize()?),
        (
            10,
            netlink::serialize_matching(|fd| retained_fds.contains(&fd))?,
        ),
        (
            11,
            bpf::serialize_matching(|fd| retained_fds.contains(&fd))?,
        ),
    ];
    if !env.is_empty() {
        sections.push((8, serialize_environment(env)?));
    }
    // Reserve the complete frame once: growing after each section repeatedly
    // copies process state, and appending image padding can copy the whole ELF.
    let capacity = sections
        .iter()
        .map(|(_, bytes)| bytes.len())
        .chain(image.map(<[u8]>::len))
        .try_fold(16usize, |total, length| {
            u32::try_from(length).map_err(|_| EIO)?;
            let aligned = length.checked_add(15).ok_or(EIO)? & !7;
            total.checked_add(aligned).ok_or(EIO)
        })?;
    let mut frame = Vec::new();
    frame.try_reserve_exact(capacity).map_err(|_| ENOMEM)?;
    frame.extend_from_slice(&FORK_STATE_MAGIC.to_le_bytes());
    let section_count = sections.len() + usize::from(image.is_some());
    frame.extend_from_slice(&(section_count as u32).to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    for (tag, section_payload) in sections {
        append_fork_section(&mut frame, tag, &section_payload)?;
    }
    if let Some(image) = image {
        append_fork_section(&mut frame, EXEC_IMAGE_SECTION, image)?;
    }
    Ok(frame)
}

/// Serializes exec state without an executable snapshot. Fork tests and tools
/// that do not launch a replacement image use this form.
#[cfg(windows)]
pub fn serialize_exec_state(env: &[String]) -> Result<Vec<u8>, i32> {
    serialize_exec_state_with_image(env, None)
}

/// Begins an exec handoff, including the target-independent half of Winsock
/// duplication. The matching [`finish_exec_state`] call is required on every
/// success and error path so its control handles cannot leak.
#[cfg(windows)]
pub fn prepare_exec_state(env: &[String], executable: &std::path::Path) -> Result<Vec<u8>, i32> {
    // Open the executable before process creation, as Linux execve resolves and
    // pins the executable inode before installing the new image. The child uses
    // this stable snapshot instead of racing a second path-based open.
    let image = read_guest_image(executable).map_err(|error| {
        error
            .raw_os_error()
            .map(|code| errno_from_win32(code as u32))
            .unwrap_or(EIO)
    })?;
    prepare_exec_state_from_image(env, &image)
}

/// Begins an exec handoff from an image the caller has already resolved.
///
/// Script detection and format probing often need to read the target anyway.
/// Accepting that stable copy here avoids reopening and rereading an ELF before
/// publishing the same bytes to the replacement loader.
#[cfg(windows)]
pub fn prepare_exec_state_from_image(env: &[String], image: &[u8]) -> Result<Vec<u8>, i32> {
    unix::prepare_process_handoff()?;
    if let Err(error) = socket::prepare_process_exec() {
        unix::finish_process_handoff(0);
        return Err(error);
    }
    match serialize_exec_state_with_image(env, Some(image)) {
        Ok(payload) => Ok(payload),
        Err(error) => {
            socket::finish_process_fork(0);
            unix::finish_process_handoff(0);
            Err(error)
        }
    }
}

/// Begins a fresh-image spawn without copying the parent's address space.
/// Uses the exec descriptor set and fork signal/parent semantics. The caller
/// serializes image transactions and pairs this with `finish_exec_state`.
#[cfg(windows)]
pub fn prepare_spawn_state_from_image(
    env: &[String],
    image: &[u8],
    mask: Option<u64>,
    defaults: u64,
) -> Result<Vec<u8>, i32> {
    publish_proc_fd_snapshot()?;
    unix::prepare_process_handoff()?;
    if let Err(error) = socket::prepare_process_exec() {
        unix::finish_process_handoff(0);
        return Err(error);
    }
    match serialize_launch_state(env, Some(image), Some((mask, defaults))) {
        Ok(payload) => Ok(payload),
        Err(error) => {
            socket::finish_process_fork(0);
            unix::finish_process_handoff(0);
            Err(error)
        }
    }
}

/// Completes or cancels the Winsock half of an exec handoff.
#[cfg(windows)]
pub fn finish_exec_state(target_pid: Option<u32>) {
    let pid = target_pid
        .and_then(|pid| i32::try_from(pid).ok())
        .unwrap_or(0);
    socket::finish_process_fork(pid);
    unix::finish_process_handoff(pid);
}

/// Stages an `execve` state payload in a named shared memory section for the child.
#[cfg(windows)]
pub struct ExecHandoff {
    _mapping: std::os::windows::io::OwnedHandle,
    ready: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl ExecHandoff {
    /// Waits until the resumed child owns its own handle to the handoff mapping.
    /// The process handle is part of the same wait so a child that fails during
    /// loader startup is reported immediately instead of consuming the entire
    /// acknowledgement timeout.
    pub fn wait_until_owned(&self, child: &impl std::os::windows::io::AsRawHandle) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForMultipleObjects;

        // Put ready first. If acknowledgement and process exit become visible in
        // the same scheduler interval, a completed handoff is still a success.
        let handles = [
            self.ready.as_raw_handle().cast(),
            child.as_raw_handle().cast(),
        ];
        unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, 30_000)
                == WAIT_OBJECT_0
        }
    }
}

#[cfg(windows)]
pub fn stage_exec_handoff(target_pid: u32, payload: &[u8]) -> Option<ExecHandoff> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows_sys::Win32::System::Threading::CreateEventW;
    if payload.is_empty() {
        return None;
    }
    let ready_name: Vec<u16> = format!("Local\\kinakaze.exec.ready.{target_pid}\0")
        .encode_utf16()
        .collect();
    let raw_ready = unsafe { CreateEventW(std::ptr::null(), 1, 0, ready_name.as_ptr()) };
    if raw_ready.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !raw_ready.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(raw_ready) };
        }
        return None;
    }
    // SAFETY: CreateEventW returned ownership of this event handle.
    let ready = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_ready.cast()) };
    let name_str = format!("Local\\kinakaze.exec.{}\0", target_pid);
    let name_wide: Vec<u16> = name_str.encode_utf16().collect();
    let total_len = payload.len().checked_add(4)?;
    let mapping_len = u32::try_from(total_len).ok()?;
    let raw_mapping = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            mapping_len,
            name_wide.as_ptr(),
        )
    };
    if raw_mapping.is_null() {
        return None;
    }
    // A PID-named handoff must be newly created. Opening an existing mapping
    // could feed stale or attacker-controlled descriptor state to this child.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(raw_mapping) };
        return None;
    }
    // SAFETY: CreateFileMappingW returned ownership of this handle.
    let mapping = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_mapping.cast()) };
    let view = unsafe {
        MapViewOfFile(
            mapping.as_raw_handle().cast(),
            FILE_MAP_ALL_ACCESS,
            0,
            0,
            total_len,
        )
    };
    if view.Value.is_null() {
        return None;
    }
    unsafe {
        let ptr = view.Value as *mut u8;
        let len_bytes = (payload.len() as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), ptr, 4);
        std::ptr::copy_nonoverlapping(payload.as_ptr(), ptr.add(4), payload.len());
        UnmapViewOfFile(view);
    }
    Some(ExecHandoff {
        _mapping: mapping,
        ready,
    })
}

/// Loads an `execve` state payload from the named shared memory section for this process.
#[cfg(windows)]
fn read_exec_handoff<T>(
    signal_ready: bool,
    retain_mapping: bool,
    read: impl FnOnce(&[u8]) -> Result<T, ()>,
) -> Result<Option<T>, ()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Memory::{
        FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
    };
    struct View(MEMORY_MAPPED_VIEW_ADDRESS);
    impl Drop for View {
        fn drop(&mut self) {
            unsafe { UnmapViewOfFile(self.0) };
        }
    }
    let name: Vec<u16> = format!("Local\\kinakaze.exec.{}\0", std::process::id())
        .encode_utf16()
        .collect();
    let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if mapping.is_null() {
        return Ok(None);
    }
    let mapping = unsafe { OwnedHandle::from_raw_handle(mapping) };
    let header = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_READ, 0, 0, 4) };
    if header.Value.is_null() {
        return Err(());
    }
    let header = View(header);
    let payload_len = unsafe { header.0.Value.cast::<u32>().read_unaligned() } as usize;
    drop(header);
    if payload_len == 0 || payload_len > 512 * 1024 * 1024 {
        return Err(());
    }
    let view = unsafe {
        MapViewOfFile(
            mapping.as_raw_handle(),
            FILE_MAP_READ,
            0,
            0,
            payload_len + 4,
        )
    };
    if view.Value.is_null() {
        return Err(());
    }
    let view = View(view);
    // Borrow the mapped frame during restoration/decoding. Copying the entire
    // frame into a Vec inflated both loader and provider arena high-water marks.
    let payload =
        unsafe { core::slice::from_raw_parts(view.0.Value.cast::<u8>().add(4), payload_len) };
    if retain_mapping {
        *RETAINED_EXEC_HANDOFF.lock().map_err(|_| ())? = Some(mapping);
    }
    if signal_ready {
        signal_exec_handoff_ready();
    }
    read(payload).map(Some)
}

#[cfg(windows)]
fn signal_exec_handoff_ready() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let ready_name: Vec<u16> = format!("Local\\kinakaze.exec.ready.{}\0", std::process::id())
        .encode_utf16()
        .collect();
    let ready = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, ready_name.as_ptr()) };
    if !ready.is_null() {
        unsafe {
            SetEvent(ready);
            CloseHandle(ready);
        }
    }
}

/// Loads, acknowledges and restores an exec handoff into this VFS instance.
///
/// This is deliberately an explicit operation instead of CRT-initializer work.
/// The loader calls it after `LoadLibraryW` has returned, so descriptor and
/// socket restoration does not run while the Windows loader lock is held.
#[cfg(windows)]
pub fn consume_exec_handoff() -> Result<bool, ()> {
    read_exec_handoff(true, false, |frame| {
        restore_fork_state(frame).then_some(()).ok_or(())
    })
    .map(|state| state.is_some())
}

/// Immutable Linux image-launch state captured at the `execve` boundary.
#[cfg(windows)]
pub struct ExecLaunchState {
    pub image: kinakaze_runtime::immutable::ImmutableBytes,
    pub environment: Vec<String>,
}

/// Copies the stable executable snapshot and exact guest environment without
/// consuming the provider DLL's exec handoff.
///
/// The ELF loader needs the root image before it can map guest libc. Descriptor
/// restoration happens from libc's own VFS instance later. This executable-side
/// copy first retains the mapping and then signals that the parent may release
/// its own reference.
#[cfg(windows)]
pub fn peek_exec_launch_state() -> Result<Option<ExecLaunchState>, ()> {
    // Retain a child-owned mapping handle before acknowledging readiness. The
    // parent may then close its handle immediately while libc opens and restores
    // the complete frame a little later in this process.
    let state = read_exec_handoff(false, true, |frame| {
        let image_range = fork_section_range(frame, EXEC_IMAGE_SECTION).ok_or(())?;
        let root_range = fork_section_range(frame, 15).ok_or(())?;
        if !path::restore_root(&frame[root_range]) {
            return Err(());
        }
        let mount_range = fork_section_range(frame, 12).ok_or(())?;
        if !mount::restore_fork_state(&frame[mount_range]) {
            return Err(());
        }
        let image = kinakaze_runtime::immutable::ImmutableBytes::from_slice(&frame[image_range])
            .map_err(|_| ())?;
        let environment = match fork_section_range(frame, 8) {
            Some(range) => deserialize_environment(&frame[range]).ok_or(())?,
            None => Vec::new(),
        };
        Ok(ExecLaunchState { image, environment })
    })?;
    if state.is_some() {
        // Both the frame handle and immutable image now have child ownership.
        signal_exec_handoff_ready();
    }
    Ok(state)
}

/// Drops the loader's temporary mapping reference after libc has borrowed and
/// restored the complete handoff frame.
#[cfg(windows)]
pub fn release_retained_exec_handoff() {
    if let Ok(mut retained) = RETAINED_EXEC_HANDOFF.lock() {
        retained.take();
    }
}

/// Releases every guest descriptor from an old image that is waiting for its
/// Windows `exec` replacement.
///
/// Windows cannot replace the loader process in place, so the old process stays
/// alive only to forward the replacement's exit status. It must not retain any
/// file-object references while it waits: a leftover pipe writer suppresses
/// EOF, and a leftover socket changes close/shutdown behaviour. Standard
/// handles are marked borrowed during normal guest execution, but this terminal
/// teardown owns the wrapper process's copies and closes them as well.
#[cfg(windows)]
pub fn close_exec_wrapper_descriptors() {
    iouring::aio::destroy_all();
    let trace = std::env::var_os("KINAKAZE_SPAWN_TRACE").is_some();
    let mut descriptors = table()
        .read()
        .map(|table| {
            table
                .slots
                .enumerated()
                .filter_map(|(fd, entry)| entry.is_some().then_some(fd as i32))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Keep the diagnostic stream alive until every non-standard descriptor has
    // been released. The order is not observable by the replaced guest image,
    // while it lets an exec failure report the exact descriptor that blocked.
    descriptors.sort_by_key(|fd| i32::from((0..=2).contains(fd)));

    if trace {
        eprintln!(
            "kinakaze exec wrapper: pid={} closing descriptors {descriptors:?}",
            std::process::id()
        );
    }

    for fd in descriptors {
        if trace {
            eprintln!(
                "kinakaze exec wrapper: pid={} closing fd={fd}",
                std::process::id()
            );
        }
        if let Ok(mut table) = table().write()
            && let Some(Some(entry)) = table.slots.get_mut(fd as usize)
        {
            entry.flags = FdFlags(entry.flags.0 & !FdFlags::BORROWED.0);
        }
        let result = close(fd);
        if trace {
            eprintln!(
                "kinakaze exec wrapper: pid={} close fd={fd} result={result:?}",
                std::process::id(),
            );
        }
    }
    if std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
        platform::trace_inheritable_pipe_handles();
    }
}

/// Updates the mutable Linux file-status flags stored by an open description.
///
/// Host sockets are already put into non-blocking mode when they are created;
/// this bit controls whether the compatibility layer waits on `EWOULDBLOCK`.
/// Files and pipes consult the same flags when issuing their I/O, so changing
/// them here gives `fcntl(F_SETFL)` an immediate effect on this descriptor.
pub fn set_status_flags(fd: i32, append: bool, nonblock: bool) -> Result<(), i32> {
    #[cfg(windows)]
    return ofd::with(fd, || set_status_flags_inner(fd, append, nonblock));
    #[cfg(not(windows))]
    set_status_flags_inner(fd, append, nonblock)
}

/// Changes only O_NONBLOCK for ioctl(FIONBIO), under the same description lock
/// as F_SETFL. Backends with their own shared flags update just this bit there.
pub fn set_nonblocking(fd: i32, enabled: bool) -> Result<(), i32> {
    let update = || {
        let entry = get(fd)?;
        if entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        if entry.kind == FdKind::Fifo {
            return fifo::set_nonblocking(fd, entry, enabled);
        }
        if matches!(
            entry.kind,
            FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::SysfsFile
        ) {
            tmpfs::set_status(fd, fs::O_NONBLOCK, if enabled { fs::O_NONBLOCK } else { 0 })?;
        }
        if entry.kind == FdKind::MessageQueue {
            mqueue::getattr(fd, Some(if enabled { fs::O_NONBLOCK as i64 } else { 0 }))?;
        }
        set_status_flags_local(
            fd,
            entry,
            FdFlags::NONBLOCK.0,
            if enabled { FdFlags::NONBLOCK.0 } else { 0 },
        )
    };
    #[cfg(windows)]
    return ofd::with(fd, update);
    #[cfg(not(windows))]
    update()
}

/// Pin immutable file bytes and their logical identity before an SCM handoff.
/// This uses the same synthetic-before-path lock order as fork publication.
#[cfg(windows)]
pub(crate) fn export_synthetic_rights(fd: i32, entry: FdEntry) -> Result<Vec<u8>, i32> {
    let table = synthetic().read().map_err(|_| EIO)?;
    let content = &table
        .get(&fd)
        .filter(|(generation, _)| *generation == entry.generation)
        .ok_or(EBADF)?
        .1;
    let cgroup = cgroup_paths().lock().map_err(|_| EIO)?;
    let sysctl = proc_sysctl_paths().lock().map_err(|_| EIO)?;
    let path = cgroup
        .get(&fd)
        .or_else(|| sysctl.get(&fd))
        .map_or("", String::as_str);
    let mut out = Vec::new();
    state_codec::bytes(&mut out, content);
    state_codec::bytes(&mut out, path.as_bytes());
    Ok(out)
}
#[cfg(windows)]
pub(crate) fn import_synthetic_rights(fd: i32, metadata: &[u8]) -> Result<(), i32> {
    let mut input = state_codec::Reader(metadata);
    let contents = input.bytes()?.to_vec();
    let path = input.text()?;
    input.end()?;
    let entry = get(fd)?;
    if entry.kind != FdKind::SyntheticDirectory && !path.starts_with('/') {
        return Err(EIO);
    }
    let mut table = synthetic().write().map_err(|_| EIO)?;
    match entry.kind {
        FdKind::SyntheticDirectory => {}
        FdKind::CgroupFile => {
            cgroup_paths().lock().map_err(|_| EIO)?.insert(fd, path);
        }
        FdKind::Synthetic | FdKind::ProcSysctl => {
            proc_sysctl_paths()
                .lock()
                .map_err(|_| EIO)?
                .insert(fd, path);
        }
        _ => return Err(EINVAL),
    }
    table.insert(fd, (entry.generation, contents));
    Ok(())
}
fn set_status_flags_inner(fd: i32, append: bool, nonblock: bool) -> Result<(), i32> {
    if fd < 0 {
        return Err(EBADF);
    }
    let snapshot = get(fd)?;
    if snapshot.kind == FdKind::Fifo {
        return fifo::set_status_flags(fd, snapshot, append, nonblock);
    }
    if matches!(
        snapshot.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::SysfsFile
    ) {
        tmpfs::set_status(
            fd,
            fs::O_APPEND | fs::O_NONBLOCK,
            (if append { fs::O_APPEND } else { 0 }) | (if nonblock { fs::O_NONBLOCK } else { 0 }),
        )?;
    }
    let mask = FdFlags::APPEND.0 | FdFlags::NONBLOCK.0;
    let status =
        if append { FdFlags::APPEND.0 } else { 0 } | if nonblock { FdFlags::NONBLOCK.0 } else { 0 };
    set_status_flags_local(fd, snapshot, mask, status)
}

fn set_status_flags_local(fd: i32, snapshot: FdEntry, mask: u32, status: u32) -> Result<(), i32> {
    let mut table = table().write().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|slot| *slot)
        .ok_or(EBADF)?;
    if entry.generation != snapshot.generation {
        return Err(EBADF);
    }
    // A shared OFD publishes its first surviving alias after the operation.
    // Update every local alias so changing a higher-numbered dup is preserved.
    for alias in table
        .slots
        .iter_mut()
        .flatten()
        .filter(|alias| alias.description_id == snapshot.description_id)
    {
        alias.flags.0 = (alias.flags.0 & !mask) | status;
    }
    Ok(())
}

pub fn set_noatime(fd: i32, enabled: bool) -> Result<(), i32> {
    ofd::with(fd, || set_noatime_inner(fd, enabled))
}
fn set_noatime_inner(fd: i32, enabled: bool) -> Result<(), i32> {
    let (entry, _pin) = pin_native_fd(fd, |entry| {
        if enabled {
            fs::check_noatime(entry.raw as _)?;
        }
        Ok(())
    })?;
    let mut table = table().write().map_err(|_| EIO)?;
    if !table
        .slots
        .get(fd as usize)
        .and_then(|e| *e)
        .is_some_and(|e| e.generation == entry.generation)
    {
        return Err(EBADF);
    }
    for slot in table
        .slots
        .iter_mut()
        .flatten()
        .filter(|e| e.description_id == entry.description_id)
    {
        slot.flags.0 =
            (slot.flags.0 & !FdFlags::NOATIME.0) | if enabled { FdFlags::NOATIME.0 } else { 0 };
    }
    Ok(())
}

/// Emits a descriptor-table trace line when `KINAKAZE_FD_TRACE` is set.
///
/// Startup failures in guests that police descriptor numbers (libuv asserts
/// `fd > STDERR_FILENO`) are otherwise diagnosed by guessing: the trace shows
/// exactly which numbers the table handed out and which ones were released.
#[inline]
fn fd_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FD_TRACE").is_some())
}

#[inline]
fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_TRACE").is_some())
}

#[inline]
pub(crate) fn fork_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_TRACE").is_some())
}

#[inline]
pub(crate) fn eventfd_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_EVENTFD_TRACE").is_some())
}

fn fd_trace(op: &str, fd: i32, kind: FdKind) {
    if !fd_trace_enabled() {
        return;
    }
    eprintln!("[fd] {op} fd={fd} kind={kind:?}");
}

/// Installs a host object into the first free Linux descriptor slot.
pub fn install(raw: usize, kind: FdKind, flags: FdFlags) -> Result<i32, i32> {
    install_with(raw, kind, flags, |_, _| Ok(()))
}

/// Publish a descriptor and its side-table state as one operation. The callback
/// runs under the descriptor-table write guard: it must not re-enter the table,
/// block on I/O, or leave side-table state behind on error. Native ownership
/// stays with the caller on failure; no close hook is run for an unpublished fd.
pub(crate) fn install_with(
    raw: usize,
    kind: FdKind,
    flags: FdFlags,
    register: impl FnOnce(i32, FdEntry) -> Result<(), i32>,
) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    if (raw == 0 && !handleless) || raw == usize::MAX {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;
    let limit = job::current_nofile_limit()?;
    let mut table = table().write().map_err(|_| EIO)?;
    // Descriptors above the standard streams; POSIX only guarantees the lowest
    // free slot, and reusing 0..2 would shadow stdin/stdout/stderr.
    let fd = table.first_free_between(0, limit).ok_or(EMFILE)?;
    if kind == FdKind::Pipe && std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
        eprintln!(
            "kinakaze pipe: pid={} install fd={fd} raw={raw:#x} flags={:#x}",
            std::process::id(),
            flags.0
        );
    }
    fd_trace("install", fd as i32, kind);
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at(fd as i32, raw, kind, flags);
    let entry = table.slots[fd].ok_or(EIO)?;
    if let Err(error) = register(fd as i32, entry) {
        table.slots.remove(fd);
        return Err(error);
    }
    // Every descriptor survives fork, including FD_CLOEXEC. Sockets use the
    // process-specific Winsock handoff rather than raw handle inheritance.
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd as i32)
}

/// Installs the lowest free descriptor as another reference to `source`'s open
/// file description.
///
/// This is intentionally separate from [`install`]: a newly opened object gets
/// a fresh identity, while `dup` must preserve the source identity and current
/// shared position.
pub fn install_duplicate(
    raw: usize,
    kind: FdKind,
    flags: FdFlags,
    source: FdEntry,
) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    if kind != source.kind
        || source.description_id == 0
        || (raw == 0 && !handleless)
        || raw == usize::MAX
    {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;
    let limit = job::current_nofile_limit()?;
    let mut table = table().write().map_err(|_| EIO)?;
    let fd = table.first_free_between(0, limit).ok_or(EMFILE)? as i32;
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at_description(fd, raw, kind, flags, source.offset, source.description_id);
    if let Err(error) = publish_duplicate_state(source, fd, table.slots[fd as usize].ok_or(EIO)?) {
        table.slots.remove(fd as usize);
        return Err(error);
    }
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd)
}

/// A descriptor number reserved by a syscall that has not installed a file yet.
pub(crate) struct FdReservation {
    fd: Option<usize>,
}

/// Reserve a number before consuming a connection. It is unavailable to other
/// allocations, but is not an installed fd and cannot be read, duped or inherited.
pub(crate) fn reserve_descriptor() -> Result<FdReservation, i32> {
    let limit = job::current_nofile_limit()?;
    let mut table = table().write().map_err(|_| EIO)?;
    let fd = table.first_free_between(0, limit).ok_or(EMFILE)?;
    table.slots.reserve(fd);
    Ok(FdReservation { fd: Some(fd) })
}
impl FdReservation {
    pub(crate) fn install_with(
        mut self,
        raw: usize,
        kind: FdKind,
        flags: FdFlags,
        register: impl FnOnce(i32, FdEntry) -> Result<(), i32>,
    ) -> Result<i32, i32> {
        if raw == usize::MAX || (raw == 0 && !kind.allows_missing_host_handle()) {
            return Err(EINVAL);
        }
        validate_descriptor_flags(kind, flags)?;
        let fd = self.fd.ok_or(EBADF)?;
        let mut table = table().write().map_err(|_| EIO)?;
        if !table.slots.is_reserved(fd) || table.slots[fd].is_some() {
            return Err(EBUSY);
        }
        #[cfg(windows)]
        let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
        table.insert_at(fd as i32, raw, kind, flags);
        if let Err(error) = register(fd as i32, table.slots[fd].ok_or(EIO)?) {
            table.slots.remove(fd);
            return Err(error);
        }
        #[cfg(windows)]
        inheritance.commit();
        table.slots.unreserve(fd);
        self.fd = None;
        Ok(fd as i32)
    }
}
impl Drop for FdReservation {
    fn drop(&mut self) {
        if let Some(fd) = self.fd {
            if let Ok(mut table) = table().write() {
                table.slots.unreserve(fd);
            }
        }
    }
}

/// Creates one Linux anonymous pipe with its final descriptor flags.
///
/// The Windows endpoints are a connected byte-mode named-pipe instance opened
/// for overlapped I/O. `CreatePipe` can only create synchronous handles, on
/// which even a nominally non-consuming readiness query may queue behind a
/// different thread's blocking read. Overlapped endpoints give read, poll,
/// signals and `O_NONBLOCK` one coherent non-blocking kernel primitive.
#[cfg(windows)]
pub fn create_pipe(common_flags: FdFlags, capacity: u32) -> Result<(i32, i32), i32> {
    let allowed = FdFlags::CLOSE_ON_EXEC.0 | FdFlags::NONBLOCK.0;
    if common_flags.0 & !allowed != 0 || capacity == 0 {
        return Err(EINVAL);
    }
    let inode = pipe_inode::Inode::new()?;
    let (read, write) = platform::create_overlapped_pipe_pair(capacity)?;
    let common_flags = common_flags.union(FdFlags::OVERLAPPED);
    let read_fd = match install_with(
        read,
        FdKind::Pipe,
        common_flags.union(FdFlags::PIPE_READ_END),
        |_, entry| pipe_inode::register(entry, inode.clone()),
    ) {
        Ok(fd) => fd,
        Err(error) => {
            platform::close_raw(read);
            platform::close_raw(write);
            return Err(error);
        }
    };
    let write_fd = match install_with(
        write,
        FdKind::Pipe,
        common_flags.union(FdFlags::PIPE_WRITE_END),
        |_, entry| pipe_inode::register(entry, inode.clone()),
    ) {
        Ok(fd) => fd,
        Err(error) => {
            let _ = close(read_fd);
            platform::close_raw(write);
            return Err(error);
        }
    };
    Ok((read_fd, write_fd))
}

/// Installs a host object into the lowest free descriptor at or above `floor`.
///
/// Selection and insertion happen under one descriptor-table write lock. This
/// is the table operation behind Linux `F_DUPFD`; walking to the floor with
/// temporary descriptors would expose intermediate state and allow unrelated
/// threads to change which slot is considered the lowest one.
pub fn install_at_least(raw: usize, kind: FdKind, flags: FdFlags, floor: i32) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    let limit = job::current_nofile_limit()?;
    if floor < 0 || floor as usize >= limit || (raw == 0 && !handleless) || raw == usize::MAX {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;

    let mut table = table().write().map_err(|_| EIO)?;
    let fd = table
        .first_free_between(floor as usize, limit)
        .ok_or(EMFILE)?;
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at(fd as i32, raw, kind, flags);
    // Fork inheritance follows descriptor ownership; sockets are reconstructed
    // separately because Winsock state is process-specific.
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd as i32)
}

/// `install_at_least` for a `dup` operation, preserving the source open
/// description identity.
pub fn install_duplicate_at_least(
    raw: usize,
    kind: FdKind,
    flags: FdFlags,
    floor: i32,
    source: FdEntry,
) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    let limit = job::current_nofile_limit()?;
    if kind != source.kind
        || source.description_id == 0
        || floor < 0
        || floor as usize >= limit
        || (raw == 0 && !handleless)
        || raw == usize::MAX
    {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;
    let mut table = table().write().map_err(|_| EIO)?;
    let fd = table
        .first_free_between(floor as usize, limit)
        .ok_or(EMFILE)? as i32;
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at_description(fd, raw, kind, flags, source.offset, source.description_id);
    if let Err(error) = publish_duplicate_state(source, fd, table.slots[fd as usize].ok_or(EIO)?) {
        table.slots.remove(fd as usize);
        return Err(error);
    }
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd)
}

/// Installs one open description at an exact, currently-free descriptor.
///
/// `dup2` closes its destination first and then uses this atomic insertion. The
/// VFS owns descriptor-number allocation, so exact placement belongs here rather
/// than in a caller that walks the table with temporary placeholder entries.
pub fn install_exact(raw: usize, kind: FdKind, flags: FdFlags, fd: i32) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    let limit = job::current_nofile_limit()?;
    if fd < 0 || fd as usize >= limit || (raw == 0 && !handleless) || raw == usize::MAX {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;

    let mut table = table().write().map_err(|_| EIO)?;
    if table.slots.is_reserved(fd as usize)
        || table.slots.get(fd as usize).is_none_or(Option::is_some)
    {
        return Err(EBUSY);
    }
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at(fd, raw, kind, flags);
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd)
}

/// `install_exact` for `dup2`/`dup3`, preserving the source open description.
pub fn install_duplicate_exact(
    raw: usize,
    kind: FdKind,
    flags: FdFlags,
    fd: i32,
    source: FdEntry,
) -> Result<i32, i32> {
    let handleless = kind.allows_missing_host_handle();
    let limit = job::current_nofile_limit()?;
    if kind != source.kind
        || source.description_id == 0
        || fd < 0
        || fd as usize >= limit
        || (raw == 0 && !handleless)
        || raw == usize::MAX
    {
        return Err(EINVAL);
    }
    validate_descriptor_flags(kind, flags)?;
    let mut table = table().write().map_err(|_| EIO)?;
    if table.slots.is_reserved(fd as usize)
        || table.slots.get(fd as usize).is_none_or(Option::is_some)
    {
        return Err(EBUSY);
    }
    #[cfg(windows)]
    let inheritance = exec_inheritance::DescriptorInheritance::prepare(raw, kind, flags)?;
    table.insert_at_description(fd, raw, kind, flags, source.offset, source.description_id);
    if let Err(error) = publish_duplicate_state(source, fd, table.slots[fd as usize].ok_or(EIO)?) {
        table.slots.remove(fd as usize);
        return Err(error);
    }
    #[cfg(windows)]
    inheritance.commit();
    Ok(fd)
}

/// Caller holds the fd-table write lock until side-table publication succeeds.
fn publish_duplicate_state(source: FdEntry, fd: i32, entry: FdEntry) -> Result<(), i32> {
    if source.kind == FdKind::IoRing {
        return Err(EOPNOTSUPP);
    }
    if entry.kind == FdKind::Fifo {
        fifo::duplicate_descriptor(source, fd, entry)?;
    }
    Ok(())
}

/// Advances a seekable descriptor's stored position after a transfer.
///
/// The generation guards against the descriptor having been closed and the slot
/// reused while the I/O was in flight; a stale completion must not move the new
/// descriptor's position.
fn advance_offset(fd: i32, generation: u32, amount: usize) {
    if amount == 0 {
        return;
    }
    let Ok(mut table) = table().write() else {
        return;
    };
    if let Some(Some(entry)) = table.slots.get_mut(fd as usize)
        && entry.generation == generation
        && entry.flags.contains(FdFlags::SEEKABLE)
    {
        entry.offset = entry.offset.saturating_add(amount as u64);
    }
}

/// Runs an overlapped read against a raw entry, reusing the interruptible path.
///
/// The Unix socket layer needs the same `EINTR`/`SA_RESTART` handling as a file
/// read but works from a handle it owns rather than a table entry, so the shared
/// implementation is exposed here rather than duplicated.
///
/// # Safety
///
/// `buffer` must be writable for `len` bytes.
#[cfg(windows)]
pub unsafe fn platform_read(entry: FdEntry, buffer: *mut u8, len: usize) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe { platform::transfer(&entry, buffer, len, true) }
}

/// One pinned read attempt, including the file integrity/data boundary. Pending
/// native I/O is retired before returning EINTR; guest signals are not invoked.
///
/// # Safety
/// `entry.raw` must remain owned for this call and `buffer` writable for `len`.
/// The caller must drop its operation-local pin before signal dispatch.
#[cfg(windows)]
pub unsafe fn platform_read_pinned_once(
    entry: FdEntry,
    buffer: *mut u8,
    len: usize,
) -> Result<usize, i32> {
    if entry.flags.contains(FdFlags::PATH_ONLY)
        || (entry.flags.contains(FdFlags::WRITE_ACCESS)
            && !entry.flags.contains(FdFlags::READ_ACCESS))
    {
        return Err(EBADF);
    }
    if entry.kind == FdKind::File {
        let bytes = if len == 0 {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(buffer, len) }
        };
        return fs::verity::verified_read(entry.raw as _, entry.offset, bytes)?.ok_or(EIO);
    }
    unsafe { platform::transfer_once(&entry, buffer, len, true) }
}

/// Runs an overlapped write against a raw entry.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes.
#[cfg(windows)]
pub unsafe fn platform_write(entry: FdEntry, buffer: *mut u8, len: usize) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe { platform::transfer(&entry, buffer, len, false) }
}

pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    #[cfg(windows)]
    if get(fd)?.flags.contains(FdFlags::SEEKABLE) {
        return ofd::with(fd, || read_inner(fd, buffer));
    }
    read_inner(fd, buffer)
}
fn read_inner(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    #[cfg(windows)]
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return tmpfs::read(fd, buffer, None);
    }
    #[cfg(windows)]
    let entry = loop {
        let (entry, pin) = native_pin::read_entry(fd)?;
        let Some(pin) = pin else { break entry };
        let outcome = unsafe { pin.read_once(entry, buffer.as_mut_ptr(), buffer.len()) };
        // A signal handler may fork or exec. Neither it nor a resumed child
        // should inherit an unregistered temporary handle from this stack.
        drop(pin);
        if outcome == Err(EINTR) && signal::deliver_pending() == signal::Delivery::Restart {
            continue;
        }
        let count = outcome?;
        advance_offset(fd, entry.generation, count);
        return Ok(count);
    };
    #[cfg(not(windows))]
    let entry = match get(fd) {
        Ok(e) => e,
        Err(err) => {
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze vfs: pid {} read fd={fd} get() failed with {err}",
                    std::process::id()
                );
            }
            return Err(err);
        }
    };
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze vfs: pid {} read fd={fd} kind={:?} raw={:#x} len={}",
            std::process::id(),
            entry.kind,
            entry.raw,
            buffer.len()
        );
    }
    if entry.kind == FdKind::Pipe && !entry.flags.contains(FdFlags::PIPE_READ_END) {
        return Err(EBADF);
    }
    if entry.kind == FdKind::FsContext {
        return mount::api::read_log(fd, buffer);
    }
    if matches!(
        entry.kind,
        FdKind::MountNamespace
            | FdKind::TimeNamespace
            | FdKind::Namespace
            | FdKind::UserNamespace
            | FdKind::MountTree
    ) {
        return Err(EINVAL);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Fifo {
        let result = fifo::read(fd, entry, buffer);
        if matches!(result, Ok(count) if count != 0) {
            epoll::readiness_consumed(entry.description_id, epoll::EPOLLIN | epoll::EPOLLRDNORM);
        }
        return result;
    }
    // Winsock handles are not reliably usable with ReadFile, and the socket path
    // has its own readiness and interruption handling.
    #[cfg(windows)]
    if entry.kind == FdKind::Socket {
        // SAFETY: the slice provides `len` writable bytes.
        return unsafe { socket::recv(fd, buffer.as_mut_ptr(), buffer.len(), 0) };
    }
    #[cfg(windows)]
    if entry.kind == FdKind::NetlinkSocket {
        // SAFETY: the slice provides `len` writable bytes.
        return unsafe { netlink::recv(fd, buffer.as_mut_ptr(), buffer.len(), 0) };
    }
    // A Unix socket is a pipe underneath, but its shutdown and connection state
    // are only known to that layer.
    #[cfg(windows)]
    if entry.kind == FdKind::UnixSocket {
        // SAFETY: the slice provides `len` writable bytes.
        return unsafe { unix::recv(fd, buffer.as_mut_ptr(), buffer.len(), 0) };
    }
    #[cfg(windows)]
    if entry.kind == FdKind::TimerFd {
        return timerfd::read(fd, buffer);
    }
    if entry.kind == FdKind::EventFd {
        let result = eventfd::read_eventfd(fd, buffer, entry.flags.contains(FdFlags::NONBLOCK));
        #[cfg(windows)]
        if matches!(result, Ok(8)) {
            // A new completion can increment the counter before epoll next
            // samples it. Retire the consumed edge at the read boundary.
            epoll::readiness_consumed(
                entry.description_id,
                epoll::EPOLLIN | epoll::EPOLLRDNORM | epoll::EPOLLERR,
            );
        }
        return result;
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Inotify {
        return inotify::read_inotify(fd, buffer, entry.flags.contains(FdFlags::NONBLOCK));
    }
    // A pseudo-terminal is not a host handle at all: its bytes live in the
    // terminal's shared state and the line discipline decides what a read may
    // take. That is also where VMIN/VTIME and the canonical line boundary are
    // enforced, so this must not fall through to a plain ReadFile.
    #[cfg(windows)]
    if matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave) {
        return tty::read(fd, buffer);
    }
    // The real console is cooked by the same line discipline, so a read on it
    // comes out of the discipline's queue rather than off conhost's own line
    // editor. `is_console` is the live probe: FdKind::Console also covers NUL
    // and other character devices, which have no terminal behaviour to apply.
    #[cfg(windows)]
    if entry.kind == FdKind::Console && tty::is_console(fd) {
        return tty::console_read(fd, buffer);
    }
    // Synthetic files have no handle; their bytes come from the side table.
    #[cfg(windows)]
    if entry.kind == FdKind::ProcSysctl {
        let read = read_proc_sysctl(fd, entry, buffer)?;
        advance_offset(fd, entry.generation, read);
        return Ok(read);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::CgroupFile && !entry.flags.contains(FdFlags::READ_ACCESS) {
        return Err(EBADF);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Synthetic || entry.kind == FdKind::CgroupFile {
        let read = read_synthetic(fd, entry, buffer)?;
        advance_offset(fd, entry.generation, read);
        return Ok(read);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::SyntheticDirectory {
        // Reading a directory is EISDIR; listings come from getdents.
        return Err(EISDIR);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::BpfProgram {
        // Linux bpf program anonymous inodes expose descriptor identity and
        // ioctl-style bpf(2) operations, but no read file operation.
        return Err(EINVAL);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Null {
        return Ok(0);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Zero || entry.kind == FdKind::Full {
        buffer.fill(0);
        advance_offset(fd, entry.generation, buffer.len());
        return Ok(buffer.len());
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Random {
        // Linux guarantees cryptographic output from both random devices after
        // the kernel RNG is initialized. BCrypt's system-preferred generator is
        // the Windows equivalent and needs no process-local algorithm handle.
        const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
        for chunk in buffer.chunks_mut(u32::MAX as usize) {
            // SAFETY: each chunk is writable for the supplied length. A null
            // algorithm handle is documented with SYSTEM_PREFERRED_RNG.
            let status = unsafe {
                BCryptGenRandom(
                    core::ptr::null_mut(),
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            };
            if status < 0 {
                return Err(EIO);
            }
        }
        advance_offset(fd, entry.generation, buffer.len());
        return Ok(buffer.len());
    }
    let result = platform::read(&entry, buffer);
    #[cfg(windows)]
    if entry.kind == FdKind::Pipe && !buffer.is_empty() && (result.is_ok() || result == Err(EAGAIN))
    {
        // The reader can drain and refill between epoll snapshots. Retaining
        // the old edge in that interval loses the next notification (including
        // the final log bytes that runc must drain before opening exec.fifo).
        epoll::readiness_consumed(entry.description_id, epoll::EPOLLIN | epoll::EPOLLRDNORM);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Pipe && fork_trace_enabled() {
        eprintln!(
            "kinakaze vfs: pid {} read fd={fd} handle={:#x} requested={} result={result:?}",
            std::process::id(),
            entry.raw,
            buffer.len()
        );
    }
    let read = result?;
    advance_offset(fd, entry.generation, read);
    Ok(read)
}

pub fn write(fd: i32, buffer: &[u8]) -> Result<usize, i32> {
    #[cfg(windows)]
    if get(fd)?.flags.contains(FdFlags::SEEKABLE) {
        return ofd::with(fd, || write_inner(fd, buffer));
    }
    write_inner(fd, buffer)
}
fn write_inner(fd: i32, buffer: &[u8]) -> Result<usize, i32> {
    #[cfg(windows)]
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return tmpfs::write(fd, buffer, None);
    }
    if trace_enabled() {
        eprintln!("kinakaze: [TRACE write] fd={fd} len={}", buffer.len());
    }
    #[cfg(windows)]
    let (entry, _inode_pin) = native_pin::read_entry(fd)?;
    #[cfg(not(windows))]
    let entry = get(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if entry.kind == FdKind::Pipe && !entry.flags.contains(FdFlags::PIPE_WRITE_END) {
        return Err(EBADF);
    }
    if matches!(
        entry.kind,
        FdKind::MountNamespace
            | FdKind::TimeNamespace
            | FdKind::Namespace
            | FdKind::UserNamespace
            | FdKind::FsContext
            | FdKind::MountTree
    ) {
        return Err(EINVAL);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Fifo {
        let result = fifo::write(fd, entry, buffer);
        if matches!(result, Ok(count) if count != 0) {
            epoll::readiness_consumed(entry.description_id, epoll::EPOLLOUT | epoll::EPOLLWRNORM);
        }
        return result;
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Socket {
        // SAFETY: the slice provides `len` readable bytes.
        return unsafe { socket::send(fd, buffer.as_ptr(), buffer.len(), 0) };
    }
    #[cfg(windows)]
    if entry.kind == FdKind::NetlinkSocket {
        // SAFETY: the slice provides `len` readable bytes.
        return unsafe { netlink::send(fd, buffer.as_ptr(), buffer.len(), 0) };
    }
    #[cfg(windows)]
    if entry.kind == FdKind::UnixSocket {
        // SAFETY: the slice provides `len` readable bytes.
        return unsafe { unix::send(fd, buffer.as_ptr(), buffer.len(), 0) };
    }
    #[cfg(windows)]
    if entry.kind == FdKind::TimerFd {
        return Err(EINVAL);
    }
    if entry.kind == FdKind::EventFd {
        return eventfd::write_eventfd(fd, buffer, entry.flags.contains(FdFlags::NONBLOCK));
    }
    // A terminal write goes through OPOST before it reaches the other end, which
    // is where ONLCR turns a bare newline into CR LF.
    #[cfg(windows)]
    if matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave) {
        return tty::write(fd, buffer);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Console && tty::is_console(fd) {
        return tty::console_write(buffer);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::CgroupFile {
        if !entry.flags.contains(FdFlags::WRITE_ACCESS) {
            return Err(EBADF);
        }
        let path = cgroup_paths()
            .lock()
            .ok()
            .and_then(|map| map.get(&fd).cloned())
            .ok_or(EIO)?;
        cgroup::write_file(&path, buffer)?;
        advance_offset(fd, entry.generation, buffer.len());
        return Ok(buffer.len());
    }
    #[cfg(windows)]
    if entry.kind == FdKind::ProcSysctl {
        if !entry.flags.contains(FdFlags::WRITE_ACCESS) {
            return Err(EBADF);
        }
        let path = proc_sysctl_file_path(fd)?;
        let written = procfs::pinned(|| procfs::write_file(&path, buffer, entry.offset))?;
        if procfs::pinned(|| procfs::write_advances_offset(&path)) {
            advance_offset(fd, entry.generation, written);
        }
        return Ok(written);
    }
    // procfs is read-only, so a write is refused rather than silently dropped.
    #[cfg(windows)]
    if matches!(entry.kind, FdKind::Synthetic | FdKind::SyntheticDirectory) {
        return Err(EACCES);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::BpfProgram {
        return Err(EINVAL);
    }
    #[cfg(windows)]
    if matches!(entry.kind, FdKind::Null | FdKind::Zero | FdKind::Random) {
        advance_offset(fd, entry.generation, buffer.len());
        return Ok(buffer.len());
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Full {
        return Err(crate::ENOSPC);
    }
    let result = platform::write(&entry, buffer);
    #[cfg(windows)]
    if entry.kind == FdKind::Pipe && !buffer.is_empty() && (result.is_ok() || result == Err(EAGAIN))
    {
        epoll::readiness_consumed(entry.description_id, epoll::EPOLLOUT | epoll::EPOLLWRNORM);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Pipe && fork_trace_enabled() {
        eprintln!(
            "kinakaze vfs: pid {} write fd={fd} handle={:#x} requested={} result={result:?}",
            std::process::id(),
            entry.raw,
            buffer.len()
        );
    }
    let written = result?;
    if entry.flags.contains(FdFlags::APPEND) {
        // The kernel placed this write at the end of file, so the resulting
        // position is the new file size rather than the previous offset plus
        // the byte count.
        #[cfg(windows)]
        if let Ok(size) = platform::file_size(entry) {
            set_offset(fd, entry.generation, size);
        }
    } else {
        advance_offset(fd, entry.generation, written);
    }
    Ok(written)
}

/// Stores an absolute position for a descriptor, guarding against reuse.
fn set_offset(fd: i32, generation: u32, offset: u64) {
    let Ok(mut table) = table().write() else {
        return;
    };
    if let Some(Some(entry)) = table.slots.get_mut(fd as usize)
        && entry.generation == generation
    {
        entry.offset = offset;
    }
}

pub fn close(fd: i32) -> Result<(), i32> {
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze vfs: pid {} close(fd={}) called",
            std::process::id(),
            fd
        );
    }
    fd_trace("close", fd, FdKind::Unknown);
    if fd < 0 {
        return Err(EBADF);
    }
    // The Unix socket state is detached while the slot is still held, not
    // afterwards: releasing the lock first leaves a window in which another
    // thread can claim the freed fd and install its own state, which the cleanup
    // below would then destroy along with that thread's live handle.
    #[cfg(windows)]
    let mut detached = None;
    let (entry, description_survivor) = {
        let mut table = table().write().map_err(|_| EIO)?;
        let slot = table.slots.get_mut(fd as usize).ok_or(EBADF)?;
        let closing = *slot.as_ref().ok_or(EBADF)?;
        #[cfg(windows)]
        {
            // Native handles may outlive their integer slot while cancellation
            // retires. Remove them from ambient fork inheritance before making
            // the slot reusable, including FIFO's auxiliary inode marker.
            if closing.kind == FdKind::Fifo {
                fifo::disable_inheritance_locked(closing)?;
            }
            if closing.raw != 0
                && !matches!(closing.kind, FdKind::Socket | FdKind::Fifo)
                && !closing.flags.contains(FdFlags::BORROWED)
            {
                platform::try_set_inheritable(closing.raw, false)?;
            }
        }
        let entry = table.slots.remove(fd as usize).ok_or(EBADF)?;
        #[cfg(windows)]
        if entry.kind == FdKind::UnixSocket {
            detached = unix::detach(fd);
        }
        #[cfg(windows)]
        if entry.kind == FdKind::NetlinkSocket {
            netlink::detach(fd);
        }
        #[cfg(windows)]
        if entry.kind == FdKind::BpfProgram {
            bpf::close(fd);
        }
        #[cfg(windows)]
        if entry.kind == FdKind::EventFd {
            // Detach before the integer slot becomes allocatable. Otherwise a
            // concurrent creator can publish a new counter at this same fd and
            // have it removed by the old descriptor's delayed cleanup.
            eventfd::forget_eventfd(fd);
        }
        let survivor = table.slots.enumerated().find_map(|(other_fd, slot)| {
            let candidate = slot.as_ref()?;
            (candidate.description_id == entry.description_id)
                .then_some((other_fd as i32, *candidate))
        });
        if survivor.is_none() {
            mount::overlay::closed(entry.description_id);
            mount::native::closed(entry.description_id);
            pipe_inode::closed(entry.description_id);
            #[cfg(windows)]
            ofd::closed(entry.description_id);
            #[cfg(windows)]
            usernet::closed(entry.description_id);
        }
        mount::api::closed(entry, table.slots.iter().flatten().copied());
        (entry, survivor)
    };
    #[cfg(windows)]
    epoll::descriptor_closed(entry.description_id, description_survivor);
    #[cfg(windows)]
    let lock_result = record_lock::descriptor_closed(entry);
    if entry.kind == FdKind::Pipe && std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
        eprintln!(
            "kinakaze pipe: pid={} close fd={fd} raw={:#x} flags={:#x}",
            std::process::id(),
            entry.raw,
            entry.flags.0
        );
    }
    if entry.kind == FdKind::MessageQueue {
        mqueue::closed(entry);
    }
    if entry.flags.contains(FdFlags::BORROWED) {
        return Ok(());
    }
    #[cfg(windows)]
    if entry.kind == FdKind::Fifo {
        return fifo::close_entry(fd, entry);
    }
    // Sockets must be released with closesocket, not CloseHandle, so Winsock can
    // run its own teardown.
    #[cfg(windows)]
    if entry.kind == FdKind::Socket {
        return socket::close_socket(entry.raw);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::NetlinkSocket {
        return Ok(());
    }
    #[cfg(windows)]
    if entry.kind == FdKind::BpfProgram {
        return Ok(());
    }
    // A Unix socket owns its pipe instance and possibly a placeholder file, which
    // that layer releases together.
    #[cfg(windows)]
    if entry.kind == FdKind::UnixSocket {
        match detached {
            Some(state) => unix::release_detached(state),
            // No side-table state, which happens only for a descriptor closed
            // between allocation and initialization. The handle, if any, is still
            // this function's to release.
            None => return platform::close(entry),
        }
        return Ok(());
    }
    // An epoll set's registrations live outside the table and would otherwise
    // outlive the descriptor.
    #[cfg(windows)]
    if entry.kind == FdKind::Event {
        epoll::forget_set(fd, entry.raw);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::IoRing {
        iouring::forget_ring(fd, entry.raw);
    }
    #[cfg(windows)]
    if entry.kind == FdKind::EventFd && entry.raw == 0 {
        return Ok(());
    }
    if entry.kind == FdKind::Inotify {
        inotify::close_inotify(fd);
        return Ok(());
    }
    // A synthetic descriptor owns no kernel object, only its side-table entry.
    #[cfg(windows)]
    if matches!(
        entry.kind,
        FdKind::Synthetic | FdKind::SyntheticDirectory | FdKind::CgroupFile | FdKind::ProcSysctl
    ) {
        forget_synthetic(fd, entry.generation);
        return Ok(());
    }
    // Linux character devices with entirely synthetic behaviour own no host
    // handle. Closing the descriptor only removes the table entry above.
    #[cfg(windows)]
    if matches!(
        entry.kind,
        FdKind::Null | FdKind::Zero | FdKind::Random | FdKind::Full
    ) {
        return Ok(());
    }
    // A console's cached terminal attributes are keyed by descriptor number and
    // would otherwise be inherited by whatever reuses the slot.
    #[cfg(windows)]
    if entry.kind == FdKind::Console {
        pty::forget(fd);
    }
    // A terminal end owns a handle to its readiness event, and closing the last
    // one on a side is what tells the other end that this side has gone.
    #[cfg(windows)]
    if matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave) {
        tty::close_descriptor(fd, entry.raw);
        return Ok(());
    }
    let result = platform::close(entry);
    #[cfg(windows)]
    lock_result?;
    result
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::mem;
    use std::ptr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_NO_DATA,
        ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_WRITE,
        GetHandleInformation, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
        WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_TYPE_CHAR,
        FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileSizeEx, GetFileType, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
    };
    use windows_sys::Win32::System::Threading::{INFINITE, ResetEvent, WaitForMultipleObjects};

    use crate::fs::NativeIoStatus as IoStatusBlock;

    #[repr(C)]
    #[derive(Default)]
    struct FileAccessInformation {
        access_flags: u32,
    }

    const FILE_ACCESS_INFORMATION_CLASS: u32 = 8;
    const FILE_READ_DATA: u32 = 0x0000_0001;
    const FILE_WRITE_DATA: u32 = 0x0000_0002;
    const FILE_APPEND_DATA: u32 = 0x0000_0004;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationFile(
            file: HANDLE,
            io_status_block: *mut IoStatusBlock,
            information: *mut core::ffi::c_void,
            length: u32,
            information_class: u32,
        ) -> i32;
    }

    /// Reads an inherited pipe handle's granted access once, when its Linux
    /// descriptor is constructed.  Failure leaves the standard descriptor
    /// absent: guessing from fd 0/1/2 would corrupt a deliberately redirected
    /// or duplex handle and would reintroduce the epoll-time compatibility
    /// fallback this metadata is meant to remove.
    fn inherited_pipe_access(handle: HANDLE) -> Result<FdFlags, ()> {
        let mut information = FileAccessInformation::default();
        let mut status = IoStatusBlock::default();
        let result = unsafe {
            NtQueryInformationFile(
                handle,
                &raw mut status,
                (&raw mut information).cast(),
                core::mem::size_of::<FileAccessInformation>() as u32,
                FILE_ACCESS_INFORMATION_CLASS,
            )
        };
        if result < 0 {
            return Err(());
        }
        let mut flags = FdFlags::NONE;
        if information.access_flags & FILE_READ_DATA != 0 {
            flags = flags.union(FdFlags::PIPE_READ_END);
        }
        if information.access_flags & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0 {
            flags = flags.union(FdFlags::PIPE_WRITE_END);
        }
        flags.has_pipe_access().then_some(flags).ok_or(())
    }

    /// Lists inheritable pipe handles still alive after an exec wrapper has
    /// released its Linux descriptor table. This is trace-only diagnostics for
    /// detecting an ambient Win32 handle that has no corresponding guest fd.
    pub(super) fn trace_inheritable_pipe_handles() {
        for raw in (4usize..=0x1_0000).step_by(4) {
            let mut information = 0u32;
            let handle = raw as HANDLE;
            if unsafe { GetHandleInformation(handle, &raw mut information) } == 0
                || information & HANDLE_FLAG_INHERIT == 0
                || unsafe { GetFileType(handle) } != FILE_TYPE_PIPE
            {
                continue;
            }
            let access = inherited_pipe_access(handle)
                .map(|flags| flags.0)
                .unwrap_or_default();
            eprintln!(
                "kinakaze pipe: pid={} wrapper-residual raw={raw:#x} access_flags={access:#x}",
                std::process::id()
            );
        }
    }

    pub(super) fn install_standard_streams(table: &mut FdTable) {
        for (fd, selector) in [
            (0, STD_INPUT_HANDLE),
            (1, STD_OUTPUT_HANDLE),
            (2, STD_ERROR_HANDLE),
        ] {
            // SAFETY: selectors are the three documented standard handles.
            let handle = unsafe { GetStdHandle(selector) };
            if !handle.is_null() && handle as isize != -1 {
                let kind = classify(handle);
                // Inherited standard handles are not opened with
                // FILE_FLAG_OVERLAPPED, so they take the synchronous path. A
                // redirected stdin/stdout is a seekable disk file and still
                // needs its position tracked.
                let mut flags = FdFlags::BORROWED;
                if kind == FdKind::File {
                    flags = flags.union(FdFlags::SEEKABLE);
                }
                if kind == FdKind::Pipe {
                    let Ok(access) = inherited_pipe_access(handle) else {
                        continue;
                    };
                    flags = flags.union(access);
                }
                table.insert_at(fd, handle as usize, kind, flags);
            }
        }
    }

    fn classify(handle: HANDLE) -> FdKind {
        // A pseudo-terminal end reaches a fresh process as an inherited handle
        // and nothing else — `execve` here starts a new Windows process and only
        // the standard handles cross. `GetFileType` cannot recognise it, because
        // the handle is an event; the terminal layer recognises it by name, and
        // asking first is what lets `login_tty` plus `exec` produce a child whose
        // fd 0 is a real terminal rather than an unusable unknown.
        let terminal = super::tty::classify_handle(handle as usize);
        // SAFETY: the handle is live while installed in the table.
        let file_type = unsafe { GetFileType(handle) };
        if super::fd_trace_enabled() {
            eprintln!(
                "[fd] classify handle={:#x} file_type={file_type} terminal={terminal:?}",
                handle as usize
            );
        }
        if let Some(kind) = terminal {
            return kind;
        }
        match file_type {
            FILE_TYPE_CHAR => FdKind::Console,
            FILE_TYPE_DISK => FdKind::File,
            FILE_TYPE_PIPE => FdKind::Pipe,
            _ => FdKind::Unknown,
        }
    }

    use crate::io_event::IoEvent;

    /// Creates a connected unidirectional byte pipe whose two handles support
    /// independent overlapped operations.
    pub(super) fn create_overlapped_pipe_pair(capacity: u32) -> Result<(usize, usize), i32> {
        static NEXT_PIPE: AtomicU64 = AtomicU64::new(1);

        let sequence = NEXT_PIPE.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            r"\\.\pipe\kinakaze-anonymous-{}-{sequence}",
            std::process::id()
        );
        let mut wide_name = name.encode_utf16().collect::<Vec<_>>();
        wide_name.push(0);

        // The server is the read endpoint. FIRST_PIPE_INSTANCE turns a name
        // collision into a hard creation error rather than attaching to an
        // object that belongs to somebody else.
        let server = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                capacity,
                capacity,
                0,
                ptr::null(),
            )
        };
        if server.is_null() || server as isize == -1 {
            return Err(map_error(unsafe { GetLastError() }));
        }

        let connect_event = IoEvent::new().ok_or_else(|| {
            unsafe { CloseHandle(server) };
            EIO
        })?;
        let mut connect: OVERLAPPED = unsafe { mem::zeroed() };
        connect.hEvent = connect_event.0;
        let connected = unsafe { ConnectNamedPipe(server, &raw mut connect) };
        let connect_pending = if connected == 0 {
            match unsafe { GetLastError() } {
                ERROR_IO_PENDING => true,
                ERROR_PIPE_CONNECTED => false,
                error => {
                    unsafe { CloseHandle(server) };
                    return Err(map_error(error));
                }
            }
        } else {
            false
        };

        // The client is the write endpoint and is opened only after the server
        // has an outstanding connect request, so no polling or retry is needed.
        let client = unsafe {
            CreateFileW(
                wide_name.as_ptr(),
                GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if client.is_null() || client as isize == -1 {
            let error = unsafe { GetLastError() };
            unsafe {
                CancelIoEx(server, &raw mut connect);
                CloseHandle(server);
            }
            return Err(map_error(error));
        }

        if connect_pending {
            let waited = unsafe { WaitForMultipleObjects(1, &connect_event.0, 0, INFINITE) };
            let mut transferred = 0u32;
            let completed = waited == WAIT_OBJECT_0
                && unsafe { GetOverlappedResult(server, &raw const connect, &mut transferred, 0) }
                    != 0;
            if !completed {
                let error = unsafe { GetLastError() };
                unsafe {
                    CloseHandle(client);
                    CloseHandle(server);
                }
                return Err(map_error(error));
            }
        }

        Ok((server as usize, client as usize))
    }

    pub(super) fn close_raw(raw: usize) {
        unsafe { CloseHandle(raw as HANDLE) };
    }

    /// Direction of a pending overlapped transfer.
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Direction {
        Read,
        Write,
    }

    /// Whether Windows is reporting the Linux EOF state of a pipe reader.
    ///
    /// The same host status means `EPIPE` to a writer, so this must remain at the
    /// operation boundary rather than being folded into the global Win32 errno
    /// mapping. Linux readers observe zero only after buffered bytes have been
    /// drained; the Windows pipe completes the corresponding read with one of
    /// these disconnect statuses at exactly that point.
    fn pipe_read_eof(entry: &FdEntry, direction: Direction, error: u32) -> bool {
        entry.kind == FdKind::Pipe
            && direction == Direction::Read
            && matches!(
                error,
                ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED
            )
    }

    /// Issues one overlapped transfer and waits for completion or interruption.
    ///
    /// The dual wait on `[io_event, interrupt_event]` is what makes a blocking
    /// call interruptible: when the interrupt event wins, the in-flight request
    /// is cancelled through `CancelIoEx` and the call reports `EINTR` unless the
    /// kernel had already transferred bytes.
    ///
    /// # Safety
    ///
    /// `buffer` must point to `len` bytes that are writable for
    /// [`Direction::Read`] and readable for [`Direction::Write`], and must stay
    /// valid until this function returns.
    unsafe fn overlapped_transfer(
        entry: &FdEntry,
        buffer: *mut u8,
        len: usize,
        direction: Direction,
    ) -> Result<usize, i32> {
        // A handler installed with SA_RESTART requires the interrupted transfer
        // to be reissued rather than reported, so the attempt sits in a loop.
        loop {
            // SAFETY: forwarded from this function's own contract.
            match unsafe { overlapped_attempt(entry, buffer, len, direction) } {
                Err(super::EINTR) => {
                    // The wait already ran the handlers; only a restart request
                    // reaches here as an interrupt that should be retried.
                    match super::signal::deliver_pending() {
                        super::signal::Delivery::Restart => continue,
                        _ => return Err(super::EINTR),
                    }
                }
                other => return other,
            }
        }
    }

    /// Issues a single overlapped transfer without restart handling.
    ///
    /// # Safety
    ///
    /// Same contract as [`overlapped_transfer`].
    unsafe fn overlapped_attempt(
        entry: &FdEntry,
        buffer: *mut u8,
        len: usize,
        direction: Direction,
    ) -> Result<usize, i32> {
        let len = if direction == Direction::Write {
            limited_write_length(entry, len)?
        } else {
            len
        };
        let handle = entry.raw as HANDLE;
        let amount = len.min(u32::MAX as usize) as u32;
        let io_event = IoEvent::new().ok_or(EIO)?;

        // SAFETY: OVERLAPPED is a plain data structure that the kernel expects
        // to be zeroed apart from the offset and event fields.
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = io_event.0;
        // An all-ones offset is the documented request to write at the current
        // end of file, which keeps O_APPEND atomic against other writers.
        let append = direction == Direction::Write && entry.flags.contains(FdFlags::APPEND);
        if append {
            overlapped.Anonymous.Anonymous.Offset = u32::MAX;
            overlapped.Anonymous.Anonymous.OffsetHigh = u32::MAX;
        } else if entry.flags.contains(FdFlags::SEEKABLE) {
            overlapped.Anonymous.Anonymous.Offset = entry.offset as u32;
            overlapped.Anonymous.Anonymous.OffsetHigh = (entry.offset >> 32) as u32;
        }

        let mut transferred = 0u32;
        // SAFETY: the caller guarantees `buffer` is valid for `amount` bytes in
        // the requested direction, and `overlapped` outlives the wait below.
        let started = unsafe {
            match direction {
                Direction::Read => {
                    ReadFile(handle, buffer, amount, ptr::null_mut(), &raw mut overlapped)
                }
                Direction::Write => {
                    WriteFile(handle, buffer, amount, ptr::null_mut(), &raw mut overlapped)
                }
            }
        };

        if started == 0 {
            // SAFETY: GetLastError has no preconditions.
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return if error == ERROR_HANDLE_EOF || pipe_read_eof(entry, direction, error) {
                    Ok(0)
                } else {
                    Err(map_error(error))
                };
            }
            if entry.flags.contains(FdFlags::NONBLOCK) {
                // A pending overlapped request is exactly Windows' statement
                // that this transfer would block. Cancel this one request and
                // reap it before returning EAGAIN so the caller's buffer is no
                // longer owned by the kernel when this function returns.
                unsafe { CancelIoEx(handle, &raw mut overlapped) };
                unsafe { WaitForMultipleObjects(1, &io_event.0, 0, INFINITE) };
                let completed = unsafe {
                    GetOverlappedResult(handle, &raw const overlapped, &mut transferred, 0)
                };
                if completed != 0 || transferred != 0 {
                    return Ok(transferred as usize);
                }
                let error = unsafe { GetLastError() };
                return match error {
                    ERROR_OPERATION_ABORTED => Err(EAGAIN),
                    ERROR_HANDLE_EOF => Ok(0),
                    _ if pipe_read_eof(entry, direction, error) => Ok(0),
                    other => Err(map_error(other)),
                };
            }
            // SAFETY: the request is pending against `overlapped`, whose event
            // is live for the duration of the wait.
            unsafe { wait_for_completion(handle, &raw mut overlapped, io_event.0)? };
        }

        // SAFETY: the operation has completed or been cancelled, so the
        // overlapped result is ready to collect.
        let ok = unsafe { GetOverlappedResult(handle, &raw const overlapped, &mut transferred, 0) };
        if ok == 0 {
            // SAFETY: GetLastError has no preconditions.
            let error = unsafe { GetLastError() };
            return match error {
                // End of file is a zero-length read, not a failure.
                ERROR_HANDLE_EOF => Ok(0),
                _ if pipe_read_eof(entry, direction, error) => Ok(0),
                // A cancelled request that moved no bytes is the interrupted
                // case; POSIX reports the partial count when bytes did move.
                ERROR_OPERATION_ABORTED if transferred == 0 => Err(EINTR),
                ERROR_OPERATION_ABORTED => Ok(transferred as usize),
                other => Err(map_error(other)),
            };
        }
        Ok(transferred as usize)
    }

    /// Waits for either I/O completion or a thread interrupt.
    ///
    /// # Safety
    ///
    /// `overlapped` must describe a request that is currently pending on
    /// `handle`, and `io_event` must be its completion event.
    unsafe fn wait_for_completion(
        handle: HANDLE,
        overlapped: *mut OVERLAPPED,
        io_event: HANDLE,
    ) -> Result<(), i32> {
        let interrupt = super::interrupt::current();
        if interrupt.is_null() {
            // Without an interrupt event the operation is simply uninterruptible.
            // SAFETY: the request is pending and the event is live.
            let waited = unsafe { WaitForMultipleObjects(1, &io_event, 0, INFINITE) };
            return if waited == WAIT_OBJECT_0 {
                Ok(())
            } else {
                // A failed multi-wait must not leave the request referencing
                // this stack or an event that another operation can reuse.
                unsafe { CancelIoEx(handle, overlapped) };
                unsafe { WaitForMultipleObjects(1, &io_event, 0, INFINITE) };
                Err(EIO)
            };
        }

        let handles = [io_event, interrupt];
        // Announce this thread as interruptible so `kill` knows to wake it.
        super::signal::register_waiter();
        // SAFETY: both handles are live for the duration of the wait.
        let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        super::signal::unregister_waiter();

        if waited == WAIT_OBJECT_0 {
            return Ok(());
        }
        if waited != WAIT_OBJECT_0 + 1 {
            unsafe { CancelIoEx(handle, overlapped) };
            unsafe { WaitForMultipleObjects(1, &io_event, 0, INFINITE) };
            return Err(EIO);
        }

        // The interrupt fired. Cancel the request and wait for the kernel to
        // retire it; the completion event is always signalled afterwards, so
        // `GetOverlappedResult` observes the final state rather than a race.
        // SAFETY: `overlapped` names a request pending on this handle.
        unsafe { CancelIoEx(handle, overlapped) };
        // SAFETY: the completion event remains live until the request retires.
        unsafe { WaitForMultipleObjects(1, &io_event, 0, INFINITE) };
        // Clear the consumed interrupt so it does not abort the next call.
        // SAFETY: the interrupt event belongs to this thread.
        unsafe { ResetEvent(interrupt) };
        Ok(())
    }

    /// Runs one overlapped transfer, exposed for callers outside the table path.
    ///
    /// # Safety
    ///
    /// `buffer` must be valid for `len` bytes in the requested direction.
    pub(super) unsafe fn transfer(
        entry: &FdEntry,
        buffer: *mut u8,
        len: usize,
        read: bool,
    ) -> Result<usize, i32> {
        if entry.kind == FdKind::File {
            if read {
                // Internal inode/tree I/O uses transfer_once and never recurses
                // through this guest-data boundary. Locks/temporary handles
                // unwind before delivering any interrupt handler.
                if entry.flags.contains(FdFlags::WRITE_ACCESS)
                    && !entry.flags.contains(FdFlags::READ_ACCESS)
                {
                    return Err(super::EBADF);
                }
                let bytes = if len == 0 {
                    &mut []
                } else {
                    unsafe { std::slice::from_raw_parts_mut(buffer, len) }
                };
                loop {
                    match super::fs::verity::verified_read(entry.raw as HANDLE, entry.offset, bytes)
                    {
                        Err(super::EINTR) => match super::signal::deliver_pending() {
                            super::signal::Delivery::Restart => continue,
                            _ => return Err(super::EINTR),
                        },
                        Ok(Some(count)) => return Ok(count),
                        Ok(None) => return Err(EIO),
                        Err(error) => return Err(error),
                    }
                }
            }
            check_file_write(entry)?;
        }
        let direction = if read {
            Direction::Read
        } else {
            Direction::Write
        };
        // SAFETY: forwarded from this function's contract.
        let result = unsafe { overlapped_transfer(entry, buffer, len, direction) };
        if !read && entry.kind == FdKind::File {
            if let Err(error) = result {
                super::mount::overlay::record_io_error(entry.raw as _, error);
            }
        }
        result
    }

    /// Internal transactions must unwind temporary native handles before signal
    /// handlers can fork/exec. Return EINTR without dispatching guest code here.
    /// Safety: same live-handle/buffer requirements as `transfer`.
    pub(super) unsafe fn transfer_once(
        entry: &FdEntry,
        buffer: *mut u8,
        len: usize,
        read: bool,
    ) -> Result<usize, i32> {
        let direction = if read {
            Direction::Read
        } else {
            Direction::Write
        };
        let result = unsafe { overlapped_attempt(entry, buffer, len, direction) };
        if !read && entry.kind == FdKind::File {
            if let Err(error) = result {
                super::mount::overlay::record_io_error(entry.raw as _, error);
            }
        }
        result
    }

    pub(super) fn read(entry: &FdEntry, buffer: &mut [u8]) -> Result<usize, i32> {
        if entry.kind == FdKind::File {
            return unsafe { transfer(entry, buffer.as_mut_ptr(), buffer.len(), true) };
        }
        if entry.flags.contains(FdFlags::OVERLAPPED) {
            // SAFETY: the slice provides `len` writable bytes for the transfer.
            return unsafe {
                overlapped_transfer(entry, buffer.as_mut_ptr(), buffer.len(), Direction::Read)
            };
        }
        if entry.kind == FdKind::Pipe && entry.flags.contains(FdFlags::NONBLOCK) {
            let mut available = 0u32;
            // CreatePipe returns synchronous handles, so issuing ReadFile first
            // would block inside the kernel. PeekNamedPipe is the atomic
            // readiness query available for this handle type.
            let ok = unsafe {
                PeekNamedPipe(
                    entry.raw as HANDLE,
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &raw mut available,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                    Ok(0)
                } else {
                    Err(last_errno())
                };
            }
            if available == 0 {
                return Err(EAGAIN);
            }
        }
        let mut read = 0u32;
        let amount = buffer.len().min(u32::MAX as usize) as u32;
        // SAFETY: the buffer is writable for `amount` bytes and the handle is live.
        let ok = unsafe {
            ReadFile(
                entry.raw as HANDLE,
                buffer.as_mut_ptr(),
                amount,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            // A byte pipe with no writers is at EOF on Linux. Windows spells
            // the same state ERROR_BROKEN_PIPE; exposing EPIPE from read would
            // make ordinary shell consumers report an error after valid data.
            if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                Ok(0)
            } else {
                Err(last_errno())
            }
        } else {
            Ok(read as usize)
        }
    }

    pub(super) fn check_file_write(entry: &FdEntry) -> Result<(), i32> {
        if entry.flags.contains(FdFlags::READ_ACCESS)
            && !entry.flags.contains(FdFlags::WRITE_ACCESS)
        {
            return Err(super::EBADF);
        }
        let access = super::fs::object::Object::granted_access(entry.raw as HANDLE)?;
        if access & (FILE_WRITE_DATA | FILE_APPEND_DATA) == 0 {
            return Err(super::EBADF);
        }
        super::fs::verity::ensure_writable(entry.raw as HANDLE)
    }

    pub(super) fn write(entry: &FdEntry, buffer: &[u8]) -> Result<usize, i32> {
        if entry.kind == FdKind::File {
            check_file_write(entry)?;
        }
        if entry.flags.contains(FdFlags::OVERLAPPED) {
            // SAFETY: the transfer only reads `len` bytes from the slice, so
            // casting away constness never produces a write.
            return unsafe {
                overlapped_transfer(
                    entry,
                    buffer.as_ptr().cast_mut(),
                    buffer.len(),
                    Direction::Write,
                )
            };
        }
        let buffer = &buffer[..limited_write_length(entry, buffer.len())?];
        let mut written = 0u32;
        let amount = buffer.len().min(u32::MAX as usize) as u32;
        // SAFETY: the buffer is readable for `amount` bytes and the handle is live.
        let ok = unsafe {
            WriteFile(
                entry.raw as HANDLE,
                buffer.as_ptr(),
                amount,
                &mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(last_errno())
        } else {
            Ok(written as usize)
        }
    }

    pub(super) fn close(entry: FdEntry) -> Result<(), i32> {
        // SAFETY: ownership of non-borrowed handles belongs to the table.
        if unsafe { CloseHandle(entry.raw as HANDLE) } == 0 {
            Err(last_errno())
        } else {
            Ok(())
        }
    }

    /// Marks a handle inheritable, which is what lets it survive `fork`.
    ///
    /// Failure is not reported: the descriptor is still usable in this process,
    /// and the only consequence is that a child will not see it.
    pub(super) fn set_inheritable(raw: usize, inheritable: bool) {
        let _ = try_set_inheritable(raw, inheritable);
    }

    pub(super) fn try_set_inheritable(raw: usize, inheritable: bool) -> Result<(), i32> {
        // SAFETY: the handle is live while installed in the table.
        if unsafe {
            SetHandleInformation(
                raw as HANDLE,
                HANDLE_FLAG_INHERIT,
                if inheritable { HANDLE_FLAG_INHERIT } else { 0 },
            )
        } == 0
        {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    }

    /// Reports the current size of a descriptor's file.
    fn limited_write_length(entry: &FdEntry, length: usize) -> Result<usize, i32> {
        if entry.kind != FdKind::File || length == 0 {
            return Ok(length);
        }
        let offset = if entry.flags.contains(FdFlags::APPEND) {
            file_size(*entry)?
        } else {
            entry.offset
        };
        super::limits::write_length(offset, length)
    }

    pub(super) fn file_size(entry: FdEntry) -> Result<u64, i32> {
        if entry.kind == FdKind::File {
            return super::fs::verity::authoritative_size(entry.raw as HANDLE);
        }
        let mut size = 0i64;
        // SAFETY: the handle is live and `size` is a writable local.
        if unsafe { GetFileSizeEx(entry.raw as HANDLE, &mut size) } == 0 {
            return Err(last_errno());
        }
        Ok(size as u64)
    }

    fn map_error(error: u32) -> i32 {
        super::errno_from_win32(error)
    }

    fn last_errno() -> i32 {
        // SAFETY: GetLastError has no preconditions.
        map_error(unsafe { GetLastError() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn delayed_cleanup_keeps_replacement_synthetic_and_epoll_instances() {
        let fd = install_procfs_file("/proc/self/status", b"new instance".to_vec(), FdFlags::NONE)
            .unwrap();
        let entry = get(fd).unwrap();
        forget_synthetic(fd, entry.generation.wrapping_sub(1));
        assert_eq!(synthetic_size(fd), Ok(12));
        close(fd).unwrap();

        let fd = epoll::epoll_create1(0).unwrap();
        // A different still-live native handle cannot identify this new set.
        epoll::forget_set(fd, 0);
        let mut events = [epoll::EpollEvent::default(); 1];
        assert_eq!(epoll::epoll_wait(fd, &mut events, 0), Ok(0));
        close(fd).unwrap();

        let fd = iouring::setup(2, 0).unwrap();
        iouring::forget_ring(fd, 0);
        assert!(iouring::is_ring(fd));
        close(fd).unwrap();
    }

    #[test]
    fn fd_table_selects_the_lowest_free_slot_at_or_above_a_floor() {
        let mut table = FdTable {
            slots: fd_slots::Slots::default(),
            next_generation: 1,
            next_description_id: 1,
        };
        table.insert_at(2, 0x20, FdKind::File, FdFlags::BORROWED);
        table.insert_at(4, 0x40, FdKind::File, FdFlags::BORROWED);
        table.insert_at(5, 0x50, FdKind::File, FdFlags::BORROWED);

        assert_eq!(table.first_free_from(0), Some(0));
        assert_eq!(table.first_free_from(2), Some(3));
        assert_eq!(table.first_free_from(4), Some(6));
        assert_eq!(table.first_free_from(7), Some(7));
        assert_eq!(table.first_free_between(8, 8), None);
    }

    #[cfg(windows)]
    #[test]
    fn descriptor_reservation_is_invisible_and_cannot_be_stolen() {
        let reservation = reserve_descriptor().unwrap();
        let reserved = reservation.fd.unwrap() as i32;
        assert!(matches!(get(reserved), Err(EBADF)));
        assert_eq!(close(reserved), Err(EBADF));
        assert_eq!(
            install_exact(0, FdKind::UnixSocket, FdFlags::NONE, reserved),
            Err(EBUSY)
        );
        let later = unix::socket(socket::SOCK_STREAM, 0).unwrap();
        assert!(later > reserved);
        drop(reservation);
        let next = unix::socket(socket::SOCK_STREAM, 0).unwrap();
        assert_eq!(next, reserved);
        close(next).unwrap();
        close(later).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn descriptor_reservation_releases_on_registration_failure() {
        let reservation = reserve_descriptor().unwrap();
        let reserved = reservation.fd.unwrap() as i32;
        assert_eq!(
            reservation.install_with(0, FdKind::UnixSocket, FdFlags::NONE, |_, _| Err(EIO)),
            Err(EIO)
        );
        let next = unix::socket(socket::SOCK_STREAM, 0).unwrap();
        assert_eq!(next, reserved);
        close(next).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn every_fd_kind_round_trips_through_the_fork_wire_format() {
        let kinds = [
            FdKind::File,
            FdKind::Directory,
            FdKind::Console,
            FdKind::Pipe,
            FdKind::Socket,
            FdKind::Event,
            FdKind::EventFd,
            FdKind::UnixSocket,
            FdKind::Synthetic,
            FdKind::SyntheticDirectory,
            FdKind::Null,
            FdKind::Zero,
            FdKind::Random,
            FdKind::Full,
            FdKind::Inotify,
            FdKind::PtyMaster,
            FdKind::PtySlave,
            FdKind::CgroupFile,
            FdKind::NetlinkSocket,
            FdKind::BpfProgram,
            FdKind::ProcSysctl,
            FdKind::Fifo,
            FdKind::IoRing,
            FdKind::Unknown,
        ];

        for kind in kinds {
            assert_eq!(FdKind::from_fork_code(kind.fork_code()), kind);
        }
        assert_eq!(FdKind::from_fork_code(u32::MAX), FdKind::Unknown);
    }

    #[cfg(windows)]
    #[test]
    fn fork_validation_accepts_every_side_table_backed_descriptor() {
        let kinds = [
            FdKind::EventFd,
            FdKind::UnixSocket,
            FdKind::Synthetic,
            FdKind::SyntheticDirectory,
            FdKind::Null,
            FdKind::Zero,
            FdKind::Random,
            FdKind::Full,
            FdKind::CgroupFile,
            FdKind::NetlinkSocket,
            FdKind::BpfProgram,
            FdKind::ProcSysctl,
        ];
        assert!(kinds.into_iter().all(FdKind::allows_missing_host_handle));
        assert!(!FdKind::Pipe.allows_missing_host_handle());
        assert!(!FdKind::File.allows_missing_host_handle());
    }

    #[cfg(windows)]
    #[test]
    fn synthetic_fork_state_preserves_cgroup_identity() {
        let fd = install(
            0,
            FdKind::CgroupFile,
            FdFlags::SEEKABLE.union(FdFlags::READ_ACCESS),
        )
        .unwrap();
        let generation = get(fd).unwrap().generation;
        synthetic()
            .write()
            .unwrap()
            .insert(fd, (generation, b"max 100000\n".to_vec()));
        cgroup_paths()
            .lock()
            .unwrap()
            .insert(fd, "/sys/fs/cgroup/cpu.max".to_owned());

        let payload = serialize_synthetic().unwrap();
        synthetic().write().unwrap().clear();
        cgroup_paths().lock().unwrap().clear();
        assert!(restore_synthetic(&payload));
        assert_eq!(cgroup_file_path(fd).unwrap(), "/sys/fs/cgroup/cpu.max");
        let mut bytes = [0u8; 32];
        let count = read(fd, &mut bytes).unwrap();
        assert_eq!(&bytes[..count], b"max 100000\n");
        close(fd).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn synthetic_identity_survives_dup_close_and_handoff() {
        for path in ["/proc/self/status", "/proc/sys/kernel/hostname"] {
            let fd = fs::open(path, fs::O_RDONLY, 0).unwrap();
            let expected = procfs::metadata(path)
                .unwrap()
                .target
                .unwrap_or_else(|| procfs::canonical_path(path));
            assert_eq!(procfs::local_fd_link_target(fd).unwrap(), expected);
            publish_proc_fd_snapshot().unwrap();
            let source = get(fd).unwrap();
            let alias = install(0, source.kind, source.flags).unwrap();
            duplicate_synthetic_description(fd, alias).unwrap();
            close(fd).unwrap();
            assert_eq!(procfs::local_fd_link_target(alias).unwrap(), expected);
            let payload = serialize_synthetic().unwrap();
            proc_sysctl_paths().lock().unwrap().remove(&alias);
            assert!(restore_synthetic(&payload));
            assert_eq!(procfs::local_fd_link_target(alias).unwrap(), expected);
            let mut bytes = [0; 32];
            assert!(read(alias, &mut bytes).unwrap() > 0);
            close(alias).unwrap();
            let replacement = fs::open("/proc/uptime", fs::O_RDONLY, 0).unwrap();
            assert_eq!(
                procfs::local_fd_link_target(replacement).unwrap(),
                "/proc/uptime"
            );
            close(replacement).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn handleless_open_paths_preserve_close_on_exec() {
        let flags = fs::O_RDONLY | fs::O_CLOEXEC;
        let descriptors = [
            fs::open("/proc/self/status", flags, 0).unwrap(),
            fs::open("/sys/fs/cgroup/cpu.max", flags, 0).unwrap(),
            fs::open("/dev/null", flags, 0).unwrap(),
        ];
        for fd in descriptors {
            assert!(get(fd).unwrap().flags.contains(FdFlags::CLOSE_ON_EXEC));
            close(fd).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn random_device_reads_from_the_system_generator() {
        let fd = install_dev_special(FdKind::Random, FdFlags::NONE).unwrap();
        let mut first = [0u8; 32];
        let mut second = [0u8; 32];
        assert_eq!(read(fd, &mut first).unwrap(), first.len());
        assert_eq!(read(fd, &mut second).unwrap(), second.len());
        assert_ne!(
            first, second,
            "independent CSPRNG reads unexpectedly matched"
        );
        close(fd).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn exec_handoff_returns_when_child_dies_before_owning_mapping() {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::CreateEventW;

        let raw_mapping = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let raw_ready = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!raw_mapping.is_null() && !raw_ready.is_null());
        let handoff = ExecHandoff {
            _mapping: unsafe { OwnedHandle::from_raw_handle(raw_mapping.cast()) },
            ready: unsafe { OwnedHandle::from_raw_handle(raw_ready.cast()) },
        };
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "exit", "0"])
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        assert!(!handoff.wait_until_owned(&child));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a dead child must not consume the 30 second ownership timeout"
        );
        child.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn exec_frame_carries_an_exact_stable_image_section() {
        let image = b"\x7fELF\0shared executable image\xff";
        let frame = serialize_exec_state_with_image(&[], Some(image))
            .expect("complete exec state must serialize");
        let range = fork_section_range(&frame, EXEC_IMAGE_SECTION)
            .expect("the exec image section must be present");

        assert_eq!(&frame[range], image);
        assert!(fork_section_range(&frame, u32::MAX).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn mapped_exec_frame_keeps_image_after_all_handoff_handles_close() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
        };
        let image = b"\x7fELF\0immutable exec snapshot\xff";
        let environment = vec!["SNAPSHOT_TEST=retained".to_owned()];
        let frame = serialize_exec_state_with_image(&environment, Some(image)).unwrap();
        let handoff = stage_exec_handoff(std::process::id(), &frame).unwrap();
        let state = peek_exec_launch_state().unwrap().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(handoff.ready.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(state.environment, environment);
        drop(handoff);
        // The loader's pin keeps the complete frame available for libc even
        // after the parent's staging handle has closed.
        assert_eq!(
            read_exec_handoff(false, false, |payload| {
                assert_eq!(payload, frame);
                Ok(())
            }),
            Ok(Some(()))
        );
        release_retained_exec_handoff();
        assert_eq!(read_exec_handoff(false, false, |_| Ok(())), Ok(None));
        assert_eq!(state.image.as_slice(), image);
    }

    #[cfg(windows)]
    #[test]
    fn exec_filter_temporarily_removes_guest_handles_from_inheritance() {
        use windows_sys::Win32::Foundation::{
            CloseHandle, GetHandleInformation, HANDLE_FLAG_INHERIT,
        };
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::System::Pipes::CreatePipe;

        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let mut read_end = std::ptr::null_mut();
        let mut write_end = std::ptr::null_mut();
        // SAFETY: both output pointers and the security attributes are valid.
        assert_ne!(
            unsafe { CreatePipe(&mut read_end, &mut write_end, &security, 0) },
            0
        );
        let fd = install(read_end as usize, FdKind::Pipe, FdFlags::PIPE_READ_END).unwrap();

        let inheritable = |handle| {
            let mut flags = 0u32;
            // SAFETY: the handle is live and `flags` is writable.
            assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
            flags & HANDLE_FLAG_INHERIT != 0
        };
        assert!(inheritable(read_end));
        with_exec_handle_filter(|| assert!(!inheritable(read_end))).unwrap();
        assert!(inheritable(read_end));

        close(fd).unwrap();
        // SAFETY: the write end was never installed in the descriptor table.
        unsafe { CloseHandle(write_end) };
    }

    #[cfg(windows)]
    #[test]
    fn winsock_descriptors_never_use_raw_handle_inheritance() {
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};

        let fd = socket::socket(socket::AF_INET, socket::SOCK_STREAM, 0).unwrap();
        let entry = get(fd).unwrap();
        let mut flags = 0u32;
        // SAFETY: `entry.raw` is the live socket owned by `fd` and `flags` is writable.
        assert_ne!(
            unsafe { GetHandleInformation(entry.raw as _, &mut flags) },
            0
        );
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        close(fd).unwrap();
    }

    #[test]
    fn all_host_object_kinds_share_the_same_fd_namespace() {
        assert!(matches!(get(-1), Err(EBADF)));
        let fd = install(0x1234, FdKind::Event, FdFlags::BORROWED).unwrap();
        let entry = get(fd).unwrap();
        assert_eq!(entry.raw, 0x1234);
        assert_eq!(entry.kind, FdKind::Event);
        close(fd).unwrap();
        assert!(matches!(get(fd), Err(EBADF)));
    }

    #[cfg(windows)]
    #[test]
    fn descriptor_publication_failure_rolls_back_without_closing_the_native_object() {
        use windows_sys::Win32::Foundation::{CloseHandle, GetHandleInformation};
        use windows_sys::Win32::System::Threading::CreateEventW;
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!handle.is_null());
        let mut description = 0;
        let result = install_with(handle as usize, FdKind::Event, FdFlags::NONE, |_, entry| {
            description = entry.description_id;
            assert_eq!(entry.raw, handle as usize);
            Err(ENOMEM)
        });
        assert_eq!(result, Err(ENOMEM));
        assert_ne!(description, 0);
        assert!(
            table()
                .read()
                .unwrap()
                .slots
                .iter()
                .flatten()
                .all(|entry| entry.description_id != description)
        );
        let mut flags = 0;
        assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
        unsafe { CloseHandle(handle) };
    }

    #[cfg(windows)]
    mod sockets {
        use crate::socket::{
            self, AF_INET, MSG_DONTWAIT, SHUT_WR, SO_REUSEADDR, SOCK_NONBLOCK, SOCK_STREAM,
            SOL_SOCKET,
        };
        use crate::{EAGAIN, close, read, write};

        /// Builds a Linux `sockaddr_in` for `127.0.0.1:port`.
        ///
        /// The payload is byte-identical to the Windows form, so only the family
        /// word is Linux-specific here.
        fn loopback(port: u16) -> [u8; 16] {
            let mut address = [0u8; 16];
            address[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
            // Ports and addresses are network byte order in both systems.
            address[2..4].copy_from_slice(&port.to_be_bytes());
            address[4..8].copy_from_slice(&u32::from(std::net::Ipv4Addr::LOCALHOST).to_be_bytes());
            address
        }

        /// Reads the port a socket actually bound to.
        fn bound_port(fd: i32) -> u16 {
            let mut address = [0u8; 128];
            let mut length = address.len() as i32;
            // SAFETY: both out-parameters are local and correctly sized.
            unsafe { socket::getsockname(fd, address.as_mut_ptr(), &mut length) }.unwrap();
            u16::from_be_bytes([address[2], address[3]])
        }

        #[test]
        fn a_tcp_loopback_round_trip_works_end_to_end() {
            let listener = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
            // SO_REUSEADDR is translated from Linux 2 to Windows 4 on the way.
            let enable: i32 = 1;
            // SAFETY: the option value is one readable i32.
            unsafe {
                socket::setsockopt(
                    listener,
                    SOL_SOCKET,
                    SO_REUSEADDR,
                    (&raw const enable).cast(),
                    size_of::<i32>() as i32,
                )
            }
            .unwrap();

            let address = loopback(0);
            // SAFETY: the address is a valid 16-byte sockaddr_in.
            unsafe { socket::bind(listener, address.as_ptr(), 16) }.unwrap();
            socket::listen(listener, 4).unwrap();
            let port = bound_port(listener);
            assert_ne!(port, 0, "the kernel should have assigned a port");

            // The client runs on another thread so accept and connect can meet.
            let client_thread = std::thread::spawn(move || {
                let client = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
                let target = loopback(port);
                // SAFETY: the address is a valid 16-byte sockaddr_in.
                unsafe { socket::connect(client, target.as_ptr(), 16) }.unwrap();
                assert_eq!(write(client, b"ping from client").unwrap(), 16);
                let mut reply = [0u8; 32];
                let read_bytes = read(client, &mut reply).unwrap();
                assert_eq!(&reply[..read_bytes], b"pong from server");
                close(client).unwrap();
            });

            let mut peer = [0u8; 128];
            let mut peer_length = peer.len() as i32;
            // SAFETY: both out-parameters are local and correctly sized.
            let accepted =
                unsafe { socket::accept(listener, peer.as_mut_ptr(), &mut peer_length, 0) }
                    .unwrap();
            // The reported family must be the Linux value, not the Windows 23.
            assert_eq!(
                u16::from_le_bytes([peer[0], peer[1]]),
                AF_INET as u16,
                "accept should report the Linux address family"
            );

            let mut request = [0u8; 32];
            let read_bytes = read(accepted, &mut request).unwrap();
            assert_eq!(&request[..read_bytes], b"ping from client");
            assert_eq!(write(accepted, b"pong from server").unwrap(), 16);

            // A half close makes the peer's next read report end of file.
            socket::shutdown(accepted, SHUT_WR).unwrap();
            client_thread.join().unwrap();
            close(accepted).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn a_nonblocking_socket_reports_eagain_instead_of_waiting() {
            let listener = socket::socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0).unwrap();
            let address = loopback(0);
            // SAFETY: the address is a valid 16-byte sockaddr_in.
            unsafe { socket::bind(listener, address.as_ptr(), 16) }.unwrap();
            socket::listen(listener, 1).unwrap();

            // Nothing is connecting, so a non-blocking accept must not block.
            let mut length = 0i32;
            // SAFETY: a null address with a live length pointer is permitted.
            let result = unsafe { socket::accept(listener, std::ptr::null_mut(), &mut length, 0) };
            assert!(
                matches!(result, Err(EAGAIN)),
                "expected EAGAIN from an idle non-blocking accept, got {result:?}"
            );
            close(listener).unwrap();
        }

        #[test]
        fn msg_dontwait_makes_a_blocking_socket_return_eagain() {
            // A connected pair where neither side has sent anything.
            let listener = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
            let address = loopback(0);
            // SAFETY: the address is a valid 16-byte sockaddr_in.
            unsafe { socket::bind(listener, address.as_ptr(), 16) }.unwrap();
            socket::listen(listener, 1).unwrap();
            let port = bound_port(listener);

            let connector = std::thread::spawn(move || {
                let client = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
                let target = loopback(port);
                // SAFETY: the address is a valid 16-byte sockaddr_in.
                unsafe { socket::connect(client, target.as_ptr(), 16) }.unwrap();
                // Hold the connection open without sending.
                std::thread::sleep(std::time::Duration::from_millis(300));
                close(client).unwrap();
            });

            let mut length = 0i32;
            // SAFETY: a null address with a live length pointer is permitted.
            let accepted =
                unsafe { socket::accept(listener, std::ptr::null_mut(), &mut length, 0) }.unwrap();

            // The socket itself is blocking, but MSG_DONTWAIT overrides that for
            // this one call.
            let mut buffer = [0u8; 8];
            // SAFETY: the buffer is writable for its length.
            let result =
                unsafe { socket::recv(accepted, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) };
            assert!(
                matches!(result, Err(EAGAIN)),
                "MSG_DONTWAIT should report EAGAIN, got {result:?}"
            );

            connector.join().unwrap();
            close(accepted).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn unsupported_families_and_options_are_refused() {
            // AF_UNIX is emulated on named pipes rather than refused, and the
            // families with no mapping at all still report EAFNOSUPPORT.
            assert!(matches!(
                socket::socket(17, SOCK_STREAM, 0),
                Err(crate::EAFNOSUPPORT)
            ));
            let fd = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
            let mut value: i32 = 0;
            let mut length = size_of::<i32>() as i32;
            // An option with no Windows equivalent reports ENOPROTOOPT rather
            // than silently succeeding.
            // SAFETY: the value/length pair is local and correctly sized.
            let result = unsafe {
                socket::getsockopt(fd, SOL_SOCKET, 0x7fff, (&raw mut value).cast(), &mut length)
            };
            assert!(matches!(result, Err(crate::ENOPROTOOPT)));
            close(fd).unwrap();
        }

        #[test]
        fn getsockopt_netfilter_revisions() {
            let fd = socket::socket(AF_INET, socket::SOCK_DGRAM, 0).unwrap();
            let mut rev = [0u8; 30];
            let mut length = 30i32;
            let res = unsafe { socket::getsockopt(fd, 0, 66, rev.as_mut_ptr(), &mut length) };
            assert_eq!(res, Ok(()));

            let res_v6 = unsafe { socket::getsockopt(fd, 41, 66, rev.as_mut_ptr(), &mut length) };
            assert_eq!(res_v6, Ok(()));
            close(fd).unwrap();
        }

        #[test]
        fn udp_datagram_loopback() {
            use crate::socket::SOCK_DGRAM;

            let s1 = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
            let s2 = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();

            let addr1 = loopback(0);
            unsafe { socket::bind(s1, addr1.as_ptr(), 16) }.unwrap();
            let port1 = bound_port(s1);

            let target = loopback(port1);
            let payload = b"udp datagram packet";
            let sent = unsafe {
                socket::sendto(s2, payload.as_ptr(), payload.len(), 0, target.as_ptr(), 16)
            }
            .unwrap();
            assert_eq!(sent, payload.len());

            let mut recv_buf = [0u8; 64];
            let mut from = [0u8; 128];
            let mut from_len = 128i32;
            let recvd = unsafe {
                socket::recvfrom(
                    s1,
                    recv_buf.as_mut_ptr(),
                    recv_buf.len(),
                    0,
                    from.as_mut_ptr(),
                    &mut from_len,
                )
            }
            .unwrap();
            assert_eq!(&recv_buf[..recvd], payload);

            close(s1).unwrap();
            close(s2).unwrap();
        }

        #[test]
        fn socket_options_translation() {
            use crate::socket::{SO_KEEPALIVE, SO_RCVBUF, SO_REUSEADDR, SO_SNDBUF};

            let s = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();

            let val: i32 = 1;
            unsafe {
                socket::setsockopt(
                    s,
                    SOL_SOCKET,
                    SO_REUSEADDR,
                    (&raw const val).cast(),
                    size_of::<i32>() as i32,
                )
            }
            .unwrap();

            let buf_size: i32 = 32768;
            unsafe {
                socket::setsockopt(
                    s,
                    SOL_SOCKET,
                    SO_RCVBUF,
                    (&raw const buf_size).cast(),
                    size_of::<i32>() as i32,
                )
            }
            .unwrap();
            unsafe {
                socket::setsockopt(
                    s,
                    SOL_SOCKET,
                    SO_SNDBUF,
                    (&raw const buf_size).cast(),
                    size_of::<i32>() as i32,
                )
            }
            .unwrap();

            let keepalive: i32 = 1;
            unsafe {
                socket::setsockopt(
                    s,
                    SOL_SOCKET,
                    SO_KEEPALIVE,
                    (&raw const keepalive).cast(),
                    size_of::<i32>() as i32,
                )
            }
            .unwrap();

            let mut read_keepalive: i32 = 0;
            let mut optlen = size_of::<i32>() as i32;
            unsafe {
                socket::getsockopt(
                    s,
                    SOL_SOCKET,
                    SO_KEEPALIVE,
                    (&raw mut read_keepalive).cast(),
                    &mut optlen,
                )
            }
            .unwrap();
            assert_ne!(read_keepalive, 0);

            close(s).unwrap();
        }
    }

    #[cfg(windows)]
    mod unix_sockets {
        use crate::socket::{
            self, AF_UNIX, MSG_PEEK, SHUT_WR, SO_TYPE, SOCK_DGRAM, SOCK_SEQPACKET, SOCK_STREAM,
            SOL_SOCKET,
        };
        use crate::unix::{self, Namespace, UnixAddress};
        use crate::{close, read, write};

        /// Builds a Linux `sockaddr_un` for a pathname socket.
        fn pathname(path: &str) -> (Vec<u8>, i32) {
            let mut address = Vec::new();
            address.extend_from_slice(&(AF_UNIX as u16).to_le_bytes());
            address.extend_from_slice(path.as_bytes());
            // The terminator is part of the reported length for a pathname.
            address.push(0);
            let length = address.len() as i32;
            address.resize(2 + 108, 0);
            (address, length)
        }

        /// Builds a `sockaddr_un` in the abstract namespace.
        fn abstract_name(name: &str) -> (Vec<u8>, i32) {
            let mut address = Vec::new();
            address.extend_from_slice(&(AF_UNIX as u16).to_le_bytes());
            // The leading NUL is what selects the abstract namespace.
            address.push(0);
            address.extend_from_slice(name.as_bytes());
            let length = address.len() as i32;
            address.resize(2 + 108, 0);
            (address, length)
        }

        /// A unique abstract name, so concurrent tests never collide.
        fn unique(tag: &str) -> String {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            format!(
                "kinakaze.test.{tag}.{}.{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        }

        #[test]
        fn pathname_and_abstract_addresses_round_trip() {
            let (address, length) = pathname("/tmp/example.sock");
            // SAFETY: the buffer is a local of at least `length` bytes.
            let parsed = unsafe { unix::parse_address(address.as_ptr(), length) }.unwrap();
            assert_eq!(parsed.namespace, Namespace::Pathname);
            assert_eq!(parsed.name, b"/tmp/example.sock");

            let mut out = [0u8; 2 + 108];
            let mut out_length = out.len() as i32;
            // SAFETY: the out-pair is local and correctly sized.
            unsafe { unix::write_address(&parsed, out.as_mut_ptr(), &mut out_length) }.unwrap();
            assert_eq!(u16::from_le_bytes([out[0], out[1]]), AF_UNIX as u16);
            assert_eq!(&out[2..19], b"/tmp/example.sock");
            // The reported length covers family, path and terminator.
            assert_eq!(out_length, 2 + 17 + 1);

            let (address, length) = abstract_name("hidden");
            // SAFETY: the buffer is a local of at least `length` bytes.
            let parsed = unsafe { unix::parse_address(address.as_ptr(), length) }.unwrap();
            assert_eq!(parsed.namespace, Namespace::Abstract);
            assert_eq!(parsed.name, b"hidden");

            let mut out = [0u8; 2 + 108];
            let mut out_length = out.len() as i32;
            // SAFETY: the out-pair is local and correctly sized.
            unsafe { unix::write_address(&parsed, out.as_mut_ptr(), &mut out_length) }.unwrap();
            // The abstract marker is a real NUL byte, not a terminator.
            assert_eq!(out[2], 0);
            assert_eq!(&out[3..9], b"hidden");
            assert_eq!(out_length, 2 + 1 + 6);
        }

        #[test]
        fn pipe_names_are_case_sensitive_and_namespace_separated() {
            let lower = UnixAddress {
                namespace: Namespace::Pathname,
                name: b"/tmp/a".to_vec(),
            };
            let upper = UnixAddress {
                namespace: Namespace::Pathname,
                name: b"/tmp/A".to_vec(),
            };
            // Pipe names are case-insensitive, so a naive mapping would collide
            // these two distinct Unix paths onto one pipe.
            assert_ne!(lower.pipe_name(), upper.pipe_name());

            let abstract_same = UnixAddress {
                namespace: Namespace::Abstract,
                name: b"/tmp/a".to_vec(),
            };
            // The same bytes in a different namespace must not share a pipe.
            assert_ne!(lower.pipe_name(), abstract_same.pipe_name());

            // A path too long to encode falls back to a hash, which must stay
            // deterministic and within the pipe name limit.
            let long = UnixAddress {
                namespace: Namespace::Pathname,
                name: vec![b'Z'; 100],
            };
            let first = long.pipe_name();
            assert_eq!(first, long.pipe_name());
            assert!(first.len() < 256);
        }

        #[test]
        fn stream_pair_carries_bytes_both_ways() {
            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();

            for fd in [first, second] {
                let stat = crate::fs::fstat(fd).unwrap();
                assert_eq!(stat.st_mode & crate::fs::S_IFMT, crate::fs::S_IFSOCK);
                assert_eq!(stat.st_nlink, 1);
            }

            assert_eq!(write(first, b"ping").unwrap(), 4);
            let mut buffer = [0u8; 16];
            assert_eq!(read(second, &mut buffer).unwrap(), 4);
            assert_eq!(&buffer[..4], b"ping");

            // The reverse direction shares the same pipe instance.
            assert_eq!(write(second, b"pong!").unwrap(), 5);
            assert_eq!(read(first, &mut buffer).unwrap(), 5);
            assert_eq!(&buffer[..5], b"pong!");

            // SO_TYPE reports what the pair was created with.
            let mut value: i32 = 0;
            let mut length = size_of::<i32>() as i32;
            // SAFETY: the value/length pair is local and correctly sized.
            unsafe {
                socket::getsockopt(
                    first,
                    SOL_SOCKET,
                    SO_TYPE,
                    (&raw mut value).cast(),
                    &mut length,
                )
            }
            .unwrap();
            assert_eq!(value, SOCK_STREAM);

            // The Windows pipe API yields a real process id, but Linux
            // SO_PEERCRED must expose the shared namespace id.
            let mut credentials = [0i32; 3];
            let mut length = size_of_val(&credentials) as i32;
            // SAFETY: `credentials` is a writable Linux `struct ucred` layout.
            unsafe {
                socket::getsockopt(
                    first,
                    SOL_SOCKET,
                    socket::SO_PEERCRED,
                    credentials.as_mut_ptr().cast(),
                    &mut length,
                )
            }
            .unwrap();
            assert_eq!(length as usize, size_of_val(&credentials));
            assert_eq!(credentials[0], crate::job::process_id() as i32);

            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn peek_leaves_bytes_and_shutdown_stops_writing() {
            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();
            assert_eq!(write(first, b"retained").unwrap(), 8);

            let mut buffer = [0u8; 16];
            // MSG_PEEK must not consume, so the following read sees the same
            // bytes again.
            // SAFETY: the buffer provides 16 writable bytes.
            let peeked =
                unsafe { socket::recv(second, buffer.as_mut_ptr(), buffer.len(), MSG_PEEK) }
                    .unwrap();
            assert_eq!(&buffer[..peeked], b"retained");
            let read_back = read(second, &mut buffer).unwrap();
            assert_eq!(&buffer[..read_back], b"retained");

            // A write-shutdown end reports EPIPE rather than transmitting.
            socket::shutdown(first, SHUT_WR).unwrap();
            assert!(matches!(write(first, b"late"), Err(crate::EPIPE)));

            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn datagram_pair_preserves_message_boundaries() {
            // A SOCK_DGRAM pair maps to a message-mode pipe, so two sends must
            // arrive as two reads rather than one merged stream.
            let (first, second) = unix::socketpair(SOCK_DGRAM).unwrap();
            assert_eq!(write(first, b"aaa").unwrap(), 3);
            assert_eq!(write(first, b"bb").unwrap(), 2);

            let mut buffer = [0u8; 16];
            assert_eq!(read(second, &mut buffer).unwrap(), 3);
            assert_eq!(&buffer[..3], b"aaa");
            assert_eq!(read(second, &mut buffer).unwrap(), 2);
            assert_eq!(&buffer[..2], b"bb");

            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn bind_listen_accept_connect_over_abstract_namespace() {
            let name = unique("listener");
            let (address, length) = abstract_name(&name);

            let listener = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(listener, address.as_ptr(), length) }.unwrap();
            socket::listen(listener, 4).unwrap();

            // getsockname reports the bound abstract name back.
            let mut out = [0u8; 2 + 108];
            let mut out_length = out.len() as i32;
            // SAFETY: the out-pair is local and correctly sized.
            unsafe { socket::getsockname(listener, out.as_mut_ptr(), &mut out_length) }.unwrap();
            assert_eq!(out[2], 0);
            assert_eq!(&out[3..3 + name.len()], name.as_bytes());

            // The client runs on another thread so the blocking accept below has
            // something to complete against.
            let client_address = address.clone();
            let client = std::thread::spawn(move || {
                let fd = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
                // SAFETY: the buffer is owned by this closure and long enough.
                unsafe { socket::connect(fd, client_address.as_ptr(), length) }.unwrap();
                assert_eq!(write(fd, b"from-client").unwrap(), 11);
                let mut buffer = [0u8; 16];
                let read_back = read(fd, &mut buffer).unwrap();
                assert_eq!(&buffer[..read_back], b"from-server");
                close(fd).unwrap();
            });

            // SAFETY: a null address pair is the documented "do not report" form.
            let accepted =
                unsafe { socket::accept(listener, std::ptr::null_mut(), std::ptr::null_mut(), 0) }
                    .unwrap();
            let mut buffer = [0u8; 32];
            let read_back = read(accepted, &mut buffer).unwrap();
            assert_eq!(&buffer[..read_back], b"from-client");
            assert_eq!(write(accepted, b"from-server").unwrap(), 11);

            client.join().unwrap();
            close(accepted).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn listener_accepts_a_peer_that_closed_after_queueing_data() {
            let (address, length) = abstract_name(&unique("queued-close"));

            let listener = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(listener, address.as_ptr(), length) }.unwrap();
            socket::listen(listener, 4).unwrap();

            // A Unix-domain connection stays in the accept queue after its peer
            // closes.  Named pipes expose that state as CLOSING, so exercise the
            // exact ordering which Go's netpoll listener can encounter.
            let client = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::connect(client, address.as_ptr(), length) }.unwrap();
            assert_eq!(write(client, b"queued").unwrap(), 6);
            close(client).unwrap();

            let readiness = unix::poll_readiness(listener).unwrap();
            assert!(readiness.contains(socket::Readiness::READABLE));
            assert!(!readiness.contains(socket::Readiness::ERROR));

            // SAFETY: a null address pair is the documented "do not report" form.
            let accepted =
                unsafe { socket::accept(listener, std::ptr::null_mut(), std::ptr::null_mut(), 0) }
                    .unwrap();
            let mut buffer = [0u8; 16];
            let read_back = read(accepted, &mut buffer).unwrap();
            assert_eq!(&buffer[..read_back], b"queued");

            close(accepted).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn concurrent_accept_does_not_fail_epoll_on_listener_rebind() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLIN, EpollEvent};
            use std::sync::{
                Arc, Barrier,
                atomic::{AtomicBool, Ordering},
            };

            let (address, length) = abstract_name(&unique("listener-rebind-race"));
            let listener = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            unsafe { socket::bind(listener, address.as_ptr(), length) }.unwrap();
            socket::listen(listener, 4).unwrap();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                listener,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 1,
                }),
            )
            .unwrap();

            let stop = Arc::new(AtomicBool::new(false));
            let started = Arc::new(Barrier::new(2));
            let poller = {
                let stop = stop.clone();
                let started = started.clone();
                std::thread::spawn(move || {
                    let mut events = [EpollEvent::default(); 1];
                    let mut polls = 0;
                    started.wait();
                    while !stop.load(Ordering::Acquire) {
                        // Epoll prepares an idle listener after its first
                        // readiness sample; an accept may run between them.
                        drop(unix::readiness::prepare(listener)?);
                        epoll::epoll_wait(epoll_fd, &mut events, 0)?;
                        polls += 1;
                    }
                    Ok::<_, i32>(polls)
                })
            };
            started.wait();
            for _ in 0..4096 {
                let client = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
                // Also poll the handleless-to-connected publication while the
                // other thread is building and retiring its wait snapshots.
                epoll::epoll_ctl(
                    epoll_fd,
                    EPOLL_CTL_ADD,
                    client,
                    Some(EpollEvent {
                        events: EPOLLIN,
                        data: 2,
                    }),
                )
                .unwrap();
                unsafe { socket::connect(client, address.as_ptr(), length) }.unwrap();
                let accepted = unsafe {
                    socket::accept(listener, std::ptr::null_mut(), std::ptr::null_mut(), 0)
                }
                .unwrap();
                close(accepted).unwrap();
                close(client).unwrap();
            }
            stop.store(true, Ordering::Release);
            let result = poller.join().unwrap();
            close(epoll_fd).unwrap();
            close(listener).unwrap();
            assert!(
                result.is_ok_and(|polls| polls > 0),
                "concurrent epoll: {result:?}"
            );
        }

        #[test]
        fn edge_triggered_listener_reports_the_replacement_instance() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLET, EPOLLIN, EpollEvent};

            let (address, length) = abstract_name(&unique("listener-rearm"));
            let listener = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(listener, address.as_ptr(), length) }.unwrap();
            socket::listen(listener, 4).unwrap();

            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                listener,
                Some(EpollEvent {
                    events: EPOLLIN | EPOLLET,
                    data: 0xfeed,
                }),
            )
            .unwrap();

            let first = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer remains live for both connections.
            unsafe { socket::connect(first, address.as_ptr(), length) }.unwrap();
            let mut events = [EpollEvent::default(); 2];
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000), Ok(1));
            // SAFETY: a null address pair asks accept not to report the peer.
            let accepted_first =
                unsafe { socket::accept(listener, std::ptr::null_mut(), std::ptr::null_mut(), 0) }
                    .unwrap();

            // Connect before epoll has observed an idle replacement instance.
            // The old EPOLLIN history must not suppress this new instance's edge.
            let second = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is still valid and correctly sized.
            unsafe { socket::connect(second, address.as_ptr(), length) }.unwrap();
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000), Ok(1));
            assert_eq!(events[0].events & EPOLLIN, EPOLLIN);
            // SAFETY: a null address pair asks accept not to report the peer.
            let accepted_second =
                unsafe { socket::accept(listener, std::ptr::null_mut(), std::ptr::null_mut(), 0) }
                    .unwrap();

            close(accepted_second).unwrap();
            close(second).unwrap();
            close(accepted_first).unwrap();
            close(first).unwrap();
            close(epoll_fd).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn edge_triggered_stream_rearms_when_a_read_drains_it() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLET, EPOLLIN, EpollEvent};

            let (writer, reader) = unix::socketpair(SOCK_STREAM).unwrap();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                reader,
                Some(EpollEvent {
                    events: EPOLLIN | EPOLLET,
                    data: 0xcafe,
                }),
            )
            .unwrap();

            assert_eq!(write(writer, b"first").unwrap(), 5);
            let mut events = [EpollEvent::default(); 2];
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000), Ok(1));
            let mut buffer = [0u8; 16];
            assert_eq!(read(reader, &mut buffer).unwrap(), 5);

            // No epoll sweep occurs between the drain and this new write. The
            // read itself must retire the old edge so this transition is seen.
            assert_eq!(write(writer, b"second").unwrap(), 6);
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000), Ok(1));
            assert_eq!(events[0].events & EPOLLIN, EPOLLIN);
            assert_eq!(read(reader, &mut buffer).unwrap(), 6);

            close(epoll_fd).unwrap();
            close(reader).unwrap();
            close(writer).unwrap();
        }

        #[test]
        fn edge_triggered_stream_rearms_after_send_backpressure() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLET, EPOLLOUT, EpollEvent};
            use crate::socket::MSG_DONTWAIT;
            let (writer, reader) = unix::socketpair(SOCK_STREAM).unwrap();
            use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            let source = crate::get(writer).unwrap();
            let mut raw_copy = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        source.raw as _,
                        GetCurrentProcess(),
                        &mut raw_copy,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            let alias =
                crate::install_duplicate(raw_copy as usize, source.kind, source.flags, source)
                    .unwrap();
            unix::duplicate(writer, alias, raw_copy as usize).unwrap();
            let epfd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epfd,
                EPOLL_CTL_ADD,
                alias,
                Some(EpollEvent {
                    events: EPOLLOUT | EPOLLET,
                    data: 0x5752,
                }),
            )
            .unwrap();
            let mut events = [EpollEvent::default(); 2];
            assert_eq!(epoll::epoll_wait(epfd, &mut events, 0), Ok(1));
            assert_eq!(epoll::epoll_wait(epfd, &mut events, 0), Ok(0));
            let block = [0xabu8; 8192];
            let mut sent = 0;
            loop {
                let result =
                    unsafe { socket::send(writer, block.as_ptr(), block.len(), MSG_DONTWAIT) };
                match result {
                    Ok(n) => {
                        assert!(n > 0);
                        sent += n;
                        assert!(sent < 4 * 1024 * 1024);
                    }
                    Err(crate::EAGAIN) => break,
                    Err(e) => panic!("send: {e}"),
                }
            }
            assert!(sent > 0);
            let mut received = 0;
            let mut buffer = [0u8; 8192];
            while received < sent {
                let n = unsafe {
                    socket::recv(reader, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT)
                }
                .unwrap();
                assert!(n > 0);
                assert!(buffer[..n].iter().all(|b| *b == 0xab));
                received += n;
            }
            // There was no epoll scan while the pipe was full. The send must
            // retire the prior edge for every fd sharing this open description.
            assert_eq!(epoll::epoll_wait(epfd, &mut events, 100), Ok(1));
            assert_ne!(events[0].events & EPOLLOUT, 0);
            assert_eq!(epoll::epoll_wait(epfd, &mut events, 0), Ok(0));
            for fd in [epfd, alias, writer, reader] {
                close(fd).unwrap();
            }
        }

        #[test]
        fn nonblocking_stream_large_send_uses_available_space() {
            use crate::socket::MSG_DONTWAIT;
            let (writer, reader) = unix::socketpair(SOCK_STREAM).unwrap();
            let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
            let result =
                unsafe { socket::send(writer, payload.as_ptr(), payload.len(), MSG_DONTWAIT) };
            assert!(
                matches!(result, Ok(n) if n > 0),
                "empty stream has write space: {result:?}"
            );
            let sent = result.unwrap();
            assert!(sent <= payload.len());
            let mut buffer = vec![0; payload.len()];
            let received =
                unsafe { socket::recv(reader, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) }
                    .unwrap();
            assert_eq!(received, sent);
            assert_eq!(&buffer[..received], &payload[..sent]);
            assert_eq!(
                unsafe { socket::recv(reader, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) },
                Err(crate::EAGAIN)
            );
            close(writer).unwrap();
            close(reader).unwrap();
        }

        #[test]
        fn binding_a_taken_address_reports_eaddrinuse() {
            let name = unique("contested");
            let (address, length) = abstract_name(&name);

            let first = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(first, address.as_ptr(), length) }.unwrap();

            let second = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // The pipe namespace, not a file check, is what makes this exclusive.
            // SAFETY: the address buffer is a local of at least `length` bytes.
            let result = unsafe { socket::bind(second, address.as_ptr(), length) };
            assert!(matches!(result, Err(crate::EADDRINUSE)));

            close(second).unwrap();
            close(first).unwrap();

            // Once released, the name is bindable again.
            let third = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(third, address.as_ptr(), length) }.unwrap();
            close(third).unwrap();
        }

        #[test]
        fn connecting_to_nothing_is_refused() {
            let (address, length) = abstract_name(&unique("absent"));
            let fd = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            let result = unsafe { socket::connect(fd, address.as_ptr(), length) };
            assert!(matches!(result, Err(crate::ECONNREFUSED)));
            close(fd).unwrap();
        }

        #[test]
        fn a_bound_path_appears_on_the_filesystem() {
            let directory = std::env::temp_dir().join(format!(
                "kinakaze-unix-{}-{}",
                std::process::id(),
                "bindpath"
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let host_path = directory.join("service.sock");
            // The guest side of the VFS speaks Linux paths, so the host path is
            // translated into one.
            let linux_path = host_path.to_string_lossy().replace('\\', "/");
            let linux_path = format!("/{}", linux_path.replacen(':', "", 1));

            let (address, length) = pathname(&linux_path);
            let fd = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the address buffer is a local of at least `length` bytes.
            unsafe { socket::bind(fd, address.as_ptr(), length) }.unwrap();

            // A pathname socket gets a persistent socket inode so every process
            // can discover its type without consulting this process's fd table.
            assert!(host_path.exists(), "bind should create {host_path:?}");
            assert_eq!(
                crate::fs::stat(&linux_path).unwrap().st_mode & crate::fs::S_IFMT,
                crate::fs::S_IFSOCK
            );

            // Linux close leaves the pathname in place; only unlink removes it.
            close(fd).unwrap();
            assert!(host_path.exists(), "close must retain {host_path:?}");
            crate::fs::unlink(&linux_path).unwrap();
            assert!(!host_path.exists(), "unlink should remove {host_path:?}");
            let _ = std::fs::remove_dir_all(&directory);
        }

        #[test]
        fn seqpackets_and_unconnected_sends_are_handled() {
            // SOCK_SEQPACKET is accepted and, like SOCK_DGRAM, keeps boundaries.
            let (first, second) = unix::socketpair(SOCK_SEQPACKET).unwrap();
            assert_eq!(write(first, b"one").unwrap(), 3);
            assert_eq!(write(first, b"two").unwrap(), 3);
            let mut buffer = [0u8; 16];
            assert_eq!(read(second, &mut buffer).unwrap(), 3);
            assert_eq!(&buffer[..3], b"one");
            close(first).unwrap();
            close(second).unwrap();

            // Sending on a socket that was never connected has no destination.
            let orphan = socket::socket(AF_UNIX, SOCK_STREAM, 0).unwrap();
            // SAFETY: the buffer is a readable local.
            let result = unsafe { socket::send(orphan, b"x".as_ptr(), 1, 0) };
            assert!(matches!(result, Err(crate::ENOTCONN)));
            // And there is no peer to name.
            let mut out = [0u8; 2 + 108];
            let mut out_length = out.len() as i32;
            // SAFETY: the out-pair is local and correctly sized.
            let named = unsafe { socket::getpeername(orphan, out.as_mut_ptr(), &mut out_length) };
            assert!(matches!(named, Err(crate::ENOTCONN)));
            close(orphan).unwrap();
        }

        #[test]
        fn a_nonblocking_send_reports_eagain_instead_of_blocking() {
            use crate::socket::{MSG_DONTWAIT, SOCK_STREAM as STREAM};

            let (first, second) = unix::socketpair(STREAM).unwrap();
            // Fill the pipe without ever draining it. The buffer is 64 KiB, so
            // this must eventually refuse rather than block forever.
            let payload = [0x5au8; 8192];
            let mut total = 0usize;
            let mut refused = false;
            for _ in 0..64 {
                // SAFETY: the payload is a readable local of its own length.
                match unsafe { socket::send(first, payload.as_ptr(), payload.len(), MSG_DONTWAIT) }
                {
                    Ok(written) => total += written,
                    Err(crate::EAGAIN) => {
                        refused = true;
                        break;
                    }
                    Err(error) => panic!("unexpected error {error}"),
                }
            }
            assert!(
                refused,
                "a full pipe must report EAGAIN; wrote {total} bytes without refusal"
            );
            assert!(
                total > 0,
                "at least the first write should have been accepted"
            );

            // Draining makes room, so the next send succeeds again.
            let mut sink = vec![0u8; 32 * 1024];
            let drained = read(second, &mut sink).unwrap();
            assert!(drained > 0);
            // SAFETY: the payload is a readable local of its own length.
            let after =
                unsafe { socket::send(first, payload.as_ptr(), payload.len(), MSG_DONTWAIT) };
            assert!(
                matches!(after, Ok(written) if written > 0),
                "a drained pipe should accept a write again, got {after:?}"
            );

            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn a_full_socket_stops_reporting_writable() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLIN, EPOLLOUT, EpollEvent};
            use crate::socket::MSG_DONTWAIT;

            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                first,
                Some(EpollEvent {
                    events: EPOLLOUT,
                    data: 0xbeef,
                }),
            )
            .unwrap();

            // An empty socket has room, so it must be writable.
            let mut events = [EpollEvent::default(); 4];
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 100).unwrap();
            assert_eq!(ready, 1, "an empty socket should be writable");
            assert_eq!(events[0].events & EPOLLOUT, EPOLLOUT);

            // Fill it without draining. Reporting writable here would make an
            // epoll caller spin on a peer that has stopped reading.
            let payload = [0u8; 8192];
            for _ in 0..64 {
                // SAFETY: the payload is a readable local of its own length.
                match unsafe { socket::send(first, payload.as_ptr(), payload.len(), MSG_DONTWAIT) }
                {
                    Ok(_) => {}
                    Err(crate::EAGAIN) => break,
                    Err(error) => panic!("unexpected error {error}"),
                }
            }

            let ready = epoll::epoll_wait(epoll_fd, &mut events, 0).unwrap();
            assert_eq!(ready, 0, "a full socket must not be reported writable");

            // Draining restores writability.
            let mut sink = vec![0u8; 32 * 1024];
            assert!(read(second, &mut sink).unwrap() > 0);
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1, "a drained socket should be writable again");
            assert_eq!(events[0].events & EPOLLOUT, EPOLLOUT);
            // The unused import is kept honest: EPOLLIN is not expected here.
            assert_eq!(events[0].events & EPOLLIN, 0);

            close(epoll_fd).unwrap();
            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn an_oversized_datagram_is_truncated_and_discarded() {
            let (first, second) = unix::socketpair(SOCK_DGRAM).unwrap();
            assert_eq!(write(first, b"0123456789").unwrap(), 10);
            // A following message proves the remainder of the first was dropped
            // rather than left queued.
            assert_eq!(write(first, b"next").unwrap(), 4);

            // Linux truncates a datagram to the buffer and discards the rest.
            let mut small = [0u8; 4];
            assert_eq!(read(second, &mut small).unwrap(), 4);
            assert_eq!(&small, b"0123");

            let mut buffer = [0u8; 16];
            let read_back = read(second, &mut buffer).unwrap();
            assert_eq!(
                &buffer[..read_back],
                b"next",
                "the tail of a truncated datagram must not resurface"
            );

            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn epoll_watches_a_unix_socket_pair() {
            use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLIN, EpollEvent};

            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                second,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 0xc0ffee,
                }),
            )
            .unwrap();

            // A named pipe has no AFD equivalent, so this exercises the polling
            // path rather than the socket one: with nothing written, the wait must
            // still report nothing ready.
            let mut events = [EpollEvent::default(); 4];
            assert_eq!(
                epoll::epoll_wait(epoll_fd, &mut events, 0).unwrap(),
                0,
                "an idle Unix socket should not be reported readable"
            );

            assert_eq!(write(first, b"wake-up").unwrap(), 7);
            // The timeout has to exceed the poll interval for the readiness to be
            // noticed, which is the cost of polling a pipe.
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1, "the Unix socket should be readable after a write");
            // A packed struct's fields cannot be referenced, so copy first.
            let (reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(reported & EPOLLIN, EPOLLIN);
            assert_eq!(cookie, 0xc0ffee);

            close(epoll_fd).unwrap();
            close(first).unwrap();
            close(second).unwrap();
        }

        #[test]
        fn a_closed_peer_reads_as_end_of_file() {
            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();
            assert_eq!(write(first, b"tail").unwrap(), 4);
            close(first).unwrap();

            let mut buffer = [0u8; 16];
            // The buffered bytes are still delivered before the end of file.
            assert_eq!(read(second, &mut buffer).unwrap(), 4);
            assert_eq!(&buffer[..4], b"tail");
            // Only then does the vanished peer read as zero rather than an error.
            assert_eq!(read(second, &mut buffer).unwrap(), 0);
            close(second).unwrap();
        }

        #[test]
        fn unix_socket_so_peercred_and_options() {
            use crate::socket::{SO_PEERCRED, SO_TYPE, SOCK_STREAM, SOL_SOCKET};

            let (first, second) = unix::socketpair(SOCK_STREAM).unwrap();

            // Query SO_TYPE
            let mut opt_type = 0i32;
            let mut opt_len = 4i32;
            assert_eq!(
                unsafe {
                    socket::getsockopt(
                        first,
                        SOL_SOCKET,
                        SO_TYPE,
                        &raw mut opt_type as *mut u8,
                        &mut opt_len,
                    )
                },
                Ok(())
            );
            assert_eq!(opt_type, SOCK_STREAM);

            // Query SO_PEERCRED (struct ucred: pid, uid, gid)
            let mut cred = [0u8; 12];
            let mut cred_len = cred.len() as i32;
            assert_eq!(
                unsafe {
                    socket::getsockopt(
                        first,
                        SOL_SOCKET,
                        SO_PEERCRED,
                        cred.as_mut_ptr(),
                        &mut cred_len,
                    )
                },
                Ok(())
            );
            assert_eq!(cred_len, 12);

            let peer_pid = u32::from_le_bytes(cred[0..4].try_into().unwrap());
            assert!(peer_pid > 0 || std::process::id() > 0);

            close(first).unwrap();
            close(second).unwrap();
        }
    }

    #[cfg(windows)]
    mod epoll_readiness {
        use std::ffi::c_void;

        use crate::epoll::{
            self, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD, EPOLLIN, EPOLLONESHOT, EPOLLOUT,
            EpollEvent,
        };
        use crate::socket::{self, AF_INET, SOCK_STREAM};
        use crate::{close, write};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        const DUPLICATE_SAME_ACCESS: u32 = 2;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn DuplicateHandle(
                source_process: *mut c_void,
                source: *mut c_void,
                target_process: *mut c_void,
                target: *mut *mut c_void,
                desired_access: u32,
                inherit: i32,
                options: u32,
            ) -> i32;
        }

        fn loopback(port: u16) -> [u8; 16] {
            let mut address = [0u8; 16];
            address[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
            address[2..4].copy_from_slice(&port.to_be_bytes());
            address[4..8].copy_from_slice(&u32::from(std::net::Ipv4Addr::LOCALHOST).to_be_bytes());
            address
        }

        fn bound_port(fd: i32) -> u16 {
            let mut address = [0u8; 128];
            let mut length = address.len() as i32;
            // SAFETY: both out-parameters are local and correctly sized.
            unsafe { socket::getsockname(fd, address.as_mut_ptr(), &mut length) }.unwrap();
            u16::from_be_bytes([address[2], address[3]])
        }

        /// Builds a connected TCP pair through the loopback interface.
        fn connected_pair() -> (i32, i32, i32) {
            let listener = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
            let address = loopback(0);
            // SAFETY: the address is a valid 16-byte sockaddr_in.
            unsafe { socket::bind(listener, address.as_ptr(), 16) }.unwrap();
            socket::listen(listener, 1).unwrap();
            let port = bound_port(listener);

            let client_thread = std::thread::spawn(move || {
                let client = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();
                let target = loopback(port);
                // SAFETY: the address is a valid 16-byte sockaddr_in.
                unsafe { socket::connect(client, target.as_ptr(), 16) }.unwrap();
                client
            });

            let mut length = 0i32;
            // SAFETY: a null address with a live length pointer is permitted.
            let accepted =
                unsafe { socket::accept(listener, std::ptr::null_mut(), &mut length, 0) }.unwrap();
            let client = client_thread.join().unwrap();
            (listener, accepted, client)
        }

        #[test]
        fn only_the_ready_socket_of_several_is_reported() {
            // Three sockets registered, one written to. This distinguishes two
            // readings of the AFD result: if the driver compacts its output to
            // only the ready handles, then pairing results with registrations by
            // array position reports the wrong descriptor's cookie.
            let mut sets = Vec::new();
            for _ in 0..3 {
                sets.push(connected_pair());
            }
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            for (index, (_, server, _)) in sets.iter().enumerate() {
                epoll::epoll_ctl(
                    epoll_fd,
                    EPOLL_CTL_ADD,
                    *server,
                    Some(EpollEvent {
                        events: EPOLLIN,
                        // The cookie identifies which registration was reported.
                        data: 0xa000 + index as u64,
                    }),
                )
                .unwrap();
            }

            // Only the last pair's client sends, so only the last server socket
            // may be reported, and it must carry its own cookie.
            let (_, _, last_client) = sets[2];
            assert_eq!(write(last_client, b"only-me").unwrap(), 7);

            let mut events = [EpollEvent::default(); 8];
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1, "exactly one socket was written to");
            let (reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(reported & EPOLLIN, EPOLLIN);
            assert_eq!(
                cookie, 0xa002,
                "the reported cookie must belong to the socket that actually \
                 received data, not to whichever registration shared its slot"
            );

            close(epoll_fd).unwrap();
            for (listener, server, client) in sets {
                close(server).unwrap();
                close(client).unwrap();
                close(listener).unwrap();
            }
        }

        #[test]
        fn epoll_reports_a_socket_readable_only_after_data_arrives() {
            let (listener, server, client) = connected_pair();
            let epoll_fd = epoll::epoll_create1(0).unwrap();

            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 0xfeed,
                }),
            )
            .unwrap();

            // Nothing sent yet, so a zero timeout must report nothing ready.
            let mut events = [EpollEvent::default(); 4];
            assert_eq!(
                epoll::epoll_wait(epoll_fd, &mut events, 0).unwrap(),
                0,
                "an idle socket should not be reported readable"
            );

            // Now send, and the same wait must find it.
            assert_eq!(write(client, b"payload").unwrap(), 7);
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1, "the socket should be readable after a send");
            // A packed struct's fields cannot be referenced, so copy first.
            let (reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(reported & EPOLLIN, EPOLLIN);
            // The cookie must come back verbatim.
            assert_eq!(cookie, 0xfeed);

            close(epoll_fd).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn closing_a_watched_socket_does_not_fail_a_blocked_epoll_wait() {
            let (listener, server, client) = connected_pair();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 0xc10e,
                }),
            )
            .unwrap();

            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let mut events = [EpollEvent::default(); 4];
                started_tx.send(()).unwrap();
                epoll::epoll_wait(epoll_fd, &mut events, 200)
            });
            started_rx.recv().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));

            // Go's netpoll closes a cancelled TCP dial from a different thread.
            // Linux removes that open description from the interest list; it
            // never turns the whole epoll_wait into EIO.
            close(server).unwrap();
            assert_eq!(waiter.join().unwrap(), Ok(0));

            close(epoll_fd).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn epoll_registration_survives_closing_one_duplicated_descriptor() {
            let (listener, server, client) = connected_pair();
            let source = crate::get(server).unwrap();
            let mut raw_copy: *mut c_void = std::ptr::null_mut();
            // SAFETY: the source socket is live, the target process is this
            // process, and `raw_copy` is a writable out-parameter.
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        source.raw as *mut c_void,
                        GetCurrentProcess(),
                        &raw mut raw_copy,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            let duplicate =
                crate::install_duplicate(raw_copy as usize, source.kind, source.flags, source)
                    .unwrap();
            assert_eq!(
                crate::get(duplicate).unwrap().description_id,
                source.description_id
            );

            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 0xd0b1,
                }),
            )
            .unwrap();

            // Closing the descriptor used for ADD must migrate the poll target
            // to the surviving alias instead of deleting the registration.
            close(server).unwrap();
            write(client, b"alias").unwrap();
            let mut events = [EpollEvent::default(); 2];
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000), Ok(1));
            let (reported, cookie) = (events[0].events, events[0].data);
            assert_ne!(reported & EPOLLIN, 0);
            assert_eq!(cookie, 0xd0b1);

            close(duplicate).unwrap();
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 0), Ok(0));
            close(epoll_fd).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn epoll_reports_writability_on_an_idle_connection() {
            let (listener, server, client) = connected_pair();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLOUT,
                    data: 7,
                }),
            )
            .unwrap();

            // An empty send buffer is immediately writable.
            let mut events = [EpollEvent::default(); 4];
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1);
            let (reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(reported & EPOLLOUT, EPOLLOUT);
            assert_eq!(cookie, 7);

            close(epoll_fd).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn oneshot_fires_once_until_rearmed() {
            let (listener, server, client) = connected_pair();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN | EPOLLONESHOT,
                    data: 1,
                }),
            )
            .unwrap();
            write(client, b"once").unwrap();

            let mut events = [EpollEvent::default(); 4];
            assert_eq!(epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap(), 1);
            // The data is still unread, so a level-triggered registration would
            // fire again; one-shot must stay silent.
            assert_eq!(
                epoll::epoll_wait(epoll_fd, &mut events, 0).unwrap(),
                0,
                "a one-shot registration should not fire twice"
            );

            // Rearming through MOD makes it deliverable again.
            epoll::epoll_ctl(
                epoll_fd,
                EPOLL_CTL_MOD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN | EPOLLONESHOT,
                    data: 2,
                }),
            )
            .unwrap();
            let ready = epoll::epoll_wait(epoll_fd, &mut events, 1000).unwrap();
            assert_eq!(ready, 1);
            let cookie = events[0].data;
            assert_eq!(cookie, 2, "MOD should have replaced the cookie");

            close(epoll_fd).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn epoll_ctl_enforces_its_error_contract() {
            let (listener, server, client) = connected_pair();
            let epoll_fd = epoll::epoll_create1(0).unwrap();
            let event = EpollEvent {
                events: EPOLLIN,
                data: 0,
            };

            // A set cannot watch itself.
            assert!(matches!(
                epoll::epoll_ctl(epoll_fd, EPOLL_CTL_ADD, epoll_fd, Some(event)),
                Err(crate::EINVAL)
            ));
            // MOD before ADD is ENOENT.
            assert!(matches!(
                epoll::epoll_ctl(epoll_fd, EPOLL_CTL_MOD, server, Some(event)),
                Err(crate::ENOENT)
            ));
            epoll::epoll_ctl(epoll_fd, EPOLL_CTL_ADD, server, Some(event)).unwrap();
            // A second ADD is EEXIST.
            assert!(matches!(
                epoll::epoll_ctl(epoll_fd, EPOLL_CTL_ADD, server, Some(event)),
                Err(crate::EEXIST)
            ));
            epoll::epoll_ctl(epoll_fd, EPOLL_CTL_DEL, server, None).unwrap();
            // DEL twice is ENOENT.
            assert!(matches!(
                epoll::epoll_ctl(epoll_fd, EPOLL_CTL_DEL, server, None),
                Err(crate::ENOENT)
            ));

            close(epoll_fd).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn the_epoll_event_struct_matches_the_linux_layout() {
            // On x86_64 this struct is packed: 4 bytes of events then an 8-byte
            // union with no padding. Guest code indexes arrays of these, so a
            // wrong size would corrupt every entry past the first.
            assert_eq!(size_of::<EpollEvent>(), 12);
        }

        #[test]
        fn epoll_create_and_flags() {
            use crate::EINVAL;
            use crate::epoll::{EPOLL_CLOEXEC, epoll_create, epoll_create1};

            assert!(matches!(epoll_create(0), Err(EINVAL)));
            assert!(matches!(epoll_create(-5), Err(EINVAL)));
            let ep1 = epoll_create(10).unwrap();
            assert!(ep1 >= 0);
            close(ep1).unwrap();

            let ep2 = epoll_create1(EPOLL_CLOEXEC).unwrap();
            assert!(ep2 >= 0);
            close(ep2).unwrap();
        }

        #[test]
        fn epoll_edge_triggered_semantics() {
            use crate::epoll::{EPOLLET, EPOLLIN};
            use crate::read;

            let (listener, server, client) = connected_pair();
            let ep = epoll::epoll_create1(0).unwrap();
            let ev = EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 0x101,
            };
            epoll::epoll_ctl(ep, epoll::EPOLL_CTL_ADD, server, Some(ev)).unwrap();

            let mut events = [EpollEvent::default(); 4];
            assert_eq!(epoll::epoll_wait(ep, &mut events, 0).unwrap(), 0);

            write(client, b"0123456789").unwrap();

            let n = epoll::epoll_wait(ep, &mut events, 500).unwrap();
            assert_eq!(n, 1);
            let (_reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(cookie, 0x101);

            let mut buf = [0u8; 4];
            assert_eq!(read(server, &mut buf).unwrap(), 4);

            let n2 = epoll::epoll_wait(ep, &mut events, 50).unwrap();
            assert_eq!(n2, 0, "ET should not refire without new edge");

            // Drain the remaining bytes
            let mut remaining = [0u8; 16];
            assert_eq!(read(server, &mut remaining).unwrap(), 6);
            let mut drain = [0u8; 1];
            let res = unsafe { socket::recv(server, drain.as_mut_ptr(), 1, socket::MSG_DONTWAIT) };
            assert_eq!(res, Err(crate::EAGAIN));

            write(client, b"more").unwrap();
            let n3 = epoll::epoll_wait(ep, &mut events, 500).unwrap();
            assert_eq!(n3, 1, "ET should fire on new data arrival");

            close(ep).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn edge_triggered_always_ready_descriptor_fires_only_once() {
            use crate::epoll::{EPOLLET, EPOLLIN, EPOLLOUT};

            let null = crate::fs::open("/dev/null", crate::fs::O_RDWR, 0).unwrap();
            let ep = epoll::epoll_create1(0).unwrap();
            epoll::epoll_ctl(
                ep,
                EPOLL_CTL_ADD,
                null,
                Some(EpollEvent {
                    events: EPOLLIN | EPOLLOUT | EPOLLET,
                    data: 0x4e554c4c,
                }),
            )
            .unwrap();

            let mut events = [EpollEvent::default(); 1];
            assert_eq!(epoll::epoll_wait(ep, &mut events, 0).unwrap(), 1);
            let cookie = events[0].data;
            assert_eq!(cookie, 0x4e554c4c);
            assert_eq!(
                epoll::epoll_wait(ep, &mut events, 0).unwrap(),
                0,
                "EPOLLET must suppress unchanged readiness for special devices"
            );

            close(ep).unwrap();
            close(null).unwrap();
        }

        #[test]
        fn epoll_rdhup_and_peer_disconnect() {
            use crate::epoll::{EPOLLIN, EPOLLRDHUP};
            use crate::socket::SHUT_WR;

            let (listener, server, client) = connected_pair();
            let ep = epoll::epoll_create1(0).unwrap();
            let ev = EpollEvent {
                events: EPOLLIN | EPOLLRDHUP,
                data: 0x202,
            };
            epoll::epoll_ctl(ep, epoll::EPOLL_CTL_ADD, server, Some(ev)).unwrap();

            socket::shutdown(client, SHUT_WR).unwrap();

            let mut events = [EpollEvent::default(); 4];
            let n = epoll::epoll_wait(ep, &mut events, 1000).unwrap();
            assert_eq!(n, 1);
            let (reported, _cookie) = (events[0].events, events[0].data);
            assert_ne!(reported & (EPOLLRDHUP | EPOLLIN), 0);

            close(client).unwrap();
            close(server).unwrap();
            close(ep).unwrap();
            close(listener).unwrap();
        }

        #[test]
        fn epoll_mixed_descriptors() {
            use crate::epoll::EPOLLIN;
            use crate::socket::SOCK_STREAM;
            use crate::unix;

            let (listener, server, client) = connected_pair();
            let (pr, pw) = unix::socketpair(SOCK_STREAM).unwrap();
            let ep = epoll::epoll_create1(0).unwrap();

            epoll::epoll_ctl(
                ep,
                epoll::EPOLL_CTL_ADD,
                server,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 1,
                }),
            )
            .unwrap();
            epoll::epoll_ctl(
                ep,
                epoll::EPOLL_CTL_ADD,
                pr,
                Some(EpollEvent {
                    events: EPOLLIN,
                    data: 2,
                }),
            )
            .unwrap();

            let mut events = [EpollEvent::default(); 4];
            assert_eq!(epoll::epoll_wait(ep, &mut events, 0).unwrap(), 0);

            write(pw, b"pipe data").unwrap();
            let n = epoll::epoll_wait(ep, &mut events, 500).unwrap();
            assert_eq!(n, 1);
            let (_reported, cookie) = (events[0].events, events[0].data);
            assert_eq!(cookie, 2);

            write(client, b"sock data").unwrap();
            let n = epoll::epoll_wait(ep, &mut events, 500).unwrap();
            assert!(n >= 1);

            close(pr).unwrap();
            close(pw).unwrap();
            close(ep).unwrap();
            close(server).unwrap();
            close(client).unwrap();
            close(listener).unwrap();
        }
    }

    #[cfg(windows)]
    mod io_uring {
        use crate::fs::{O_CREAT, O_RDWR, O_TRUNC};
        use crate::iouring::{
            self, CompletionEntry, IORING_OP_NOP, IORING_OP_READ, IORING_OP_WRITE,
            IORING_SETUP_SQPOLL, SubmissionEntry,
        };
        use crate::{close, fs};

        fn temp_path(label: &str) -> String {
            format!(
                "/kinakaze-uring-{}-{}-{label}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_nanos())
                    .unwrap_or(0)
            )
        }

        #[test]
        fn the_ring_structs_match_the_linux_layout() {
            // Guest code fills and reads arrays of these, so a wrong size would
            // misalign every entry past the first.
            assert_eq!(size_of::<SubmissionEntry>(), 64);
            assert_eq!(size_of::<CompletionEntry>(), 16);
        }

        #[test]
        fn setup_rejects_the_flags_it_cannot_honour() {
            if !iouring::available() {
                // Before Windows 11 there is no IoRing; the ENOSYS path is the
                // documented behaviour and is asserted below instead.
                assert!(matches!(iouring::setup(8, 0), Err(crate::ENOSYS)));
                return;
            }
            // A caller asking for a kernel polling thread would wrongly assume
            // submissions happen without an enter call.
            assert!(matches!(
                iouring::setup(8, IORING_SETUP_SQPOLL),
                Err(crate::EINVAL)
            ));
            assert!(matches!(iouring::setup(0, 0), Err(crate::EINVAL)));
        }

        #[test]
        fn a_nop_completes_with_its_cookie() {
            if !iouring::available() {
                return;
            }
            let ring = iouring::setup(8, 0).unwrap();
            let entry = SubmissionEntry {
                opcode: IORING_OP_NOP,
                user_data: 0xabcd,
                ..SubmissionEntry::default()
            };
            // SAFETY: a NOP reads no memory, so the address fields are unused.
            unsafe { iouring::push(ring, &entry) }.unwrap();
            iouring::enter(ring, 1, Some(500)).unwrap();

            let mut completions = [CompletionEntry::default(); 4];
            let reaped = iouring::reap(ring, &mut completions).unwrap();
            assert_eq!(reaped, 1);
            assert_eq!(completions[0].user_data, 0xabcd);
            assert_eq!(completions[0].result, 0);
            close(ring).unwrap();
        }

        #[test]
        fn a_ring_write_then_read_moves_real_bytes() {
            if !iouring::available() {
                return;
            }
            let path = temp_path("transfer");
            let fd = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            let ring = iouring::setup(8, 0).unwrap();

            let payload = *b"through the ring";
            let write_entry = SubmissionEntry {
                opcode: IORING_OP_WRITE,
                fd,
                offset: 0,
                address: payload.as_ptr() as u64,
                length: payload.len() as u32,
                user_data: 1,
                ..SubmissionEntry::default()
            };
            // SAFETY: `payload` outlives the reap below.
            unsafe { iouring::push(ring, &write_entry) }.unwrap();
            iouring::enter(ring, 1, Some(1000)).unwrap();

            let mut completions = [CompletionEntry::default(); 4];
            let reaped = iouring::reap(ring, &mut completions).unwrap();
            assert_eq!(reaped, 1, "the write should have produced one completion");
            assert_eq!(completions[0].user_data, 1);
            assert_eq!(
                completions[0].result,
                payload.len() as i32,
                "the completion should report the byte count"
            );

            // Read it back through the same ring.
            let mut buffer = [0u8; 32];
            let read_entry = SubmissionEntry {
                opcode: IORING_OP_READ,
                fd,
                offset: 0,
                address: buffer.as_mut_ptr() as u64,
                length: payload.len() as u32,
                user_data: 2,
                ..SubmissionEntry::default()
            };
            // SAFETY: `buffer` outlives the reap below.
            unsafe { iouring::push(ring, &read_entry) }.unwrap();
            iouring::enter(ring, 1, Some(1000)).unwrap();
            let reaped = iouring::reap(ring, &mut completions).unwrap();
            assert_eq!(reaped, 1);
            assert_eq!(completions[0].user_data, 2);
            assert_eq!(completions[0].result, payload.len() as i32);
            assert_eq!(&buffer[..payload.len()], &payload);

            close(ring).unwrap();
            close(fd).unwrap();
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn reaping_an_empty_queue_reports_eagain() {
            if !iouring::available() {
                return;
            }
            let ring = iouring::setup(8, 0).unwrap();
            let mut completions = [CompletionEntry::default(); 2];
            assert!(matches!(
                iouring::reap(ring, &mut completions),
                Err(crate::EAGAIN)
            ));
            close(ring).unwrap();
        }
    }

    #[cfg(windows)]
    mod procfs_through_the_file_api {
        use crate::fs::{self, O_RDONLY, O_WRONLY};
        use crate::{close, read, write};

        /// Reads a whole `/proc` file through open/read/close.
        fn slurp(path: &str) -> String {
            let fd = fs::open(path, O_RDONLY, 0)
                .unwrap_or_else(|error| panic!("open({path}) failed with errno {error}"));
            let mut text = Vec::new();
            let mut chunk = [0u8; 512];
            loop {
                let count = read(fd, &mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                text.extend_from_slice(&chunk[..count]);
            }
            close(fd).unwrap();
            String::from_utf8(text).unwrap()
        }

        #[test]
        fn cpuinfo_is_readable_through_the_normal_file_path() {
            let text = slurp("/proc/cpuinfo");
            // One stanza per logical core, generated from the real machine.
            let processors = text
                .lines()
                .filter(|line| line.starts_with("processor"))
                .count();
            assert_eq!(
                processors,
                std::thread::available_parallelism()
                    .map(|count| count.get())
                    .unwrap_or(1),
                "cpuinfo should report one stanza per logical core"
            );
            assert!(text.contains("model name"), "cpuinfo lacks a model name");
        }

        #[test]
        fn self_maps_reports_this_processs_own_regions() {
            let text = slurp("/proc/self/maps");
            assert!(!text.is_empty(), "maps should not be empty");
            // Every line must carry a well-formed address range and perms.
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let range = fields.next().expect("a maps line needs a range");
                let (start, end) = range.split_once('-').expect("range needs a dash");
                assert!(
                    u64::from_str_radix(start, 16).is_ok(),
                    "bad start in {line:?}"
                );
                assert!(u64::from_str_radix(end, 16).is_ok(), "bad end in {line:?}");
                let perms = fields.next().expect("a maps line needs perms");
                assert_eq!(perms.len(), 4, "perms should be 4 chars in {line:?}");
            }
            // An executable mapping must exist: this test's own code is in one.
            assert!(
                text.lines().any(|line| line
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|perms| perms.contains('x'))),
                "maps should contain at least one executable region"
            );
        }

        #[test]
        fn stat_reports_procfs_kinds_and_refuses_writes() {
            // A directory.
            let directory = fs::stat("/proc").unwrap();
            assert_eq!(directory.st_mode & fs::S_IFMT, fs::S_IFDIR);
            // Procfs regular files are generated at read time. Like Linux,
            // stat reports zero instead of rendering the file just to compute
            // a synthetic length.
            let file = fs::stat("/proc/cpuinfo").unwrap();
            assert_eq!(file.st_mode & fs::S_IFMT, fs::S_IFREG);
            assert_eq!(file.st_size, 0);
            // lstat on /proc/self/exe sees the symlink itself.
            let link = fs::lstat("/proc/self/exe").unwrap();
            assert_eq!(link.st_mode & fs::S_IFMT, fs::S_IFLNK);
            // stat follows it to the real executable.
            let target = fs::stat("/proc/self/exe").unwrap();
            assert_eq!(target.st_mode & fs::S_IFMT, fs::S_IFREG);

            // procfs is read-only.
            assert!(matches!(
                fs::open("/proc/cpuinfo", O_WRONLY, 0),
                Err(crate::EACCES)
            ));
            // O_DIRECTORY is an assertion about the target, not a prerequisite:
            // Linux permits a directory to be opened read-only without it.
            let directory_fd = fs::open("/proc", O_RDONLY, 0).unwrap();
            assert_eq!(
                crate::get(directory_fd).unwrap().kind,
                crate::FdKind::SyntheticDirectory
            );
            close(directory_fd).unwrap();
            assert!(matches!(fs::open("/proc", O_WRONLY, 0), Err(crate::EISDIR)));
            // A nonexistent procfs path is ENOENT, not a host lookup.
            assert!(matches!(fs::stat("/proc/nope"), Err(crate::ENOENT)));
        }

        #[test]
        fn writable_numeric_sysctl_round_trips_through_the_file_api() {
            let _sysctl = crate::procfs::sysctl_test_lock();
            let path = "/proc/sys/net/ipv4/ip_forward";

            // Exercise the kernel-state publisher independently first, so a
            // failure cannot be hidden behind open-file-description state.
            assert_eq!(crate::procfs::write_file(path, b"0\n", 0), Ok(2));

            let fd = fs::open(path, O_WRONLY, 0).expect("open writable sysctl");
            assert_eq!(write(fd, b"1\n"), Ok(2));
            // Numeric sysctls use Linux's strict write mode by default: a
            // second write on the same open description is not a continuation.
            assert_eq!(write(fd, b"0\n"), Err(crate::EINVAL));
            close(fd).unwrap();

            assert_eq!(slurp(path), "1\n");
            let read_only = fs::open(path, O_RDONLY, 0).expect("open read-only sysctl");
            assert_eq!(write(read_only, b"0\n"), Err(crate::EBADF));
            close(read_only).unwrap();

            // Keep repeated and full-suite runs independent.
            assert_eq!(crate::procfs::write_file(path, b"0\n", 0), Ok(2));
        }

        #[test]
        fn cmdline_round_trips_the_real_argv() {
            let text = slurp("/proc/self/cmdline");
            let recovered: Vec<&str> = text.split('\0').filter(|part| !part.is_empty()).collect();
            let actual: Vec<String> = std::env::args().collect();
            assert_eq!(
                recovered.len(),
                actual.len(),
                "cmdline should hold every argument"
            );
            assert_eq!(recovered[0], actual[0]);
        }

        #[test]
        fn the_proc_directory_lists_its_contents() {
            let entries = fs::read_directory("/proc").unwrap();
            let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
            for expected in ["self", "cpuinfo", "meminfo", "uptime", "version"] {
                assert!(names.contains(&expected), "/proc should list {expected}");
            }
            // The pid directory is an alias for self.
            let pid = crate::job::process_id().to_string();
            assert!(
                names.contains(&pid.as_str()),
                "/proc should list its own pid"
            );
        }
    }

    #[cfg(windows)]
    mod file_operations {
        use crate::fs::{self, O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
        use crate::{EEXIST, ENOENT, ESPIPE, close, read, write};

        /// The `/dev` nodes a terminal-driving program opens by name.
        ///
        /// These are the paths the previous version stat'ed as directories or
        /// refused outright, so a guest that followed `ptsname` with an `open`
        /// got `ENOENT` from a name this layer had just handed it.
        #[test]
        fn the_terminal_device_nodes_open_stat_and_list() {
            use crate::fs::{O_DIRECTORY, O_NOCTTY, S_IFCHR, S_IFDIR, S_IFMT};

            // /dev/ptmx allocates a terminal, and stats as the character device
            // Linux numbers 5:2.
            let attributes = fs::stat("/dev/ptmx").expect("/dev/ptmx should exist");
            assert_eq!(attributes.st_mode & S_IFMT, S_IFCHR);
            assert_eq!(attributes.st_rdev, (5 << 8) | 2);

            let master = fs::open("/dev/ptmx", O_RDWR | O_NOCTTY, 0).expect("opening /dev/ptmx");
            let number = crate::tty::pty_number(master).unwrap();
            crate::tty::set_slave_lock(master, false).unwrap();

            // The slave's own path stats as a devpts node before it is opened,
            // which is what `ls -l` on the name reports.
            let slave_path = format!("/dev/pts/{number}");
            let attributes = fs::stat(&slave_path).expect("the slave path should exist");
            assert_eq!(attributes.st_mode & S_IFMT, S_IFCHR);
            assert_eq!(attributes.st_rdev, (136 << 8) | u64::from(number));

            // /dev/pts is a directory that can be opened and listed, and the
            // listing agrees with what ptsname handed out.
            assert_eq!(fs::stat("/dev/pts").unwrap().st_mode & S_IFMT, S_IFDIR);
            let listing = fs::read_directory("/dev/pts").expect("listing /dev/pts");
            let names: Vec<&str> = listing.iter().map(|entry| entry.name.as_str()).collect();
            assert!(
                names.contains(&"ptmx"),
                "devpts should carry its multiplexer"
            );
            assert!(
                names.contains(&number.to_string().as_str()),
                "{names:?} should list terminal {number}"
            );
            let directory = fs::open("/dev/pts", O_RDONLY | O_DIRECTORY, 0)
                .expect("/dev/pts should be openable as a directory");
            close(directory).unwrap();

            let slave = fs::open(&slave_path, O_RDWR | O_NOCTTY, 0).expect("opening the slave");
            assert!(crate::tty::isatty(slave));

            // A number nobody allocated is ENOENT rather than a fabricated node.
            assert!(matches!(fs::stat("/dev/pts/250"), Err(crate::ENOENT)));

            close(slave).unwrap();
            close(master).unwrap();
        }

        /// Builds a unique guest-visible path under the virtual root.
        ///
        /// The root is the executable directory, which for tests is the target
        /// directory, so these files are created and removed in-tree.
        fn temp_path(label: &str) -> String {
            format!(
                "/kinakaze-fs-{}-{}-{label}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_nanos())
                    .unwrap_or(0)
            )
        }

        #[test]
        fn open_creates_reads_and_reports_metadata() {
            let path = temp_path("basic");
            let fd = fs::open(&path, O_WRONLY | O_CREAT | O_TRUNC, 0o644).unwrap();
            assert_eq!(write(fd, b"kinakaze").unwrap(), 8);
            close(fd).unwrap();

            let info = fs::stat(&path).unwrap();
            assert_eq!(info.st_size, 8);
            assert_eq!(info.st_mode & fs::S_IFMT, fs::S_IFREG);
            // A freshly written file must carry a plausible modern timestamp,
            // which is what catches a botched FILETIME epoch conversion.
            assert!(
                info.st_mtime > 1_600_000_000 && info.st_mtime < 4_000_000_000,
                "st_mtime {} is outside the plausible range",
                info.st_mtime
            );

            let fd = fs::open(&path, O_RDONLY, 0).unwrap();
            let mut buffer = [0u8; 16];
            assert_eq!(read(fd, &mut buffer).unwrap(), 8);
            assert_eq!(&buffer[..8], b"kinakaze");
            // fstat on the open descriptor must agree with the path lookup.
            assert_eq!(fs::fstat(fd).unwrap().st_size, 8);
            close(fd).unwrap();

            fs::unlink(&path).unwrap();
            assert!(matches!(fs::stat(&path), Err(ENOENT)));
        }

        #[test]
        fn exclusive_create_refuses_an_existing_file() {
            let path = temp_path("excl");
            let fd = fs::open(&path, O_WRONLY | O_CREAT, 0o644).unwrap();
            close(fd).unwrap();
            assert!(matches!(
                fs::open(&path, O_WRONLY | O_CREAT | O_EXCL, 0o644),
                Err(EEXIST)
            ));
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn lseek_positions_reads_and_rejects_streams() {
            let path = temp_path("seek");
            let fd = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"0123456789").unwrap();

            assert_eq!(fs::lseek(fd, 4, fs::SEEK_SET).unwrap(), 4);
            let mut buffer = [0u8; 3];
            assert_eq!(read(fd, &mut buffer).unwrap(), 3);
            assert_eq!(&buffer, b"456");
            // The read advanced the position, so SEEK_CUR is relative to 7.
            assert_eq!(fs::lseek(fd, -2, fs::SEEK_CUR).unwrap(), 5);
            assert_eq!(fs::lseek(fd, 0, fs::SEEK_END).unwrap(), 10);
            // Seeking before the start is an error, not a clamp.
            assert!(matches!(
                fs::lseek(fd, -1, fs::SEEK_SET),
                Err(crate::EINVAL)
            ));
            close(fd).unwrap();
            fs::unlink(&path).unwrap();

            // stdout is a stream: it has no file position at all.
            assert!(matches!(fs::lseek(1, 0, fs::SEEK_CUR), Err(ESPIPE)));
        }

        #[test]
        fn append_writes_always_land_at_end_of_file() {
            let path = temp_path("append");
            let fd = fs::open(&path, O_WRONLY | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"first").unwrap();
            close(fd).unwrap();

            // A fresh O_APPEND descriptor starts at offset zero; each write
            // independently appends, even after an explicit seek.
            let fd = fs::open(&path, O_WRONLY | O_APPEND, 0o644).unwrap();
            assert_eq!(fs::lseek(fd, 0, fs::SEEK_CUR), Ok(0));
            fs::lseek(fd, 0, fs::SEEK_SET).unwrap();
            write(fd, b"-second").unwrap();
            close(fd).unwrap();

            let fd = fs::open(&path, O_RDONLY, 0).unwrap();
            let mut buffer = [0u8; 32];
            let read_bytes = read(fd, &mut buffer).unwrap();
            close(fd).unwrap();
            assert_eq!(&buffer[..read_bytes], b"first-second");
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn unlink_detaches_the_name_while_a_descriptor_stays_open() {
            let path = temp_path("posix-unlink");
            let fd = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"still here").unwrap();

            // POSIX semantics: the name goes away immediately, the data does not.
            fs::unlink(&path).unwrap();
            assert!(matches!(fs::stat(&path), Err(ENOENT)));

            fs::lseek(fd, 0, fs::SEEK_SET).unwrap();
            let mut buffer = [0u8; 16];
            let read_bytes = read(fd, &mut buffer).unwrap();
            assert_eq!(&buffer[..read_bytes], b"still here");

            // The name is free again, so a new file can take it.
            let replacement = fs::open(&path, O_WRONLY | O_CREAT | O_EXCL, 0o644).unwrap();
            close(replacement).unwrap();
            fs::unlink(&path).unwrap();
            close(fd).unwrap();
        }

        #[test]
        fn directories_are_created_walked_and_removed() {
            let directory = temp_path("dir");
            fs::mkdir(&directory, 0o755).unwrap();
            let info = fs::stat(&directory).unwrap();
            assert_eq!(info.st_mode & fs::S_IFMT, fs::S_IFDIR);

            // A relative open must resolve against the current directory.
            fs::chdir(&directory).unwrap();
            assert_eq!(fs::getcwd(), directory);
            let fd = fs::open("inside.txt", O_WRONLY | O_CREAT, 0o644).unwrap();
            close(fd).unwrap();
            assert!(fs::stat(&format!("{directory}/inside.txt")).is_ok());

            // A non-empty directory cannot be removed.
            fs::chdir("/").unwrap();
            assert!(matches!(fs::rmdir(&directory), Err(crate::ENOTEMPTY)));
            fs::unlink(&format!("{directory}/inside.txt")).unwrap();
            fs::rmdir(&directory).unwrap();
            assert!(matches!(fs::stat(&directory), Err(ENOENT)));
        }

        #[test]
        fn opening_a_directory_as_a_file_reports_eisdir() {
            let directory = temp_path("as-file");
            fs::mkdir(&directory, 0o755).unwrap();
            assert!(matches!(
                fs::open(&directory, O_WRONLY, 0),
                Err(crate::EISDIR)
            ));
            // O_DIRECTORY on a plain file is the mirrored failure.
            let file = temp_path("not-a-dir");
            let fd = fs::open(&file, O_WRONLY | O_CREAT, 0o644).unwrap();
            close(fd).unwrap();
            assert!(matches!(
                fs::open(&file, O_RDONLY | fs::O_DIRECTORY, 0),
                Err(crate::ENOTDIR)
            ));
            fs::unlink(&file).unwrap();
            fs::rmdir(&directory).unwrap();
        }

        #[test]
        fn rename_replaces_the_target_atomically() {
            let source = temp_path("rename-src");
            let target = temp_path("rename-dst");
            let fd = fs::open(&source, O_WRONLY | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"moved").unwrap();
            close(fd).unwrap();
            let fd = fs::open(&target, O_WRONLY | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"replaced").unwrap();
            close(fd).unwrap();

            fs::rename(&source, &target).unwrap();
            assert!(matches!(fs::stat(&source), Err(ENOENT)));
            assert_eq!(fs::stat(&target).unwrap().st_size, 5);
            fs::unlink(&target).unwrap();
        }

        #[test]
        fn ftruncate_shrinks_and_extends() {
            let path = temp_path("truncate");
            let fd = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"0123456789").unwrap();

            fs::ftruncate(fd, 4).unwrap();
            assert_eq!(fs::fstat(fd).unwrap().st_size, 4);
            // Extending fills the gap with zeroes.
            fs::ftruncate(fd, 6).unwrap();
            fs::lseek(fd, 0, fs::SEEK_SET).unwrap();
            let mut buffer = [0xffu8; 6];
            let mut filled = 0;
            while filled < buffer.len() {
                let count = read(fd, &mut buffer[filled..]).unwrap();
                assert_ne!(count, 0);
                filled += count;
            }
            assert_eq!(&buffer, b"0123\0\0");
            close(fd).unwrap();
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn ftruncate_preserves_position_and_checks_original_access() {
            let path = temp_path("truncate-access");
            let writer = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(writer, b"abcdef").unwrap();
            let readonly = fs::open(&path, O_RDONLY, 0).unwrap();
            let path_only = fs::open(&path, fs::O_PATH | O_APPEND, 0).unwrap();
            assert_eq!(fs::ftruncate(readonly, 2), Err(crate::EINVAL));
            assert_eq!(fs::ftruncate(path_only, 2), Err(crate::EBADF));
            assert_eq!(fs::ftruncate(-1, 2), Err(crate::EBADF));
            assert_eq!(fs::ftruncate(-1, -1), Err(crate::EINVAL));
            assert_eq!(fs::fstat(writer).unwrap().st_size, 6);
            assert_eq!(fs::lseek(writer, 3, fs::SEEK_SET), Ok(3));
            fs::ftruncate(writer, 2).unwrap();
            assert_eq!(fs::lseek(writer, 0, fs::SEEK_CUR), Ok(3));
            // Native position must also remain unchanged for borrowed native
            // synchronous users sharing this file object.
            let raw = crate::get(writer).unwrap().raw as windows_sys::Win32::Foundation::HANDLE;
            let mut position = -1i64;
            assert_ne!(
                unsafe {
                    windows_sys::Win32::Storage::FileSystem::SetFilePointerEx(
                        raw,
                        0,
                        &mut position,
                        windows_sys::Win32::Storage::FileSystem::FILE_CURRENT,
                    )
                },
                0
            );
            assert_eq!(position, 0);
            close(path_only).unwrap();
            close(readonly).unwrap();
            close(writer).unwrap();
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn append_mode_can_be_cleared_and_truncated_without_reopening() {
            let path = temp_path("append-flags");
            let fd = fs::open(&path, O_RDWR | O_CREAT | O_TRUNC, 0o644).unwrap();
            write(fd, b"abcdef").unwrap();
            close(fd).unwrap();
            let fd = fs::open(&path, O_WRONLY | O_APPEND, 0).unwrap();
            assert_eq!(fs::lseek(fd, 0, fs::SEEK_CUR), Ok(0));
            fs::ftruncate(fd, 3).unwrap();
            assert_eq!(fs::lseek(fd, 0, fs::SEEK_CUR), Ok(0));
            crate::set_status_flags(fd, false, false).unwrap();
            write(fd, b"!").unwrap();
            close(fd).unwrap();
            let fd = fs::open(&path, O_RDONLY, 0).unwrap();
            let mut bytes = [0u8; 3];
            assert_eq!(read(fd, &mut bytes), Ok(3));
            assert_eq!(&bytes, b"!bc");
            close(fd).unwrap();
            // Linux accepts O_RDONLY|O_TRUNC but the resulting fd stays read-only.
            let fd = fs::open(&path, O_RDONLY | O_TRUNC, 0).unwrap();
            assert_eq!(fs::fstat(fd).unwrap().st_size, 0);
            assert_eq!(write(fd, b"x"), Err(crate::EBADF));
            close(fd).unwrap();
            fs::unlink(&path).unwrap();
        }

        #[test]
        fn missing_paths_and_bad_flags_report_posix_errno() {
            assert!(matches!(
                fs::open("/definitely/missing", O_RDONLY, 0),
                Err(ENOENT)
            ));
            assert!(matches!(fs::stat("/definitely/missing"), Err(ENOENT)));
            assert!(matches!(fs::unlink("/definitely/missing"), Err(ENOENT)));
            // All three access-mode bits set is not a valid mode.
            let path = temp_path("flags");
            assert!(matches!(
                fs::open(&path, 0o3 | O_CREAT, 0o644),
                Err(crate::EINVAL)
            ));
        }
    }

    #[cfg(windows)]
    mod windows_overlapped {
        use super::*;
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE,
            FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING, PIPE_ACCESS_DUPLEX, WriteFile,
        };
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

        fn wide(value: &str) -> Vec<u16> {
            OsStr::new(value).encode_wide().chain(Some(0)).collect()
        }

        /// Opens a self-deleting temporary file with overlapped I/O enabled.
        fn overlapped_temp_file() -> (i32, HANDLE) {
            let mut path = std::env::temp_dir();
            path.push(format!("kinakaze-vfs-{}.tmp", std::process::id()));
            let wide_path = wide(&path.to_string_lossy());
            // SAFETY: the path is a valid null-terminated wide string.
            let handle = unsafe {
                CreateFileW(
                    wide_path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    CREATE_ALWAYS,
                    FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_OVERLAPPED | FILE_FLAG_DELETE_ON_CLOSE,
                    ptr::null_mut(),
                )
            };
            assert!(
                !handle.is_null() && handle as isize != -1,
                "CreateFileW failed"
            );
            let fd = install(
                handle as usize,
                FdKind::File,
                FdFlags::OVERLAPPED.union(FdFlags::SEEKABLE),
            )
            .unwrap();
            (fd, handle)
        }

        #[test]
        fn overlapped_file_round_trip_tracks_its_own_offset() {
            let (fd, _handle) = overlapped_temp_file();

            // Two writes must land back to back, proving the table advances the
            // offset rather than relying on a kernel file pointer.
            assert_eq!(write(fd, b"hello ").unwrap(), 6);
            assert_eq!(write(fd, b"world").unwrap(), 5);
            assert_eq!(get(fd).unwrap().offset, 11);

            // Rewind and read the whole file back through the same descriptor.
            seek_to(fd, 0);
            let mut buffer = [0u8; 11];
            let mut filled = 0;
            while filled < buffer.len() {
                let read = read(fd, &mut buffer[filled..]).unwrap();
                assert_ne!(read, 0, "unexpected EOF at {filled}");
                filled += read;
            }
            assert_eq!(&buffer, b"hello world");

            // Reading at end of file reports EOF instead of an error.
            assert_eq!(read(fd, &mut [0u8; 4]).unwrap(), 0);
            close(fd).unwrap();
        }

        #[test]
        fn overlapped_pipe_reader_observes_eof_after_the_last_writer_closes() {
            let (reader, writer) = create_pipe(FdFlags::NONE, 4096).unwrap();
            let reader_entry = get(reader).unwrap();
            assert!(reader_entry.flags.contains(FdFlags::OVERLAPPED));
            assert!(reader_entry.flags.contains(FdFlags::PIPE_READ_END));

            close(writer).unwrap();
            let result = read(reader, &mut [0u8; 8]);
            assert_eq!(
                result,
                Ok(0),
                "a disconnected Linux pipe reader must report EOF, got {result:?}"
            );
            close(reader).unwrap();
        }

        fn seek_to(fd: i32, offset: u64) {
            let mut table = table().write().unwrap();
            let entry = table.slots.get_mut(fd as usize).unwrap().as_mut().unwrap();
            entry.offset = offset;
        }

        /// Creates an overlapped named-pipe server with a connected client.
        ///
        /// `CreatePipe` only produces synchronous handles, so a named pipe is
        /// the practical way to get an overlapped stream that genuinely blocks.
        fn overlapped_pipe_pair() -> (HANDLE, HANDLE) {
            let name = format!(
                r"\\.\pipe\kinakaze-vfs-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            );
            let wide_name = wide(&name);
            // SAFETY: the name is a valid null-terminated wide string.
            let server = unsafe {
                CreateNamedPipeW(
                    wide_name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_BYTE | PIPE_WAIT,
                    1,
                    4096,
                    4096,
                    0,
                    ptr::null(),
                )
            };
            assert!(
                !server.is_null() && server as isize != -1,
                "CreateNamedPipeW failed"
            );
            // SAFETY: the server exists, so the client open resolves the name.
            let client = unsafe {
                CreateFileW(
                    wide_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                )
            };
            assert!(
                !client.is_null() && client as isize != -1,
                "client CreateFileW failed"
            );
            (server, client)
        }

        /// Runs `body` with `signal` handled by `handler` and `flags`, then
        /// restores the default disposition.
        fn with_handler(
            signal: i32,
            handler: crate::signal::Handler,
            flags: i32,
            body: impl FnOnce(),
        ) {
            crate::signal::sigaction(
                signal,
                Some(crate::signal::Action {
                    disposition: crate::signal::Disposition::Handle(handler, 0),
                    flags,
                    mask: 0,
                    restorer: 0,
                }),
            )
            .unwrap();
            body();
            crate::signal::sigaction(signal, Some(crate::signal::Action::default())).unwrap();
        }

        static HANDLER_RUNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

        unsafe extern "sysv64" fn count_run(_signal: i32) {
            HANDLER_RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        use crate::signal::test_lock as signal_test_lock;

        #[test]
        fn a_signal_interrupts_a_blocking_read_and_reports_eintr() {
            let _serialized = signal_test_lock();
            let (server, client) = overlapped_pipe_pair();
            let fd = install(
                server as usize,
                FdKind::Pipe,
                FdFlags::OVERLAPPED
                    .union(FdFlags::PIPE_READ_END)
                    .union(FdFlags::PIPE_WRITE_END),
            )
            .unwrap();
            assert!(!interrupt::current().is_null());
            HANDLER_RUNS.store(0, std::sync::atomic::Ordering::SeqCst);

            // No SA_RESTART: the handler runs and the read reports EINTR.
            with_handler(crate::signal::SIGUSR1, count_run, 0, || {
                let raiser = std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    crate::signal::raise_signal(crate::signal::SIGUSR1).unwrap();
                });
                let result = read(fd, &mut [0u8; 8]);
                raiser.join().unwrap();
                assert!(
                    matches!(result, Err(EINTR)),
                    "expected EINTR from a signalled read, got {result:?}"
                );
            });
            assert_eq!(
                HANDLER_RUNS.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "the guest handler should have run exactly once"
            );

            close(fd).unwrap();
            // SAFETY: the client handle is owned by this test.
            unsafe { CloseHandle(client) };
        }

        #[test]
        fn sa_restart_resumes_the_read_instead_of_reporting_eintr() {
            let _serialized = signal_test_lock();
            let (server, client) = overlapped_pipe_pair();
            let fd = install(
                server as usize,
                FdKind::Pipe,
                FdFlags::OVERLAPPED
                    .union(FdFlags::PIPE_READ_END)
                    .union(FdFlags::PIPE_WRITE_END),
            )
            .unwrap();
            assert!(!interrupt::current().is_null());
            HANDLER_RUNS.store(0, std::sync::atomic::Ordering::SeqCst);

            with_handler(
                crate::signal::SIGUSR2,
                count_run,
                crate::signal::SA_RESTART,
                || {
                    let writer_client = client as usize;
                    let helper = std::thread::spawn(move || {
                        // Interrupt first: with SA_RESTART the read must resume
                        // rather than fail.
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        crate::signal::raise_signal(crate::signal::SIGUSR2).unwrap();
                        // Then supply data so the resumed read can complete.
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        let mut written = 0u32;
                        // SAFETY: the handle is live for the test's duration.
                        unsafe {
                            WriteFile(
                                writer_client as HANDLE,
                                b"go".as_ptr(),
                                2,
                                &mut written,
                                ptr::null_mut(),
                            )
                        };
                    });

                    let mut buffer = [0u8; 8];
                    let result = read(fd, &mut buffer);
                    helper.join().unwrap();
                    // The restart is the whole point: the call returns data, not EINTR.
                    assert_eq!(
                        result,
                        Ok(2),
                        "SA_RESTART should have resumed the read, got {result:?}"
                    );
                    assert_eq!(&buffer[..2], b"go");
                },
            );
            assert_eq!(
                HANDLER_RUNS.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "the handler should have run once before the restart"
            );

            close(fd).unwrap();
            // SAFETY: the client handle is owned by this test.
            unsafe { CloseHandle(client) };
        }

        #[test]
        fn interrupting_a_blocked_overlapped_read_reports_eintr() {
            let _serialized = signal_test_lock();
            let (server, client) = overlapped_pipe_pair();
            // A stream descriptor: no offset tracking, reads block when empty.
            let fd = install(
                server as usize,
                FdKind::Pipe,
                FdFlags::OVERLAPPED
                    .union(FdFlags::PIPE_READ_END)
                    .union(FdFlags::PIPE_WRITE_END),
            )
            .unwrap();

            // Register this thread's interrupt event before the waker races us.
            assert!(!interrupt::current().is_null());
            let target = interrupt::current_thread_id();
            let waker = std::thread::spawn(move || {
                // Long enough for the reader to reach the dual wait.
                std::thread::sleep(std::time::Duration::from_millis(200));
                assert!(
                    interrupt::interrupt_thread(target),
                    "target thread was not registered"
                );
            });

            // Nothing was ever written, so this read reaches the interruptible
            // wait and only returns because the interrupt cancels it.
            let result = read(fd, &mut [0u8; 8]);
            waker.join().unwrap();
            assert!(
                matches!(result, Err(EINTR)),
                "expected EINTR, got {result:?}"
            );

            // The consumed interrupt must not leak into the next call: after a
            // write the same descriptor reads normally.
            let mut written = 0u32;
            // SAFETY: the client handle is live and the buffer is readable.
            let ok = unsafe { WriteFile(client, b"ok".as_ptr(), 2, &mut written, ptr::null_mut()) };
            assert_ne!(ok, 0, "priming write failed");
            let mut buffer = [0u8; 8];
            assert_eq!(read(fd, &mut buffer).unwrap(), 2);
            assert_eq!(&buffer[..2], b"ok");

            close(fd).unwrap();
            // SAFETY: the client handle is owned by this test.
            unsafe { CloseHandle(client) };
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod platform {
    use super::*;

    pub(super) fn install_standard_streams(table: &mut FdTable) {
        for fd in 0..=2 {
            // Linux keeps the file position in the kernel, so these descriptors
            // are never marked SEEKABLE for the table to track.
            table.insert_at(fd, fd as usize, FdKind::File, FdFlags::BORROWED);
        }
    }

    pub(super) fn read(entry: FdEntry, buffer: &mut [u8]) -> Result<usize, i32> {
        syscall3(0, entry.raw, buffer.as_mut_ptr() as usize, buffer.len())
    }

    pub(super) fn write(entry: FdEntry, buffer: &[u8]) -> Result<usize, i32> {
        syscall3(1, entry.raw, buffer.as_ptr() as usize, buffer.len())
    }

    pub(super) fn close(entry: FdEntry) -> Result<(), i32> {
        syscall3(3, entry.raw, 0, 0).map(|_| ())
    }

    fn syscall3(number: usize, first: usize, second: usize, third: usize) -> Result<usize, i32> {
        let result: isize;
        // SAFETY: forwards arguments using the Linux x86_64 syscall ABI.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") number as isize => result,
                in("rdi") first,
                in("rsi") second,
                in("rdx") third,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        if result < 0 {
            Err((-result) as i32)
        } else {
            Ok(result as usize)
        }
    }
}

#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
mod platform {
    use super::*;

    pub(super) fn install_standard_streams(_table: &mut FdTable) {}
    pub(super) fn read(_entry: FdEntry, _buffer: &mut [u8]) -> Result<usize, i32> {
        Err(EIO)
    }
    pub(super) fn write(_entry: FdEntry, _buffer: &[u8]) -> Result<usize, i32> {
        Err(EIO)
    }
    pub(super) fn close(_entry: FdEntry) -> Result<(), i32> {
        Err(EIO)
    }
}
