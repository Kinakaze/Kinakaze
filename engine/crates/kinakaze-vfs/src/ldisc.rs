//! The N_TTY line discipline, as pure logic over a `termios` and two byte queues.
//!
//! Everything a terminal does between the wire and `read`/`write` lives here:
//! canonical line editing, echo, flow control, signal generation and output
//! post-processing. Nothing in this module touches a handle, a thread or a
//! clock, which is deliberate — it is the part of a terminal that has real
//! behaviour to get wrong, so it is the part that has to be testable without a
//! terminal.
//!
//! Two consequences of that split are worth stating, because they shape the API:
//!
//! * Signals are *returned*, not sent. [`Ldisc::receive`] hands back the signal
//!   numbers the input generated and the caller delivers them to the terminal's
//!   foreground process group. A line discipline that knew about process groups
//!   would need a process table to test against.
//! * `VMIN`/`VTIME` are *classified*, not waited on. [`Ldisc::read_policy`]
//!   reduces the four combinations to a decision the caller implements with
//!   whatever wait primitive it has. The classification is the part that is
//!   routinely wrong; the waiting is not.
//!
//! The reference is Linux's `drivers/tty/n_tty.c`. Where this diverges, the
//! divergence is named in the item's own documentation rather than left for a
//! reader to discover.

use std::collections::VecDeque;

use crate::pty::{
    BRKINT, ECHO, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, IEXTEN, IGNCR, INLCR, ISIG, ISTRIP, IXON,
    NOFLSH, ONLCR, OPOST, Termios, VEOF, VERASE, VINTR, VKILL, VMIN, VQUIT, VSTART, VSTOP, VSUSP,
    VTIME,
};
use crate::signal::{SIGINT, SIGQUIT, SIGTSTP};

// ---------------------------------------------------------------------------
// The `termios` bits a line discipline needs that the console layer never did.
//
// `pty` defines the handful of flags a Windows console mode can express. Those
// are re-used above. Everything below exists because a *software* line
// discipline can enforce it, and the console layer had no reason to name a bit
// it could not act on. The values are Linux's x86_64 numbers and are ABI: guest
// code writes them into `c_iflag` and friends directly.
// ---------------------------------------------------------------------------

// `c_iflag`.
pub const IGNBRK: u32 = 0x0000_0001;
pub const IGNPAR: u32 = 0x0000_0004;
pub const PARMRK: u32 = 0x0000_0008;
pub const INPCK: u32 = 0x0000_0010;
pub const IUCLC: u32 = 0x0000_0200;
pub const IXANY: u32 = 0x0000_0800;
pub const IXOFF: u32 = 0x0000_1000;
pub const IMAXBEL: u32 = 0x0000_2000;
pub const IUTF8: u32 = 0x0000_4000;

// `c_oflag`.
pub const OLCUC: u32 = 0x0000_0002;
pub const OCRNL: u32 = 0x0000_0008;
pub const ONOCR: u32 = 0x0000_0010;
pub const ONLRET: u32 = 0x0000_0020;
pub const OFILL: u32 = 0x0000_0040;
pub const OFDEL: u32 = 0x0000_0080;
/// `TABDLY`, the tab-delay field `XTABS` is the third value of.
pub const TABDLY: u32 = 0o0014000;
/// `XTABS` (also spelled `TAB3`): expand tabs into spaces on output.
pub const XTABS: u32 = 0o0014000;

// `c_lflag`.
pub const XCASE: u32 = 0x0000_0004;
pub const ECHOCTL: u32 = 0x0000_0200;
pub const ECHOPRT: u32 = 0x0000_0400;
pub const ECHOKE: u32 = 0x0000_0800;
/// Output is being discarded because `VDISCARD` was typed.
///
/// Kernel-internal on Linux but readable and writable through `termios`. It
/// round-trips here and is set only in response to `VDISCARD`.
pub const FLUSHO: u32 = 0x0000_1000;
/// Input is waiting to be reprocessed after a `termios` change.
///
/// Purely internal to Linux's discipline. It round-trips and drives nothing,
/// because this implementation re-derives its state on every `tcsetattr` rather
/// than deferring the work.
pub const PENDIN: u32 = 0x0000_4000;
/// The line discipline is bypassed by an external processor.
///
/// Nothing here implements an external discipline, so the bit round-trips and
/// changes no behaviour.
pub const EXTPROC: u32 = 0x0001_0000;

// `c_cflag`.
pub const CSIZE: u32 = 0x0000_0030;
pub const CSTOPB: u32 = 0x0000_0040;
pub const PARENB: u32 = 0x0000_0100;
pub const PARODD: u32 = 0x0000_0200;
pub const HUPCL: u32 = 0x0000_0400;
pub const CRTSCTS: u32 = 0x8000_0000;

// `c_cc` indices past the ones a console mode could reach.
pub const VSWTC: usize = 7;
pub const VEOL: usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE: usize = 14;
pub const VLNEXT: usize = 15;
pub const VEOL2: usize = 16;

/// The `termios` a pseudo-terminal is created with.
///
/// Linux's `tty_std_termios`, which is a strict superset of what
/// [`Termios::default`] can describe: the console layer's default predates the
/// flags above and cannot name `ECHOCTL`, `ECHOKE` or the extended control
/// characters. A pty starts from this instead, so a shell finds the terminal it
/// would find on Linux — `^W` erasing a word, `^C` echoing as `^C`.
pub fn default_termios() -> Termios {
    let mut termios = Termios::default();
    termios.c_iflag |= IMAXBEL;
    termios.c_cflag |= HUPCL;
    termios.c_lflag |= ECHOCTL | ECHOKE;
    termios.c_cc[VREPRINT] = 0x12; // ^R
    termios.c_cc[VDISCARD] = 0x0f; // ^O
    termios.c_cc[VWERASE] = 0x17; // ^W
    termios.c_cc[VLNEXT] = 0x16; // ^V
    // VEOL, VEOL2 and VSWTC stay at _POSIX_VDISABLE: a terminal with no
    // secondary line terminator is the normal case, and assigning one would make
    // an ordinary NUL end a line.
    termios
}

/// Applies the `cfmakeraw` transformation, in full.
///
/// [`crate::pty::make_raw`] predates the flags above and so cannot clear
/// `IGNBRK`, `PARMRK` or `PARENB`, which glibc's `cfmakeraw` does. The two
/// differ only in those three bits, and only this one is used for terminals
/// whose behaviour a software line discipline actually enforces — where leaving
/// `IGNBRK` set would silently swallow a break a raw reader asked to see.
pub fn make_raw(termios: &mut Termios) {
    termios.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    termios.c_oflag &= !OPOST;
    termios.c_lflag &= !(ECHO | ECHONL | ICANON | IEXTEN | ISIG);
    // A raw reader wants all eight bits of every byte, and eight data bits plus
    // a parity bit is nine — which is why PARENB goes with CSIZE.
    termios.c_cflag = (termios.c_cflag & !(CSIZE | PARENB)) | crate::pty::CS8;
    termios.c_cc[VMIN] = 1;
    termios.c_cc[VTIME] = 0;
}

/// How many cooked input bytes the discipline will hold.
///
/// Linux's `N_TTY_BUF_SIZE`. The number is load-bearing rather than arbitrary:
/// a canonical line longer than the buffer cannot be edited, so Linux stops
/// accepting input at that point and this does the same. Raising it would make
/// this layer accept lines a real terminal would have refused.
pub const INPUT_LIMIT: usize = 4096;

/// How many post-processed output bytes the discipline will hold.
///
/// A slave write that would exceed this is short rather than refused, which is
/// what lets the caller block the writer and retry — the behaviour of a real
/// tty whose output queue is full. Echo is different: it is generated by input
/// rather than requested by a caller, so there is nobody to report a short
/// write to and the overflow is dropped, exactly as Linux drops echo it cannot
/// place.
///
/// Sixteen kilobytes is Linux's order of magnitude and also the size this state
/// is copied at on every operation when it lives in shared memory, so a much
/// larger number would be paid for on every byte.
pub const OUTPUT_LIMIT: usize = 16384;

/// `_POSIX_VDISABLE`: a `c_cc` slot set to this has no character assigned.
///
/// Zero is a real byte, so "disabled" has to be checked before comparing, or a
/// terminal with `VEOL` disabled would treat every NUL as a line terminator.
pub const DISABLED: u8 = 0;

/// Input-queue occupancy at which `IXOFF` sends `VSTOP` to the writer.
///
/// Two thirds full, which leaves room for the bytes already in flight when the
/// stop is sent. A high-water mark at the very top would drop data, since a
/// writer cannot react to a stop it has not read yet.
const IXOFF_HIGH_WATER: usize = INPUT_LIMIT * 2 / 3;

/// Occupancy at which `IXOFF` releases the writer again with `VSTART`.
const IXOFF_LOW_WATER: usize = INPUT_LIMIT / 3;

/// A terminal tab stop, in columns. Fixed at 8 on every Unix terminal.
const TAB_WIDTH: usize = 8;

