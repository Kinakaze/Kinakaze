//! Pseudo-terminal layer: Linux terminal semantics over Windows ConPTY.
//!
//! Two separate problems live here. The first is *owning a terminal*:
//! [`open_pty`] wraps `CreatePseudoConsole` so hosted code can run a child under
//! a real VT100/ANSI-speaking pseudoconsole, with the master side exposed as two
//! ordinary pipe handles the descriptor table can adopt.
//!
//! The second is *describing a terminal*: [`tcgetattr`] and [`tcsetattr`] speak
//! the Linux `struct termios` ABI, but Windows console modes only cover a
//! fraction of it. Rather than invent behaviour, the translation applies the bits
//! Windows genuinely implements and keeps the remainder in a per-descriptor cache
//! so a get-after-set round-trips. [`enforcement`] documents exactly where the
//! line falls, because a program that believes `VTIME` is honoured when it is not
//! will misbehave in ways that are very hard to trace back to here.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, ClosePseudoConsole, CreatePseudoConsole,
    DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_LINE_INPUT,
    ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_QUICK_EDIT_MODE,
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT,
    FlushConsoleInputBuffer, GetConsoleMode, GetConsoleScreenBufferInfo, HPCON,
    ResizePseudoConsole, SMALL_RECT, SetConsoleMode, SetConsoleScreenBufferSize,
    SetConsoleWindowInfo,
};
use windows_sys::Win32::System::Pipes::CreatePipe;

use crate::{EINVAL, EIO, ENOTTY, FdKind};

/// Linux x86_64 `struct termios`.
///
/// Field order, types and padding are ABI: guest code memcpys this structure
/// through `ioctl`/`tcgetattr` and indexes `c_cc` by the `V*` constants, so the
/// layout is verified by a test rather than trusted.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

// `c_lflag` bits.
pub const ISIG: u32 = 0x0000_0001;
pub const ICANON: u32 = 0x0000_0002;
pub const ECHO: u32 = 0x0000_0008;
pub const ECHOE: u32 = 0x0000_0010;
pub const ECHOK: u32 = 0x0000_0020;
pub const ECHONL: u32 = 0x0000_0040;
pub const NOFLSH: u32 = 0x0000_0080;
pub const TOSTOP: u32 = 0x0000_0100;
pub const IEXTEN: u32 = 0x0000_8000;

// `c_iflag` bits.
pub const BRKINT: u32 = 0x0000_0002;
pub const ISTRIP: u32 = 0x0000_0020;
pub const INLCR: u32 = 0x0000_0040;
pub const IGNCR: u32 = 0x0000_0080;
pub const ICRNL: u32 = 0x0000_0100;
pub const IXON: u32 = 0x0000_0400;

// `c_oflag` bits.
pub const OPOST: u32 = 0x0000_0001;
pub const ONLCR: u32 = 0x0000_0004;

// `c_cflag` bits.
pub const CS8: u32 = 0x0000_0030;
pub const CREAD: u32 = 0x0000_0080;
pub const CLOCAL: u32 = 0x0000_0800;

// `c_cc` indices.
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSTART: usize = 8;
pub const VSTOP: usize = 9;
pub const VSUSP: usize = 10;

// `tcsetattr` actions.
pub const TCSANOW: i32 = 0;
pub const TCSADRAIN: i32 = 1;
pub const TCSAFLUSH: i32 = 2;

// `tcflush` queue selectors.
pub const TCIFLUSH: i32 = 0;
pub const TCOFLUSH: i32 = 1;
pub const TCIOFLUSH: i32 = 2;

/// `B38400`, the conventional line speed a pty reports.
///
/// Real serial rates are meaningless for a pseudo-terminal; Linux ptys report
/// this value and some programs divide by it, so zero would be a poor choice.
pub const B38400: u32 = 0x0000_000f;

impl Default for Termios {
    /// Returns the cooked-mode settings a freshly opened Linux tty reports.
    ///
    /// A descriptor with no cached entry has to answer *something*, and answering
    /// with the state a login shell would see means a program that inspects the
    /// terminal before touching it draws the same conclusions it would on Linux.
    fn default() -> Self {
        let mut c_cc = [0u8; 32];
        c_cc[VINTR] = 0x03; // ^C
        c_cc[VQUIT] = 0x1c; // ^\
        c_cc[VERASE] = 0x7f; // DEL
        c_cc[VKILL] = 0x15; // ^U
        c_cc[VEOF] = 0x04; // ^D
        c_cc[VSTART] = 0x11; // ^Q
        c_cc[VSTOP] = 0x13; // ^S
        c_cc[VSUSP] = 0x1a; // ^Z
        // Canonical mode reads by line, so the byte-granularity controls are
        // inactive and Linux leaves VMIN at 1 with no timeout.
        c_cc[VMIN] = 1;
        c_cc[VTIME] = 0;
        Self {
            c_iflag: ICRNL | IXON | BRKINT,
            c_oflag: OPOST | ONLCR,
            c_cflag: CS8 | CREAD | CLOCAL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
            c_line: 0,
            c_cc,
            c_ispeed: B38400,
            c_ospeed: B38400,
        }
    }
}

/// Linux `struct winsize`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

