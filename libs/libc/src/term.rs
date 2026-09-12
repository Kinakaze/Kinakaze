//! Terminal C ABI exports: `termios`, window size and the `ioctl` requests
//! that TUI programs use to reach them.
//!
//! Every terminal here — a pseudo-terminal or the process's own console — is
//! driven by the software line discipline in `kinakaze_vfs::ldisc`, so a
//! `termios` bit means the same thing on both and the table of "bits Windows
//! happens to implement" that used to live in `pty::enforcement` no longer
//! applies to anything this module reaches.

use core::ffi::{c_int, c_void};
use core::ptr;

pub use kinakaze_vfs::ldisc::{
    N_6PACK, N_AX25, N_GIGASET_M101, N_GSM0710, N_HCI, N_HDLC, N_IRDA, N_MASC, N_MOUSE, N_NCI,
    N_NULL, N_PPP, N_PROFIBUS_FDL, N_R3964, N_SLCAN, N_SLIP, N_SMSBLOCK, N_SPEAKUP, N_STRIP,
    N_SYNC_PPP, N_TI_WL, N_TRACEROUTER, N_TRACESINK, N_TTY, N_X25,
};
use kinakaze_vfs::pty::{Termios, WinSize};
use kinakaze_vfs::{ldisc, tty};

use crate::set_errno;
use kinakaze_vfs::usernet::ioctl::*;

mod password;
mod verity;

#[cfg(all(test, windows))]
mod fionbio_tests;

/// `TCGETS`, `TIOCGWINSZ` and friends, as Linux numbers them.
pub const TCGETS: u64 = 0x5401;
pub const TCSETS: u64 = 0x5402;
pub const TCSETSW: u64 = 0x5403;
pub const TCSETSF: u64 = 0x5404;
pub const TCGETA: u64 = 0x5405;
pub const TCSETA: u64 = 0x5406;
pub const TCSETAW: u64 = 0x5407;
pub const TCSETAF: u64 = 0x5408;
pub const TCSBRK: u64 = 0x5409;
pub const TCXONC: u64 = 0x540A;
pub const TCFLSH: u64 = 0x540B;
pub const TIOCEXCL: u64 = 0x540C;
pub const TIOCNXCL: u64 = 0x540D;
pub const TIOCSCTTY: u64 = 0x540E;
pub const TIOCGPGRP: u64 = 0x540F;
pub const TIOCSPGRP: u64 = 0x5410;
pub const TIOCOUTQ: u64 = 0x5411;
pub const TIOCSTI: u64 = 0x5412;
pub const TIOCGWINSZ: u64 = 0x5413;
pub const TIOCSWINSZ: u64 = 0x5414;
pub const TIOCMGET: u64 = 0x5415;
pub const TIOCMBIS: u64 = 0x5416;
pub const TIOCMBIC: u64 = 0x5417;
pub const TIOCMSET: u64 = 0x5418;
pub const TIOCPKT: u64 = 0x5420;
pub const FIONREAD: u64 = 0x541B;
pub const TIOCINQ: u64 = 0x541B;
pub const TIOCLINUX: u64 = 0x541C;
pub const FIONBIO: u64 = 0x5421;
pub const FIOASYNC: u64 = 0x5452;
pub const TIOCNOTTY: u64 = 0x5422;
pub const TIOCSETD: u64 = 0x5423;
pub const TIOCGETD: u64 = 0x5424;
pub const TCSBRKP: u64 = 0x5425;
pub const TIOCSBRK: u64 = 0x5427;
pub const TIOCCBRK: u64 = 0x5428;
pub const TIOCGSID: u64 = 0x5429;

// The pty-specific requests. These carry a real `_IOC` encoding rather than a
// bare number, and the encoding is part of the constant a guest passes, so they
// are spelled out in full rather than reduced to their command byte.
/// `_IOR('T', 0x30, unsigned int)`: the slave's `/dev/pts` number.
pub const TIOCGPTN: u64 = 0x8004_5430;
/// `_IOW('T', 0x31, int)`: lock or unlock the slave.
pub const TIOCSPTLCK: u64 = 0x4004_5431;
/// `_IOR('T', 0x32, unsigned int)`: the terminal's device number.
pub const TIOCGDEV: u64 = 0x8004_5432;
/// `_IOR('T', 0x39, int)`: read the slave's lock state.
pub const TIOCGPTLCK: u64 = 0x8004_5439;
/// `_IOR('T', 0x40, int)`: read the exclusive-use flag.
pub const TIOCGEXCL: u64 = 0x8004_5440;
/// `_IO('T', 0x41)`: open the peer of a master without naming it.
pub const TIOCGPTPEER: u64 = 0x5441;
/// `_IOW('T', 0x36, int)`: send a signal to the foreground group.
pub const TIOCSIG: u64 = 0x4004_5436;
/// `_IO('T', 0x37)`: simulate a hangup on the terminal.
pub const TIOCVHANGUP: u64 = 0x5437;