/// `N_TTY`, standard terminal line discipline.
pub const N_TTY: i32 = 0;
/// `N_SLIP`, Serial Line IP.
pub const N_SLIP: i32 = 1;
/// `N_MOUSE`, mouse line discipline.
pub const N_MOUSE: i32 = 2;
/// `N_PPP`, Point-to-Point Protocol.
pub const N_PPP: i32 = 3;
/// `N_STRIP`, Metricom Starmode IP.
pub const N_STRIP: i32 = 4;
/// `N_AX25`, Amateur radio AX.25.
pub const N_AX25: i32 = 5;
/// `N_X25`, X.25 framing.
pub const N_X25: i32 = 6;
/// `N_6PACK`, 6pack framing.
pub const N_6PACK: i32 = 7;
/// `N_MASC`, MASC protocol.
pub const N_MASC: i32 = 8;
/// `N_R3964`, SIMATIC R3964.
pub const N_R3964: i32 = 9;
/// `N_PROFIBUS_FDL`, PROFIBUS FDL.
pub const N_PROFIBUS_FDL: i32 = 10;
/// `N_IRDA`, IrDA framing.
pub const N_IRDA: i32 = 11;
/// `N_SMSBLOCK`, SMS block mode.
pub const N_SMSBLOCK: i32 = 12;
/// `N_HDLC`, High-Level Data Link Control framing.
pub const N_HDLC: i32 = 13;
/// `N_SYNC_PPP`, Synchronous PPP.
pub const N_SYNC_PPP: i32 = 14;
/// `N_HCI`, Bluetooth HCI UART.
pub const N_HCI: i32 = 15;
/// `N_GIGASET_M101`, Gigaset M101.
pub const N_GIGASET_M101: i32 = 16;
/// `N_SLCAN`, CAN bus over serial.
pub const N_SLCAN: i32 = 17;
/// `N_GSM0710`, GSM 07.10 multiplexing.
pub const N_GSM0710: i32 = 21;
/// `N_TI_WL`, TI WiLink.
pub const N_TI_WL: i32 = 22;
/// `N_TRACESINK`, Trace sink.
pub const N_TRACESINK: i32 = 23;
/// `N_TRACEROUTER`, Trace router.
pub const N_TRACEROUTER: i32 = 24;
/// `N_NCI`, NFC NCI.
pub const N_NCI: i32 = 25;
/// `N_SPEAKUP`, Speakup screen reader.
pub const N_SPEAKUP: i32 = 26;
/// `N_NULL`, Null line discipline (raw transparent byte pipe).
pub const N_NULL: i32 = 27;

/// Whether a number names a recognised Linux line discipline.
pub fn is_valid_ldisc(number: i32) -> bool {
    matches!(
        number,
        N_TTY
            | N_SLIP
            | N_MOUSE
            | N_PPP
            | N_STRIP
            | N_AX25
            | N_X25
            | N_6PACK
            | N_MASC
            | N_R3964
            | N_PROFIBUS_FDL
            | N_IRDA
            | N_SMSBLOCK
            | N_HDLC
            | N_SYNC_PPP
            | N_HCI
            | N_GIGASET_M101
            | N_SLCAN
            | N_GSM0710
            | N_TI_WL
            | N_TRACESINK
            | N_TRACEROUTER
            | N_NCI
            | N_SPEAKUP
            | N_NULL
    )
}

/// What a read may do right now, given `VMIN` and `VTIME`.
///
/// The four combinations are the classic source of confusion, so they are named
/// rather than reconstructed from two numbers at every call site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadPolicy {
    /// Canonical mode: a read returns one line, however long the wait.
    ///
    /// `VMIN` and `VTIME` are not consulted at all in canonical mode, which is
    /// the detail programs get wrong when they set `VTIME` and wonder why the
    /// read still blocks.
    Canonical,
    /// `VMIN == 0`, `VTIME == 0`: return whatever is queued, including nothing.
    Poll,
    /// `VMIN > 0`, `VTIME == 0`: block until `min` bytes have arrived.
    Blocking { min: usize },
    /// `VMIN == 0`, `VTIME > 0`: a read timer.
    ///
    /// Wait up to `timeout_ms` for the *first* byte and return as soon as any
    /// arrive. A timeout that expires with nothing is a zero-length read, not
    /// an error.
    ReadTimer { timeout_ms: u32 },
    /// `VMIN > 0`, `VTIME > 0`: an inter-byte timer.
    ///
    /// The first byte is waited for indefinitely; after that each subsequent
    /// byte restarts a `timeout_ms` timer, and the read ends when either `min`
    /// bytes have arrived or the timer expires. A timer that expires after at
    /// least one byte returns those bytes, so this form never returns zero.
    Interbyte { min: usize, timeout_ms: u32 },
}

/// The result of a read attempt against the input queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Read {
    /// This many bytes were copied into the caller's buffer.
    Data(usize),
    /// A `VEOF` typed at the start of a line: the read returns zero.
    ///
    /// Distinct from `Data(0)` on purpose. A zero-length read means end of file
    /// to every caller, and it must only be produced when the user really asked
    /// for one — never because the queue happened to be empty.
    EndOfFile,
    /// Nothing is available; the caller must wait or report `EAGAIN`.
    WouldBlock,
}

/// Which queues a flush affects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Queue {
    Input,
    Output,
    Both,
}

/// One terminal's line discipline state.
///
/// Owned by the terminal, not by a descriptor: both ends of a pty and every
/// duplicate of either share one of these, which is why `termios` set through
/// the slave is visible through the master.
pub struct Ldisc {
    /// The terminal settings this discipline enforces.
    ///
    /// Public because `tcsetattr` replaces it wholesale and the surrounding code
    /// needs to read individual flags; changing it mid-line is legal and is
    /// handled by [`Ldisc::retune`].
    pub termios: Termios,
    /// Cooked input: committed bytes first, then the line still being edited.
    input: VecDeque<u8>,
    /// How many bytes at the tail of `input` belong to the uncommitted line.
    pending: usize,
    /// Byte counts of the complete lines a canonical reader may take, in order.
    ///
    /// A zero entry is a `VEOF` on an empty line. Keeping the lengths separately
    /// is what makes "a read never crosses a line boundary" a property of the
    /// data structure rather than a rule the read path has to remember.
    lines: VecDeque<usize>,
    /// Post-processed bytes waiting for the other end to read.
    output: VecDeque<u8>,
    /// The column the cursor is in, tracked across everything written.
    column: usize,
    /// The column the current canonical line started in.
    ///
    /// `ECHOE` must not erase back past the prompt, and the prompt is whatever
    /// was on the line when input started, so its width has to be remembered.
    canon_column: usize,
    /// `IXON`: output is suspended because `VSTOP` was received.
    stopped: bool,
    /// `IXOFF`: a `VSTOP` has been sent and no `VSTART` yet.
    throttled: bool,
    /// `VLNEXT` was seen, so the next byte is taken literally.
    literal: bool,
    /// `ECHOPRT` is mid-erase and owes a closing `/`.
    erase_printing: bool,
    /// The line discipline number, for `TIOCGETD`/`TIOCSETD`.
    ///
    /// Only `N_TTY` is implemented, so this exists to be reported rather than
    /// to select anything.
    pub number: i32,
}

impl Default for Ldisc {
    /// A discipline set up the way a freshly created terminal is.
    fn default() -> Self {
        Self::new(default_termios())
    }
}

impl Ldisc {
    /// Creates a discipline with the given settings and empty queues.
    pub fn new(termios: Termios) -> Self {
        Self {
            termios,
            input: VecDeque::new(),
            pending: 0,
            lines: VecDeque::new(),
            output: VecDeque::new(),
            column: 0,
            canon_column: 0,
            stopped: false,
            throttled: false,
            literal: false,
            erase_printing: false,
            number: N_TTY,
        }
    }

    /// Installs new settings, reconciling any line that was being edited.
    ///
    /// Leaving canonical mode has to commit the partial line rather than discard
    /// it: a program that switches to raw mode expects the bytes already typed
    /// to still be readable, and `tcsetattr` without `TCSAFLUSH` promises not to
    /// throw input away. Entering canonical mode from raw leaves the queued
    /// bytes as an unterminated line, which is what Linux does — they become
    /// readable once a terminator arrives.
    pub fn retune(&mut self, termios: Termios) {
        let was_canonical = self.canonical();
        self.termios = termios;
        let is_canonical = self.canonical();
        if was_canonical && !is_canonical {
            // The pending line becomes ordinary queued bytes.
            self.pending = 0;
        } else if !was_canonical && is_canonical {
            // Everything queued is now an unterminated line under edit.
            self.pending = self.input.len();
            self.canon_column = self.column;
        }
        if self.termios.c_iflag & IXON == 0 {
            // Flow control was turned off while output was stopped, which would
            // otherwise wedge the terminal with no way to send a `VSTART`.
            self.stopped = false;
        }
    }

    /// Changes the active line discipline number, reconciling buffered input.
    pub fn set_number(&mut self, number: i32) {
        let was_canonical = self.canonical();
        self.number = number;
        let is_canonical = self.canonical();
        if was_canonical && !is_canonical {
            self.pending = 0;
        } else if !was_canonical && is_canonical {
            self.pending = self.input.len();
            self.canon_column = self.column;
        }
    }

    /// Whether canonical (line-at-a-time) mode is in effect.
    pub fn canonical(&self) -> bool {
        self.number == N_TTY && (self.termios.c_lflag & ICANON != 0)
    }

    /// Whether `IXON` has suspended output.
    pub fn output_stopped(&self) -> bool {
        self.stopped
    }

    /// Suspends or resumes output, as `tcflow`'s `TCOOFF`/`TCOON` request.
    pub fn set_output_stopped(&mut self, stopped: bool) {
        self.stopped = stopped;
    }

    /// Bytes queued for the reader, whether or not they are readable yet.
    ///
    /// This is the `TIOCINQ` answer for a non-canonical terminal and the upper
    /// bound for a canonical one; [`Ldisc::readable`] is the stricter number.
    pub fn queued(&self) -> usize {
        self.input.len()
    }