/// What the Windows console layer actually enforces.
///
/// Windows console modes are a much smaller vocabulary than `termios`. Exactly
/// three `termios` bits have a host counterpart that the kernel acts on:
///
/// | `termios` | Windows console mode | enforced by |
/// |---|---|---|
/// | `ECHO` (lflag) | `ENABLE_ECHO_INPUT` | conhost echoes typed input |
/// | `ICANON` (lflag) | `ENABLE_LINE_INPUT` | conhost buffers until Enter |
/// | `ISIG` (lflag) | `ENABLE_PROCESSED_INPUT` | conhost turns ^C into a control event |
/// | `OPOST` (oflag) | `ENABLE_PROCESSED_OUTPUT` | conhost interprets CR/LF/BS |
/// | `ONLCR` (oflag) | `DISABLE_NEWLINE_AUTO_RETURN` (inverted) | conhost supplies the CR |
///
/// Everything else round-trips out of [`REGISTRY`] and changes no host
/// behaviour. That specifically includes:
///
/// - `VMIN` / `VTIME`. Windows has no read-granularity or inter-byte timer, so
///   a read still returns whatever the pipe or console hands over. Code that
///   relies on `VTIME` for a timed read will block instead.
/// - `VINTR`, `VQUIT`, `VSUSP`, `VERASE`, `VKILL`, `VEOF`, `VSTART`, `VSTOP`.
///   conhost's control characters are fixed; remapping `VINTR` away from `^C`
///   is remembered but `^C` keeps generating the interrupt.
/// - `IXON` / `IXOFF` flow control, `ISTRIP`, `INLCR`, `IGNCR`, `BRKINT`.
/// - `ICRNL`. Console input in VT mode delivers CR and the translation to NL is
///   the caller's job; the bit is stored, not applied.
/// - `ECHOE`, `ECHOK`, `ECHONL`, `NOFLSH`, `TOSTOP`, `IEXTEN`. conhost's line
///   editor is not configurable at this granularity.
/// - The whole of `c_cflag` and both speeds. There is no line discipline to
///   configure and no baud rate to set.
///
/// `ENABLE_VIRTUAL_TERMINAL_PROCESSING` is forced on for output regardless of
/// `termios`, since the entire premise of this layer is that ANSI sequences
/// reach the terminal intact. Likewise `ENABLE_VIRTUAL_TERMINAL_INPUT` is
/// enabled whenever the caller leaves canonical mode, because a raw-mode reader
/// wants escape sequences for arrow keys rather than console key records.
///
/// One structural difference is worth stating outright: on Linux a pty *master*
/// is a character device and `tcgetattr` on it succeeds, whereas the master side
/// here is a pipe handle that `GetConsoleMode` rejects, so it reports `ENOTTY`.
/// Terminal attributes therefore have to be set on the slave side, which is the
/// console the child process is attached to. [`set_window_size`] is the exception,
/// since a resize is a property of the pseudoconsole rather than of a console
/// mode and is routed through the pty registry.
///
/// This module is intentionally empty: it exists to give the table above a
/// stable anchor in the generated documentation.
pub mod enforcement {}

/// Owns a Windows pseudoconsole and the master ends of its two pipes.
///
/// ConPTY is one-directional per pipe: the pseudoconsole *reads* keystrokes from
/// one pipe and *writes* rendered VT output to the other. The master side is
/// therefore two handles, not one, which is the main structural difference from a
/// Unix pty where both directions share a file descriptor.
pub struct Pty {
    console: HPCON,
    /// Master write end: bytes pushed here arrive as terminal input.
    input: HANDLE,
    /// Master read end: rendered VT100/ANSI output from the attached client.
    output: HANDLE,
    size: WinSize,
}

// The two pipe handles and the HPCON are process-wide kernel objects that Win32
// accepts from any thread; nothing in this type is thread-affine.
unsafe impl Send for Pty {}

impl Pty {
    /// Master write end, to be installed as the pty's input descriptor.
    ///
    /// Ownership stays with this [`Pty`]; use [`Pty::into_handles`] before
    /// handing the handle to anything that will close it.
    pub fn input_handle(&self) -> HANDLE {
        self.input
    }

    /// Master read end, carrying the client's VT output.
    pub fn output_handle(&self) -> HANDLE {
        self.output
    }

    /// The pseudoconsole itself, for `ProcThreadAttributePseudoConsole`.
    pub fn console(&self) -> HPCON {
        self.console
    }

    /// The size last requested, since a pipe cannot be queried for one.
    pub fn window_size(&self) -> WinSize {
        self.size
    }

    /// Resizes the pseudoconsole, which makes the client observe `SIGWINCH`.
    pub fn resize(&mut self, rows: u16, columns: u16) -> Result<(), i32> {
        let size = coord(rows, columns)?;
        // SAFETY: `self.console` is a live pseudoconsole owned by this value.
        let status = unsafe { ResizePseudoConsole(self.console, size) };
        if status < 0 {
            return Err(errno_from_hresult(status));
        }
        self.size = WinSize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        Ok(())
    }

    /// Surrenders ownership of all three handles to the caller.
    ///
    /// Returned as `(console, input, output)`. The caller becomes responsible
    /// for `ClosePseudoConsole` and both `CloseHandle` calls, in that order; see
    /// the [`Drop`] implementation for why the order matters. This is the path to
    /// use when the pipe handles are installed into the descriptor table, which
    /// would otherwise close them a second time.
    pub fn into_handles(self) -> (HPCON, HANDLE, HANDLE) {
        let parts = (self.console, self.input, self.output);
        // The pty stays in `PTY_MASTERS` on purpose: the handles are still open
        // and `set_window_size` must keep resolving them. The new owner calls
        // [`deregister`] when it finally closes them.
        std::mem::forget(self);
        parts
    }
}

/// Forgets a pseudoconsole taken over through [`Pty::into_handles`].
///
/// Only needed on that path; a dropped [`Pty`] deregisters itself.
pub fn deregister(console: HPCON) {
    if let Ok(mut registry) = pty_masters().lock() {
        registry.retain(|&(_, _, registered)| registered != console);
    }
}

impl Drop for Pty {
    /// Tears the pseudoconsole down before the pipes, which is load-bearing.
    ///
    /// `ClosePseudoConsole` does not return until the attached client has exited
    /// and conhost has flushed the last of its output. Two consequences follow:
    ///
    /// 1. Dropping a [`Pty`] whose client is still running *blocks* until that
    ///    client exits. Callers that need a bounded teardown must terminate the
    ///    client first.
    /// 2. Something must keep draining [`Pty::output_handle`] during the close,
    ///    or conhost's final write fills the pipe buffer and both sides wait on
    ///    each other forever. Closing the read end first does not help; it makes
    ///    conhost's write fail instead of blocking, which loses trailing output
    ///    and is why the pipes are closed *after* the pseudoconsole rather than
    ///    before.
    fn drop(&mut self) {
        deregister(self.console);
        // SAFETY: this type owns the pseudoconsole created in `open_pty`.
        unsafe { ClosePseudoConsole(self.console) };
        // SAFETY: both handles are the master ends this type owns; conhost holds
        // its own duplicates, which it released above.
        unsafe {
            CloseHandle(self.input);
            CloseHandle(self.output);
        }
    }
}