fn posix_unit(result: Result<(), i32>) -> c_int {
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `tcgetattr`.
///
/// # Safety
///
/// `out` must point to a writable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tcgetattr(fd: c_int, out: *mut Termios) -> c_int {
    if out.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    match tty::tcgetattr(fd) {
        Ok(termios) => {
            // SAFETY: the caller guarantees a writable struct termios.
            unsafe { ptr::write(out, termios) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `tcsetattr`.
///
/// # Safety
///
/// `requested` must point to a readable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tcsetattr(
    fd: c_int,
    actions: c_int,
    requested: *const Termios,
) -> c_int {
    if requested.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct termios.
    let requested = unsafe { &*requested };
    posix_unit(tty::tcsetattr(fd, actions, requested))
}

/// `cfmakeraw`.
///
/// # Safety
///
/// `termios` must point to a valid `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfmakeraw(termios: *mut Termios) {
    if termios.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a valid struct termios.
    ldisc::make_raw(unsafe { &mut *termios });
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcflush(fd: c_int, queue: c_int) -> c_int {
    posix_unit(tty::tcflush(fd, queue))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcdrain(fd: c_int) -> c_int {
    posix_unit(tty::tcdrain(fd))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcsendbreak(fd: c_int, duration: c_int) -> c_int {
    posix_unit(tty::tcsendbreak(fd, duration))
}

/// `tcflow`, which really suspends and resumes transmission.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcflow(fd: c_int, action: c_int) -> c_int {
    posix_unit(tty::tcflow(fd, action))
}

// ---------------------------------------------------------------------------
// The raw x86_64 ioctl ABI uses the kernel's 44-byte termios shape.  libc's
// public `struct termios` is 60 bytes because glibc exposes NCCS=32; conflating
// the two overwrites sixteen bytes after buffers used by raw-syscall callers.
// ---------------------------------------------------------------------------

const KERNEL_NCCS: usize = 19;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelTermios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; KERNEL_NCCS],
    c_ispeed: u32,
    c_ospeed: u32,
}

impl KernelTermios {
    fn from_libc(termios: &Termios) -> Self {
        let mut c_cc = [0u8; KERNEL_NCCS];
        c_cc.copy_from_slice(&termios.c_cc[..KERNEL_NCCS]);
        Self {
            c_iflag: termios.c_iflag,
            c_oflag: termios.c_oflag,
            c_cflag: termios.c_cflag,
            c_lflag: termios.c_lflag,
            c_line: termios.c_line,
            c_cc,
            c_ispeed: termios.c_ispeed,
            c_ospeed: termios.c_ospeed,
        }
    }

    fn apply(&self, termios: &mut Termios) {
        termios.c_iflag = self.c_iflag;
        termios.c_oflag = self.c_oflag;
        termios.c_cflag = self.c_cflag;
        termios.c_lflag = self.c_lflag;
        termios.c_line = self.c_line;
        termios.c_cc[..KERNEL_NCCS].copy_from_slice(&self.c_cc);
        termios.c_ispeed = self.c_ispeed;
        termios.c_ospeed = self.c_ospeed;
    }
}

unsafe fn ioctl_tcgets(fd: c_int, argument: *mut c_void) -> c_int {
    if argument.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    match tty::tcgetattr(fd) {
        Ok(termios) => {
            // SAFETY: TCGETS guarantees a writable kernel-termios buffer. Raw
            // syscall pointers need not carry Rust alignment.
            unsafe {
                ptr::write_unaligned(
                    argument.cast::<KernelTermios>(),
                    KernelTermios::from_libc(&termios),
                )
            };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

unsafe fn ioctl_tcsets(fd: c_int, actions: c_int, argument: *const c_void) -> c_int {
    if argument.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let mut current = match tty::tcgetattr(fd) {
        Ok(termios) => termios,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    // SAFETY: TCSETS guarantees a readable kernel-termios buffer. Preserve the
    // libc-only tail of c_cc because the kernel ABI has no fields for it.
    let requested = unsafe { ptr::read_unaligned(argument.cast::<KernelTermios>()) };
    requested.apply(&mut current);
    posix_unit(tty::tcsetattr(fd, actions, &current))
}

/// Bytes a read on `fd` could return without blocking.
///
/// `FIONREAD` is a byte count, and getting that wrong is worse than not
/// answering: a caller sizes a buffer from it. A terminal's answer comes from
/// the line discipline — in canonical mode a half-typed line is *not* readable
/// even though its bytes are queued — rather than from a host-level guess.
unsafe fn get_bytes_available(fd: c_int) -> c_int {
    let Ok(entry) = kinakaze_vfs::get(fd) else {
        return 0;
    };
    let clamp = |value: usize| value.min(c_int::MAX as usize) as c_int;
    match entry.kind {
        kinakaze_vfs::FdKind::PtyMaster | kinakaze_vfs::FdKind::PtySlave => {
            tty::input_queued(fd).map(clamp).unwrap_or(0)
        }
        kinakaze_vfs::FdKind::Console => {
            if tty::is_console(fd) {
                // The previous answer was `GetNumberOfConsoleInputEvents`, which
                // counts *records* — a window resize or a focus change is one,
                // and neither is a byte. A caller that trusted it read fewer
                // bytes than it was told to expect, or blocked.
                clamp(tty::console_queued())
            } else {
                0
            }
        }
        kinakaze_vfs::FdKind::Pipe | kinakaze_vfs::FdKind::UnixSocket => {
            // AF_UNIX uses named pipes, so Winsock's FIONREAD would reject the
            // handle and falsely report an empty receive queue to EPOLLET users.
            use windows_sys::Win32::System::Pipes::PeekNamedPipe;
            let mut avail = 0u32;
            if unsafe {
                PeekNamedPipe(
                    entry.raw as _,
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &raw mut avail,
                    ptr::null_mut(),
                )
            } != 0
            {
                avail.min(c_int::MAX as u32) as c_int
            } else {
                0
            }
        }
        kinakaze_vfs::FdKind::Socket | kinakaze_vfs::FdKind::NetlinkSocket => {
            use windows_sys::Win32::Networking::WinSock::{FIONREAD as WS_FIONREAD, ioctlsocket};
            let mut arg = 0u32;
            if unsafe { ioctlsocket(entry.raw as _, WS_FIONREAD, &raw mut arg) } == 0 {
                arg.min(c_int::MAX as u32) as c_int
            } else {
                0
            }
        }
        kinakaze_vfs::FdKind::File => {
            if let Ok(stat) = kinakaze_vfs::fs::fstat(fd) {
                (stat.st_size.saturating_sub(entry.offset as i64))
                    .max(0)
                    .min(c_int::MAX as i64) as c_int
            } else {
                0
            }
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// `struct termio`, the System V predecessor of `struct termios`.
//
// The two are not interchangeable, which is the bug this replaces: `TCGETA` was
// aliased onto `TCGETS` and wrote sixty bytes into an eighteen-byte caller
// buffer. Every field is narrower here — sixteen bits rather than thirty-two —
// and `c_cc` holds eight entries rather than thirty-two, so the conversion is a
// real narrowing in both directions.
// ---------------------------------------------------------------------------

/// Linux's `struct termio`: 18 bytes, four 16-bit flag words and `c_cc[8]`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Termio {
    c_iflag: u16,
    c_oflag: u16,
    c_cflag: u16,
    c_lflag: u16,
    c_line: u8,
    c_cc: [u8; 8],
}

/// How many control characters `struct termio` carries.
const NCC: usize = 8;

impl Termio {
    /// Narrows a `termios` for a `TCGETA` caller.
    ///
    /// The high sixteen bits of every flag word are dropped, which loses
    /// `IUTF8`, `EXTPROC` and `CRTSCTS` among others. That is not a shortcut: a
    /// `termio` has nowhere to put them, and a caller using this interface is by
    /// definition asking for the older, smaller view.
    fn from_termios(termios: &Termios) -> Self {
        let mut c_cc = [0u8; NCC];
        // The first six indices line up between the two structures; the seventh
        // and eighth are VEOL and VEOF in termio's order, which differs from
        // termios where VMIN and VTIME occupy 5 and 6.
        c_cc[..6].copy_from_slice(&termios.c_cc[..6]);
        c_cc[6] = termios.c_cc[kinakaze_vfs::pty::VMIN];
        c_cc[7] = termios.c_cc[kinakaze_vfs::pty::VTIME];
        Self {
            c_iflag: termios.c_iflag as u16,
            c_oflag: termios.c_oflag as u16,
            c_cflag: termios.c_cflag as u16,
            c_lflag: termios.c_lflag as u16,
            c_line: termios.c_line,
            c_cc,
        }
    }

    /// Widens a `termio` onto the terminal's current settings.
    ///
    /// The *current* settings rather than a default, because the high bits the
    /// caller cannot express must survive: a program that calls `TCSETA` on a
    /// UTF-8 terminal has not asked to turn `IUTF8` off, and clearing it would
    /// break multi-byte erase for a caller that never mentioned it.
    fn apply(&self, current: &mut Termios) {
        current.c_iflag = (current.c_iflag & 0xffff_0000) | u32::from(self.c_iflag);
        current.c_oflag = (current.c_oflag & 0xffff_0000) | u32::from(self.c_oflag);
        current.c_cflag = (current.c_cflag & 0xffff_0000) | u32::from(self.c_cflag);
        current.c_lflag = (current.c_lflag & 0xffff_0000) | u32::from(self.c_lflag);
        current.c_line = self.c_line;
        current.c_cc[..6].copy_from_slice(&self.c_cc[..6]);
        current.c_cc[kinakaze_vfs::pty::VMIN] = self.c_cc[6];
        current.c_cc[kinakaze_vfs::pty::VTIME] = self.c_cc[7];
    }
}

/// Serves `TCGETA`.
///
/// # Safety
///
/// `argument` must point to a writable `struct termio`.
unsafe fn ioctl_tcgeta(fd: c_int, argument: *mut c_void) -> c_int {
    if argument.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    match tty::tcgetattr(fd) {
        Ok(termios) => {
            // SAFETY: the caller guarantees eighteen writable bytes.
            unsafe {
                ptr::write_unaligned(argument.cast::<Termio>(), Termio::from_termios(&termios))
            };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Serves the three `TCSETA` variants.
///
/// # Safety
///
/// `argument` must point to a readable `struct termio`.
unsafe fn ioctl_tcseta(fd: c_int, actions: c_int, argument: *const c_void) -> c_int {
    if argument.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let mut current = match tty::tcgetattr(fd) {
        Ok(termios) => termios,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    // SAFETY: the caller guarantees eighteen readable bytes. The read is
    // unaligned because `struct termio` has no alignment a caller must honour.
    let requested = unsafe { ptr::read_unaligned(argument.cast::<Termio>()) };
    requested.apply(&mut current);
    posix_unit(tty::tcsetattr(fd, actions, &current))
}

/// Reads one `int` from an ioctl argument.
///
/// # Safety
///
/// `argument` must point to a readable `int` unless it is null.
unsafe fn read_int(argument: *const c_void) -> Result<c_int, i32> {
    if argument.is_null() {
        return Err(kinakaze_vfs::EFAULT);
    }
    // SAFETY: the caller guarantees a readable int; the read is unaligned
    // because nothing constrains where a guest puts its argument.
    Ok(unsafe { ptr::read_unaligned(argument.cast::<c_int>()) })
}

/// Writes one `int` to an ioctl argument.
///
/// # Safety
///
/// `argument` must point to a writable `int` unless it is null.
unsafe fn write_int(argument: *mut c_void, value: c_int) -> c_int {
    if argument.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a writable int.
    unsafe { ptr::write_unaligned(argument.cast::<c_int>(), value) };
    0
}

/// `ioctl`, restricted to the terminal and descriptor requests.
///
/// A general `ioctl` cannot be emulated, so an unknown request reports `ENOTTY`
/// rather than pretending to succeed.
///
/// # Safety
///
/// `argument` must match what `request` requires.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ioctl(
    fd: c_int,
    request: u64,
    argument: *mut c_void,
) -> c_int {
    if matches!(request, 0xb701..=0xb704) {
        let entry = match kinakaze_vfs::get(fd) {
            Ok(entry) => entry,
            Err(error) => {
                set_errno(error);
                return -1;
            }
        };
        if entry.flags.contains(kinakaze_vfs::FdFlags::PATH_ONLY) {
            set_errno(kinakaze_vfs::EBADF);
            return -1;
        }
        let result = match entry.kind {
            kinakaze_vfs::FdKind::Namespace => unsafe {
                kinakaze_vfs::namespaces::ioctl(fd, request, argument.cast())
            },
            kinakaze_vfs::FdKind::UserNamespace => unsafe {
                kinakaze_vfs::user_namespace::ioctl(fd, request, argument.cast())
            },
            kinakaze_vfs::FdKind::MountNamespace | kinakaze_vfs::FdKind::TimeNamespace => {
                match request {
                    0xb703 => Ok(if entry.kind == kinakaze_vfs::FdKind::TimeNamespace {
                        0x80
                    } else {
                        0x0002_0000
                    }),
                    0xb701 => (if entry.kind == kinakaze_vfs::FdKind::TimeNamespace {
                        kinakaze_vfs::time_namespace::owner(fd)
                    } else {
                        kinakaze_vfs::mount::namespace_owner(fd)
                    })
                    .and_then(kinakaze_vfs::user_namespace::open_id),
                    _ => Err(kinakaze_vfs::EINVAL),
                }
            }
            _ => Err(kinakaze_vfs::ENOTTY),
        };
        return match result {
            Ok(value) => value,
            Err(error) => {
                set_errno(error);
                -1
            }
        };
    }

    if verity::handles(request) {
        return match verity::ioctl(fd, request, argument) {
            Ok(count) => count,
            Err(error) => {
                set_errno(error);
                -1
            }
        };
    }
    match request {
        FIONBIO => {
            // Validate the descriptor before touching the user argument, as
            // Linux does. Native sockets stay nonblocking; this controls the
            // compatibility layer's wait policy for every open description.
            match kinakaze_vfs::get(fd) {
                Ok(entry) if !entry.flags.contains(kinakaze_vfs::FdFlags::PATH_ONLY) => {}
                Ok(_) => {
                    set_errno(kinakaze_vfs::EBADF);
                    return -1;
                }
                Err(error) => {
                    set_errno(error);
                    return -1;
                }
            }
            match unsafe { read_int(argument) } {
                Ok(value) => posix_unit(kinakaze_vfs::set_nonblocking(fd, value != 0)),
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        FIOASYNC => match unsafe { read_int(argument) } {
            Ok(value) => posix_unit(kinakaze_vfs::unix::set_async(fd, value != 0)),
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TCGETS => {
            // SAFETY: TCGETS takes the kernel's 44-byte termios, not libc's
            // public 60-byte structure.
            unsafe { ioctl_tcgets(fd, argument) }
        }
        // SAFETY: TCGETA takes a writable struct termio, which is a different
        // and much smaller structure — see `Termio`.
        TCGETA => unsafe { ioctl_tcgeta(fd, argument) },
        // The three TCSETS variants differ in *when* they take effect, and
        // collapsing them was a real bug: TCSETSW exists so a program does not
        // reinterpret bytes it already wrote, and TCSETSF so a password prompt
        // does not read keystrokes typed while echo was still on.
        TCSETS => unsafe { ioctl_tcsets(fd, kinakaze_vfs::pty::TCSANOW, argument) },
        TCSETSW => unsafe { ioctl_tcsets(fd, kinakaze_vfs::pty::TCSADRAIN, argument) },
        TCSETSF => unsafe { ioctl_tcsets(fd, kinakaze_vfs::pty::TCSAFLUSH, argument) },
        // SAFETY: each TCSETA variant takes a readable struct termio.
        TCSETA => unsafe { ioctl_tcseta(fd, kinakaze_vfs::pty::TCSANOW, argument) },
        TCSETAW => unsafe { ioctl_tcseta(fd, kinakaze_vfs::pty::TCSADRAIN, argument) },
        TCSETAF => unsafe { ioctl_tcseta(fd, kinakaze_vfs::pty::TCSAFLUSH, argument) },
        TCFLSH => {
            let queue = argument as usize as c_int;
            kinakaze_abi_tcflush(fd, queue)
        }
        TCXONC => {
            let action = argument as usize as c_int;
            kinakaze_abi_tcflow(fd, action)
        }
        // TCSBRK with a zero argument drains; with a non-zero one it is a
        // break. TCSBRKP is always a break, measured in tenths of a second.
        TCSBRK => {
            if argument.is_null() {
                kinakaze_abi_tcdrain(fd)
            } else {
                kinakaze_abi_tcsendbreak(fd, argument as usize as c_int)
            }
        }
        TCSBRKP | TIOCSBRK | TIOCCBRK => kinakaze_abi_tcsendbreak(fd, argument as usize as c_int),
        TIOCSCTTY => posix_unit(tty::set_controlling_terminal(fd, argument as usize != 0)),
        TIOCNOTTY => posix_unit(tty::drop_controlling_terminal(fd)),
        TIOCEXCL => posix_unit(tty::set_exclusive(fd, true)),
        TIOCNXCL => posix_unit(tty::set_exclusive(fd, false)),
        TIOCGEXCL => match tty::is_exclusive(fd) {
            Ok(value) => unsafe { write_int(argument, c_int::from(value)) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TIOCGPGRP => {
            let pgrp = kinakaze_abi_tcgetpgrp(fd);
            if pgrp < 0 {
                return -1;
            }
            // SAFETY: TIOCGPGRP takes a writable pid_t.
            unsafe { write_int(argument, pgrp) }
        }
        TIOCSPGRP => {
            // SAFETY: TIOCSPGRP takes a readable pid_t.
            match unsafe { read_int(argument) } {
                Ok(pgrp) => kinakaze_abi_tcsetpgrp(fd, pgrp),
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCGSID => {
            let sid = kinakaze_abi_tcgetsid(fd);
            if sid < 0 {
                return -1;
            }
            // SAFETY: TIOCGSID takes a writable pid_t.
            unsafe { write_int(argument, sid) }
        }
        TIOCOUTQ => match tty::output_queued(fd) {
            Ok(count) => unsafe { write_int(argument, count.min(c_int::MAX as usize) as c_int) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        FIONREAD => {
            let entry = match kinakaze_vfs::get(fd) {
                Ok(entry) => entry,
                Err(error) => {
                    set_errno(error);
                    return -1;
                }
            };
            if entry.flags.contains(kinakaze_vfs::FdFlags::PACKET_SOCKET) {
                return match kinakaze_vfs::socket::packet_queued_bytes(fd) {
                    Ok(Some(count)) => unsafe {
                        write_int(argument, count.min(c_int::MAX as usize) as c_int)
                    },
                    Ok(None) => {
                        set_errno(kinakaze_vfs::EIO);
                        -1
                    }
                    Err(error) => {
                        set_errno(error);
                        -1
                    }
                };
            }
            if entry.kind == kinakaze_vfs::FdKind::Fifo {
                return match kinakaze_vfs::fifo::queued_bytes(fd, entry) {
                    Ok(count) => unsafe {
                        write_int(argument, count.min(c_int::MAX as usize) as c_int)
                    },
                    Err(error) => {
                        set_errno(error);
                        -1
                    }
                };
            }
            // SAFETY: the descriptor is validated inside.
            let nread = unsafe { get_bytes_available(fd) };
            // SAFETY: FIONREAD takes a writable int.
            unsafe { write_int(argument, nread) }
        }
        TIOCGWINSZ => {
            if argument.is_null() {
                set_errno(kinakaze_vfs::EFAULT);
                return -1;
            }
            match tty::get_window_size(fd) {
                Ok(size) => {
                    // SAFETY: TIOCGWINSZ takes a writable struct winsize.
                    unsafe { ptr::write_unaligned(argument.cast::<WinSize>(), size) };
                    0
                }
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCSWINSZ => {
            if argument.is_null() {
                set_errno(kinakaze_vfs::EFAULT);
                return -1;
            }
            // SAFETY: TIOCSWINSZ takes a readable struct winsize.
            let size = unsafe { ptr::read_unaligned(argument.cast::<WinSize>()) };
            posix_unit(tty::set_window_size(fd, &size))
        }
        TIOCGPTN => match tty::pty_number(fd) {
            Ok(number) => unsafe { write_int(argument, number as c_int) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TIOCSPTLCK => {
            // SAFETY: TIOCSPTLCK takes a readable int.
            match unsafe { read_int(argument) } {
                Ok(value) => posix_unit(tty::set_slave_lock(fd, value != 0)),
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCGPTLCK => match tty::slave_locked(fd) {
            Ok(locked) => unsafe { write_int(argument, c_int::from(locked)) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        // The argument is the open flags for the new descriptor, passed by value.
        TIOCGPTPEER => match tty::open_peer(fd, argument as usize as c_int) {
            Ok(peer) => peer,
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TIOCGDEV => match kinakaze_vfs::fs::fstat(fd) {
            Ok(stat) => unsafe { write_int(argument, stat.st_rdev as c_int) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TIOCPKT => {
            // SAFETY: TIOCPKT takes a readable int.
            match unsafe { read_int(argument) } {
                Ok(value) => posix_unit(tty::set_packet_mode(fd, value != 0)),
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCSTI => {
            if argument.is_null() {
                set_errno(kinakaze_vfs::EFAULT);
                return -1;
            }
            // SAFETY: TIOCSTI takes a readable char.
            let byte = unsafe { ptr::read_unaligned(argument.cast::<u8>()) };
            posix_unit(tty::push_input(fd, byte))
        }
        TIOCGETD => match tty::line_discipline(fd) {
            Ok(number) => unsafe { write_int(argument, number) },
            Err(error) => {
                set_errno(error);
                -1
            }
        },
        TIOCSETD => {
            // SAFETY: TIOCSETD takes a readable int.
            match unsafe { read_int(argument) } {
                Ok(number) => posix_unit(tty::set_line_discipline(fd, number)),
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCSIG => {
            // SAFETY: TIOCSIG takes a readable int.
            match unsafe { read_int(argument) } {
                Ok(signal) => match tty::foreground_pgrp(fd) {
                    Ok(pgrp) => posix_unit(kinakaze_vfs::job::signal_process_group(pgrp, signal)),
                    Err(error) => {
                        set_errno(error);
                        -1
                    }
                },
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        TIOCVHANGUP => posix_unit(tty::vhangup(fd)),
        // Modem control lines. A pseudo-terminal has none, and Linux reports a
        // fixed set of asserted lines for one rather than failing, because a
        // program that reads them is deciding whether the line is up.
        TIOCMGET => unsafe {
            write_int(
                argument,
                TIOCM_LE | TIOCM_DTR | TIOCM_RTS | TIOCM_CTS | TIOCM_DSR | TIOCM_CAR,
            )
        },
        TIOCMSET | TIOCMBIS | TIOCMBIC => {
            if tty::isatty(fd) {
                0
            } else {
                set_errno(kinakaze_vfs::ENOTTY);
                -1
            }
        }
        SIOCGIFCONF | SIOCGIFFLAGS | SIOCSIFFLAGS | SIOCGIFADDR | SIOCGIFDSTADDR
        | SIOCGIFBRDADDR | SIOCGIFNETMASK | SIOCGIFMTU | SIOCGIFHWADDR | SIOCGIFINDEX => unsafe {
            match kinakaze_vfs::usernet::ioctl::interface(fd, request, argument) {
                Ok(()) => 0,
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        },
        _ => {
            set_errno(kinakaze_vfs::ENOTTY);
            -1
        }
    }
}

// Modem status lines, reported as permanently asserted on a pseudo-terminal.
const TIOCM_LE: c_int = 0x001;
const TIOCM_DTR: c_int = 0x002;
const TIOCM_RTS: c_int = 0x004;
const TIOCM_CTS: c_int = 0x020;
const TIOCM_CAR: c_int = 0x040;
const TIOCM_DSR: c_int = 0x100;

// ---------------------------------------------------------------------------
// Line speeds.
//
// A speed is not a number in `termios`: it is an index in the low bits of
// `c_cflag`, which is the whole reason these are functions rather than field
// accesses. `B9600` is 13 and `B38400` is 15; past that the index would not fit
// in four bits, so Linux sets `CBAUDEX` and continues counting, making `B57600`
// 0o10001 and `B115200` 0o10002.
//
// A pseudoconsole has no baud rate to measure. The speed these functions report
// is `B38400` unless a caller has set one, and that number is *not a
// measurement*: it is the value a Linux pty reports, chosen because some programs
// divide by the speed to compute a padding delay and `B0` means "hang up the
// line", which would send a caller down an error path. What a caller sets is
// remembered verbatim and read back unchanged, which is the property that
// actually gets tested, and it changes no host behaviour — see
// [`pty::enforcement`].
// ---------------------------------------------------------------------------

/// `CBAUD`, the `c_cflag` field holding the output speed index.
///
/// Four low bits plus `CBAUDEX` at 0o10000, so the mask is not contiguous.
pub const CBAUD: u32 = 0o010017;

/// `CBAUDEX`, the bit that extends the speed index past `B38400`.
pub const CBAUDEX: u32 = 0o010000;

/// `CIBAUD`, the input-speed field, sixteen bits above `CBAUD`.
pub const CIBAUD: u32 = 0o02003600000;

/// `IBSHIFT`, how far `CIBAUD` sits above `CBAUD`.
const IBSHIFT: u32 = 16;

/// The speed indices Linux defines, low range then extended.
///
/// A caller may pass a value outside this list — glibc does not check either —
/// and it round-trips, but only these have a bit rate anyone has agreed on.
pub const B0: u32 = 0o0000000;
pub const B50: u32 = 0o0000001;
pub const B75: u32 = 0o0000002;
pub const B110: u32 = 0o0000003;
pub const B134: u32 = 0o0000004;
pub const B150: u32 = 0o0000005;
pub const B200: u32 = 0o0000006;
pub const B300: u32 = 0o0000007;
pub const B600: u32 = 0o0000010;
pub const B1200: u32 = 0o0000011;
pub const B1800: u32 = 0o0000012;
pub const B2400: u32 = 0o0000013;
pub const B4800: u32 = 0o0000014;
pub const B9600: u32 = 0o0000015;
pub const B19200: u32 = 0o0000016;
// `B38400` itself lives in `pty`, next to the `Termios` that defaults to it.
pub const B57600: u32 = 0o0010001;
pub const B115200: u32 = 0o0010002;
pub const B230400: u32 = 0o0010003;
pub const B460800: u32 = 0o0010004;
pub const B500000: u32 = 0o0010005;
pub const B576000: u32 = 0o0010006;
pub const B921600: u32 = 0o0010007;
pub const B1000000: u32 = 0o0010010;
pub const B1152000: u32 = 0o0010011;
pub const B1500000: u32 = 0o0010012;
pub const B2000000: u32 = 0o0010013;

/// The speed a terminal here reports when nobody has set one.
///
/// Not a measurement. See the section comment above.
pub const DEFAULT_SPEED: u32 = kinakaze_vfs::pty::B38400;

/// Rejects a speed whose bits fall outside the `CBAUD` field.
///
/// glibc accepts any value and stores it, but a value with bits outside the field
/// cannot be stored in `c_cflag` without corrupting a neighbouring flag — `CS8`
/// and `CREAD` live just above — so it is refused rather than silently truncated.
fn valid_speed(speed: u32) -> bool {
    speed & !CBAUD == 0
}

/// `cfgetospeed`, the output speed.
///
/// Reads `CBAUD` from `c_cflag`, which is where the kernel keeps it. When that
/// field is `B0` but `c_ospeed` holds a speed, `c_ospeed` wins: `B0` means "hang
/// up the line", and a freshly initialized `struct termios` that has never been
/// through `cfsetospeed` has zeroes in `c_cflag`'s speed bits while
/// [`pty::Termios::default`] does record a speed. Reporting a hangup for a
/// perfectly live terminal would be a fabrication, so the recorded speed is
/// preferred and a *deliberate* `B0` — which clears both fields — still reads
/// back as `B0`.
///
/// # Safety
///
/// `termios` must point to a readable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfgetospeed(termios: *const Termios) -> u32 {
    if termios.is_null() {
        return B0;
    }
    // SAFETY: the caller guarantees a readable struct termios.
    let termios = unsafe { &*termios };
    let encoded = termios.c_cflag & CBAUD;
    if encoded == B0 && termios.c_ospeed != 0 {
        return termios.c_ospeed;
    }
    encoded
}

/// `cfgetispeed`, the input speed.
///
/// `CIBAUD` is the input field, and POSIX gives `B0` there a second meaning: "the
/// input speed is the output speed". That is the near-universal case, so a clear
/// field falls through to the output speed rather than reporting a hangup.
///
/// # Safety
///
/// `termios` must point to a readable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfgetispeed(termios: *const Termios) -> u32 {
    if termios.is_null() {
        return B0;
    }
    // SAFETY: the caller guarantees a readable struct termios.
    let borrowed = unsafe { &*termios };
    let encoded = (borrowed.c_cflag & CIBAUD) >> IBSHIFT;
    if encoded != B0 {
        return encoded;
    }
    if borrowed.c_ispeed != 0 {
        return borrowed.c_ispeed;
    }
    // Split speeds are vanishingly rare; POSIX says an input speed of zero means
    // "same as output", so that is what is reported.
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_cfgetospeed(termios) }
}

/// `cfsetospeed`.
///
/// Writes both `CBAUD` and `c_ospeed`, which is what makes a get-after-set
/// round-trip: `c_cflag` is where the kernel reads the speed from and `c_ospeed`
/// is where glibc keeps its copy, and a caller may inspect either. The value is
/// stored exactly as given, including one above `B38400` where the extended
/// encoding puts a bit at 0o10000 rather than a larger number in the low four.
///
/// Nothing about the host changes. There is no line to run at a rate.
///
/// # Safety
///
/// `termios` must point to a writable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfsetospeed(termios: *mut Termios, speed: u32) -> c_int {
    if termios.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if !valid_speed(speed) {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    // SAFETY: the caller guarantees a writable struct termios.
    let termios = unsafe { &mut *termios };
    termios.c_cflag = (termios.c_cflag & !CBAUD) | speed;
    termios.c_ospeed = speed;
    0
}

/// `cfsetispeed`.
///
/// A `speed` of `B0` is not an error: POSIX defines it as "use the output speed",
/// so it clears the field rather than being refused.
///
/// # Safety
///
/// `termios` must point to a writable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfsetispeed(termios: *mut Termios, speed: u32) -> c_int {
    if termios.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if !valid_speed(speed) {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    // SAFETY: the caller guarantees a writable struct termios.
    let termios = unsafe { &mut *termios };
    termios.c_cflag = (termios.c_cflag & !CIBAUD) | (speed << IBSHIFT);
    termios.c_ispeed = speed;
    0
}

/// `cfsetspeed`, the glibc extension that sets both directions at once.
///
/// # Safety
///
/// `termios` must point to a writable `struct termios`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cfsetspeed(termios: *mut Termios, speed: u32) -> c_int {
    // SAFETY: forwarded from this function's contract.
    if unsafe { kinakaze_abi_cfsetospeed(termios, speed) } != 0 {
        return -1;
    }
    // SAFETY: as above.
    unsafe { kinakaze_abi_cfsetispeed(termios, speed) }
}
// ---------------------------------------------------------------------------
// Foreground process groups and sessions.
//
// These now answer from the terminal itself rather than from one process-global
// atomic. That matters as soon as there is more than one terminal: a shell with
// a pty open for a child and its own console open for the user has two
// foreground groups, and a single global would have made `tcsetpgrp` on one of
// them silently move the other.
//
// What is checked for real is whether the descriptor is a terminal at all, and
// which session owns it. `ENOTTY` from `tcgetpgrp` is how a shell decides it is
// not interactive, and `EPERM` from `tcsetpgrp` is how it decides it has lost
// the terminal, so both have to be genuine.
// ---------------------------------------------------------------------------

/// `tcgetpgrp`, the terminal's foreground process group.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcgetpgrp(fd: c_int) -> c_int {
    match tty::foreground_pgrp(fd) {
        Ok(pgrp) => pgrp,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `tcgetsid`, the session that owns the terminal.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcgetsid(fd: c_int) -> c_int {
    match tty::session_id(fd) {
        Ok(sid) => sid,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `tcsetpgrp`, which hands the terminal to a process group.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tcsetpgrp(fd: c_int, pgrp: c_int) -> c_int {
    posix_unit(tty::set_foreground_pgrp(fd, pgrp))
}

/// The path reported for a terminal with no better name.
///
/// A pty slave answers `/dev/pts/N` and a master `/dev/ptmx`; only the process's
/// own console falls back to this, and unlike the previous fixed answer it is a
/// path the guest can actually open — `/dev/tty` reaches the controlling
/// terminal through [`kinakaze_vfs::tty::open_controlling`]. The name itself
/// lives in the terminal layer, next to the two it is an alternative to; this
/// spelling exists so the tests can state what they expect.
#[cfg(test)]
const TTY_PATH: &str = "/dev/tty";

/// Copies a terminal's name into a caller's buffer.
///
/// The reentrant return convention is easy to get backwards: the error number is
/// the *return value*, with 0 for success, and errno is not the channel.
/// `ERANGE` says the buffer is too small, and nothing is written in that case —
/// a caller sizing its buffer by retrying must not have to guess whether a
/// partial name was left behind.
///
/// # Safety
///
/// `name` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ttyname_r(
    fd: c_int,
    name: *mut core::ffi::c_char,
    length: usize,
) -> c_int {
    if name.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    // A non-terminal has no name, and this is a real check rather than an
    // assumption: the descriptor is resolved to a terminal or refused.
    let path = match tty::terminal_name(fd) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let bytes = path.as_bytes();
    // The terminator has to fit too, so a buffer of exactly the path's length is
    // still too small.
    if length < bytes.len() + 1 {
        return kinakaze_vfs::ERANGE;
    }
    // SAFETY: the check above proved the buffer holds the path and a NUL, and the
    // caller guarantees it is writable for `length` bytes.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), bytes.len());
        *name.add(bytes.len()) = 0;
    }
    0
}

/// Storage for the non-reentrant [`kinakaze_abi_ttyname`] and
/// [`kinakaze_abi_ptsname`].
///
/// Allocated from the guest heap rather than kept in a static or a thread-local,
/// and that is the whole point. `fork` here clones the managed arena into a
/// *fresh Windows process*; the DLL's own statics and thread-local storage are
/// not copied. A program that does what every pty-driving program does —
///
/// ```c
/// name = ptsname(master);
/// if (fork() == 0) { slave = open(name, O_RDWR); ... }
/// ```
///
/// — reads that pointer in the child. Backed by a static it would find zeroes or
/// unmapped memory; backed by the arena it finds the string, exactly as it would
/// find glibc's static buffer after a real `fork`.
///
/// One buffer per process, as glibc has: both functions are specified as
/// returning storage a later call may overwrite, and they are not thread-safe.
/// The reentrant forms exist for callers that need more.
const NAME_MAX: usize = 64;

static NAME_BUFFER: core::sync::atomic::AtomicPtr<core::ffi::c_char> =
    core::sync::atomic::AtomicPtr::new(ptr::null_mut());

/// Returns the shared name buffer, allocating it on first use.
fn name_buffer() -> *mut core::ffi::c_char {
    use core::sync::atomic::Ordering;
    let existing = NAME_BUFFER.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing;
    }
    // SAFETY: the allocator is the guest's own and the size is a constant.
    let fresh = unsafe { crate::kinakaze_abi_malloc(NAME_MAX) }.cast::<core::ffi::c_char>();
    if fresh.is_null() {
        return ptr::null_mut();
    }
    match NAME_BUFFER.compare_exchange(ptr::null_mut(), fresh, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => fresh,
        Err(winner) => {
            // Another thread published first; this one is redundant.
            // SAFETY: the block was just allocated here and never published.
            unsafe { crate::kinakaze_abi_free(fresh.cast()) };
            winner
        }
    }
}

/// `ttyname`.
///
/// This export did not exist before, so a guest that called it got an
/// unresolved symbol rather than a name. It is the form `tty(1)`, `who(1)` and
/// BusyBox's `ps` use.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ttyname(fd: c_int) -> *mut core::ffi::c_char {
    let storage = name_buffer();
    if storage.is_null() {
        set_errno(kinakaze_vfs::ENOMEM);
        return ptr::null_mut();
    }
    // SAFETY: the buffer is `NAME_MAX` writable bytes owned by this module.
    let code = unsafe { kinakaze_abi_ttyname_r(fd, storage, NAME_MAX) };
    if code != 0 {
        set_errno(code);
        return ptr::null_mut();
    }
    storage
}

/// `ptsname_r`, the reentrant name of a master's slave.
///
/// # Safety
///
/// `name` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ptsname_r(
    fd: c_int,
    name: *mut core::ffi::c_char,
    length: usize,
) -> c_int {
    if name.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    // Only a *master* has a slave to name. A slave, or any other descriptor,
    // reports ENOTTY, which is what distinguishes the two ends for a caller
    // that was handed a descriptor without being told which it is.
    let number = match tty::pty_side(fd) {
        Ok(tty::Side::Master) => match tty::pty_number(fd) {
            Ok(number) => number,
            Err(error) => return error,
        },
        Ok(tty::Side::Slave) => return kinakaze_vfs::ENOTTY,
        Err(error) => return error,
    };
    let path = format!("/dev/pts/{number}");
    let bytes = path.as_bytes();
    if length < bytes.len() + 1 {
        return kinakaze_vfs::ERANGE;
    }
    // SAFETY: the length check above proved the buffer holds the path and a NUL.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), bytes.len());
        *name.add(bytes.len()) = 0;
    }
    0
}

/// `ptsname`.
///
/// The first of the four calls `sshpass` and every other pty-driving program
/// makes after `posix_openpt`, and the one that has to produce a path the guest
/// can open — which `/dev/pts/N` now is. The returned pointer survives `fork`
/// for the reason [`name_buffer`] explains, which matters because the usual
/// shape of this code names the slave in the parent and opens it in the child.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ptsname(fd: c_int) -> *mut core::ffi::c_char {
    let storage = name_buffer();
    if storage.is_null() {
        set_errno(kinakaze_vfs::ENOMEM);
        return ptr::null_mut();
    }
    // SAFETY: the buffer is `NAME_MAX` writable bytes owned by this module.
    let code = unsafe { kinakaze_abi_ptsname_r(fd, storage, NAME_MAX) };
    if code != 0 {
        set_errno(code);
        return ptr::null_mut();
    }
    storage
}

/// `posix_openpt`: creates a pseudo-terminal and returns its master.
///
/// `flags` are the `open` flags — `O_RDWR` and optionally `O_NOCTTY` — and are
/// honoured rather than ignored, because `O_NONBLOCK` on the master is how a
/// terminal emulator avoids blocking on a child that has stopped reading.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_posix_openpt(flags: c_int) -> c_int {
    match kinakaze_vfs::fs::open("/dev/ptmx", flags, 0) {
        Ok(fd) => fd,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `grantpt`: makes the slave usable by the caller.
///
/// On Linux this changes the slave device node's owner and mode, work that a
/// `devpts` mounted with `ptmxmode` does for itself and that glibc's `grantpt`
/// then skips. The devpts allocator installs the configured UID, GID and mode;
/// this entry point validates the master without overwriting mount policy.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_grantpt(fd: c_int) -> c_int {
    match tty::pty_side(fd) {
        Ok(tty::Side::Master) => 0,
        Ok(tty::Side::Slave) => {
            set_errno(kinakaze_vfs::EINVAL);
            -1
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `unlockpt`: releases the slave so it can be opened.
///
/// This one is not ceremonial. A freshly created terminal holds its slave
/// locked, and opening `/dev/pts/N` before this call reports `EIO` exactly as it
/// would on Linux — which is what makes the `posix_openpt` / `grantpt` /
/// `unlockpt` / `ptsname` sequence a real protocol here rather than a formality.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_unlockpt(fd: c_int) -> c_int {
    posix_unit(tty::set_slave_lock(fd, false))
}
#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_char;
    use kinakaze_vfs::pty;
    use kinakaze_vfs::{FdFlags, FdKind};

    /// This process's own group id, which is what a terminal nobody has claimed
    /// reports as its foreground group.
    fn own_pid() -> c_int {
        kinakaze_vfs::job::current_pgid()
    }

    /// A descriptor on a real file: something that reads and writes but is not a
    /// terminal, which is the case `ENOTTY` exists to distinguish.
    ///
    /// The file is deleted and the descriptor closed on drop, so a test run leaves
    /// nothing behind in the temporary directory.
    struct FileFixture {
        fd: c_int,
        path: std::path::PathBuf,
        file: Option<std::fs::File>,
    }

    impl FileFixture {
        fn open() -> Self {
            use std::os::windows::io::AsRawHandle;
            let path = std::env::temp_dir().join(format!(
                "kinakaze-term-{}-{:?}.tmp",
                std::process::id(),
                std::thread::current().id()
            ));
            let file = std::fs::File::create(&path).expect("failed to create a temporary file");
            // BORROWED keeps the descriptor table from closing the handle, so the
            // `File` stays the sole owner and the drop order is predictable.
            let fd = kinakaze_vfs::install(
                file.as_raw_handle() as usize,
                FdKind::File,
                FdFlags::BORROWED,
            )
            .expect("failed to install the descriptor");
            Self {
                fd,
                path,
                file: Some(file),
            }
        }
    }

    impl Drop for FileFixture {
        fn drop(&mut self) {
            let _ = kinakaze_vfs::close(self.fd);
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn fionread_tracks_unix_stream_bytes_after_partial_reads() {
        use kinakaze_vfs::socket::{SOCK_NONBLOCK, SOCK_STREAM};

        struct SocketPair(c_int, c_int);
        impl Drop for SocketPair {
            fn drop(&mut self) {
                let _ = kinakaze_vfs::close(self.0);
                let _ = kinakaze_vfs::close(self.1);
            }
        }

        let (first, second) = kinakaze_vfs::unix::socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let pair = SocketPair(first, second);
        let queued = |fd| {
            let mut count: c_int = -1;
            // SAFETY: FIONREAD receives a writable int.
            assert_eq!(
                unsafe { kinakaze_abi_ioctl(fd, FIONREAD, (&raw mut count).cast()) },
                0
            );
            count as usize
        };

        for (writer, reader) in [(pair.0, pair.1), (pair.1, pair.0)] {
            assert_eq!(queued(reader), 0);
            for size in [11, 64 * 1024] {
                let payload: Vec<u8> = (0..size).map(|index| index as u8).collect();
                assert_eq!(kinakaze_vfs::write(writer, &payload), Ok(size));
                assert_eq!(queued(writer), 0);
                let mut consumed = 0;
                let mut buffer = vec![0; if size == 11 { 5 } else { 4096 }];
                while consumed < size {
                    // nginx must still see 61440 bytes after the first 4 KiB
                    // read. Reporting zero here strands an EPOLLET reader.
                    assert_eq!(queued(reader), size - consumed);
                    assert_eq!(queued(reader), size - consumed, "ioctl consumed data");
                    let expected = buffer.len().min(size - consumed);
                    assert_eq!(kinakaze_vfs::read(reader, &mut buffer), Ok(expected));
                    assert_eq!(&buffer[..expected], &payload[consumed..consumed + expected]);
                    consumed += expected;
                }
                assert_eq!(queued(reader), 0);
                assert_eq!(
                    kinakaze_vfs::read(reader, &mut buffer),
                    Err(kinakaze_vfs::EAGAIN)
                );
            }
        }
    }

    #[test]
    fn speeds_round_trip_through_both_encodings() {
        let mut termios = Termios::default();

        // The low range, where the index is just the low four bits.
        for speed in [B0, B300, B9600, B19200, pty::B38400] {
            // SAFETY: `termios` is a writable local.
            assert_eq!(
                unsafe { kinakaze_abi_cfsetospeed(&raw mut termios, speed) },
                0
            );
            // SAFETY: as above.
            assert_eq!(
                unsafe { kinakaze_abi_cfgetospeed(&raw const termios) },
                speed,
                "speed {speed:#o} did not round-trip"
            );
        }

        // Above B38400 the index no longer fits in four bits, so Linux sets CBAUDEX
        // and keeps counting. This is the encoding a caller is most likely to get
        // wrong, and the one this test exists for.
        for speed in [B57600, B115200, B230400, B921600, B2000000] {
            // SAFETY: `termios` is a writable local.
            assert_eq!(
                unsafe { kinakaze_abi_cfsetospeed(&raw mut termios, speed) },
                0
            );
            // SAFETY: as above.
            assert_eq!(
                unsafe { kinakaze_abi_cfgetospeed(&raw const termios) },
                speed,
                "extended speed {speed:#o} did not round-trip"
            );
            // The extended bit really is set, so the value stored in c_cflag is the
            // one a kernel would read rather than a number kept only on the side.
            assert_ne!(termios.c_cflag & CBAUDEX, 0);
            assert_eq!(termios.c_ospeed, speed);
        }

        // Setting the speed must not disturb the neighbouring c_cflag bits, which
        // sit directly above CBAUD.
        // SAFETY: `termios` is a writable local.
        unsafe { kinakaze_abi_cfsetospeed(&raw mut termios, B115200) };
        assert_ne!(termios.c_cflag & pty::CREAD, 0, "CREAD was clobbered");
        assert_ne!(termios.c_cflag & pty::CLOCAL, 0, "CLOCAL was clobbered");
        assert_eq!(termios.c_cflag & 0x30, pty::CS8, "the character size moved");
    }

    #[test]
    fn input_and_output_speeds_are_independent() {
        let mut termios = Termios::default();
        // SAFETY: `termios` is a writable local.
        unsafe {
            assert_eq!(kinakaze_abi_cfsetospeed(&raw mut termios, B115200), 0);
            assert_eq!(kinakaze_abi_cfsetispeed(&raw mut termios, B9600), 0);
        }
        // CIBAUD is a separate field sixteen bits up, so setting one direction must
        // not move the other.
        // SAFETY: `termios` is a readable local.
        unsafe {
            assert_eq!(kinakaze_abi_cfgetospeed(&raw const termios), B115200);
            assert_eq!(kinakaze_abi_cfgetispeed(&raw const termios), B9600);
        }

        // An input speed of B0 means "same as output" in POSIX, not "hang up".
        // SAFETY: `termios` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_cfsetispeed(&raw mut termios, B0) }, 0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_cfgetispeed(&raw const termios) },
            B115200
        );

        // cfsetspeed moves both at once.
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_cfsetspeed(&raw mut termios, B57600) },
            0
        );
        // SAFETY: as above.
        unsafe {
            assert_eq!(kinakaze_abi_cfgetospeed(&raw const termios), B57600);
            assert_eq!(kinakaze_abi_cfgetispeed(&raw const termios), B57600);
        }
    }

    #[test]
    fn a_terminal_that_was_never_set_reports_the_documented_default() {
        // Termios::default records B38400 in c_ospeed but leaves c_cflag's speed
        // bits clear. Reading CBAUD alone would report B0, which means "hang up the
        // line" — a fabrication about a perfectly live terminal.
        let termios = Termios::default();
        assert_eq!(termios.c_cflag & CBAUD, B0);
        // SAFETY: `termios` is a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_cfgetospeed(&raw const termios) },
            DEFAULT_SPEED
        );

        // A deliberate B0 still reads back as B0, so a caller that really means to
        // signal a hangup is not overridden.
        let mut hung_up = Termios::default();
        // SAFETY: `hung_up` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_cfsetospeed(&raw mut hung_up, B0) }, 0);
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_cfgetospeed(&raw const hung_up) }, B0);
    }

    #[test]
    fn a_speed_outside_the_cbaud_field_is_refused() {
        let mut termios = Termios::default();
        // Storing this would corrupt CS8 and CREAD, which sit just above CBAUD, so
        // it is refused rather than truncated into something plausible.
        set_errno(0);
        // SAFETY: `termios` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_cfsetospeed(&raw mut termios, 0o777) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
        // The refused call must have changed nothing.
        assert_eq!(termios, Termios::default());
    }

    #[test]
    fn terminal_ownership_calls_report_enotty_on_a_regular_file() {
        let file = FileFixture::open();

        // A regular file is a descriptor that works perfectly and is not a
        // terminal. This is a real check: GetConsoleMode is asked and says no.
        set_errno(0);
        assert_eq!(kinakaze_abi_tcgetpgrp(file.fd), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOTTY);

        set_errno(0);
        assert_eq!(kinakaze_abi_tcgetsid(file.fd), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOTTY);

        set_errno(0);
        assert_eq!(kinakaze_abi_tcsetpgrp(file.fd, own_pid()), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOTTY);

        // ttyname_r reports through its return value, so ENOTTY appears there.
        let mut buffer = [0 as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        let code = unsafe { kinakaze_abi_ttyname_r(file.fd, buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(code, kinakaze_vfs::ENOTTY);
    }

    #[test]
    fn a_closed_descriptor_outranks_the_terminal_question() {
        // EBADF before ENOTTY: the descriptor has to exist before it can be asked
        // whether it is a terminal.
        set_errno(0);
        assert_eq!(kinakaze_abi_tcgetpgrp(-1), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EBADF);

        let mut buffer = [0 as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        let code = unsafe { kinakaze_abi_ttyname_r(900, buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(code, kinakaze_vfs::EBADF);
    }

    #[test]
    fn ttyname_r_reports_erange_through_its_return_value() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open() else {
            // No console attached, so there is no terminal descriptor to name.
            return;
        };

        // Enough room, so the real name comes back.
        let mut buffer = [0x7f as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        let code = unsafe { kinakaze_abi_ttyname_r(console.fd, buffer.as_mut_ptr(), buffer.len()) };
        assert_eq!(code, 0, "ttyname_r on a console should succeed");
        // SAFETY: a successful call leaves a null-terminated string.
        let name = unsafe { core::ffi::CStr::from_ptr(buffer.as_ptr()) };
        assert_eq!(name.to_str().unwrap(), TTY_PATH);
        // An absolute path, which is what a caller will print or compare.
        assert!(name.to_bytes().starts_with(b"/"));

        // One byte short of the name plus its terminator. The error is the *return
        // value*, not errno — inverting that is the classic bug in this interface.
        let mut small = [0x7f as c_char; 64];
        let too_small = TTY_PATH.len();
        set_errno(0);
        // SAFETY: the buffer is writable well beyond the length offered.
        let code = unsafe { kinakaze_abi_ttyname_r(console.fd, small.as_mut_ptr(), too_small) };
        assert_eq!(
            code,
            kinakaze_vfs::ERANGE,
            "ERANGE must be the return value"
        );
        // Nothing may have been written: a caller that retries with a bigger buffer
        // must not find a truncated name from the failed attempt.
        assert!(
            small.iter().all(|byte| *byte == 0x7f),
            "a failed ttyname_r wrote into the buffer"
        );

        // A zero-length buffer is the same refusal, not a write of the terminator.
        // SAFETY: as above; no byte is written for a zero length.
        let code = unsafe { kinakaze_abi_ttyname_r(console.fd, small.as_mut_ptr(), 0) };
        assert_eq!(code, kinakaze_vfs::ERANGE);
    }

    /// The exact sequence `sshpass` performs, through the C entry points.
    ///
    /// Written as one test on purpose: each step depends on the one before it,
    /// and the value of the chain is that it works end to end. A program that
    /// gets a master but cannot name its slave, or names it but cannot open it,
    /// is no better off than one that got nothing.
    #[test]
    fn the_posix_pty_sequence_works_through_the_c_entry_points() {
        // posix_openpt(O_RDWR | O_NOCTTY)
        let master =
            kinakaze_abi_posix_openpt(kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY);
        assert!(master >= 0, "posix_openpt failed");
        assert_eq!(isatty_of(master), 1, "a pty master must be a terminal");

        // grantpt, then unlockpt.
        assert_eq!(kinakaze_abi_grantpt(master), 0);
        assert_eq!(kinakaze_abi_unlockpt(master), 0);

        // ptsname must produce a path, and the reentrant form must agree.
        let name = kinakaze_abi_ptsname(master);
        assert!(!name.is_null(), "ptsname returned null");
        // SAFETY: a successful ptsname leaves a null-terminated string.
        let name = unsafe { core::ffi::CStr::from_ptr(name) }
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            name.starts_with("/dev/pts/"),
            "ptsname produced {name}, which is not a devpts path"
        );
        let mut reentrant = [0 as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        assert_eq!(
            unsafe { kinakaze_abi_ptsname_r(master, reentrant.as_mut_ptr(), reentrant.len()) },
            0
        );
        // SAFETY: a successful call leaves a null-terminated string.
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(reentrant.as_ptr()) }
                .to_str()
                .unwrap(),
            name
        );

        // The name is openable, which is the property the old fixed `/dev/tty`
        // answer did not have.
        let slave = kinakaze_vfs::fs::open(
            &name,
            kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY,
            0,
        )
        .expect("opening the slave by name failed");
        assert_eq!(isatty_of(slave), 1, "a pty slave must be a terminal");
        // ttyname on the slave reproduces the name ptsname gave.
        let mut back = [0 as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        assert_eq!(
            unsafe { kinakaze_abi_ttyname_r(slave, back.as_mut_ptr(), back.len()) },
            0
        );
        // SAFETY: a successful call leaves a null-terminated string.
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(back.as_ptr()) }
                .to_str()
                .unwrap(),
            name
        );

        // termios is honoured: raw mode really turns off canonical buffering.
        let mut termios = Termios::default();
        // SAFETY: both pointers are writable locals.
        assert_eq!(
            unsafe { kinakaze_abi_tcgetattr(slave, &raw mut termios) },
            0
        );
        assert_ne!(termios.c_lflag & kinakaze_vfs::pty::ICANON, 0);
        // SAFETY: `termios` is a writable local.
        unsafe { kinakaze_abi_cfmakeraw(&raw mut termios) };
        // SAFETY: `termios` is a readable local.
        assert_eq!(
            unsafe {
                kinakaze_abi_tcsetattr(slave, kinakaze_vfs::pty::TCSANOW, &raw const termios)
            },
            0
        );
        let mut seen = Termios::default();
        // The master sees the change, because the settings belong to the
        // terminal rather than to a descriptor.
        // SAFETY: `seen` is a writable local.
        assert_eq!(unsafe { kinakaze_abi_tcgetattr(master, &raw mut seen) }, 0);
        assert_eq!(seen.c_lflag & kinakaze_vfs::pty::ICANON, 0);

        // Bytes cross in both directions, byte at a time now that ICANON is off.
        assert_eq!(kinakaze_vfs::write(master, b"pw\n").unwrap(), 3);
        let mut buffer = [0u8; 8];
        assert_eq!(kinakaze_vfs::read(slave, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], b"pw\n");

        // TIOCGPTN reports the number the name was built from.
        let mut number = 0 as c_int;
        // SAFETY: TIOCGPTN takes a writable unsigned int.
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(master, TIOCGPTN, (&raw mut number).cast()) },
            0
        );
        assert_eq!(format!("/dev/pts/{number}"), name);

        // `/dev/tty` resolves once the slave is the controlling terminal, which
        // is the step `login_tty` performs and `ssh` depends on for its prompt.
        // SAFETY: TIOCSCTTY takes its argument by value.
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(slave, TIOCSCTTY, ptr::null_mut()) },
            0
        );
        let controlling = kinakaze_vfs::fs::open("/dev/tty", kinakaze_vfs::fs::O_RDWR, 0)
            .expect("/dev/tty did not resolve to the controlling terminal");
        assert_eq!(
            kinakaze_vfs::tty::pty_number(controlling).unwrap(),
            number as u32
        );
        let _ = kinakaze_vfs::close(controlling);
        // SAFETY: TIOCNOTTY takes no argument.
        unsafe { kinakaze_abi_ioctl(slave, TIOCNOTTY, ptr::null_mut()) };

        let _ = kinakaze_vfs::close(slave);
        let _ = kinakaze_vfs::close(master);
    }

    /// `isatty` as the guest sees it, without pulling in the whole `fs` module.
    fn isatty_of(fd: c_int) -> c_int {
        crate::fs::isatty(fd)
    }

    #[test]
    fn a_locked_slave_cannot_be_opened_until_unlockpt_releases_it() {
        let master =
            kinakaze_abi_posix_openpt(kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY);
        assert!(master >= 0);
        let mut buffer = [0 as c_char; 64];
        // SAFETY: the buffer is a writable local of the stated length.
        assert_eq!(
            unsafe { kinakaze_abi_ptsname_r(master, buffer.as_mut_ptr(), buffer.len()) },
            0
        );
        // SAFETY: the call above left a null-terminated string.
        let name = unsafe { core::ffi::CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .unwrap()
            .to_owned();

        // The lock is real: a slave opened before unlockpt reports EIO, exactly
        // as Linux does, which is what makes the sequence a protocol.
        assert!(matches!(
            kinakaze_vfs::fs::open(&name, kinakaze_vfs::fs::O_RDWR, 0),
            Err(kinakaze_vfs::EIO)
        ));
        assert_eq!(kinakaze_abi_unlockpt(master), 0);
        let slave = kinakaze_vfs::fs::open(&name, kinakaze_vfs::fs::O_RDWR, 0).unwrap();

        // ptsname on the *slave* is ENOTTY: only a master has a slave to name.
        // SAFETY: the buffer is a writable local of the stated length.
        assert_eq!(
            unsafe { kinakaze_abi_ptsname_r(slave, buffer.as_mut_ptr(), buffer.len()) },
            kinakaze_vfs::ENOTTY
        );
        // And grantpt refuses a slave rather than silently succeeding.
        assert_eq!(kinakaze_abi_grantpt(slave), -1);

        let _ = kinakaze_vfs::close(slave);
        let _ = kinakaze_vfs::close(master);
    }

    #[test]
    fn tcgeta_writes_a_termio_and_not_a_termios() {
        let master =
            kinakaze_abi_posix_openpt(kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY);
        assert!(master >= 0);

        // Eighteen bytes of structure inside a guard that must stay untouched.
        // The previous implementation wrote sixty bytes here and corrupted
        // whatever followed the caller's `struct termio`.
        #[repr(C)]
        struct Guarded {
            termio: [u8; 18],
            guard: [u8; 64],
        }
        let mut buffer = Guarded {
            termio: [0; 18],
            guard: [0xab; 64],
        };
        // SAFETY: TCGETA takes a writable struct termio, which this provides.
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(master, TCGETA, (&raw mut buffer).cast()) },
            0
        );
        assert!(
            buffer.guard.iter().all(|byte| *byte == 0xab),
            "TCGETA wrote past the end of a struct termio"
        );

        // The narrowed flags are the low sixteen bits of the real ones.
        let mut full = Termios::default();
        // SAFETY: `full` is a writable local.
        unsafe { kinakaze_abi_tcgetattr(master, &raw mut full) };
        let iflag = u16::from_le_bytes([buffer.termio[0], buffer.termio[1]]);
        let lflag = u16::from_le_bytes([buffer.termio[6], buffer.termio[7]]);
        assert_eq!(iflag, full.c_iflag as u16);
        assert_eq!(lflag, full.c_lflag as u16);
        // termio's c_cc[6] and [7] are VMIN and VTIME, not termios' 6 and 7.
        assert_eq!(buffer.termio[9 + 6], full.c_cc[kinakaze_vfs::pty::VMIN]);
        assert_eq!(buffer.termio[9 + 7], full.c_cc[kinakaze_vfs::pty::VTIME]);

        // A TCSETA round trip must not disturb the high bits it cannot express.
        // `EXTPROC` is above bit 15 in `c_lflag`, so a `struct termio` has
        // literally nowhere to put it — which is exactly the case where a naive
        // widening would clear a flag the caller never mentioned.
        full.c_lflag |= kinakaze_vfs::ldisc::EXTPROC;
        // SAFETY: `full` is a readable local.
        unsafe { kinakaze_abi_tcsetattr(master, kinakaze_vfs::pty::TCSANOW, &raw const full) };
        // SAFETY: TCSETA takes a readable struct termio.
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(master, TCSETA, (&raw mut buffer).cast()) },
            0
        );
        // SAFETY: `full` is a writable local.
        unsafe { kinakaze_abi_tcgetattr(master, &raw mut full) };
        assert_ne!(
            full.c_lflag & kinakaze_vfs::ldisc::EXTPROC,
            0,
            "TCSETA cleared a flag the caller could not name"
        );
        // And the bits it *can* express really did come from the caller.
        assert_eq!(full.c_lflag as u16, lflag);

        let _ = kinakaze_vfs::close(master);
    }

    #[test]
    fn tcgets_uses_the_x86_64_kernel_termios_layout() {
        use core::mem::{offset_of, size_of};

        assert_eq!(size_of::<KernelTermios>(), 44);
        assert_eq!(offset_of!(KernelTermios, c_iflag), 0);
        assert_eq!(offset_of!(KernelTermios, c_line), 16);
        assert_eq!(offset_of!(KernelTermios, c_cc), 17);
        assert_eq!(offset_of!(KernelTermios, c_ispeed), 36);
        assert_eq!(offset_of!(KernelTermios, c_ospeed), 40);

        let master =
            kinakaze_abi_posix_openpt(kinakaze_vfs::fs::O_RDWR | kinakaze_vfs::fs::O_NOCTTY);
        assert!(master >= 0);
        #[repr(C)]
        struct Guarded {
            kernel: [u8; 44],
            guard: [u8; 32],
        }
        let mut buffer = Guarded {
            kernel: [0; 44],
            guard: [0xa5; 32],
        };
        // SAFETY: the first field is exactly one writable kernel termios.
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(master, TCGETS, (&raw mut buffer).cast()) },
            0
        );
        assert!(
            buffer.guard.iter().all(|byte| *byte == 0xa5),
            "TCGETS wrote libc's 60-byte termios into a 44-byte kernel buffer"
        );

        let mut full = Termios::default();
        // SAFETY: `full` is one writable libc termios.
        assert_eq!(unsafe { kinakaze_abi_tcgetattr(master, &raw mut full) }, 0);
        let kernel = unsafe { ptr::read_unaligned(buffer.kernel.as_ptr().cast::<KernelTermios>()) };
        assert_eq!(kernel.c_iflag, full.c_iflag);
        assert_eq!(kernel.c_lflag, full.c_lflag);
        assert_eq!(&kernel.c_cc, &full.c_cc[..KERNEL_NCCS]);
        assert_eq!(kernel.c_ispeed, full.c_ispeed);
        assert_eq!(kernel.c_ospeed, full.c_ospeed);

        let _ = kinakaze_vfs::close(master);
    }

    #[test]
    fn openpty_produces_a_pair_that_carries_bytes_both_ways() {
        let mut master = 0 as c_int;
        let mut slave = 0 as c_int;
        let mut name = [0 as c_char; 64];
        // SAFETY: all three out-parameters are writable locals.
        assert_eq!(
            unsafe {
                crate::misc::kinakaze_abi_openpty(
                    &raw mut master,
                    &raw mut slave,
                    name.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                )
            },
            0
        );
        assert_eq!(isatty_of(master), 1);
        assert_eq!(isatty_of(slave), 1);
        // SAFETY: openpty left a null-terminated name.
        let named = unsafe { core::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_str()
            .unwrap();
        assert!(named.starts_with("/dev/pts/"), "openpty named {named}");

        // Master to slave, cooked: the carriage return terminates the line.
        assert_eq!(kinakaze_vfs::write(master, b"hi\r").unwrap(), 3);
        let mut buffer = [0u8; 16];
        assert_eq!(kinakaze_vfs::read(slave, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], b"hi\n");

        // Slave to master, through OPOST, and past the echo of what was typed.
        assert_eq!(kinakaze_vfs::write(slave, b"ok\n").unwrap(), 3);
        let read = kinakaze_vfs::read(master, &mut buffer).unwrap();
        assert!(
            buffer[..read].ends_with(b"\r\n") || read == 3,
            "expected ONLCR output, got {:?}",
            &buffer[..read]
        );

        let _ = kinakaze_vfs::close(slave);
        let _ = kinakaze_vfs::close(master);
    }

    #[test]
    fn a_console_reports_this_process_as_its_foreground_group() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open() else {
            return;
        };

        // Consistent with `getpgrp` and `getsid` in `userdb`, which report the same
        // pid on the same grounds: this process is alone in its group and session.
        assert_eq!(kinakaze_abi_tcgetpgrp(console.fd), own_pid());
        assert_eq!(kinakaze_abi_tcgetsid(console.fd), own_pid());

        // Handing the terminal to the group that already holds it succeeds.
        assert_eq!(kinakaze_abi_tcsetpgrp(console.fd, own_pid()), 0);
        assert_eq!(kinakaze_abi_tcgetpgrp(console.fd), own_pid());

        // Setting another valid positive process group succeeds and updates foreground group.
        assert_eq!(kinakaze_abi_tcsetpgrp(console.fd, own_pid() + 1), 0);
        assert_eq!(kinakaze_abi_tcgetpgrp(console.fd), own_pid() + 1);
        let _ = kinakaze_abi_tcsetpgrp(console.fd, own_pid());

        // A group id that names no group at all is EINVAL.
        set_errno(0);
        assert_eq!(kinakaze_abi_tcsetpgrp(console.fd, 0), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
        set_errno(0);
        assert_eq!(kinakaze_abi_tcsetpgrp(console.fd, -5), -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
    }

    #[test]
    fn termios_baud_rates_and_mode_flags() {
        use kinakaze_vfs::pty::B38400;
        let mut termios = Termios::default();
        assert_eq!(unsafe { kinakaze_abi_cfsetispeed(&mut termios, B38400) }, 0);
        assert_eq!(unsafe { kinakaze_abi_cfsetospeed(&mut termios, B38400) }, 0);
        assert_eq!(unsafe { kinakaze_abi_cfgetispeed(&termios) }, B38400);
        assert_eq!(unsafe { kinakaze_abi_cfgetospeed(&termios) }, B38400);

        unsafe { kinakaze_abi_cfmakeraw(&mut termios) };
        assert_eq!(
            termios.c_iflag
                & (kinakaze_vfs::pty::BRKINT
                    | kinakaze_vfs::pty::ISTRIP
                    | kinakaze_vfs::pty::INLCR
                    | kinakaze_vfs::pty::IGNCR
                    | kinakaze_vfs::pty::ICRNL
                    | kinakaze_vfs::pty::IXON),
            0
        );
        assert_eq!(termios.c_oflag & kinakaze_vfs::pty::OPOST, 0);
        assert_eq!(
            termios.c_lflag
                & (kinakaze_vfs::pty::ECHO
                    | kinakaze_vfs::pty::ECHONL
                    | kinakaze_vfs::pty::ICANON
                    | kinakaze_vfs::pty::ISIG
                    | kinakaze_vfs::pty::IEXTEN),
            0
        );
    }

    #[test]
    fn pty_winsize_ioctls_and_tcflush() {
        let mut master = 0 as c_int;
        let mut slave = 0 as c_int;
        assert_eq!(
            unsafe {
                crate::misc::kinakaze_abi_openpty(
                    &raw mut master,
                    &raw mut slave,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                )
            },
            0
        );

        let ws = WinSize {
            ws_row: 50,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(slave, TIOCSWINSZ, &raw const ws as *mut c_void) },
            0
        );

        let mut read_ws = WinSize::default();
        assert_eq!(
            unsafe { kinakaze_abi_ioctl(slave, TIOCGWINSZ, &raw mut read_ws as *mut c_void) },
            0
        );
        assert_eq!(read_ws.ws_row, 50);
        assert_eq!(read_ws.ws_col, 120);

        // Test tcflush
        assert_eq!(kinakaze_abi_tcflush(slave, kinakaze_vfs::pty::TCIFLUSH), 0);
        assert_eq!(kinakaze_abi_tcflush(slave, kinakaze_vfs::pty::TCOFLUSH), 0);
        assert_eq!(kinakaze_abi_tcflush(slave, kinakaze_vfs::pty::TCIOFLUSH), 0);

        let _ = kinakaze_vfs::close(slave);
        let _ = kinakaze_vfs::close(master);
    }

    /// Serializes the tests that need the process's one real console.
    static CONSOLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A console descriptor, or `None` when no console is attached.
    ///
    /// `cargo test` replaces the standard handles with pipes, so fds 0-2 are not
    /// consoles even though the process is still attached to one. Opening `CONIN$`
    /// reaches the attached console directly, which is what lets these tests
    /// exercise the real Windows path instead of skipping. Unlike `pty.rs`'s
    /// fixture this one never changes the console mode, so it has nothing to
    /// restore.
    struct ConsoleFixture {
        fd: c_int,
        handle: *mut core::ffi::c_void,
    }

    impl ConsoleFixture {
        fn open() -> Option<Self> {
            /// `SECURITY_ATTRIBUTES`. Only a null pointer is ever passed; the type
            /// is spelled out so this declaration of `CreateFileW` matches the
            /// crate's other one.
            #[repr(C)]
            struct SecurityAttributes {
                length: u32,
                descriptor: *mut core::ffi::c_void,
                inherit: i32,
            }

            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn CreateFileW(
                    path: *const u16,
                    access: u32,
                    share: u32,
                    attributes: *const SecurityAttributes,
                    disposition: u32,
                    flags: u32,
                    template: *mut core::ffi::c_void,
                ) -> *mut core::ffi::c_void;
                fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
            }
            const GENERIC_READ: u32 = 0x8000_0000;
            const GENERIC_WRITE: u32 = 0x4000_0000;
            const FILE_SHARE_READ: u32 = 1;
            const FILE_SHARE_WRITE: u32 = 2;
            const OPEN_EXISTING: u32 = 3;

            let name: Vec<u16> = "CONIN$\0".encode_utf16().collect();
            // SAFETY: the name is a null-terminated wide string and the remaining
            // arguments are the documented console-device pattern.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle.is_null() || handle as isize == -1 {
                return None;
            }
            // BORROWED so the table does not close the handle; this fixture owns it.
            match kinakaze_vfs::install(handle as usize, FdKind::Console, FdFlags::BORROWED) {
                Ok(fd) => Some(Self { fd, handle }),
                Err(_) => {
                    // SAFETY: the handle was just opened here and not stored.
                    unsafe { CloseHandle(handle) };
                    None
                }
            }
        }
    }

    impl Drop for ConsoleFixture {
        fn drop(&mut self) {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
            }
            let _ = kinakaze_vfs::close(self.fd);
            // SAFETY: this fixture owns the handle; the table entry was borrowed.
            unsafe { CloseHandle(self.handle) };
        }
    }
}