    /// Bytes a read could return right now.
    ///
    /// In canonical mode that is the length of the first complete line and zero
    /// when no line is complete, which is what `FIONREAD` reports on Linux: a
    /// half-typed line is not readable, so claiming its bytes are available
    /// would make a caller poll-then-block.
    pub fn readable(&self) -> usize {
        if self.canonical() {
            self.lines.front().copied().unwrap_or(0)
        } else {
            self.input.len()
        }
    }

    /// Whether a read would find something, including a pending end of file.
    ///
    /// A `VEOF` on an empty line is a zero-length readable event: `readable()`
    /// reports zero for it, but a reader must still wake up and return.
    pub fn read_ready(&self) -> bool {
        if self.canonical() {
            !self.lines.is_empty()
        } else {
            !self.input.is_empty()
        }
    }

    /// Bytes waiting to be read from the other end of the terminal.
    ///
    /// The `TIOCOUTQ` answer: how much this terminal has written that the far
    /// side has not yet consumed.
    pub fn pending_output(&self) -> usize {
        self.output.len()
    }

    /// Room left in the output queue.
    pub fn output_space(&self) -> usize {
        OUTPUT_LIMIT.saturating_sub(self.output.len())
    }

    /// Whether any post-processed output is waiting.
    pub fn output_ready(&self) -> bool {
        !self.output.is_empty()
    }

    /// How a read should behave, given the current settings.
    ///
    /// See [`ReadPolicy`]; this is the whole of `VMIN`/`VTIME`, reduced once so
    /// no caller has to reconstruct it.
    pub fn read_policy(&self) -> ReadPolicy {
        if self.canonical() {
            return ReadPolicy::Canonical;
        }
        let min = self.termios.c_cc[VMIN] as usize;
        // VTIME is in tenths of a second, which is the unit programs forget.
        let timeout_ms = u32::from(self.termios.c_cc[VTIME]) * 100;
        match (min, timeout_ms) {
            (0, 0) => ReadPolicy::Poll,
            (0, timeout_ms) => ReadPolicy::ReadTimer { timeout_ms },
            (min, 0) => ReadPolicy::Blocking { min },
            (min, timeout_ms) => ReadPolicy::Interbyte { min, timeout_ms },
        }
    }

    /// Copies out at most one line, or as much raw input as fits.
    ///
    /// The canonical guarantee lives here: the copy is bounded by the first
    /// line's length as well as by the buffer, so a read can never return the
    /// start of a second line and a partially consumed line stays at the front
    /// of the queue for the next read.
    pub fn read(&mut self, buffer: &mut [u8]) -> Read {
        if self.canonical() {
            let Some(line) = self.lines.front().copied() else {
                return Read::WouldBlock;
            };
            if line == 0 {
                // A VEOF typed on an empty line. Consumed here so the next read
                // sees whatever follows rather than a permanent end of file.
                self.lines.pop_front();
                return Read::EndOfFile;
            }
            let count = line.min(buffer.len());
            self.drain_into(&mut buffer[..count]);
            match self.lines.front_mut() {
                Some(remaining) if *remaining > count => *remaining -= count,
                _ => {
                    self.lines.pop_front();
                }
            }
            self.release_throttle();
            return Read::Data(count);
        }

        if self.input.is_empty() {
            return Read::WouldBlock;
        }
        let count = self.input.len().min(buffer.len());
        self.drain_into(&mut buffer[..count]);
        self.release_throttle();
        Read::Data(count)
    }

    /// Moves `buffer.len()` bytes off the front of the input queue.
    fn drain_into(&mut self, buffer: &mut [u8]) {
        for slot in buffer.iter_mut() {
            // The caller sized the request from the queue, so this cannot fail;
            // a zero fill rather than a panic keeps a future miscount benign.
            *slot = self.input.pop_front().unwrap_or(0);
        }
    }

    /// Copies out post-processed output for the other end of the terminal.
    pub fn read_output(&mut self, buffer: &mut [u8]) -> usize {
        let count = self.output.len().min(buffer.len());
        for slot in buffer.iter_mut().take(count) {
            *slot = self.output.pop_front().unwrap_or(0);
        }
        count
    }

    /// Discards queued bytes, as `tcflush` requests.
    ///
    /// Flushing input abandons the line under edit as well as the complete ones;
    /// that is the point of `TCIFLUSH` after a mode change, and leaving the
    /// partial line would let bytes typed under the old settings be delivered
    /// under the new ones.
    pub fn flush(&mut self, queue: Queue) {
        if matches!(queue, Queue::Input | Queue::Both) {
            self.input.clear();
            self.lines.clear();
            self.pending = 0;
            self.literal = false;
            self.release_throttle();
        }
        if matches!(queue, Queue::Output | Queue::Both) {
            self.output.clear();
        }
    }

    // -----------------------------------------------------------------------
    // Input.
    // -----------------------------------------------------------------------