/// Creates a pseudo-terminal with the given window size.
///
/// The four pipe ends are split so that conhost keeps the slave side and this
/// process keeps the master side: conhost reads input from `input_read` and
/// writes output to `output_write`, both of which it duplicates internally, so
/// they are closed here as soon as `CreatePseudoConsole` returns. Leaving them
/// open would keep the pipes alive after conhost exits and turn a clean EOF on
/// the master read end into an indefinite block.
pub fn open_pty(rows: u16, columns: u16) -> Result<Pty, i32> {
    let size = coord(rows, columns)?;

    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = match pipe() {
        Ok(pair) => pair,
        Err(error) => {
            close_pair(input_read, input_write);
            return Err(error);
        }
    };

    let mut console: HPCON = 0;
    // SAFETY: both handles are live pipe ends of the direction ConPTY expects,
    // and `console` is a writable local for the out-parameter.
    let status =
        unsafe { CreatePseudoConsole(size, input_read, output_write, 0, &raw mut console) };

    // conhost duplicated whatever it needed, so this process is done with the
    // slave ends whether the call succeeded or not.
    // SAFETY: these two handles are owned here and not stored anywhere.
    unsafe {
        CloseHandle(input_read);
        CloseHandle(output_write);
    }

    if status < 0 {
        close_pair(output_read, input_write);
        return Err(errno_from_hresult(status));
    }

    // Registered before returning so a `TIOCSWINSZ` on either master descriptor
    // can find its way back to this pseudoconsole.
    if let Ok(mut registry) = pty_masters().lock() {
        registry.push((input_write as usize, output_read as usize, console));
    }

    Ok(Pty {
        console,
        input: input_write,
        output: output_read,
        size: WinSize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    })
}

/// Builds a `COORD` from a Linux window size, rejecting values a console cannot
/// hold.
///
/// A zero dimension is what a terminal reports when the size is *unknown*, and
/// handing it to `CreatePseudoConsole` yields an unusable console rather than an
/// error, so it is rejected here instead.
fn coord(rows: u16, columns: u16) -> Result<COORD, i32> {
    if rows == 0 || columns == 0 || rows > i16::MAX as u16 || columns > i16::MAX as u16 {
        return Err(EINVAL);
    }
    Ok(COORD {
        X: columns as i16,
        Y: rows as i16,
    })
}

/// Creates an anonymous pipe, returned as `(read, write)`.
fn pipe() -> Result<(HANDLE, HANDLE), i32> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    // SAFETY: both out-parameters are writable locals; a null attribute pointer
    // requests the default descriptor and non-inheritable handles, and a zero
    // size requests the system default buffer.
    let ok = unsafe { CreatePipe(&raw mut read, &raw mut write, std::ptr::null(), 0) };
    if ok == 0 {
        return Err(last_errno());
    }
    Ok((read, write))
}

/// Closes both ends of a pipe on an error path.
fn close_pair(first: HANDLE, second: HANDLE) {
    // SAFETY: called only with handles this module created and still owns.
    unsafe {
        CloseHandle(first);
        CloseHandle(second);
    }
}

/// Maps a failed `HRESULT` onto an errno.
///
/// The ConPTY entry points report Win32 failures wrapped as `HRESULT_FROM_WIN32`,
/// so unwrapping the low 16 bits recovers the original code and reuses the
/// crate's single error-translation table. Anything not of that shape is a COM
/// error with no Linux counterpart.
fn errno_from_hresult(status: i32) -> i32 {
    const FACILITY_WIN32: i32 = 7;
    if (status >> 16) & 0x1fff == FACILITY_WIN32 {
        crate::errno_from_win32((status & 0xffff) as u32)
    } else {
        EIO
    }
}

fn last_errno() -> i32 {
    // SAFETY: GetLastError has no preconditions.
    crate::errno_from_win32(unsafe { GetLastError() })
}

/// Cached `termios` state for descriptors, keyed by fd and table generation.
///
/// The generation is part of the key because a closed descriptor's slot is
/// reused: without it, a freshly opened terminal would inherit the raw-mode
/// settings of whatever previously held the same fd number.
static REGISTRY: OnceLock<Mutex<HashMap<(i32, u32), Termios>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<(i32, u32), Termios>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Which half of a console a handle refers to.
///
/// The console mode bits are two disjoint namespaces that share numeric values:
/// `ENABLE_ECHO_INPUT` and `ENABLE_VIRTUAL_TERMINAL_PROCESSING` are both `4`.
/// Writing input bits to an output handle therefore silently means something
/// else, so the direction has to be established before any mode is touched.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Direction {
    Input,
    Output,
}

/// A console handle together with the direction its mode bits belong to.
struct ConsoleFd {
    handle: HANDLE,
    direction: Direction,
    mode: u32,
    /// The table generation the handle was read at, forming half the cache key.
    ///
    /// Carried along rather than looked up a second time, so the cache lookup
    /// cannot land on a different generation than the handle came from.
    generation: u32,
}

/// Resolves a descriptor to a console handle, or reports `ENOTTY`.
///
/// Two checks are needed. The table's [`FdKind::Console`] classification comes
/// from `GetFileType`, which answers `FILE_TYPE_CHAR` for `NUL` and for printers
/// as well as for real consoles, so it is necessary but not sufficient.
/// `GetConsoleMode` succeeding is the actual test for "this is a console
/// device", and it is also the value the translation needs.
fn console_fd(fd: i32) -> Result<ConsoleFd, i32> {
    let entry = crate::get(fd)?;
    if entry.kind != FdKind::Console && entry.kind != FdKind::Unknown {
        return Err(ENOTTY);
    }
    let handle = entry.raw as HANDLE;
    let mut mode = 0u32;
    // SAFETY: the handle is live while the descriptor is installed, and `mode`
    // is a writable local.
    if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
        return Err(ENOTTY);
    }
    // Inspecting the mode bits cannot tell the halves apart, because raw mode
    // leaves the input word at almost zero and the two words overlap
    // numerically. Screen-buffer info exists only for an output handle, so
    // querying it is an unambiguous and non-destructive discriminator.
    let direction = if screen_buffer_info(handle).is_some() {
        Direction::Output
    } else {
        Direction::Input
    };
    Ok(ConsoleFd {
        handle,
        direction,
        mode,
        generation: entry.generation,
    })
}

/// Queries a handle's screen buffer, or `None` when it is not a console output.
fn screen_buffer_info(handle: HANDLE) -> Option<CONSOLE_SCREEN_BUFFER_INFO> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the handle is live and `info` is a writable local of the size the
    // call expects.
    if unsafe { GetConsoleScreenBufferInfo(handle, &raw mut info) } == 0 {
        None
    } else {
        Some(info)
    }
}

/// Reads the cached `termios`, overlaid with the modes Windows really holds.
///
/// The cache supplies the fields Windows cannot represent and the live console
/// mode supplies the ones it can, so an external change to the console (another
/// process, or conhost itself) is visible here rather than being masked by a
/// stale cache entry.
pub fn tcgetattr(fd: i32) -> Result<Termios, i32> {
    let console = console_fd(fd)?;
    let mut termios = registry()
        .lock()
        .map_err(|_| EIO)?
        .get(&(fd, console.generation))
        .copied()
        .unwrap_or_default();

    match console.direction {
        Direction::Input => {
            set_flag(
                &mut termios.c_lflag,
                ECHO,
                console.mode & ENABLE_ECHO_INPUT != 0,
            );
            set_flag(
                &mut termios.c_lflag,
                ICANON,
                console.mode & ENABLE_LINE_INPUT != 0,
            );
            set_flag(
                &mut termios.c_lflag,
                ISIG,
                console.mode & ENABLE_PROCESSED_INPUT != 0,
            );
        }
        Direction::Output => {
            set_flag(
                &mut termios.c_oflag,
                OPOST,
                console.mode & ENABLE_PROCESSED_OUTPUT != 0,
            );
            // Windows states the negative, so the sense is inverted: the newline
            // is auto-completed with a CR unless the disable bit is set.
            set_flag(
                &mut termios.c_oflag,
                ONLCR,
                console.mode & DISABLE_NEWLINE_AUTO_RETURN == 0,
            );
        }
    }
    Ok(termios)
}

/// Sets or clears `bits` in `flags`.
fn set_flag(flags: &mut u32, bits: u32, enabled: bool) {
    if enabled {
        *flags |= bits;
    } else {
        *flags &= !bits;
    }
}

/// Applies `requested` to the console and caches the whole structure.
///
/// The cache is written even for fields Windows ignores, which is what makes a
/// subsequent [`tcgetattr`] round-trip: a TUI that saves the terminal state,
/// switches to raw mode and restores on exit has to get its original structure
/// back byte for byte, or it will restore something it never saved.
pub fn tcsetattr(fd: i32, actions: i32, requested: &Termios) -> Result<(), i32> {
    if !matches!(actions, TCSANOW | TCSADRAIN | TCSAFLUSH) {
        return Err(EINVAL);
    }
    let console = console_fd(fd)?;

    // TCSADRAIN and TCSAFLUSH must not change the mode until pending output has
    // left, otherwise bytes written under the old settings get interpreted under
    // the new ones.
    if actions != TCSANOW {
        drain(console.handle);
    }
    if actions == TCSAFLUSH && console.direction == Direction::Input {
        // SAFETY: the handle is a live console input handle.
        unsafe { FlushConsoleInputBuffer(console.handle) };
    }

    let mode = match console.direction {
        Direction::Input => {
            let mut mode = console.mode;
            set_flag(&mut mode, ENABLE_ECHO_INPUT, requested.c_lflag & ECHO != 0);
            set_flag(
                &mut mode,
                ENABLE_LINE_INPUT,
                requested.c_lflag & ICANON != 0,
            );
            set_flag(
                &mut mode,
                ENABLE_PROCESSED_INPUT,
                requested.c_lflag & ISIG != 0,
            );
            // A reader that left canonical mode wants arrow keys as escape
            // sequences, which is what VT input delivers; in canonical mode
            // conhost's own line editor is doing the work and VT input would
            // bypass it.
            set_flag(
                &mut mode,
                ENABLE_VIRTUAL_TERMINAL_INPUT,
                requested.c_lflag & ICANON == 0,
            );
            // Always disable mouse/window input and quick edit so mouse moves don't spam SGR sequences into stdin
            mode &= !(ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT | ENABLE_QUICK_EDIT_MODE);
            mode |= ENABLE_EXTENDED_FLAGS;
            mode
        }
        Direction::Output => {
            let mut mode = console.mode;
            set_flag(
                &mut mode,
                ENABLE_PROCESSED_OUTPUT,
                requested.c_oflag & OPOST != 0,
            );
            set_flag(
                &mut mode,
                DISABLE_NEWLINE_AUTO_RETURN,
                requested.c_oflag & ONLCR == 0,
            );
            // Never negotiable: ANSI passthrough is the point of this layer, and
            // a program that clears OPOST is asking for *less* processing, not
            // for its escape sequences to be printed literally.
            mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING;
            mode
        }
    };

    // SAFETY: the handle is live and `mode` was derived from the same handle's
    // own mode word, so only defined bits for this direction are set.
    if unsafe { SetConsoleMode(console.handle, mode) } == 0 {
        return Err(last_errno());
    }
    registry()
        .lock()
        .map_err(|_| EIO)?
        .insert((fd, console.generation), *requested);
    Ok(())
}

/// Reports the terminal's visible size, as `TIOCGWINSZ` would.
///
/// The *window* rectangle is what a terminal reports, not `dwSize`: a Windows
/// console's screen buffer is usually far taller than the viewport (that is what
/// the scrollback is), and reporting the buffer height would tell a full-screen
/// program it has hundreds of rows it cannot draw on.
pub fn get_window_size(fd: i32) -> Result<WinSize, i32> {
    let console = console_fd(fd)?;
    // The size lives with the screen buffer, so an input handle has to consult
    // the console's output side; that is the same object a Unix pty reports for
    // either direction.
    let info = screen_buffer_info(console.handle)
        .or_else(|| screen_buffer_info(current_screen_buffer()?))
        .ok_or(ENOTTY)?;
    let window = info.srWindow;
    Ok(WinSize {
        // The rectangle is inclusive on both edges.
        ws_row: (window.Bottom - window.Top + 1).max(0) as u16,
        ws_col: (window.Right - window.Left + 1).max(0) as u16,
        // Character cells have no pixel size to report, which is what a Linux
        // terminal emulator says too unless it is a framebuffer console.
        ws_xpixel: 0,
        ws_ypixel: 0,
    })
}

/// Returns this process's console output handle, if it has one.
fn current_screen_buffer() -> Option<HANDLE> {
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    // SAFETY: STD_OUTPUT_HANDLE is one of the three documented selectors.
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if handle.is_null() || handle as isize == -1 {
        None
    } else {
        Some(handle)
    }
}