    /// Feeds bytes in from the wire, returning the signals they generated.
    ///
    /// The return is a list rather than one signal because a single write to the
    /// master can contain several control characters, and dropping the later
    /// ones would lose a `^C` that arrived in the same burst as a `^Z`.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<i32> {
        if self.number != N_TTY {
            for byte in bytes {
                if self.input.len() < INPUT_LIMIT {
                    self.input.push_back(*byte);
                }
            }
            return Vec::new();
        }
        let mut signals = Vec::new();
        for byte in bytes {
            self.receive_byte(*byte, &mut signals);
        }
        self.apply_throttle();
        signals
    }

    /// Handles a break condition on the line.
    ///
    /// A pseudo-terminal has no line to break, so the only source is an explicit
    /// `TCSBRK`/`TIOCSBRK` from the master. `IGNBRK` drops it, `BRKINT` turns it
    /// into a flush plus `SIGINT`, and otherwise it is delivered to the reader
    /// as a NUL, which is what a real line discipline does.
    pub fn receive_break(&mut self) -> Vec<i32> {
        if self.termios.c_iflag & IGNBRK != 0 {
            return Vec::new();
        }
        if self.termios.c_iflag & BRKINT != 0 {
            self.flush(Queue::Both);
            self.stopped = false;
            return vec![SIGINT];
        }
        let mut signals = Vec::new();
        self.receive_byte(0, &mut signals);
        self.apply_throttle();
        signals
    }

    /// The whole of `n_tty_receive_char`, for one byte.
    fn receive_byte(&mut self, byte: u8, signals: &mut Vec<i32>) {
        let iflag = self.termios.c_iflag;
        let lflag = self.termios.c_lflag;

        // A literal byte skips every interpretation below, which is exactly what
        // VLNEXT is for: it is how a user types the erase character itself.
        if self.literal {
            self.literal = false;
            self.store(byte);
            return;
        }

        let mut byte = byte;
        if iflag & ISTRIP != 0 {
            byte &= 0x7f;
        }

        // CR/NL translation happens before anything compares against a control
        // character, so a terminal with ICRNL set matches VEOL against the
        // translated newline rather than the carriage return that arrived.
        if byte == b'\r' {
            if iflag & IGNCR != 0 {
                return;
            }
            if iflag & ICRNL != 0 {
                byte = b'\n';
            }
        } else if byte == b'\n' && iflag & INLCR != 0 {
            byte = b'\r';
        }

        // Software flow control is consumed here and never reaches the reader.
        if iflag & IXON != 0 {
            let start = self.termios.c_cc[VSTART];
            let stop = self.termios.c_cc[VSTOP];
            if stop != DISABLED && byte == stop {
                // A terminal configured with one character for both directions
                // toggles rather than deadlocking on a stop it can never undo.
                self.stopped = !(start == stop && self.stopped);
                return;
            }
            if start != DISABLED && byte == start {
                self.stopped = false;
                return;
            }
        }
        // IXANY: any byte at all restarts stopped output, which is what makes a
        // paused `more` resume on the next keypress rather than only on ^Q.
        if self.stopped && iflag & (IXON | IXANY) == (IXON | IXANY) {
            self.stopped = false;
        }

        if lflag & ISIG != 0 {
            let cc = self.termios.c_cc;
            let signal = if cc[VINTR] != DISABLED && byte == cc[VINTR] {
                Some(SIGINT)
            } else if cc[VQUIT] != DISABLED && byte == cc[VQUIT] {
                Some(SIGQUIT)
            } else if cc[VSUSP] != DISABLED && byte == cc[VSUSP] {
                Some(SIGTSTP)
            } else {
                None
            };
            if let Some(signal) = signal {
                self.signal_char(byte, signal, signals);
                return;
            }
        }

        if lflag & ICANON != 0 {
            self.receive_canonical(byte);
        } else {
            self.store(byte);
        }
    }

    /// Generates a signal from a control character.
    ///
    /// The order is load-bearing. Flushing first and echoing afterwards is what
    /// puts the `^C` on the screen instead of behind the discarded output; and
    /// `NOFLSH` suppresses only the flush, never the signal.
    fn signal_char(&mut self, byte: u8, signal: i32, signals: &mut Vec<i32>) {
        if self.termios.c_lflag & NOFLSH == 0 {
            self.flush(Queue::Both);
        }
        // A signal releases flow control: a stopped terminal that could not be
        // restarted would swallow the very output the handler is about to print.
        self.stopped = false;
        if self.termios.c_lflag & ECHO != 0 {
            self.echo_char(byte);
        }
        signals.push(signal);
    }

    /// Canonical-mode handling for one byte: editing, terminators, echo.
    fn receive_canonical(&mut self, byte: u8) {
        let lflag = self.termios.c_lflag;
        let cc = self.termios.c_cc;
        let iexten = lflag & IEXTEN != 0;

        if cc[VERASE] != DISABLED && byte == cc[VERASE] {
            self.erase(Erase::Character);
            return;
        }
        if cc[VKILL] != DISABLED && byte == cc[VKILL] {
            self.erase(Erase::Line);
            return;
        }
        if iexten && cc[VWERASE] != DISABLED && byte == cc[VWERASE] {
            self.erase(Erase::Word);
            return;
        }
        if iexten && cc[VLNEXT] != DISABLED && byte == cc[VLNEXT] {
            self.literal = true;
            if lflag & ECHO != 0 {
                // The caret is printed without its letter so the user can see
                // that the terminal is waiting for a literal byte, and it is
                // backed over as soon as that byte is echoed.
                self.finish_erase_printing();
                self.echo_raw(b'^');
                self.echo_raw(0x08);
            }
            return;
        }
        if iexten && lflag & ECHO != 0 && cc[VREPRINT] != DISABLED && byte == cc[VREPRINT] {
            self.reprint();
            return;
        }

        if byte == b'\n' {
            self.finish_erase_printing();
            if lflag & (ECHO | ECHONL) != 0 {
                // ECHONL echoes the newline even with ECHO off, which is how a
                // password prompt still moves to the next line.
                self.echo_raw(b'\n');
            }
            self.commit(Some(byte));
            return;
        }
        if cc[VEOF] != DISABLED && byte == cc[VEOF] {
            // The EOF character terminates the line but is not part of it, so a
            // read sees the text without it and an EOF on an empty line becomes
            // the zero-length read that means end of file.
            self.finish_erase_printing();
            self.commit(None);
            return;
        }
        if (cc[VEOL] != DISABLED && byte == cc[VEOL])
            || (iexten && cc[VEOL2] != DISABLED && byte == cc[VEOL2])
        {
            self.finish_erase_printing();
            if lflag & ECHO != 0 {
                self.echo_char(byte);
            }
            self.commit(Some(byte));
            return;
        }

        self.finish_erase_printing();
        self.store(byte);
    }

    /// Appends one byte to the input queue, echoing it if echo is on.
    ///
    /// A full queue drops the byte. That is what Linux does — there is nowhere
    /// to put it and no caller to report a short write to — and it is why
    /// `IXOFF` exists: a writer that honours flow control never reaches this.
    fn store(&mut self, byte: u8) {
        if self.input.len() >= INPUT_LIMIT {
            return;
        }
        // The first character of a line fixes where that line starts on screen.
        // Everything to its left belongs to whatever the program printed — a
        // shell prompt, usually — and `ECHOE` must never back over it. Linux
        // only refreshes this on a newline, which is why erasing a tab typed
        // after a prompt that did not end in one misbehaves there; anchoring it
        // when input actually begins costs nothing and is simply right.
        if self.canonical() && self.pending == 0 {
            self.canon_column = self.column;
        }
        if self.termios.c_lflag & ECHO != 0 {
            self.echo_char(byte);
        }
        self.input.push_back(byte);
        if self.canonical() {
            self.pending += 1;
        }
    }

    /// Completes the line under edit, optionally storing a terminator.
    fn commit(&mut self, terminator: Option<u8>) {
        if let Some(byte) = terminator
            && self.input.len() < INPUT_LIMIT
        {
            self.input.push_back(byte);
            self.pending += 1;
        }
        self.lines.push_back(self.pending);
        self.pending = 0;
        // The next line starts wherever the cursor now is, which is column zero
        // after an echoed newline and wherever the program left it otherwise.
        self.canon_column = self.column;
    }

    /// Sends `VSTOP` upstream once the queue is too full to keep accepting.
    fn apply_throttle(&mut self) {
        if self.termios.c_iflag & IXOFF == 0 || self.throttled {
            return;
        }
        if self.input.len() >= IXOFF_HIGH_WATER {
            let stop = self.termios.c_cc[VSTOP];
            if stop != DISABLED {
                self.emit_raw(stop);
                self.throttled = true;
            }
        }
    }

    /// Releases an `IXOFF` throttle once the reader has drained enough.
    fn release_throttle(&mut self) {
        if !self.throttled {
            return;
        }
        if self.input.len() <= IXOFF_LOW_WATER {
            let start = self.termios.c_cc[VSTART];
            if start != DISABLED {
                self.emit_raw(start);
            }
            self.throttled = false;
        }
    }

    // -----------------------------------------------------------------------
    // Editing.
    // -----------------------------------------------------------------------

    /// Removes characters from the line under edit and un-draws them.
    fn erase(&mut self, kind: Erase) {
        let lflag = self.termios.c_lflag;
        if self.pending == 0 {
            // Nothing to erase. Linux rings the bell here when IMAXBEL is set;
            // silence is the safer choice for a layer that also drives a real
            // Windows console, where a BEL is an audible system sound.
            return;
        }

        if kind == Erase::Line {
            if lflag & ECHO == 0 {
                self.discard_pending(self.pending);
                return;
            }
            // Only the full ECHOKE/ECHOE/ECHOK trio erases the line visually.
            // With any of them missing, the terminal echoes the kill character
            // itself and starts a fresh line, which is what a printing terminal
            // did and what several editors still expect.
            if lflag & (ECHOKE | ECHOE | ECHOK) != (ECHOKE | ECHOE | ECHOK) {
                self.discard_pending(self.pending);
                self.finish_erase_printing();
                self.echo_char(self.termios.c_cc[VKILL]);
                if lflag & ECHOK != 0 {
                    self.echo_raw(b'\n');
                }
                return;
            }
        }

        let mut seen_word = false;
        loop {
            if self.pending == 0 {
                break;
            }
            let count = self.character_width_bytes();
            let byte = self.pending_byte(self.pending - count);
            if kind == Erase::Word {
                // BSD's ALTWERASE rule, which Linux adopted: skip the trailing
                // separators, then stop at the first separator after a word.
                let word = byte.is_ascii_alphanumeric() || byte == b'_';
                if word {
                    seen_word = true;
                } else if seen_word {
                    break;
                }
            }

            let target_column = self.replay_column(self.pending - count);
            self.discard_pending(count);

            if lflag & ECHO != 0 {
                if lflag & ECHOPRT != 0 {
                    // A printing terminal cannot take ink off the page, so the
                    // erased characters are echoed back between slashes.
                    if !self.erase_printing {
                        self.echo_raw(b'\\');
                        self.erase_printing = true;
                    }
                    self.echo_char(byte);
                } else if kind == Erase::Character && lflag & ECHOE == 0 {
                    // Without ECHOE the erase character is simply echoed, so the
                    // user sees what they typed rather than the effect of it.
                    self.echo_char(self.termios.c_cc[VERASE]);
                } else {
                    self.erase_to_column(target_column, byte == b'\t');
                }
            }

            if kind == Erase::Character {
                break;
            }
        }

        if self.pending == 0 && lflag & ECHO != 0 {
            self.finish_erase_printing();
        }
    }

    /// Backs the cursor up to `target`, blanking what it passes over.
    ///
    /// A tab is only backed over: the columns it occupied were never printed, so
    /// there is nothing to blank and writing spaces would erase whatever the
    /// program had put there.
    fn erase_to_column(&mut self, target: usize, was_tab: bool) {
        while self.column > target {
            if was_tab {
                self.echo_raw(0x08);
            } else {
                self.echo_raw(0x08);
                self.echo_raw(b' ');
                self.echo_raw(0x08);
            }
        }
    }

    /// Closes an `ECHOPRT` erase sequence with its trailing slash.
    fn finish_erase_printing(&mut self) {
        if self.erase_printing {
            self.erase_printing = false;
            self.echo_raw(b'/');
        }
    }

    /// Re-echoes the line under edit on a fresh line, as `VREPRINT` asks.
    fn reprint(&mut self) {
        self.finish_erase_printing();
        self.echo_char(self.termios.c_cc[VREPRINT]);
        self.echo_raw(b'\n');
        self.canon_column = self.column;
        let line: Vec<u8> = (0..self.pending).map(|at| self.pending_byte(at)).collect();
        for byte in line {
            self.echo_char(byte);
        }
    }

    /// How many bytes the last character of the pending line occupies.
    ///
    /// With `IUTF8` a multi-byte character is erased as a unit; without it every
    /// byte is its own character, which is the right answer for a terminal in a
    /// single-byte locale and the wrong one for UTF-8 — hence the flag.
    fn character_width_bytes(&self) -> usize {
        if self.termios.c_iflag & IUTF8 == 0 || self.pending == 0 {
            return 1;
        }
        let mut count = 1usize;
        while count < self.pending {
            let byte = self.pending_byte(self.pending - count);
            // 10xxxxxx is a continuation byte and never starts a character.
            if byte & 0xc0 != 0x80 {
                break;
            }
            count += 1;
        }
        count
    }

    /// Byte at `index` within the line under edit.
    fn pending_byte(&self, index: usize) -> u8 {
        let base = self.input.len() - self.pending;
        self.input.get(base + index).copied().unwrap_or(0)
    }

    /// Drops `count` bytes from the end of the line under edit.
    fn discard_pending(&mut self, count: usize) {
        for _ in 0..count.min(self.pending) {
            self.input.pop_back();
            self.pending -= 1;
        }
    }

    /// The column the cursor would be in after echoing `count` bytes of the line.
    ///
    /// Replaying is exact where an incremental count is not: a tab's width
    /// depends on every character before it, so erasing one requires knowing the
    /// column it started from rather than a fixed width.
    fn replay_column(&self, count: usize) -> usize {
        let echoctl = self.termios.c_lflag & ECHOCTL != 0;
        let utf8 = self.termios.c_iflag & IUTF8 != 0;
        let mut column = self.canon_column;
        for index in 0..count {
            let byte = self.pending_byte(index);
            match byte {
                b'\t' => column += TAB_WIDTH - (column % TAB_WIDTH),
                0x08 => column = column.saturating_sub(1),
                byte if byte < 0x20 || byte == 0x7f => {
                    if echoctl {
                        column += 2;
                    }
                }
                byte if utf8 && byte & 0xc0 == 0x80 => {}
                _ => column += 1,
            }
        }
        column
    }

    // -----------------------------------------------------------------------
    // Output.
    // -----------------------------------------------------------------------

    /// Writes bytes toward the other end, applying `OPOST` when it is set.
    ///
    /// Returns how many *source* bytes were consumed, which is short of
    /// `bytes.len()` when the output queue filled. A caller that must not lose
    /// data blocks and retries with the remainder; that is how a tty write
    /// behaves when the reader has stopped reading.
    pub fn write_output(&mut self, bytes: &[u8]) -> usize {
        if self.number != N_TTY || self.termios.c_oflag & OPOST == 0 {
            // Raw output or non-N_TTY discipline writes directly to output queue.
            let count = bytes.len().min(self.output_space());
            for byte in &bytes[..count] {
                self.track_column(*byte);
                self.output.push_back(*byte);
            }
            return count;
        }
        let mut consumed = 0;
        for byte in bytes {
            if !self.output_char(*byte) {
                break;
            }
            consumed += 1;
        }
        consumed
    }

    /// Applies `OPOST` to one byte. Returns false when the queue had no room.
    ///
    /// The room check is per source byte rather than per emitted byte: `ONLCR`
    /// turns one newline into two, and emitting the CR without the LF would
    /// leave the queue holding half a translation.
    fn output_char(&mut self, byte: u8) -> bool {
        let oflag = self.termios.c_oflag;
        match byte {
            b'\n' => {
                if oflag & ONLRET != 0 {
                    // ONLRET says the newline also performs the carriage return,
                    // so the column moves even though no CR is emitted.
                    self.column = 0;
                }
                if oflag & ONLCR != 0 {
                    if self.output_space() < 2 {
                        return false;
                    }
                    self.column = 0;
                    self.canon_column = 0;
                    self.output.push_back(b'\r');
                    self.output.push_back(b'\n');
                    return true;
                }
                if self.output_space() < 1 {
                    return false;
                }
                self.canon_column = self.column;
                self.output.push_back(b'\n');
                true
            }
            b'\r' => {
                // ONOCR suppresses a carriage return that would do nothing,
                // which keeps a program that ends every line with CR LF from
                // leaving blank lines on a terminal that already wrapped.
                if oflag & ONOCR != 0 && self.column == 0 {
                    return true;
                }
                if self.output_space() < 1 {
                    return false;
                }
                if oflag & OCRNL != 0 {
                    if oflag & ONLRET != 0 {
                        self.column = 0;
                        self.canon_column = 0;
                    }
                    self.output.push_back(b'\n');
                    return true;
                }
                self.column = 0;
                self.canon_column = 0;
                self.output.push_back(b'\r');
                true
            }
            b'\t' => {
                let spaces = TAB_WIDTH - (self.column % TAB_WIDTH);
                if oflag & TABDLY == XTABS {
                    // XTABS expands tabs into spaces for terminals that have no
                    // tab stops of their own.
                    if self.output_space() < spaces {
                        return false;
                    }
                    self.column += spaces;
                    for _ in 0..spaces {
                        self.output.push_back(b' ');
                    }
                    return true;
                }
                if self.output_space() < 1 {
                    return false;
                }
                self.column += spaces;
                self.output.push_back(b'\t');
                true
            }
            _ => {
                if self.output_space() < 1 {
                    return false;
                }
                self.track_column(byte);
                self.output.push_back(byte);
                true
            }
        }
    }

    /// Advances the column for a byte emitted without translation.
    fn track_column(&mut self, byte: u8) {
        match byte {
            b'\n' => self.column = 0,
            b'\r' => self.column = 0,
            0x08 => self.column = self.column.saturating_sub(1),
            b'\t' => self.column += TAB_WIDTH - (self.column % TAB_WIDTH),
            // A UTF-8 continuation byte is part of a character already counted.
            byte if self.termios.c_iflag & IUTF8 != 0 && byte & 0xc0 == 0x80 => {}
            byte if byte >= 0x20 && byte != 0x7f => self.column += 1,
            _ => {}
        }
    }

    /// Echoes one input byte, rendering control characters when `ECHOCTL` is set.
    ///
    /// Echo goes through the same output path as a program's writes, so `ONLCR`
    /// applies to an echoed newline and the column stays consistent between the
    /// two. That shared column is what makes `ECHOE` erase the right number of
    /// cells after a program has printed a prompt.
    fn echo_char(&mut self, byte: u8) {
        let control = byte < 0x20 || byte == 0x7f;
        if self.termios.c_lflag & ECHOCTL != 0 && control && byte != b'\t' && byte != b'\n' {
            self.echo_raw(b'^');
            // 0x03 becomes 'C' and 0x7f becomes '?', which is the conventional
            // rendering and the reason this is an exclusive-or rather than a
            // table.
            self.echo_raw(byte ^ 0x40);
            return;
        }
        self.echo_raw(byte);
    }

    /// Places one byte on the output queue through `OPOST`.
    ///
    /// Echo that does not fit is dropped rather than short-written: there is no
    /// caller waiting on a byte count, and blocking input processing on a reader
    /// that has stopped reading would wedge the terminal.
    fn echo_raw(&mut self, byte: u8) {
        if self.termios.c_oflag & OPOST != 0 {
            self.output_char(byte);
        } else {
            if self.output_space() == 0 {
                return;
            }
            self.track_column(byte);
            self.output.push_back(byte);
        }
    }

    /// Places one byte on the output queue with no processing at all.
    ///
    /// Only flow-control characters take this path: a `VSTOP` emitted by `IXOFF`
    /// is a signal to the writer's own line discipline, not text, so translating
    /// it would corrupt it.
    fn emit_raw(&mut self, byte: u8) {
        if self.output_space() == 0 {
            return;
        }
        self.output.push_back(byte);
    }
}