/// Resizes the terminal, as `TIOCSWINSZ` would.
///
/// Two cases, because a pty master and a real console are different objects. If
/// the descriptor is the master side of a pty this process opened, the resize
/// goes to `ResizePseudoConsole`, which is what makes the attached client see the
/// new size. If it is an actual console, the *window* is moved rather than the
/// screen buffer, matching what [`get_window_size`] reports; the buffer is only
/// grown when the requested window would not fit inside it, since a window can
/// never exceed its buffer.
pub fn set_window_size(fd: i32, size: &WinSize) -> Result<(), i32> {
    // Validated first so a nonsensical size is rejected identically on every
    // kind of descriptor.
    let requested = coord(size.ws_row, size.ws_col)?;
    let entry = crate::get(fd)?;

    if let Some(console) = pty_master_console(entry.raw as HANDLE) {
        // SAFETY: the registry only holds pseudoconsoles that are still open,
        // since `open_pty` registers and the teardown paths deregister.
        let status = unsafe { ResizePseudoConsole(console, requested) };
        return if status < 0 {
            Err(errno_from_hresult(status))
        } else {
            Ok(())
        };
    }

    let console = console_fd(fd)?;
    let handle = if screen_buffer_info(console.handle).is_some() {
        console.handle
    } else {
        current_screen_buffer().ok_or(ENOTTY)?
    };
    let info = screen_buffer_info(handle).ok_or(ENOTTY)?;

    if info.dwSize.X < requested.X || info.dwSize.Y < requested.Y {
        let buffer = COORD {
            X: info.dwSize.X.max(requested.X),
            Y: info.dwSize.Y.max(requested.Y),
        };
        // SAFETY: the handle is a live console output handle and the size is
        // clamped to the i16 range by `coord`.
        if unsafe { SetConsoleScreenBufferSize(handle, buffer) } == 0 {
            return Err(last_errno());
        }
    }

    // The window is anchored at its current origin so the resize does not also
    // scroll the view.
    let window = SMALL_RECT {
        Left: info.srWindow.Left,
        Top: info.srWindow.Top,
        Right: info.srWindow.Left + requested.X - 1,
        Bottom: info.srWindow.Top + requested.Y - 1,
    };
    // SAFETY: the handle is a live console output handle and the rectangle is a
    // readable local; `1` requests absolute rather than relative coordinates.
    if unsafe { SetConsoleWindowInfo(handle, 1, &raw const window) } == 0 {
        return Err(last_errno());
    }
    Ok(())
}

/// Pseudoconsoles opened by this process, keyed by their master pipe handles.
///
/// `TIOCSWINSZ` arrives with a descriptor, but the descriptor table stores the
/// master *pipe* handles for a pty, not the `HPCON` that `ResizePseudoConsole`
/// needs. This maps back from either master handle to the pseudoconsole so a
/// resize through the fd reaches the right object.
static PTY_MASTERS: OnceLock<Mutex<Vec<(usize, usize, HPCON)>>> = OnceLock::new();

fn pty_masters() -> &'static Mutex<Vec<(usize, usize, HPCON)>> {
    PTY_MASTERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Finds the pseudoconsole owning `handle`, if this process created it.
fn pty_master_console(handle: HANDLE) -> Option<HPCON> {
    let registry = pty_masters().lock().ok()?;
    let needle = handle as usize;
    registry
        .iter()
        .find(|&&(input, output, _)| input == needle || output == needle)
        .map(|&(_, _, console)| console)
}

/// Applies the standard `cfmakeraw` transformation.
///
/// This is the transformation every TUI applies at startup: no line buffering, no
/// echo, no signal generation from keystrokes, no input or output translation, and
/// a read that returns as soon as one byte is available.
pub fn make_raw(termios: &mut Termios) {
    termios.c_iflag &= !(BRKINT | ICRNL | INLCR | IGNCR | ISTRIP | IXON);
    termios.c_oflag &= !OPOST;
    termios.c_lflag &= !(ECHO | ECHONL | ICANON | IEXTEN | ISIG);
    // cfmakeraw also normalises the character size, since a raw reader wants all
    // eight bits of every byte.
    termios.c_cflag = (termios.c_cflag & !CSIZE) | CS8;
    termios.c_cc[VMIN] = 1;
    termios.c_cc[VTIME] = 0;
}

/// `CSIZE`, the character-size field `CS8` is one value of.
const CSIZE: u32 = 0x0000_0030;

/// Discards pending input, output, or both.
///
/// Only the input queue has a real counterpart: `FlushConsoleInputBuffer` drops
/// unread records. Console output is not buffered on this side of the boundary,
/// so there is nothing to discard for `TCOFLUSH`; it is reported as success
/// because the post-condition ("no pending output remains") already holds.
pub fn tcflush(fd: i32, queue: i32) -> Result<(), i32> {
    if !matches!(queue, TCIFLUSH | TCOFLUSH | TCIOFLUSH) {
        return Err(EINVAL);
    }
    let console = console_fd(fd)?;
    if queue != TCOFLUSH && console.direction == Direction::Input {
        // SAFETY: the handle is a live console input handle.
        if unsafe { FlushConsoleInputBuffer(console.handle) } == 0 {
            return Err(last_errno());
        }
    }
    Ok(())
}

/// Waits for pending output to be written.
pub fn tcdrain(fd: i32) -> Result<(), i32> {
    let console = console_fd(fd)?;
    drain(console.handle);
    Ok(())
}

/// Flushes a handle's write buffer, ignoring "nothing to flush".
///
/// `FlushFileBuffers` fails with `ERROR_INVALID_FUNCTION` on a console handle,
/// which is not a problem to report: it means the writes already went straight
/// through, so `tcdrain`'s guarantee is satisfied.
fn drain(handle: HANDLE) {
    // SAFETY: the handle is live for the duration of the call.
    unsafe { FlushFileBuffers(handle) };
}

/// Sends a break condition: a no-op.
///
/// A break is a physical line condition on a serial port. There is no serial
/// line under a ConPTY or a console window, so there is nothing to assert and
/// nothing for a receiver to observe. Returning success rather than `ENOSYS` is
/// the deliberate choice: real programs (`ssh`, terminal emulators) call this on
/// terminals that cannot break and treat a failure as fatal, whereas the
/// no-op matches what Linux itself does for a pty master, where `tcsendbreak`
/// succeeds without any break reaching the slave.
pub fn tcsendbreak(fd: i32, _duration: i32) -> Result<(), i32> {
    // The descriptor is still validated, so a non-terminal is rejected.
    console_fd(fd)?;
    Ok(())
}

/// Reports whether a descriptor is a terminal, as `isatty` would.
pub fn isatty(fd: i32) -> bool {
    console_fd(fd).is_ok()
}