/// Which editing operation an erase character requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Erase {
    /// `VERASE`: one character.
    Character,
    /// `VWERASE`: one word.
    Word,
    /// `VKILL`: the whole line.
    Line,
}

/// A flat copy of a discipline's whole state.
///
/// A pseudo-terminal is shared by processes that do not share a heap: the master
/// may be held by a shell while the slave is held by a program that was `exec`ed
/// into a different Windows process. The discipline therefore cannot live in one
/// process's memory, and this is the form it travels in — loaded, operated on,
/// and stored back under the terminal's lock.
///
/// Everything the discipline needs to resume mid-line is here. Leaving out
/// `column` or `pending` would work until the first erase that crossed a process
/// boundary and then draw the wrong number of backspaces.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub termios: Termios,
    pub input: Vec<u8>,
    pub pending: usize,
    pub lines: Vec<usize>,
    pub output: Vec<u8>,
    pub column: usize,
    pub canon_column: usize,
    pub stopped: bool,
    pub throttled: bool,
    pub literal: bool,
    pub erase_printing: bool,
    pub number: i32,
}

impl Ldisc {
    /// Copies the discipline's state out for transport.
    pub fn save(&self) -> State {
        State {
            termios: self.termios,
            input: self.input.iter().copied().collect(),
            pending: self.pending,
            lines: self.lines.iter().copied().collect(),
            output: self.output.iter().copied().collect(),
            column: self.column,
            canon_column: self.canon_column,
            stopped: self.stopped,
            throttled: self.throttled,
            literal: self.literal,
            erase_printing: self.erase_printing,
            number: self.number,
        }
    }

    /// Rebuilds a discipline from transported state.
    pub fn load(state: State) -> Self {
        Self {
            termios: state.termios,
            input: state.input.into(),
            pending: state.pending,
            lines: state.lines.into(),
            output: state.output.into(),
            column: state.column,
            canon_column: state.canon_column,
            stopped: state.stopped,
            throttled: state.throttled,
            literal: state.literal,
            erase_printing: state.erase_printing,
            number: state.number,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::CS8;

    /// A discipline in the state a login shell would find: cooked, echoing.
    fn cooked() -> Ldisc {
        Ldisc::new(default_termios())
    }

    /// A discipline as `cfmakeraw` leaves it.
    fn raw() -> Ldisc {
        let mut termios = default_termios();
        make_raw(&mut termios);
        Ldisc::new(termios)
    }

    /// Everything the terminal has echoed or written so far.
    fn drain_output(ldisc: &mut Ldisc) -> Vec<u8> {
        let mut out = vec![0u8; ldisc.pending_output()];
        let count = ldisc.read_output(&mut out);
        out.truncate(count);
        out
    }

    /// Reads one line, failing the test if the read would have blocked.
    fn read_line(ldisc: &mut Ldisc) -> Vec<u8> {
        let mut buffer = [0u8; 256];
        match ldisc.read(&mut buffer) {
            Read::Data(count) => buffer[..count].to_vec(),
            other => panic!("expected data, got {other:?}"),
        }
    }

    #[test]
    fn a_canonical_read_waits_for_a_terminator_and_then_yields_one_line() {
        let mut ldisc = cooked();
        ldisc.receive(b"hello");
        // Half a line is not readable: a caller that polled and then read must
        // not block inside the read.
        assert_eq!(ldisc.readable(), 0);
        assert!(!ldisc.read_ready());
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);

        // ICRNL turns the Enter key's carriage return into the newline that
        // terminates the line.
        ldisc.receive(b"\r");
        assert!(ldisc.read_ready());
        assert_eq!(ldisc.readable(), 6);
        assert_eq!(read_line(&mut ldisc), b"hello\n");
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);
    }