/// Drops any cached `termios` for a descriptor.
///
/// Called when a descriptor is closed so the cache does not grow without bound
/// over a long-running process's lifetime.
pub fn forget(fd: i32) {
    let Ok(mut registry) = registry().lock() else {
        return;
    };
    registry.retain(|&(cached, _), _| cached != fd);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EBADF;
    use std::mem::{offset_of, size_of};

    /// A console descriptor that restores the console's mode when dropped.
    ///
    /// Probing fds 0-2 is not enough: `cargo test` replaces the standard handles
    /// with pipes, so those descriptors are not consoles even though the process
    /// is still attached to one. Opening `CONIN$`/`CONOUT$` reaches the attached
    /// console directly, which is what lets these tests genuinely exercise the
    /// Windows path under the test harness instead of skipping.
    ///
    /// Restoring the mode is not optional: these tests put the *developer's own
    /// terminal* into raw mode, and leaving it there would break their shell.
    struct ConsoleFixture {
        fd: i32,
        handle: HANDLE,
        mode: u32,
    }

    impl ConsoleFixture {
        /// Opens the attached console, or `None` when there is none.
        ///
        /// A genuinely console-less environment (a CI service, or a detached
        /// process) makes the open fail, and the tests skip rather than fail.
        fn open(direction: Direction) -> Option<Self> {
            use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };

            let name: Vec<u16> = match direction {
                Direction::Input => "CONIN$\0",
                Direction::Output => "CONOUT$\0",
            }
            .encode_utf16()
            .collect();
            // Both accesses are required: the console device rejects a
            // read-only open of CONOUT$ for mode changes.
            // SAFETY: the name is a null-terminated wide string and the
            // remaining arguments are the documented console-device pattern.
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

            let mut mode = 0u32;
            // SAFETY: the handle was just opened and `mode` is a writable local.
            if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
                // SAFETY: the handle is owned here and not stored.
                unsafe { CloseHandle(handle) };
                return None;
            }

            // BORROWED keeps the table from closing the handle, so this fixture
            // stays the sole owner and the drop order is predictable.
            let fd =
                crate::install(handle as usize, FdKind::Console, crate::FdFlags::BORROWED).ok()?;
            Some(Self { fd, handle, mode })
        }
    }

    impl Drop for ConsoleFixture {
        fn drop(&mut self) {
            // SAFETY: the handle is still open and the mode is the one read from
            // it at open time.
            unsafe { SetConsoleMode(self.handle, self.mode) };
            let _ = crate::close(self.fd);
            forget(self.fd);
            // SAFETY: this fixture owns the handle; the table entry was borrowed.
            unsafe { CloseHandle(self.handle) };
        }
    }

    #[test]
    fn termios_matches_the_linux_x86_64_abi() {
        // Guest code memcpys this structure across the boundary, so a layout
        // change is a silent ABI break rather than a compile error.
        assert_eq!(size_of::<Termios>(), 60);
        assert_eq!(offset_of!(Termios, c_iflag), 0);
        assert_eq!(offset_of!(Termios, c_oflag), 4);
        assert_eq!(offset_of!(Termios, c_cflag), 8);
        assert_eq!(offset_of!(Termios, c_lflag), 12);
        assert_eq!(offset_of!(Termios, c_line), 16);
        assert_eq!(offset_of!(Termios, c_cc), 17);
        // c_cc is 32 bytes ending at 49, and the u32 speeds realign to 52.
        assert_eq!(offset_of!(Termios, c_ispeed), 52);
        assert_eq!(offset_of!(Termios, c_ospeed), 56);
        assert_eq!(size_of::<WinSize>(), 8);
    }

    #[test]
    fn make_raw_clears_exactly_the_cfmakeraw_bits() {
        // Start from all-ones so every cleared bit is observable and every
        // untouched bit is too.
        let mut termios = Termios {
            c_iflag: u32::MAX,
            c_oflag: u32::MAX,
            c_cflag: u32::MAX,
            c_lflag: u32::MAX,
            c_line: 7,
            c_cc: [0xaa; 32],
            c_ispeed: B38400,
            c_ospeed: B38400,
        };
        make_raw(&mut termios);

        let cleared_iflag = BRKINT | ICRNL | INLCR | IGNCR | ISTRIP | IXON;
        assert_eq!(termios.c_iflag & cleared_iflag, 0);
        // Everything outside cfmakeraw's mask must survive untouched, which is
        // what starting from all-ones lets this assert prove.
        assert_eq!(termios.c_iflag, !cleared_iflag);

        assert_eq!(termios.c_oflag & OPOST, 0);
        assert_eq!(termios.c_oflag, !OPOST);

        let cleared_lflag = ECHO | ECHONL | ICANON | IEXTEN | ISIG;
        assert_eq!(termios.c_lflag & cleared_lflag, 0);
        assert_eq!(termios.c_lflag, !cleared_lflag);
        // ECHOE and ECHOK are not part of cfmakeraw, so they stay.
        assert_ne!(termios.c_lflag & (ECHOE | ECHOK), 0);

        // CS8 replaces the character size field rather than being OR'd in.
        assert_eq!(termios.c_cflag & CSIZE, CS8);
        assert_ne!(termios.c_cflag & CREAD, 0);

        assert_eq!(termios.c_cc[VMIN], 1);
        assert_eq!(termios.c_cc[VTIME], 0);
        // The other control characters are cfmakeraw's business to leave alone.
        assert_eq!(termios.c_cc[VINTR], 0xaa);
        assert_eq!(termios.c_line, 7);
        assert_eq!(termios.c_ispeed, B38400);
    }

    #[test]
    fn make_raw_is_idempotent() {
        // A TUI that re-enters raw mode must not accumulate changes.
        let mut once = Termios::default();
        make_raw(&mut once);
        let mut twice = once;
        make_raw(&mut twice);
        assert_eq!(once, twice);
    }

    #[test]
    fn open_pty_yields_distinct_master_handles_and_resizes() {
        let mut pty = open_pty(24, 80).expect("CreatePseudoConsole failed");

        assert!(!pty.input_handle().is_null());
        assert!(!pty.output_handle().is_null());
        assert_ne!(pty.input_handle() as isize, -1);
        assert_ne!(pty.output_handle() as isize, -1);
        // The two directions are separate pipes; the same handle for both would
        // mean the pipe ends were paired up wrongly.
        assert_ne!(pty.input_handle(), pty.output_handle());
        assert_ne!(pty.console(), 0);
        assert_eq!(
            pty.window_size(),
            WinSize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }
        );

        pty.resize(50, 132).expect("ResizePseudoConsole failed");
        assert_eq!(pty.window_size().ws_row, 50);
        assert_eq!(pty.window_size().ws_col, 132);

        // No client was ever attached, so ClosePseudoConsole has nothing to wait
        // for and this drop returns promptly.
        drop(pty);
    }

    #[test]
    fn a_pty_master_is_a_pipe_and_carries_bytes_toward_the_terminal() {
        let pty = open_pty(24, 80).unwrap();
        // The master write end must be a real writable pipe: this is what a
        // guest's write to the pty master does, and it is the only part of the
        // data path testable without a client process.
        let fd = crate::install(
            pty.input_handle() as usize,
            FdKind::Pipe,
            crate::FdFlags::BORROWED.union(crate::FdFlags::PIPE_WRITE_END),
        )
        .unwrap();
        assert_eq!(crate::write(fd, b"ls -l\r").unwrap(), 6);
        crate::close(fd).unwrap();
        drop(pty);
    }

    #[test]
    fn zero_and_oversized_window_sizes_are_rejected() {
        // Zero means "size unknown" to a terminal, and ConPTY would accept it
        // and produce an unusable console.
        assert!(matches!(open_pty(0, 80), Err(EINVAL)));
        assert!(matches!(open_pty(24, 0), Err(EINVAL)));
        // A console dimension is an i16, so anything above 32767 cannot be
        // represented and must not wrap into a negative.
        assert!(matches!(open_pty(24, 40000), Err(EINVAL)));

        let mut pty = open_pty(24, 80).unwrap();
        assert!(matches!(pty.resize(0, 80), Err(EINVAL)));
        // A rejected resize must not have disturbed the recorded size.
        assert_eq!(pty.window_size().ws_row, 24);
    }

    #[test]
    fn terminal_calls_on_a_non_tty_report_enotty() {
        // A pipe is a descriptor that reads and writes fine but is not a
        // terminal, which is exactly the case isatty exists to distinguish.
        let (read, write) = pipe().unwrap();
        let fd =
            crate::install(read as usize, FdKind::Pipe, crate::FdFlags::PIPE_READ_END).unwrap();

        assert!(matches!(tcgetattr(fd), Err(ENOTTY)));
        assert!(matches!(
            tcsetattr(fd, TCSANOW, &Termios::default()),
            Err(ENOTTY)
        ));
        assert!(matches!(get_window_size(fd), Err(ENOTTY)));
        assert!(matches!(tcflush(fd, TCIFLUSH), Err(ENOTTY)));
        assert!(matches!(tcdrain(fd), Err(ENOTTY)));
        assert!(matches!(tcsendbreak(fd, 0), Err(ENOTTY)));
        assert!(!isatty(fd));

        crate::close(fd).unwrap();
        // SAFETY: the write end was never installed, so it is still owned here.
        unsafe { CloseHandle(write) };
    }

    #[test]
    fn a_pty_master_reports_enotty_for_attributes_but_still_resizes() {
        // The documented divergence from Linux, pinned down: the master is a
        // pipe, so it has no console mode, but it does have a pseudoconsole.
        let pty = open_pty(24, 80).unwrap();
        let fd = crate::install(
            pty.input_handle() as usize,
            FdKind::Pipe,
            crate::FdFlags::BORROWED.union(crate::FdFlags::PIPE_WRITE_END),
        )
        .unwrap();

        assert!(matches!(tcgetattr(fd), Err(ENOTTY)));
        set_window_size(
            fd,
            &WinSize {
                ws_row: 30,
                ws_col: 90,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();

        crate::close(fd).unwrap();
        drop(pty);
    }

    #[test]
    fn a_closed_or_absent_descriptor_reports_ebadf() {
        // EBADF outranks ENOTTY: the descriptor has to exist before asking
        // whether it is a terminal.
        assert!(matches!(tcgetattr(-1), Err(EBADF)));
        assert!(matches!(tcgetattr(900), Err(EBADF)));
    }

    #[test]
    fn tcsetattr_rejects_an_unknown_action() {
        let (read, write) = pipe().unwrap();
        let fd =
            crate::install(read as usize, FdKind::Pipe, crate::FdFlags::PIPE_READ_END).unwrap();
        // The action is validated before the terminal check, so a bad action is
        // EINVAL even on a non-terminal.
        assert!(matches!(
            tcsetattr(fd, 99, &Termios::default()),
            Err(EINVAL)
        ));
        assert!(matches!(tcflush(fd, 99), Err(EINVAL)));
        crate::close(fd).unwrap();
        // SAFETY: the write end is still owned by this test.
        unsafe { CloseHandle(write) };
    }

    #[test]
    fn the_default_termios_describes_a_cooked_terminal() {
        // A program that inspects the terminal before touching it must draw the
        // same conclusion it would on a Linux login tty.
        let termios = Termios::default();
        assert_ne!(termios.c_lflag & ICANON, 0);
        assert_ne!(termios.c_lflag & ECHO, 0);
        assert_ne!(termios.c_lflag & ISIG, 0);
        assert_ne!(termios.c_oflag & OPOST, 0);
        assert_eq!(termios.c_cflag & CSIZE, CS8);
        assert_eq!(termios.c_cc[VINTR], 0x03);
        assert_eq!(termios.c_cc[VEOF], 0x04);
        assert_eq!(termios.c_cc[VSUSP], 0x1a);
        assert_eq!(termios.c_cc[VERASE], 0x7f);
    }

    /// Serializes the tests that mutate this process's real console mode.
    ///
    /// There is one console per process, so running these in parallel would let
    /// one test's restore undo another's setup.
    static CONSOLE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn tcsetattr_round_trips_the_fields_windows_cannot_enforce() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open(Direction::Input) else {
            // No console attached at all: the cache is unreachable without a
            // terminal descriptor, so there is nothing to assert.
            return;
        };
        let fd = console.fd;

        let mut requested = Termios::default();
        // Fields with no Windows counterpart at all. If these come back changed,
        // the cache is not doing its job and a TUI's save/restore would corrupt
        // the terminal state it thought it saved.
        requested.c_cc[VMIN] = 17;
        requested.c_cc[VTIME] = 42;
        requested.c_cc[VINTR] = 0x07;
        requested.c_cc[VERASE] = 0x08;
        requested.c_iflag = IXON | ISTRIP | INLCR;
        requested.c_cflag = CS8 | CREAD | CLOCAL;
        requested.c_line = 3;
        requested.c_ispeed = B38400;
        requested.c_ospeed = B38400;

        tcsetattr(fd, TCSANOW, &requested).unwrap();
        let observed = tcgetattr(fd).unwrap();

        assert_eq!(observed.c_cc[VMIN], 17);
        assert_eq!(observed.c_cc[VTIME], 42);
        assert_eq!(observed.c_cc[VINTR], 0x07);
        assert_eq!(observed.c_cc[VERASE], 0x08);
        assert_eq!(observed.c_iflag, requested.c_iflag);
        assert_eq!(observed.c_cflag, requested.c_cflag);
        assert_eq!(observed.c_line, 3);
        assert_eq!(observed.c_ispeed, B38400);
        assert_eq!(observed.c_ospeed, B38400);
    }

    #[test]
    fn raw_mode_reaches_the_windows_console_mode() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console_input) = ConsoleFixture::open(Direction::Input) else {
            return;
        };
        let fd = console_input.fd;

        let original = tcgetattr(fd).unwrap();
        let mut raw = original;
        make_raw(&mut raw);
        tcsetattr(fd, TCSAFLUSH, &raw).unwrap();

        // The enforced bits must be observable in the host's own mode word, not
        // merely in the cache; going through console_fd re-reads the real mode
        // and so cannot be satisfied by a cache that lied.
        let console = console_fd(fd).unwrap();
        assert!(matches!(console.direction, Direction::Input));
        assert_eq!(console.mode & ENABLE_ECHO_INPUT, 0, "ECHO was not cleared");
        assert_eq!(
            console.mode & ENABLE_LINE_INPUT,
            0,
            "ICANON was not cleared"
        );
        assert_eq!(
            console.mode & ENABLE_PROCESSED_INPUT,
            0,
            "ISIG was not cleared"
        );
        // A raw reader needs arrow keys as escape sequences, not key records.
        assert_ne!(console.mode & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);

        // And the reported termios agrees with what was asked for.
        let observed = tcgetattr(fd).unwrap();
        assert_eq!(observed.c_lflag & (ECHO | ICANON | ISIG), 0);

        // Restoring must put the enforced bits back, which is the second half of
        // every TUI's contract with the terminal.
        tcsetattr(fd, TCSANOW, &original).unwrap();
        let restored = tcgetattr(fd).unwrap();
        assert_eq!(restored.c_lflag & ICANON, original.c_lflag & ICANON);
        assert_eq!(restored.c_lflag & ECHO, original.c_lflag & ECHO);
    }

    #[test]
    fn ansi_passthrough_survives_clearing_opost() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console_output) = ConsoleFixture::open(Direction::Output) else {
            return;
        };
        let fd = console_output.fd;

        // The output half must be recognised as such, or the input mode bits
        // would be written to it and mean something entirely different.
        assert!(matches!(
            console_fd(fd).unwrap().direction,
            Direction::Output
        ));

        let mut raw = tcgetattr(fd).unwrap();
        make_raw(&mut raw);
        tcsetattr(fd, TCSANOW, &raw).unwrap();

        let mode = console_fd(fd).unwrap().mode;
        assert_eq!(mode & ENABLE_PROCESSED_OUTPUT, 0, "OPOST was not cleared");
        // The whole premise of this layer: escape sequences must never start
        // printing literally, whatever termios says.
        assert_ne!(
            mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            0,
            "ANSI passthrough was disabled"
        );
        assert_eq!(tcgetattr(fd).unwrap().c_oflag & OPOST, 0);

        // ONLCR maps to the inverse of DISABLE_NEWLINE_AUTO_RETURN, so a
        // round trip through both senses must land back where it started.
        let mut cooked = Termios::default();
        cooked.c_oflag |= ONLCR;
        tcsetattr(fd, TCSANOW, &cooked).unwrap();
        assert_eq!(
            console_fd(fd).unwrap().mode & DISABLE_NEWLINE_AUTO_RETURN,
            0
        );
        assert_ne!(tcgetattr(fd).unwrap().c_oflag & ONLCR, 0);

        cooked.c_oflag &= !ONLCR;
        tcsetattr(fd, TCSANOW, &cooked).unwrap();
        assert_ne!(
            console_fd(fd).unwrap().mode & DISABLE_NEWLINE_AUTO_RETURN,
            0
        );
        assert_eq!(tcgetattr(fd).unwrap().c_oflag & ONLCR, 0);
    }

    #[test]
    fn get_window_size_reports_the_visible_window() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open(Direction::Output) else {
            return;
        };
        let size = get_window_size(console.fd).unwrap();
        // A real terminal always has a positive viewport.
        assert!(size.ws_row > 0, "window height was {}", size.ws_row);
        assert!(size.ws_col > 0, "window width was {}", size.ws_col);
        // The window, not the scrollback buffer: a console buffer is routinely
        // 9000 rows tall, and reporting that would make a full-screen program
        // draw far outside the viewport.
        assert!(
            size.ws_row < 1000,
            "window height {} looks like dwSize",
            size.ws_row
        );
        // Character cells have no pixel dimensions to report.
        assert_eq!(size.ws_xpixel, 0);
        assert_eq!(size.ws_ypixel, 0);
    }

    #[test]
    fn flush_and_drain_succeed_on_a_console() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open(Direction::Input) else {
            return;
        };
        let fd = console.fd;
        tcflush(fd, TCIFLUSH).unwrap();
        tcflush(fd, TCOFLUSH).unwrap();
        tcflush(fd, TCIOFLUSH).unwrap();
        tcdrain(fd).unwrap();
        // A no-op that reports success, per the comment on tcsendbreak.
        tcsendbreak(fd, 0).unwrap();
    }

    #[test]
    fn set_window_size_resizes_the_pseudoconsole_behind_a_master_descriptor() {
        let pty = open_pty(24, 80).unwrap();
        // The master handles are pipes, so this only works because the pty
        // registry maps them back to their HPCON.
        let fd = crate::install(
            pty.output_handle() as usize,
            FdKind::Pipe,
            crate::FdFlags::BORROWED.union(crate::FdFlags::PIPE_READ_END),
        )
        .unwrap();

        set_window_size(
            fd,
            &WinSize {
                ws_row: 40,
                ws_col: 100,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        // A rejected size must still be rejected on this path.
        assert!(matches!(
            set_window_size(fd, &WinSize::default()),
            Err(EINVAL)
        ));

        crate::close(fd).unwrap();
        drop(pty);
    }

    #[test]
    fn a_dropped_pty_stops_resolving_for_resize() {
        let pty = open_pty(24, 80).unwrap();
        let master = pty.output_handle();
        assert!(pty_master_console(master).is_some());
        drop(pty);
        // The registry must not keep a dangling HPCON, or a later pty reusing
        // the same handle value would resize the wrong console.
        assert!(pty_master_console(master).is_none());
    }

    #[test]
    fn the_termios_cache_does_not_survive_descriptor_reuse() {
        let _serialized = CONSOLE_LOCK.lock().unwrap();
        let Some(console) = ConsoleFixture::open(Direction::Input) else {
            return;
        };
        let fd = console.fd;

        let mut raw = tcgetattr(fd).unwrap();
        make_raw(&mut raw);
        raw.c_cc[VMIN] = 99;
        tcsetattr(fd, TCSANOW, &raw).unwrap();
        assert_eq!(tcgetattr(fd).unwrap().c_cc[VMIN], 99);

        // Forgetting the descriptor is what close() does, so the next terminal
        // to occupy this fd number starts from the cooked defaults instead of
        // inheriting a raw-mode VMIN it never asked for.
        forget(fd);
        assert_eq!(
            tcgetattr(fd).unwrap().c_cc[VMIN],
            Termios::default().c_cc[VMIN]
        );
    }
}