    #[test]
    fn a_read_never_crosses_a_line_boundary() {
        let mut ldisc = cooked();
        ldisc.receive(b"one\rtwo\r");
        // Two complete lines are queued, but a read that could hold both must
        // still stop at the first terminator.
        let mut buffer = [0u8; 64];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(4));
        assert_eq!(&buffer[..4], b"one\n");
        assert_eq!(ldisc.read(&mut buffer), Read::Data(4));
        assert_eq!(&buffer[..4], b"two\n");
    }

    #[test]
    fn a_short_buffer_resumes_inside_the_same_line() {
        let mut ldisc = cooked();
        ldisc.receive(b"abcdef\r");
        let mut buffer = [0u8; 3];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(3));
        assert_eq!(&buffer, b"abc");
        assert_eq!(ldisc.read(&mut buffer), Read::Data(3));
        assert_eq!(&buffer, b"def");
        assert_eq!(ldisc.read(&mut buffer), Read::Data(1));
        assert_eq!(buffer[0], b'\n');
        assert_eq!(ldisc.read(&mut buffer), Read::WouldBlock);
    }

    #[test]
    fn eof_on_an_empty_line_is_a_zero_length_read_and_after_text_is_a_short_one() {
        let mut ldisc = cooked();
        // ^D with nothing typed is end of file.
        ldisc.receive(&[0x04]);
        assert!(ldisc.read_ready(), "an EOF must wake a blocked reader");
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::EndOfFile);
        // Consumed: the terminal is not permanently at end of file.
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);

        // ^D after text delivers the text with no terminator, which is how a
        // shell reads a line the user did not end with Enter.
        ldisc.receive(b"partial");
        ldisc.receive(&[0x04]);
        assert_eq!(read_line(&mut ldisc), b"partial");
    }

    #[test]
    fn verase_removes_one_character_and_erases_it_from_the_display() {
        let mut ldisc = cooked();
        ldisc.receive(b"abc");
        assert_eq!(drain_output(&mut ldisc), b"abc");
        // DEL is the default VERASE.
        ldisc.receive(&[0x7f]);
        // ECHOE draws the erase as backspace, space, backspace.
        assert_eq!(drain_output(&mut ldisc), b"\x08 \x08");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"ab\n");
    }

    #[test]
    fn erase_stops_at_the_start_of_the_line_and_never_eats_the_prompt() {
        let mut ldisc = cooked();
        // A prompt written by the program, before any input.
        ldisc.write_output(b"$ ");
        drain_output(&mut ldisc);
        ldisc.receive(b"x");
        drain_output(&mut ldisc);

        ldisc.receive(&[0x7f, 0x7f, 0x7f]);
        // One character was typed, so exactly one erase is drawn however many
        // times the key is pressed.
        assert_eq!(drain_output(&mut ldisc), b"\x08 \x08");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\n");
    }

    #[test]
    fn erasing_a_tab_backs_over_every_column_it_occupied() {
        let mut ldisc = cooked();
        // A tab from column 2 runs to the next stop at column 8, so it occupies
        // six columns and needs six backspaces to undo.
        ldisc.receive(b"ab\t");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x7f]);
        assert_eq!(
            drain_output(&mut ldisc),
            b"\x08\x08\x08\x08\x08\x08",
            "a tab must be backed over by column, with no blanking"
        );
    }

    #[test]
    fn vwerase_removes_a_word_and_its_trailing_separators() {
        let mut ldisc = cooked();
        ldisc.receive(b"alpha beta   ");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x17]); // ^W
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"alpha \n");

        // Underscores count as part of a word, matching BSD's ALTWERASE rule
        // that Linux adopted.
        let mut ldisc = cooked();
        ldisc.receive(b"a foo_bar");
        ldisc.receive(&[0x17]);
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"a \n");
    }

    #[test]
    fn vkill_discards_the_line_and_erases_it_when_echoke_is_set() {
        let mut ldisc = cooked();
        ldisc.receive(b"discard me");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x15]); // ^U
        // Ten characters typed, ten erases drawn.
        assert_eq!(drain_output(&mut ldisc), b"\x08 \x08".repeat(10));
        ldisc.receive(b"kept\r");
        assert_eq!(read_line(&mut ldisc), b"kept\n");
    }

    #[test]
    fn vkill_without_echoke_echoes_the_character_and_starts_a_new_line() {
        let mut ldisc = cooked();
        ldisc.termios.c_lflag &= !ECHOKE;
        ldisc.receive(b"gone");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x15]);
        // ECHOCTL renders ^U, and ECHOK adds the newline; ONLCR makes it CR LF.
        assert_eq!(drain_output(&mut ldisc), b"^U\r\n");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\n");
    }

    #[test]
    fn vlnext_makes_the_following_control_character_ordinary_text() {
        let mut ldisc = cooked();
        // ^V then ^C: the interrupt character must reach the reader as a byte.
        let signals = ldisc.receive(&[0x16, 0x03]);
        assert!(signals.is_empty(), "a quoted ^C must not raise SIGINT");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\x03\n");

        // And the erase character itself, which is the reason VLNEXT exists.
        let mut ldisc = cooked();
        ldisc.receive(b"ab");
        ldisc.receive(&[0x16, 0x7f]);
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"ab\x7f\n");
    }

    #[test]
    fn vreprint_redraws_the_line_under_edit() {
        let mut ldisc = cooked();
        ldisc.receive(b"typed");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x12]); // ^R
        // The reprint character itself, a newline, then the line again.
        assert_eq!(drain_output(&mut ldisc), b"^R\r\ntyped");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"typed\n");
    }

    #[test]
    fn veol_and_veol2_terminate_a_line_without_being_newlines() {
        let mut ldisc = cooked();
        ldisc.termios.c_cc[VEOL] = b';';
        ldisc.termios.c_cc[VEOL2] = b'!';
        ldisc.receive(b"first;");
        assert_eq!(read_line(&mut ldisc), b"first;");
        ldisc.receive(b"second!");
        assert_eq!(read_line(&mut ldisc), b"second!");

        // VEOL2 is an IEXTEN extension and must stop terminating without it.
        ldisc.termios.c_lflag &= !IEXTEN;
        ldisc.receive(b"third!");
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);
    }

    #[test]
    fn echoctl_renders_control_characters_as_carets() {
        let mut ldisc = cooked();
        // ^A is 0x01 and prints as "^A"; DEL prints as "^?".
        ldisc.receive(&[0x01]);
        assert_eq!(drain_output(&mut ldisc), b"^A");
        // Tab and newline are never rendered as carets: they have real cursor
        // effects that the caret form would destroy.
        ldisc.receive(b"\t");
        assert_eq!(drain_output(&mut ldisc), b"\t");

        ldisc.termios.c_lflag &= !ECHOCTL;
        ldisc.receive(&[0x01]);
        assert_eq!(drain_output(&mut ldisc), &[0x01]);
    }

    #[test]
    fn echonl_echoes_the_newline_even_with_echo_off() {
        let mut ldisc = cooked();
        ldisc.termios.c_lflag &= !ECHO;
        ldisc.termios.c_lflag |= ECHONL;
        ldisc.receive(b"secret\r");
        // The password itself is not echoed; the newline is, so the cursor moves
        // off the prompt line.
        assert_eq!(drain_output(&mut ldisc), b"\r\n");
        assert_eq!(read_line(&mut ldisc), b"secret\n");
    }

    #[test]
    fn vintr_raises_sigint_flushes_both_queues_and_echoes_the_character() {
        let mut ldisc = cooked();
        ldisc.receive(b"half typed");
        ldisc.write_output(b"pending output");
        let signals = ldisc.receive(&[0x03]);
        assert_eq!(signals, vec![SIGINT]);
        // The discarded output is gone and only the echoed ^C remains.
        assert_eq!(drain_output(&mut ldisc), b"^C");
        assert_eq!(ldisc.queued(), 0);
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);
    }

    #[test]
    fn noflsh_keeps_the_queues_while_still_raising_the_signal() {
        let mut ldisc = cooked();
        ldisc.termios.c_lflag |= NOFLSH;
        ldisc.receive(b"kept\r");
        drain_output(&mut ldisc);
        let signals = ldisc.receive(&[0x03]);
        assert_eq!(signals, vec![SIGINT]);
        // NOFLSH suppresses the flush, never the signal.
        assert_eq!(read_line(&mut ldisc), b"kept\n");
    }

    #[test]
    fn vquit_and_vsusp_map_to_their_own_signals() {
        let mut ldisc = cooked();
        assert_eq!(ldisc.receive(&[0x1c]), vec![SIGQUIT]);
        assert_eq!(ldisc.receive(&[0x1a]), vec![SIGTSTP]);
        // Several control characters in one burst all generate their signals;
        // dropping the later ones would lose a ^C typed alongside a ^Z.
        assert_eq!(ldisc.receive(&[0x03, 0x1a]), vec![SIGINT, SIGTSTP]);
    }

    #[test]
    fn remapping_vintr_moves_the_signal_with_it() {
        let mut ldisc = cooked();
        ldisc.termios.c_cc[VINTR] = b'q';
        assert_eq!(ldisc.receive(b"q"), vec![SIGINT]);
        // The old character is now ordinary input, which is the half of a
        // remapping that a console mode bit cannot express.
        assert!(ldisc.receive(&[0x03]).is_empty());
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\x03\n");
    }

    #[test]
    fn clearing_isig_turns_control_characters_into_data() {
        let mut ldisc = cooked();
        ldisc.termios.c_lflag &= !ISIG;
        assert!(ldisc.receive(&[0x03]).is_empty());
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\x03\n");
    }

    #[test]
    fn a_disabled_control_character_generates_nothing() {
        let mut ldisc = cooked();
        // A c_cc slot of _POSIX_VDISABLE means the function has no key. Zero is
        // a real byte, so the check has to precede the comparison.
        ldisc.termios.c_cc[VINTR] = DISABLED;
        assert!(ldisc.receive(&[0x00]).is_empty());
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\x00\n");
    }

    #[test]
    fn ixon_suspends_and_resumes_output_without_delivering_the_characters() {
        let mut ldisc = cooked();
        ldisc.receive(&[0x13]); // ^S
        assert!(ldisc.output_stopped());
        ldisc.receive(&[0x11]); // ^Q
        assert!(!ldisc.output_stopped());
        // Neither character reaches the reader.
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\n");

        // Without IXON they are ordinary bytes.
        let mut ldisc = cooked();
        ldisc.termios.c_iflag &= !IXON;
        ldisc.receive(&[0x13, b'\r']);
        assert_eq!(read_line(&mut ldisc), b"\x13\n");
        assert!(!ldisc.output_stopped());
    }

    #[test]
    fn ixany_restarts_stopped_output_on_any_key() {
        let mut ldisc = cooked();
        ldisc.termios.c_iflag |= IXANY;
        ldisc.receive(&[0x13]);
        assert!(ldisc.output_stopped());
        ldisc.receive(b"z");
        assert!(!ldisc.output_stopped(), "IXANY should restart on any byte");
        // The restarting byte is still delivered; only VSTART and VSTOP are eaten.
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"z\n");
    }

    #[test]
    fn ixoff_sends_a_stop_when_the_queue_fills_and_a_start_when_it_drains() {
        let mut ldisc = raw();
        ldisc.termios.c_iflag |= IXOFF;
        ldisc.receive(&vec![b'x'; IXOFF_HIGH_WATER]);
        let output = drain_output(&mut ldisc);
        assert_eq!(
            output.last().copied(),
            Some(0x13),
            "a full input queue should emit VSTOP"
        );

        // Draining below the low-water mark releases the writer again.
        let mut sink = vec![0u8; IXOFF_HIGH_WATER];
        assert_eq!(ldisc.read(&mut sink), Read::Data(IXOFF_HIGH_WATER));
        assert_eq!(drain_output(&mut ldisc), &[0x11]);
    }

    #[test]
    fn a_full_input_queue_drops_bytes_rather_than_growing() {
        let mut ldisc = raw();
        ldisc.receive(&vec![b'x'; INPUT_LIMIT * 2]);
        assert_eq!(ldisc.queued(), INPUT_LIMIT);
    }

    #[test]
    fn istrip_igncr_icrnl_and_inlcr_rewrite_input_before_anything_else_sees_it() {
        let mut ldisc = raw();
        ldisc.termios.c_iflag = ISTRIP;
        ldisc.receive(&[0xc1]);
        let mut buffer = [0u8; 4];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(1));
        assert_eq!(buffer[0], 0x41, "ISTRIP should clear the eighth bit");

        let mut ldisc = raw();
        ldisc.termios.c_iflag = IGNCR;
        ldisc.receive(b"a\rb");
        assert_eq!(ldisc.read(&mut buffer), Read::Data(2));
        assert_eq!(&buffer[..2], b"ab");

        let mut ldisc = raw();
        ldisc.termios.c_iflag = INLCR;
        ldisc.receive(b"\n");
        assert_eq!(ldisc.read(&mut buffer), Read::Data(1));
        assert_eq!(buffer[0], b'\r');
    }

    #[test]
    fn iutf8_erases_a_multibyte_character_as_one_unit() {
        let mut ldisc = cooked();
        ldisc.termios.c_iflag |= IUTF8;
        // U+00E9 is two bytes; one erase must remove both and back over one cell.
        ldisc.receive("é".as_bytes());
        drain_output(&mut ldisc);
        ldisc.receive(&[0x7f]);
        assert_eq!(drain_output(&mut ldisc), b"\x08 \x08");
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), b"\n");

        // Without IUTF8 the same erase removes one byte, leaving half a
        // character — which is the historically correct single-byte behaviour.
        let mut ldisc = cooked();
        ldisc.receive("é".as_bytes());
        ldisc.receive(&[0x7f]);
        ldisc.receive(b"\r");
        assert_eq!(read_line(&mut ldisc), &[0xc3, b'\n']);
    }

    #[test]
    fn vmin_and_vtime_classify_into_the_four_documented_policies() {
        let mut ldisc = raw();
        assert_eq!(ldisc.read_policy(), ReadPolicy::Blocking { min: 1 });

        ldisc.termios.c_cc[VMIN] = 0;
        ldisc.termios.c_cc[VTIME] = 0;
        assert_eq!(ldisc.read_policy(), ReadPolicy::Poll);

        ldisc.termios.c_cc[VMIN] = 0;
        ldisc.termios.c_cc[VTIME] = 5;
        assert_eq!(
            ldisc.read_policy(),
            ReadPolicy::ReadTimer { timeout_ms: 500 }
        );

        ldisc.termios.c_cc[VMIN] = 4;
        ldisc.termios.c_cc[VTIME] = 0;
        assert_eq!(ldisc.read_policy(), ReadPolicy::Blocking { min: 4 });

        ldisc.termios.c_cc[VMIN] = 4;
        ldisc.termios.c_cc[VTIME] = 2;
        assert_eq!(
            ldisc.read_policy(),
            ReadPolicy::Interbyte {
                min: 4,
                timeout_ms: 200
            }
        );

        // Canonical mode ignores both, which is the rule programs forget.
        ldisc.termios.c_lflag |= ICANON;
        assert_eq!(ldisc.read_policy(), ReadPolicy::Canonical);
    }

    #[test]
    fn opost_translates_newlines_and_carriage_returns() {
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | ONLCR;
        ldisc.write_output(b"a\nb");
        assert_eq!(drain_output(&mut ldisc), b"a\r\nb");

        // OCRNL is the other direction.
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | OCRNL;
        ldisc.write_output(b"a\rb");
        assert_eq!(drain_output(&mut ldisc), b"a\nb");

        // Without OPOST nothing is rewritten at all, which is what a full-screen
        // program depends on when it emits escape sequences.
        let mut ldisc = raw();
        ldisc.write_output(b"a\nb\r");
        assert_eq!(drain_output(&mut ldisc), b"a\nb\r");
    }

    #[test]
    fn onocr_suppresses_a_carriage_return_at_column_zero() {
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | ONOCR;
        // Already at column zero: the CR would do nothing and is dropped.
        ldisc.write_output(b"\r");
        assert_eq!(drain_output(&mut ldisc), b"");
        // After printing, the CR is real.
        ldisc.write_output(b"ab\r");
        assert_eq!(drain_output(&mut ldisc), b"ab\r");
    }

    #[test]
    fn onlret_moves_the_column_without_emitting_a_carriage_return() {
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | ONLRET | ONOCR;
        ldisc.write_output(b"ab\n");
        assert_eq!(drain_output(&mut ldisc), b"ab\n");
        // The newline performed the return, so ONOCR now drops the CR that
        // follows it — the check that proves the column really moved.
        ldisc.write_output(b"\r");
        assert_eq!(drain_output(&mut ldisc), b"");
    }

    #[test]
    fn xtabs_expands_tabs_to_the_next_stop() {
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | XTABS;
        ldisc.write_output(b"ab\tc");
        assert_eq!(drain_output(&mut ldisc), b"ab      c");
        assert_eq!(ldisc.termios.c_oflag & TABDLY, XTABS);

        // Without XTABS the tab is passed through for the terminal to expand.
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST;
        ldisc.write_output(b"ab\tc");
        assert_eq!(drain_output(&mut ldisc), b"ab\tc");
    }

    #[test]
    fn a_full_output_queue_short_writes_rather_than_dropping() {
        let mut ldisc = raw();
        let payload = vec![b'x'; OUTPUT_LIMIT + 100];
        let written = ldisc.write_output(&payload);
        assert_eq!(written, OUTPUT_LIMIT);
        assert_eq!(ldisc.pending_output(), OUTPUT_LIMIT);
        // The caller retries with the remainder once the reader drains.
        let mut sink = vec![0u8; 200];
        assert_eq!(ldisc.read_output(&mut sink), 200);
        assert_eq!(ldisc.write_output(&payload[written..]), 100);
    }

    #[test]
    fn onlcr_never_leaves_half_a_translation_in_a_full_queue() {
        let mut ldisc = raw();
        ldisc.termios.c_oflag = OPOST | ONLCR;
        // Fill to exactly one byte short, then offer a newline that needs two.
        ldisc.write_output(&vec![b'x'; OUTPUT_LIMIT - 1]);
        assert_eq!(ldisc.write_output(b"\n"), 0);
        assert_eq!(ldisc.pending_output(), OUTPUT_LIMIT - 1);
    }

    #[test]
    fn echo_shares_the_output_column_with_program_writes() {
        let mut ldisc = cooked();
        // A prompt three columns wide, then a tab: the tab must run to column 8,
        // which is only right if echo and program output share one column.
        ldisc.write_output(b"$ >");
        ldisc.receive(b"\t");
        assert_eq!(drain_output(&mut ldisc), b"$ >\t");
        ldisc.receive(&[0x7f]);
        // Columns 3 through 8 is five backspaces.
        assert_eq!(drain_output(&mut ldisc), b"\x08\x08\x08\x08\x08");
    }

    #[test]
    fn leaving_canonical_mode_keeps_the_bytes_already_typed() {
        let mut ldisc = cooked();
        ldisc.receive(b"typed");
        // A program switching to raw mode must still be able to read what the
        // user typed at the prompt; discarding it here would lose input that
        // tcsetattr without TCSAFLUSH promised to keep.
        let mut raw_termios = ldisc.termios;
        make_raw(&mut raw_termios);
        ldisc.retune(raw_termios);
        let mut buffer = [0u8; 16];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(5));
        assert_eq!(&buffer[..5], b"typed");
    }

    #[test]
    fn entering_canonical_mode_leaves_the_queue_as_an_unterminated_line() {
        let mut ldisc = raw();
        ldisc.receive(b"pending");
        let mut cooked_termios = ldisc.termios;
        cooked_termios.c_lflag |= ICANON;
        ldisc.retune(cooked_termios);
        // Not readable until a terminator arrives, and then readable in full.
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);
        ldisc.receive(b"\n");
        assert_eq!(read_line(&mut ldisc), b"pending\n");
    }

    #[test]
    fn tcflush_discards_the_line_under_edit_as_well_as_the_complete_ones() {
        let mut ldisc = cooked();
        ldisc.receive(b"done\rhalf");
        ldisc.write_output(b"output");
        ldisc.flush(Queue::Input);
        assert_eq!(ldisc.queued(), 0);
        assert_eq!(ldisc.read(&mut [0u8; 16]), Read::WouldBlock);
        // Output is a separate queue and must survive an input flush.
        assert!(ldisc.pending_output() > 0);
        ldisc.flush(Queue::Output);
        assert_eq!(ldisc.pending_output(), 0);
    }

    #[test]
    fn a_break_honours_ignbrk_brkint_and_the_plain_case() {
        let mut ldisc = cooked();
        assert_eq!(ldisc.receive_break(), vec![SIGINT]);

        ldisc.termios.c_iflag = IGNBRK;
        assert!(ldisc.receive_break().is_empty());
        assert_eq!(ldisc.queued(), 0);

        // Neither flag: the break arrives as a NUL byte.
        let mut ldisc = raw();
        ldisc.termios.c_iflag = 0;
        assert!(ldisc.receive_break().is_empty());
        let mut buffer = [0xffu8; 4];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(1));
        assert_eq!(buffer[0], 0);
    }

    #[test]
    fn echoprt_writes_the_erased_characters_back_between_slashes() {
        let mut ldisc = cooked();
        ldisc.termios.c_lflag |= ECHOPRT;
        ldisc.receive(b"abc");
        drain_output(&mut ldisc);
        ldisc.receive(&[0x7f, 0x7f]);
        // A hard-copy terminal cannot unprint, so it shows what was removed.
        assert_eq!(drain_output(&mut ldisc), b"\\cb");
        // The next ordinary character closes the sequence.
        ldisc.receive(b"z");
        assert_eq!(drain_output(&mut ldisc), b"/z");
    }

    #[test]
    fn the_default_termios_is_the_one_a_login_shell_expects() {
        let ldisc = Ldisc::default();
        assert!(ldisc.canonical());
        assert_eq!(ldisc.termios.c_cflag & CSIZE, CS8);
        assert_eq!(ldisc.read_policy(), ReadPolicy::Canonical);

        // A pty starts from the fuller default, which the console layer's own
        // `Termios::default` predates and cannot describe.
        let pty = default_termios();
        assert_ne!(pty.c_lflag & ECHOCTL, 0);
        assert_ne!(pty.c_lflag & ECHOKE, 0);
        assert_eq!(pty.c_cc[VWERASE], 0x17);
        assert_eq!(pty.c_cc[VLNEXT], 0x16);
        assert_eq!(pty.c_cc[VEOL], DISABLED);
        // PENDIN is a kernel-internal bit that must never be set in the value a
        // guest reads back from a freshly opened terminal.
        assert_eq!(pty.c_lflag & PENDIN, 0);
    }

    #[test]
    fn make_raw_clears_the_bits_cfmakeraw_names_and_no_others() {
        // Start from all-ones so every cleared bit is observable and every
        // untouched bit is too.
        let mut termios = Termios {
            c_iflag: u32::MAX,
            c_oflag: u32::MAX,
            c_cflag: u32::MAX,
            c_lflag: u32::MAX,
            c_line: 7,
            c_cc: [0xaa; 32],
            c_ispeed: crate::pty::B38400,
            c_ospeed: crate::pty::B38400,
        };
        make_raw(&mut termios);
        let cleared_iflag = IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON;
        assert_eq!(termios.c_iflag, !cleared_iflag);
        assert_eq!(termios.c_oflag, !OPOST);
        assert_eq!(termios.c_lflag, !(ECHO | ECHONL | ICANON | IEXTEN | ISIG));
        // Eight data bits plus a parity bit is nine, which is why PARENB goes
        // with CSIZE — the clause glibc has and a shorter cfmakeraw forgets.
        assert_eq!(termios.c_cflag & CSIZE, CS8);
        assert_eq!(termios.c_cflag & PARENB, 0);
        assert_eq!(termios.c_cc[VMIN], 1);
        assert_eq!(termios.c_cc[VTIME], 0);
        // Everything cfmakeraw does not name is left alone.
        assert_eq!(termios.c_cc[VINTR], 0xaa);
        assert_eq!(termios.c_line, 7);
    }

    #[test]
    fn saving_and_loading_preserves_a_line_that_is_half_edited() {
        // The state crosses a process boundary between every operation on a
        // shared pty, so anything left out of the copy is a bug that only
        // appears when the master and the slave are in different processes.
        let mut ldisc = cooked();
        ldisc.write_output(b"prompt> ");
        ldisc.receive(b"ab\tc");
        let mut carried = Ldisc::load(ldisc.save());
        assert_eq!(carried.queued(), ldisc.queued());
        assert_eq!(carried.pending_output(), ldisc.pending_output());

        // The proof that the column travelled: an erase after the round trip
        // draws exactly what it would have drawn before it.
        drain_output(&mut ldisc);
        drain_output(&mut carried);
        ldisc.receive(&[0x7f, 0x7f]);
        carried.receive(&[0x7f, 0x7f]);
        let original = drain_output(&mut ldisc);
        assert!(
            !original.is_empty(),
            "the erase should have drawn something"
        );
        assert_eq!(original, drain_output(&mut carried));
    }

    #[test]
    fn line_discipline_validation_accepts_standard_linux_disciplines() {
        for valid in [
            N_TTY,
            N_SLIP,
            N_MOUSE,
            N_PPP,
            N_STRIP,
            N_AX25,
            N_X25,
            N_6PACK,
            N_MASC,
            N_R3964,
            N_PROFIBUS_FDL,
            N_IRDA,
            N_SMSBLOCK,
            N_HDLC,
            N_SYNC_PPP,
            N_HCI,
            N_GIGASET_M101,
            N_SLCAN,
            N_GSM0710,
            N_TI_WL,
            N_TRACESINK,
            N_TRACEROUTER,
            N_NCI,
            N_SPEAKUP,
            N_NULL,
        ] {
            assert!(is_valid_ldisc(valid), "discipline {valid} should be valid");
        }
        for invalid in [-1, 18, 19, 20, 28, 99, 1000] {
            assert!(
                !is_valid_ldisc(invalid),
                "discipline {invalid} should be invalid"
            );
        }
    }

    #[test]
    fn raw_line_disciplines_passthrough_bytes_without_editing_or_signals() {
        for discipline in [N_NULL, N_SLIP, N_PPP, N_HDLC] {
            let mut ldisc = cooked();
            ldisc.set_number(discipline);
            assert_eq!(ldisc.number, discipline);
            assert!(!ldisc.canonical());

            // Sending control characters (^C = 0x03, ^Z = 0x1a) produces no signals.
            let signals = ldisc.receive(&[0x03, b'a', b'b', 0x1a]);
            assert!(
                signals.is_empty(),
                "raw discipline must not generate signals"
            );

            // Input is queued directly and readable without waiting for newline.
            assert_eq!(ldisc.queued(), 4);
            assert_eq!(ldisc.readable(), 4);
            assert!(ldisc.read_ready());

            let mut buffer = [0u8; 8];
            match ldisc.read(&mut buffer) {
                Read::Data(count) => {
                    assert_eq!(count, 4);
                    assert_eq!(&buffer[..4], &[0x03, b'a', b'b', 0x1a]);
                }
                other => panic!("expected Read::Data, got {other:?}"),
            }

            // Output is written directly without ONLCR (bare \n stays \n).
            let written = ldisc.write_output(b"hello\n");
            assert_eq!(written, 6);
            assert_eq!(drain_output(&mut ldisc), b"hello\n");
        }
    }

    #[test]
    fn switching_between_n_tty_and_raw_discipline_reconciles_buffered_input() {
        let mut ldisc = cooked();
        // Type partial line in N_TTY canonical mode
        ldisc.receive(b"partial");
        assert_eq!(ldisc.readable(), 0);

        // Switch to N_NULL: pending line becomes readable raw data
        ldisc.set_number(N_NULL);
        assert_eq!(ldisc.readable(), 7);

        let mut buffer = [0u8; 16];
        assert_eq!(ldisc.read(&mut buffer), Read::Data(7));
        assert_eq!(&buffer[..7], b"partial");

        // Switch back to N_TTY
        ldisc.set_number(N_TTY);
        assert_eq!(ldisc.number, N_TTY);
        assert!(ldisc.canonical());
    }
}
