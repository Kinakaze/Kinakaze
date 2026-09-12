//! Mount table, login records, shadow entries and the system log.
//!
//! Four Linux "system databases" live here. None of them exists on Windows in
//! the shape Linux describes, so each one is either reconstructed from a real
//! Windows source or reports its absence, and every substitution is documented
//! at the site that makes it. Following [`crate::userdb`], nothing here reports a
//! plausible-looking value in place of one it could have queried.
//!
//! Four decisions are load-bearing enough to state up front.
//!
//! **The mount table is real.** `/etc/mtab` and `/proc/mounts` are synthesized
//! from `GetLogicalDriveStringsW`, `GetVolumeInformationW` and `GetDriveTypeW`,
//! so `df` and a bare `mount` print this machine's actual volumes with their
//! actual filesystem types. Mount points follow the VFS convention in
//! [`kinakaze_vfs::path`] — `C:\` is reachable as `/c` — because a path `df`
//! prints must be a path the guest can then `stat`.
//!
//! **The login database is the guest's own file, and it starts out empty.**
//! `/var/run/utmp` is read and written for real through the VFS. It is *not*
//! synthesized from `WTSEnumerateSessionsW`: see the utmp section for why a
//! Windows desktop session cannot be reshaped into a `utmpx` line without
//! inventing the three fields that matter most. So `who` prints nothing until
//! the guest itself records a login, which is the truth about a process that has
//! no Unix logins — and it is what Linux itself reports in a container whose
//! `/var/run/utmp` has not been created.
//!
//! **The shadow database reports absence**, exactly as [`crate::userdb`] does,
//! and for the same reason: `struct spwd` exists to carry a password hash, and
//! there is no value that field could hold that would not be a lie. The
//! difference here is the *encoding* of that absence — `getspnam_r` reports "no
//! such entry" as 0 with a null result pointer, not as an error.
//!
//! **The system log goes to the Windows Event Log, and to stderr when that is
//! unavailable.** `ReportEventW` is the real structural counterpart to a syslog
//! socket, and Linux priorities map onto Windows event types. This was measured
//! rather than assumed: an unprivileged process can register an unregistered
//! source name and write to the Application log, so the Event Log is the normal
//! path and stderr is the genuine fallback. The order attempted is spelled out
//! at [`deliver`].

use core::cell::UnsafeCell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use kinakaze_vfs::{EINVAL, EIO, ERANGE, errno_from_win32};

/// `EROFS`, which the VFS error list does not carry.
const EROFS: i32 = 30;

// Win32 entry points this module queries directly.
//
// Declared inline rather than imported from `windows-sys`, whose enabled feature
// set here covers `Win32_System_Threading` only. Widening that feature list is a
// change to a shared manifest; declaring the four signatures needed keeps the
// module self-contained, which is the same choice [`crate::userdb`] made.
//
// Every signature matches the Windows headers: `BOOL` is `i32`, a `DWORD` is
// `u32`, and each buffer length is counted in `WCHAR`s including the terminator.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `GetLogicalDriveStringsW`: every mounted drive root, NUL-separated and
    /// terminated by an empty string.
    fn GetLogicalDriveStringsW(length: u32, buffer: *mut u16) -> u32;
    /// `GetVolumeInformationW`: label, serial, flags and filesystem name.
    fn GetVolumeInformationW(
        root: *const u16,
        volume_name: *mut u16,
        volume_name_size: u32,
        serial: *mut u32,
        maximum_component_length: *mut u32,
        filesystem_flags: *mut u32,
        filesystem_name: *mut u16,
        filesystem_name_size: u32,
    ) -> i32;
    /// `GetDriveTypeW`: fixed, removable, network, optical or RAM.
    fn GetDriveTypeW(root: *const u16) -> u32;
    /// `GetLastError`: the failure reason for the calls above.
    fn GetLastError() -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    /// `RegisterEventSourceW`: opens a handle to an Event Log source.
    ///
    /// Succeeds for an unregistered source name too, in which case the entry is
    /// written to the Application log without a message-table description.
    fn RegisterEventSourceW(server: *const u16, source: *const u16) -> *mut c_void;
    /// `DeregisterEventSource`: closes an Event Log handle.
    fn DeregisterEventSource(handle: *mut c_void) -> i32;
    /// `ReportEventW`: appends one record to the Event Log.
    fn ReportEventW(
        handle: *mut c_void,
        event_type: u16,
        category: u16,
        event_id: u32,
        user_sid: *mut c_void,
        strings: u16,
        data_size: u32,
        string_array: *const *const u16,
        raw_data: *const c_void,
    ) -> i32;
}

// `GetDriveTypeW` results. Windows defines seven: 0 unknown, 1 no root
// directory, 2 removable, 3 fixed, 4 remote, 5 optical, 6 RAM disk. Only the four
// this module distinguishes are named; the rest are handled by the catch-all in
// [`DriveEntry::options`], because there is no Linux mount option that says
// "RAM disk" or "removable" and inventing one would be a claim Windows did not
// make.
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

/// `FILE_READ_ONLY_VOLUME`, the one `GetVolumeInformationW` flag that maps onto a
/// Linux mount option.
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

/// Encodes a Rust string as a NUL-terminated UTF-16 buffer for a Win32 call.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Decodes a NUL-terminated UTF-16 buffer, stopping at the first terminator.
///
/// The written length a Win32 call reports is not trusted: some include the
/// terminator and some do not, so the NUL is the authority.
fn narrow(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

// ---------------------------------------------------------------------------
// The mount table.
//
// This is the one database in this module that can be answered in full, and the
// answer is this machine's real volumes. `GetLogicalDriveStringsW` enumerates
// them, `GetVolumeInformationW` names the filesystem and reveals whether the
// volume is read-only, and `GetDriveTypeW` distinguishes fixed from removable
// from network. BusyBox `df` and a bare `mount` read exactly this table, so the
// output is directly visible to a user.
//
// Two entries are not drives, and both are true. `/` is the directory the VFS
// exposes as the guest's root, which is genuinely a mount point in the guest
// namespace, and `/proc` is served by `kinakaze_vfs::procfs`. When the root
// directory happens to live on a drive that is also listed, the volume appears
// twice under two paths — which is the same thing Linux reports for a bind
// mount, and both rows are individually correct. GNU `df` would collapse them by
// device; BusyBox does not, so both are shown.
// ---------------------------------------------------------------------------

/// The Linux x86_64 `struct mntent`.
///
/// Layout is ABI: BusyBox was compiled against glibc's `<mntent.h>` and reads
/// these offsets directly. Four pointers then two `int`s, so on x86_64 the
/// offsets are 0, 8, 16, 24, 32, 36 and the struct is 40 bytes with 8-byte
/// alignment. The `offset_of!` test at the bottom of this file pins every one.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MntEnt {
    /// The device. See [`DriveEntry::device`] for why this is not a `/dev` node.
    pub mnt_fsname: *mut c_char,
    /// The mount point, in the guest's Linux namespace.
    pub mnt_dir: *mut c_char,
    /// The filesystem type.
    pub mnt_type: *mut c_char,
    /// A comma-separated option list.
    pub mnt_opts: *mut c_char,
    /// Dump frequency. Always 0, which is what `/proc/mounts` reports.
    pub mnt_freq: c_int,
    /// `fsck` pass number. Always 0, as above.
    pub mnt_passno: c_int,
}

impl MntEnt {
    /// An all-null entry, used as the initial value of a stream's scratch slot.
    const fn empty() -> Self {
        Self {
            mnt_fsname: ptr::null_mut(),
            mnt_dir: ptr::null_mut(),
            mnt_type: ptr::null_mut(),
            mnt_opts: ptr::null_mut(),
            mnt_freq: 0,
            mnt_passno: 0,
        }
    }
}

/// One mount table row, owning the strings its `mntent` will point at.
///
/// The owned form exists because a synthesized row has no backing text to point
/// into. `Clone` is what lets the returned row outlive its stream; see
/// [`RETURNED_ROW`].
#[derive(Clone)]
struct Row {
    device: CString,
    directory: CString,
    filesystem: CString,
    options: CString,
    frequency: c_int,
    pass: c_int,
}

impl Row {
    /// Builds a row, replacing any interior NUL rather than failing.
    ///
    /// A NUL cannot occur in a Windows path, a filesystem name or an option
    /// list, so the replacement is unreachable; it keeps the function total
    /// instead of introducing an error path no caller could trigger.
    fn new(
        device: &str,
        directory: &str,
        filesystem: &str,
        options: &str,
        frequency: c_int,
        pass: c_int,
    ) -> Self {
        let clean = |text: &str| {
            CString::new(text.replace('\0', "")).unwrap_or_else(|_| CString::from(c""))
        };
        Self {
            device: clean(device),
            directory: clean(directory),
            filesystem: clean(filesystem),
            options: clean(options),
            frequency,
            pass,
        }
    }

    /// Projects this row into the C struct, borrowing its own storage.
    fn as_mntent(&self) -> MntEnt {
        MntEnt {
            mnt_fsname: self.device.as_ptr().cast_mut(),
            mnt_dir: self.directory.as_ptr().cast_mut(),
            mnt_type: self.filesystem.as_ptr().cast_mut(),
            mnt_opts: self.options.as_ptr().cast_mut(),
            mnt_freq: self.frequency,
            mnt_passno: self.pass,
        }
    }
}

/// What `GetVolumeInformationW` and `GetDriveTypeW` report about one drive.
struct DriveEntry {
    /// The Windows drive root, `C:\`.
    root: String,
    /// The filesystem name, lowercased: `ntfs`, `fat32`, `exfat`.
    filesystem: String,
    /// Whether the volume refuses writes.
    read_only: bool,
    /// The `GetDriveTypeW` classification.
    kind: u32,
}

impl DriveEntry {
    /// The `mnt_fsname` for this drive: the Windows volume spelling, `C:`.
    ///
    /// Deliberately not `/dev/sda1`. There is no block device node behind a
    /// Windows volume that a guest could open, so a `/dev` path would be a
    /// fabrication that invites `fsck` or `dd` to act on it. `C:` is the name
    /// Windows gives the volume and the name a user recognizes, and its shape
    /// makes it self-evidently not a Unix device node.
    fn device(&self) -> String {
        self.root.trim_end_matches(['\\', '/']).to_string()
    }

    /// The guest mount point, following [`kinakaze_vfs::resolve_linux_path`].
    ///
    /// The VFS maps a single-letter first component onto a drive, so `C:\`
    /// round-trips as `/c`. Reporting anything else would print a path in `df`
    /// that the guest could not then `stat`.
    fn mount_point(&self) -> Option<String> {
        let letter = self.root.chars().next()?;
        letter
            .is_ascii_alphabetic()
            .then(|| format!("/{}", letter.to_ascii_lowercase()))
    }

    /// The option list, built only from facts Windows actually reported.
    ///
    /// `rw` or `ro` comes from `FILE_READ_ONLY_VOLUME`. The remaining entries
    /// restate the drive type, which is the one other thing known about the
    /// volume and the closest Linux has to it. Nothing common in Linux mount
    /// output — `relatime`, `nosuid`, `data=ordered` — appears, because Windows
    /// reports no such property and listing one would be invention.
    fn options(&self) -> String {
        let mut options = String::from(if self.read_only { "ro" } else { "rw" });
        match self.kind {
            // A network redirector really is a remote filesystem, and `_netdev`
            // is the option Linux uses to say so.
            DRIVE_REMOTE => options.push_str(",_netdev"),
            // Optical media is read-only in the sense Linux means it, even when
            // the volume flags did not say so.
            DRIVE_CDROM if !self.read_only => options.push_str(",ro"),
            // A fixed disk is the ordinary case and adds nothing: Linux would
            // show `relatime` or `data=ordered` here, and Windows reports no
            // property that either of those describes.
            DRIVE_FIXED => {}
            _ => {}
        }
        options
    }
}

/// Reads the drive roots Windows currently has mounted.
///
/// An empty result means the call failed, which is reported to the caller rather
/// than smoothed over: a mount table with no drives would be a false statement
/// about the machine.
fn drive_roots() -> Result<Vec<String>, i32> {
    // 26 drives at 4 WCHARs each plus the final terminator fits well inside
    // this, and the call reports the size it wanted if it somehow did not.
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` is writable for its own length in WCHARs.
    let written = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
    if written == 0 {
        // SAFETY: read immediately after the failing call on this thread.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let written = (written as usize).min(buffer.len());
    Ok(buffer[..written]
        .split(|unit| *unit == 0)
        .filter(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect())
}

/// Describes one drive, or `None` when Windows will not talk about it.
///
/// A drive that is present but unreadable — an empty optical bay, a disconnected
/// network mapping — makes `GetVolumeInformationW` fail. Such a drive is dropped
/// from the table rather than listed with a guessed filesystem, because `df`
/// would then try to `statfs` a volume that is not there.
fn describe_drive(root: &str) -> Option<DriveEntry> {
    let wide_root = wide(root);
    // SAFETY: `wide_root` is NUL-terminated; the call has no other precondition.
    let kind = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
    if matches!(kind, DRIVE_NO_ROOT_DIR) {
        return None;
    }

    // MAX_PATH+1 is the documented bound for both name buffers.
    let mut filesystem = [0u16; 261];
    let mut flags = 0u32;
    // SAFETY: the root is NUL-terminated, `filesystem` is writable for the
    // length passed, `flags` is a writable local, and every buffer this call is
    // allowed to skip is passed as null.
    let ok = unsafe {
        GetVolumeInformationW(
            wide_root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut flags,
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }

    let name = narrow(&filesystem);
    if name.is_empty() {
        return None;
    }
    Some(DriveEntry {
        root: root.to_string(),
        // Lowercased so it reads as a filesystem type rather than a brand, but
        // otherwise verbatim. A FAT32 volume is reported as `fat32`, not as
        // Linux's driver name `vfat`: the volume genuinely is FAT32, and naming
        // the Linux driver would assert that driver is in use when it is not.
        filesystem: name.to_ascii_lowercase(),
        read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        kind,
    })
}

/// The filesystem type of the volume holding `path`, if it can be determined.
///
/// Used for the `/` row, whose type is whatever the drive under the guest root
/// is formatted with.
fn filesystem_of(path: &std::path::Path) -> Option<String> {
    let text = path.to_str()?;
    let bytes = text.as_bytes();
    // A drive-qualified absolute path, which is what `current_exe` yields.
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    describe_drive(&format!("{}:\\", &text[..1])).map(|drive| drive.filesystem)
}

/// Builds the synthesized mount table.
///
/// Ordered as Linux orders `/proc/mounts`: the root first, then the pseudo
/// filesystems, then the real volumes.
fn synthesized_table() -> Result<Vec<Row>, i32> {
    let mut rows = Vec::new();

    // The guest's root really is a mount point: it is the host directory the VFS
    // publishes as `/`. Its device is that host directory's real path, which is
    // the honest answer to "what is mounted here" and is more useful in `df`
    // output than any Linux-shaped name would be.
    if let Ok(root) = kinakaze_vfs::system_root() {
        let device = root.to_string_lossy().into_owned();
        let filesystem = filesystem_of(&root).unwrap_or_else(|| String::from("unknown"));
        rows.push(Row::new(&device, "/", &filesystem, "rw", 0, 0));
    }

    // `/proc` is served by `kinakaze_vfs::procfs`, so it is a real mount in the
    // guest namespace. It is `ro` because that module generates file contents and
    // accepts no writes, which is a narrower claim than Linux's writable procfs.
    rows.push(Row::new("proc", "/proc", "proc", "ro", 0, 0));

    for root in drive_roots()? {
        let Some(drive) = describe_drive(&root) else {
            continue;
        };
        let Some(mount_point) = drive.mount_point() else {
            continue;
        };
        let options = drive.options();
        rows.push(Row::new(
            &drive.device(),
            &mount_point,
            &drive.filesystem,
            &options,
            0,
            0,
        ));
    }
    Ok(rows)
}

/// The filenames whose contents are the synthesized table.
///
/// These are the files that describe *currently mounted* filesystems. `/etc/fstab`
/// is deliberately absent: it is configuration rather than state, and answering a
/// request for it with the live table would make `mount -a` try to re-mount
/// everything that is already mounted.
const SYNTHETIC_TABLES: &[&str] = &[
    "/etc/mtab",
    "/etc/mnttab",
    "/proc/mounts",
    "/proc/self/mounts",
];

/// Whether `path` names the mount table rather than an ordinary file.
fn is_synthetic_table(path: &str) -> bool {
    // A trailing slash cannot change which file is meant.
    let trimmed = path.trim_end_matches('/');
    SYNTHETIC_TABLES.contains(&trimmed)
}

/// The process-global storage `getmntent` returns a pointer to.
///
/// This is glibc's contract, and matching it exactly is load-bearing rather than
/// a stylistic choice. The obvious design — keep the scratch `mntent` inside the
/// stream, so two interleaved walks cannot disturb each other — is *stronger*
/// than glibc and still wrong, because real callers rely on the returned pointer
/// outliving the stream. BusyBox's `find_mount_point` is exactly that shape:
///
/// ```c
/// while ((mountEntry = getmntent(mtab)) != NULL) { ... }
/// endmntent(mtab);
/// return mountEntry;          /* read by the caller, after the close */
/// ```
///
/// With per-stream storage that pointer dangles the moment `endmntent` frees the
/// stream, and `df` prints a corrupted `Filesystem` column — which is how this
/// was found, running BusyBox rather than the unit tests.
///
/// So the entry is process-global and the `Row` backing its strings is held alive
/// beside it. The cost is glibc's own hazard, faithfully reproduced: a second
/// `getmntent` on *any* stream overwrites the first result. That is what callers
/// are written against.
struct StaticEntry(UnsafeCell<MntEnt>);

// SAFETY: the cell is only written while `RETURNED_ROW`'s mutex is held, and the
// pointer handed out afterwards carries glibc's documented "valid until the next
// call" contract. The wrapper exists because `UnsafeCell` is not `Sync`.
unsafe impl Sync for StaticEntry {}

static RETURNED_ENTRY: StaticEntry = StaticEntry(UnsafeCell::new(MntEnt::empty()));

/// Keeps the strings [`RETURNED_ENTRY`] points at alive.
///
/// Replacing this drops the previous row, which is precisely what invalidates the
/// previously returned pointer — the documented behaviour.
static RETURNED_ROW: Mutex<Option<Row>> = Mutex::new(None);

/// Publishes `row` as the result of a `getmntent` call.
///
/// Returns a pointer to the process-global entry, or null if the lock is
/// unusable, which can only happen after another thread panicked here.
fn publish(row: &Row) -> *mut MntEnt {
    let Ok(mut held) = RETURNED_ROW.lock() else {
        return ptr::null_mut();
    };
    // The clone is what lets the result outlive the stream it came from.
    let owned = row.clone();
    let entry = owned.as_mntent();
    *held = Some(owned);
    let slot = RETURNED_ENTRY.0.get();
    // SAFETY: the mutex is held, which is the discipline `StaticEntry`
    // documents, and `slot` addresses a live static of the right type. The
    // strings `entry` points at are owned by the `Row` just stored in `held`, so
    // they outlive this call.
    unsafe { *slot = entry };
    slot
}

/// Undoes the escaping `/etc/mtab` uses for characters that would split a field.
///
/// glibc's `getmntent` decodes these, so a mount point containing a space is
/// reported with the space rather than as `\040`. Only the four sequences glibc
/// handles are decoded; anything else is left alone, including a trailing
/// backslash.
fn unescape(field: &str) -> String {
    let mut result = String::with_capacity(field.len());
    let bytes = field.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        // An escape is a backslash and exactly three octal digits.
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &field[index + 1..index + 4];
            if let Ok(value) = u8::from_str_radix(digits, 8)
                && matches!(value, b' ' | b'\t' | b'\n' | b'\\')
            {
                result.push(char::from(value));
                index += 4;
                continue;
            }
        }
        result.push(char::from(bytes[index]));
        index += 1;
    }
    result
}

/// Parses `/etc/mtab`-shaped text into rows.
///
/// Blank lines and `#` comments are skipped, as glibc does. A line with fewer
/// than two fields cannot name a mount, so it is dropped rather than half-read;
/// missing type and options default to empty, and missing numbers to 0, which is
/// what glibc's `sscanf`-based parser produces.
fn parse_table(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_ascii_whitespace();
        let (Some(device), Some(directory)) = (fields.next(), fields.next()) else {
            continue;
        };
        let filesystem = fields.next().unwrap_or("");
        let options = fields.next().unwrap_or("");
        let frequency = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
        let pass = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
        rows.push(Row::new(
            &unescape(device),
            &unescape(directory),
            &unescape(filesystem),
            &unescape(options),
            frequency,
            pass,
        ));
    }
    rows
}

/// Escapes one field in the spelling used by `/etc/mtab`.
fn escape_mtab_field(field: &CStr) -> String {
    let mut escaped = String::new();
    for byte in field.to_bytes() {
        match *byte {
            b' ' => escaped.push_str("\\040"),
            b'\t' => escaped.push_str("\\011"),
            b'\n' => escaped.push_str("\\012"),
            b'\\' => escaped.push_str("\\134"),
            byte => escaped.push(char::from(byte)),
        }
    }
    escaped
}

/// Serializes rows into the text a real mount-table `FILE` contains.
fn render_table(rows: &[Row]) -> String {
    let mut text = String::new();
    for row in rows {
        use std::fmt::Write as _;
        let _ = writeln!(
            text,
            "{} {} {} {} {} {}",
            escape_mtab_field(&row.device),
            escape_mtab_field(&row.directory),
            escape_mtab_field(&row.filesystem),
            escape_mtab_field(&row.options),
            row.frequency,
            row.pass,
        );
    }
    text
}

/// Reads and parses the next usable mount-table line from a real stdio stream.
///
/// Blank lines, comments and malformed records are skipped, matching
/// [`parse_table`]. The stream is left immediately after the returned row.
unsafe fn next_mount_row(file: *mut crate::stdio::File) -> Option<Row> {
    loop {
        let mut line = Vec::new();
        loop {
            let byte = crate::stdio::fgetc(file);
            if byte == crate::stdio::EOF {
                break;
            }
            line.push(byte as u8);
            if byte as u8 == b'\n' {
                break;
            }
        }
        if line.is_empty() {
            return None;
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(row) = parse_table(&text).into_iter().next() {
            return Some(row);
        }
    }
}

/// Whether a `fopen`-style mode string asks to write.
fn mode_writes(mode: &str) -> bool {
    mode.contains(['w', 'a', '+'])
}

/// Reads a NUL-terminated argument as a `str`.
///
/// # Safety
///
/// `text` must be null or a null-terminated string.
unsafe fn borrow(text: *const c_char) -> Option<&'static str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

/// Translates a guest path into a host path, reporting the VFS's own refusals.
fn host_path(path: &str) -> Result<PathBuf, i32> {
    kinakaze_vfs::resolve_linux_path(path).map_err(|_| EINVAL)
}

/// Maps an I/O failure onto an errno, preferring the real OS code.
fn io_errno(error: &std::io::Error) -> i32 {
    error.raw_os_error().map_or(EIO, |code| {
        // A Windows I/O error arrives as a Win32 code, which the VFS knows how to
        // translate; a code that is already a Linux errno passes through.
        errno_from_win32(code as u32)
    })
}

/// `setmntent`, which opens the mount table or a file shaped like one.
///
/// The filename decides what happens, and both branches are real work:
///
/// * `/etc/mtab`, `/etc/mnttab`, `/proc/mounts` and `/proc/self/mounts` yield the
///   table synthesized from this machine's volumes. Opening one of them for
///   writing fails with `EROFS`, because a synthesized table has nothing behind
///   it to write to and reporting success would promise a persistence this layer
///   cannot provide.
/// * Any other path is opened through the ordinary stdio/VFS path, which is what
///   `mount -f` and a private mtab need.
///
/// Returns null with errno set on failure. The result is a genuine [`File`], as
/// glibc promises: callers may use `getline`, `rewind` and other stdio operations
/// before releasing it with `endmntent`.
///
/// # Safety
///
/// `path` and `mode` must be null or null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setmntent(
    path: *const c_char,
    mode: *const c_char,
) -> *mut c_void {
    // SAFETY: forwarded from this function's contract.
    let Some(path_text) = (unsafe { borrow(path) }) else {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    };
    // glibc defaults to read when the mode is unreadable rather than failing.
    // SAFETY: forwarded from this function's contract.
    let borrowed_mode = unsafe { borrow(mode) };
    let mode = borrowed_mode.unwrap_or("r");
    let mode_pointer = if borrowed_mode.is_some() {
        mode.as_bytes().as_ptr().cast::<c_char>()
    } else {
        c"r".as_ptr()
    };
    let writing = mode_writes(mode);

    if is_synthetic_table(path_text) {
        if writing {
            // The table is derived from the volume manager, so there is no file
            // to update. EROFS is what Linux reports for the same situation,
            // where /etc/mtab is a symlink to the read-only /proc/self/mounts.
            crate::set_errno(EROFS);
            return ptr::null_mut();
        }
        let rows = match synthesized_table() {
            Ok(rows) => rows,
            Err(error) => {
                crate::set_errno(error);
                return ptr::null_mut();
            }
        };
        let text = render_table(&rows);
        let file = crate::stdio::exports::kinakaze_abi_tmpfile();
        if file.is_null() {
            return ptr::null_mut();
        }
        if !text.is_empty() {
            // SAFETY: `text` is readable for its full length and `file` is live.
            let written = unsafe {
                crate::stdio::fwrite(text.as_ptr().cast::<c_void>(), 1, text.len(), file)
            };
            if written != text.len() {
                // SAFETY: `file` is live and is not used after this failure.
                let _ = unsafe { crate::stdio::fclose(file) };
                return ptr::null_mut();
            }
        }
        crate::stdio::rewind(file);
        return file.cast::<c_void>();
    }

    // SAFETY: `path` and `mode_pointer` are live null-terminated strings.
    unsafe { crate::stdio::fopen(path, mode_pointer).cast::<c_void>() }
}

/// `getmntent`, which returns the next row.
///
/// The returned pointer addresses process-global storage and stays valid until
/// the next `getmntent` on *any* stream — including after the `endmntent` that
/// closes this one. See [`StaticEntry`]: BusyBox depends on surviving the close,
/// so this is glibc's contract reproduced deliberately rather than tightened.
///
/// Returns null at end of table with errno untouched, because exhausting the
/// table is not an error and a caller checking errno after the loop must not see
/// a stale value.
///
/// # Safety
///
/// `stream` must be a pointer returned by [`kinakaze_abi_setmntent`] that has not
/// been passed to [`kinakaze_abi_endmntent`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getmntent(stream: *mut c_void) -> *mut MntEnt {
    if stream.is_null() {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees `stream` is a live stdio stream.
    let Some(row) = (unsafe { next_mount_row(stream.cast::<crate::stdio::File>()) }) else {
        return ptr::null_mut();
    };
    publish(&row)
}

/// `getmntent_r`, which fills a caller-owned entry from a caller-owned buffer.
///
/// This is the one reentrant function in this module that does **not** return an
/// errno. glibc declares it as returning `struct mntent *` — `entry` on success
/// and null on failure or end of table — and BusyBox tests the pointer. Returning
/// an errno here would be read as a non-null pointer and dereferenced.
///
/// `length` is a signed `int` in glibc's declaration, not a `size_t`; a negative
/// value is treated as no space at all.
///
/// # Safety
///
/// `stream` must come from [`kinakaze_abi_setmntent`], `entry` must be writable,
/// and `buffer` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getmntent_r(
    stream: *mut c_void,
    entry: *mut MntEnt,
    buffer: *mut c_char,
    length: c_int,
) -> *mut MntEnt {
    if stream.is_null() {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    }
    if entry.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    let file = stream.cast::<crate::stdio::File>();
    let position = crate::stdio::ftell(file);
    // SAFETY: the caller guarantees `stream` is a live stdio stream.
    let Some(row) = (unsafe { next_mount_row(file) }) else {
        return ptr::null_mut();
    };

    // Every string is reserved before the caller's struct is touched, so a
    // buffer that turns out to be too small leaves the entry as it was found and
    // the cursor unmoved — the caller can retry with a larger buffer.
    let capacity = usize::try_from(length).unwrap_or(0);
    let needed = row.device.as_bytes().len()
        + row.directory.as_bytes().len()
        + row.filesystem.as_bytes().len()
        + row.options.as_bytes().len()
        + 4;
    if buffer.is_null() || capacity < needed {
        if position >= 0 {
            let _ = crate::stdio::fseek(file, position, 0);
        }
        crate::set_errno(ERANGE);
        return ptr::null_mut();
    }
    let mut packer = Packer::new(buffer, capacity);
    let device = packer
        .string(row.device.as_bytes())
        .expect("capacity prechecked");
    let directory = packer
        .string(row.directory.as_bytes())
        .expect("capacity prechecked");
    let filesystem = packer
        .string(row.filesystem.as_bytes())
        .expect("capacity prechecked");
    let options = packer
        .string(row.options.as_bytes())
        .expect("capacity prechecked");

    // SAFETY: the caller guarantees `entry` is writable.
    unsafe {
        *entry = MntEnt {
            mnt_fsname: device,
            mnt_dir: directory,
            mnt_type: filesystem,
            mnt_opts: options,
            mnt_freq: row.frequency,
            mnt_passno: row.pass,
        };
    }
    entry
}

/// `endmntent`, which closes a stream and releases it.
///
/// Returns 1 on success, which is glibc's convention here rather than the 0 that
/// most of libc uses; a caller that checks for 0 would report a spurious failure.
/// A null stream returns 1 as well, because glibc's `endmntent` cannot fail and
/// callers do not check it.
///
/// # Safety
///
/// `stream` must be null or a live stdio stream that has not already been closed.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_endmntent(stream: *mut c_void) -> c_int {
    if stream.is_null() {
        return 1;
    }
    // SAFETY: forwarded from this function's contract.
    let _ = unsafe { crate::stdio::fclose(stream.cast::<crate::stdio::File>()) };
    1
}

/// `hasmntopt`, which finds one option in a `mnt_opts` list.
///
/// Returns a pointer *into* `mnt_opts` at the start of the match, or null. The
/// boundary rules are the whole substance of this function: a match must begin at
/// the start of the list or just after a comma, and must end at the end of the
/// list, a comma, or an `=`. Without them `ro` matches inside `root` and a
/// caller concludes a read-write mount is read-only — which is the classic bug
/// this interface exists to avoid, and what the test below pins.
///
/// # Safety
///
/// `entry` must point at a readable `struct mntent` whose `mnt_opts` is null or
/// null-terminated, and `option` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_hasmntopt(
    entry: *const MntEnt,
    option: *const c_char,
) -> *mut c_char {
    if entry.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable struct.
    let options = unsafe { (*entry).mnt_opts };
    if options.is_null() || option.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees both are null-terminated. Bytes are compared
    // rather than `str`s so that a non-UTF-8 option list still works.
    let (haystack, needle) = unsafe {
        (
            CStr::from_ptr(options).to_bytes(),
            CStr::from_ptr(option).to_bytes(),
        )
    };
    // An empty option would match at every position, which no caller means.
    if needle.is_empty() {
        return ptr::null_mut();
    }

    let mut start = 0usize;
    while start + needle.len() <= haystack.len() {
        let Some(found) = find(&haystack[start..], needle) else {
            return ptr::null_mut();
        };
        let at = start + found;
        let ends = at + needle.len();
        let opens = at == 0 || haystack[at - 1] == b',';
        let closes = ends == haystack.len() || matches!(haystack[ends], b',' | b'=');
        if opens && closes {
            // SAFETY: `at` indexes inside the option list, so the pointer stays
            // within the allocation the caller supplied.
            return unsafe { options.add(at) };
        }
        // Resume after the next comma, so an option that merely contains the
        // needle cannot be examined twice.
        match haystack[at..].iter().position(|byte| *byte == b',') {
            Some(comma) => start = at + comma + 1,
            None => return ptr::null_mut(),
        }
    }
    ptr::null_mut()
}

/// The first position of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Packs C strings into a caller-supplied buffer for the reentrant entry points.
///
/// Each reservation is bounds-checked before a byte is written, so a buffer that
/// turns out to be too small is left exactly as it was found. Getting that wrong
/// is a guest heap overwrite, and callers exercise the path deliberately by
/// starting small and growing on `ERANGE`.
struct Packer {
    base: *mut u8,
    capacity: usize,
    used: usize,
}

impl Packer {
    /// Wraps a caller buffer. A null buffer becomes zero capacity, so every
    /// reservation fails with `ERANGE` instead of dereferencing it.
    fn new(buffer: *mut c_char, capacity: usize) -> Self {
        Self {
            base: if buffer.is_null() {
                ptr::null_mut()
            } else {
                buffer.cast::<u8>()
            },
            capacity: if buffer.is_null() { 0 } else { capacity },
            used: 0,
        }
    }

    /// Copies `bytes` in as a null-terminated string, or fails.
    fn string(&mut self, bytes: &[u8]) -> Result<*mut c_char, ()> {
        let needed = bytes.len() + 1;
        if self.used + needed > self.capacity {
            return Err(());
        }
        // SAFETY: the bound above proved `needed` bytes are free at `used`.
        let target = unsafe { self.base.add(self.used) };
        // SAFETY: same bound. The source is owned Rust storage and the
        // destination the caller's buffer, so they cannot overlap.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            *target.add(bytes.len()) = 0;
        }
        self.used += needed;
        Ok(target.cast::<c_char>())
    }
}

// ---------------------------------------------------------------------------
// utmp and wtmp: the login records behind `who`, `last` and `uptime`.
//
// This database is the guest's own `/var/run/utmp`, read and written for real
// through the VFS. It is *not* synthesized from Windows sessions, and that is a
// decision rather than an omission.
//
// `WTSEnumerateSessionsW` and `LsaEnumerateLogonSessions` do describe genuinely
// related information — somebody really is logged in to this desktop. But a
// Windows interactive session is not a `utmpx` line, and reshaping one into the
// other requires inventing exactly the three fields a caller acts on:
//
// * `ut_line` is a terminal name. `who` prints it, and `write` and `wall` open
//   `/dev/<ut_line>` to send to it. Windows offers `Console` or `RDP-Tcp#0`,
//   which are session names and not devices; publishing one would send `write`
//   to a `/dev` path that does not exist.
// * `ut_pid` is the login process. Nothing on Windows spawned a login shell for
//   this session, so any pid here would name a process that is not one.
// * `ut_tv` is the moment this login happened. The WTS logon time is when the
//   *desktop* session was established, which may predate this process by days
//   and is shared by every process on that desktop. `last` would then display it
//   as the login time of a Unix session that never existed.
//
// Reporting nothing is the truthful answer to the question the database asks —
// which users are logged in through this host's Unix login records — and it is
// the same answer real Linux gives in a container whose `/var/run/utmp` was
// never created. Reading the guest's own file keeps that honest in both
// directions: `who` prints nothing until something records a login, and when the
// guest's own tooling writes a record, `who` shows precisely what the guest
// wrote and nothing this layer made up.
//
// Because the file is real, the writes are real too. `pututxline` and `updwtmpx`
// genuinely append to it and genuinely fail — with the true errno — when the path
// cannot be created. Neither silently discards a record.
// ---------------------------------------------------------------------------

/// `ut_type` values, in the order Linux numbers them.
pub const EMPTY: i16 = 0;
pub const RUN_LVL: i16 = 1;
pub const BOOT_TIME: i16 = 2;
pub const NEW_TIME: i16 = 3;
pub const OLD_TIME: i16 = 4;
mod login;
pub const INIT_PROCESS: i16 = 5;
pub const LOGIN_PROCESS: i16 = 6;
pub const USER_PROCESS: i16 = 7;
pub const DEAD_PROCESS: i16 = 8;
pub const ACCOUNTING: i16 = 9;

/// The exit status pair inside `struct utmpx`.
///
/// Two `short`s, so 4 bytes. Linux declares it as an anonymous struct; it is
/// named here because Rust has no anonymous struct fields.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UtmpExit {
    pub e_termination: i16,
    pub e_exit: i16,
}

/// The timestamp inside `struct utmpx`.
///
/// **Two 32-bit ints, not a `timeval`.** utmp keeps 32-bit timestamps even on
/// 64-bit Linux so that the on-disk format stays readable across word sizes; on
/// x86_64 glibc sets `__WORDSIZE_TIME64_COMPAT32`, which selects exactly this.
/// Using a 64-bit `timeval` here would add 8 bytes and shift `ut_addr_v6`,
/// `__glibc_reserved` and the record stride — silent corruption of every record
/// after the first.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UtmpTimeVal {
    pub tv_sec: i32,
    pub tv_usec: i32,
}

/// The Linux x86_64 `struct utmpx`, which is 384 bytes.
///
/// The layout is ABI and the arithmetic is worth showing, because a wrong offset
/// is not a compile error:
///
/// ```text
///   0    ut_type            short, 2 bytes, then 2 bytes of padding
///   4    ut_pid             pid_t, 4
///   8    ut_line            char[32]
///   40   ut_id              char[4]
///   44   ut_user            char[32]
///   76   ut_host            char[256]
///   332  ut_exit            two shorts, 4
///   336  ut_session         int, 4    (32-bit; a long here would shift the rest)
///   340  ut_tv              two ints, 8
///   348  ut_addr_v6         int[4], 16
///   364  __glibc_reserved   char[20]
///   384  end
/// ```
///
/// Alignment is 4, so there is no tail padding and the record stride on disk is
/// exactly 384 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Utmpx {
    pub ut_type: i16,
    pub ut_pid: i32,
    pub ut_line: [c_char; 32],
    pub ut_id: [c_char; 4],
    pub ut_user: [c_char; 32],
    pub ut_host: [c_char; 256],
    pub ut_exit: UtmpExit,
    /// 32-bit, matching `ut_tv`'s compatibility layout.
    pub ut_session: i32,
    pub ut_tv: UtmpTimeVal,
    pub ut_addr_v6: [i32; 4],
    pub __glibc_reserved: [c_char; 20],
}

impl Utmpx {
    /// An all-zero record, which is `EMPTY` and means "unused slot".
    const fn zeroed() -> Self {
        Self {
            ut_type: EMPTY,
            ut_pid: 0,
            ut_line: [0; 32],
            ut_id: [0; 4],
            ut_user: [0; 32],
            ut_host: [0; 256],
            ut_exit: UtmpExit {
                e_termination: 0,
                e_exit: 0,
            },
            ut_session: 0,
            ut_tv: UtmpTimeVal {
                tv_sec: 0,
                tv_usec: 0,
            },
            ut_addr_v6: [0; 4],
            __glibc_reserved: [0; 20],
        }
    }

    /// Reinterprets a record as the bytes it occupies on disk.
    ///
    /// Sound because the type is `repr(C)` with no padding bytes that are not
    /// part of a field, holds no pointers and has no invalid bit patterns.
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: `Utmpx` is `repr(C)`, `Copy`, contains only integers and byte
        // arrays, and every one of its `size_of` bytes is initialized.
        unsafe { core::slice::from_raw_parts((self as *const Self).cast::<u8>(), RECORD_SIZE) }
    }

    /// Reads a record from exactly `RECORD_SIZE` bytes of file content.
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != RECORD_SIZE {
            return None;
        }
        let mut record = Self::zeroed();
        // SAFETY: the length check proved the source holds exactly the bytes the
        // destination occupies, and every bit pattern is a valid `Utmpx`.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), (&raw mut record).cast::<u8>(), RECORD_SIZE);
        }
        Some(record)
    }
}

/// The on-disk stride of one record, which is the struct's own size.
const RECORD_SIZE: usize = size_of::<Utmpx>();

/// The default utmp path, matching glibc's `_PATH_UTMPX`.
const DEFAULT_UTMP: &str = "/var/run/utmp";

/// The path the utmp entry points read and write.
///
/// Held in a `Mutex` because `utmpxname` can change it. glibc keeps the same
/// process-global setting.
fn utmp_path() -> &'static Mutex<String> {
    static PATH: OnceLock<Mutex<String>> = OnceLock::new();
    PATH.get_or_init(|| Mutex::new(String::from(DEFAULT_UTMP)))
}

/// Reads the current utmp path, falling back to the default if it is poisoned.
///
/// A poisoned lock means another thread panicked mid-update. The default is the
/// right recovery: it is what the path started as, and refusing to answer would
/// turn an unrelated panic into a failure of every later `who`.
fn current_utmp_path() -> String {
    utmp_path()
        .lock()
        .map(|path| path.clone())
        .unwrap_or_else(|_| String::from(DEFAULT_UTMP))
}

/// The enumeration state shared by `setutxent`, `getutxent` and `endutxent`.
///
/// The record list is snapshotted at `setutxent`, which is what makes the
/// iteration protocol terminate: a writer appending during a walk cannot extend
/// the walk indefinitely. glibc holds an open descriptor and has the same
/// practical behaviour for a file that is only appended to.
struct UtmpCursor {
    records: Vec<Utmpx>,
    position: usize,
    /// Whether `setutxent` has run. glibc's `getutxent` opens the file itself if
    /// the caller skipped `setutxent`, and BusyBox relies on that.
    opened: bool,
}

fn utmp_cursor() -> &'static Mutex<UtmpCursor> {
    static CURSOR: OnceLock<Mutex<UtmpCursor>> = OnceLock::new();
    CURSOR.get_or_init(|| {
        Mutex::new(UtmpCursor {
            records: Vec::new(),
            position: 0,
            opened: false,
        })
    })
}

/// Loads every whole record from the utmp file.
///
/// A missing file is an empty database, not an error: it is the normal state of a
/// system where nothing has logged in, and it is the state this layer starts in.
/// A trailing partial record is discarded, which is what glibc does when it reads
/// a file another process is midway through extending.
fn read_records(path: &str) -> Vec<Utmpx> {
    let Ok(host) = host_path(path) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&host) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(RECORD_SIZE)
        .filter_map(Utmpx::from_bytes)
        .collect()
}

/// The static `struct utmpx` the non-reentrant getters return.
///
/// `getutxent` is documented to return a pointer to storage overwritten by the
/// next call, and this is that storage. It is process-global exactly as glibc's
/// is, so two threads walking the database concurrently see one another's
/// records — a property of the interface, not of this implementation, and the
/// reason POSIX added the `_r` forms elsewhere.
struct StaticRecord(UnsafeCell<Utmpx>);

// SAFETY: the cell is only ever written while `utmp_cursor()`'s mutex is held,
// and the pointer handed out afterwards carries glibc's documented "valid until
// the next call" contract. The wrapper exists because `UnsafeCell` is not `Sync`.
unsafe impl Sync for StaticRecord {}

static RETURNED_RECORD: StaticRecord = StaticRecord(UnsafeCell::new(Utmpx::zeroed()));

/// `setutxent`, which rewinds the database and snapshots it.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setutxent() {
    let path = current_utmp_path();
    let records = read_records(&path);
    if let Ok(mut cursor) = utmp_cursor().lock() {
        cursor.records = records;
        cursor.position = 0;
        cursor.opened = true;
    }
}

/// `getutxent`, which returns the next record or null at the end.
///
/// Returns records verbatim, including `EMPTY` slots — glibc does the same, and
/// `who` filters on `ut_type` itself. On a system where nothing has logged in the
/// very first call returns null, which is the truthful report described in this
/// section's header.
///
/// The returned pointer addresses static storage that the next call overwrites.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getutxent() -> *mut Utmpx {
    let Ok(mut cursor) = utmp_cursor().lock() else {
        return ptr::null_mut();
    };
    // glibc opens the database on first use when the caller skipped `setutxent`.
    if !cursor.opened {
        let path = current_utmp_path();
        cursor.records = read_records(&path);
        cursor.position = 0;
        cursor.opened = true;
    }
    let Some(record) = cursor.records.get(cursor.position).copied() else {
        // End of database, which is not an error: errno is left alone so a caller
        // checking it after the loop does not see a stale failure.
        return ptr::null_mut();
    };
    cursor.position += 1;
    let slot = RETURNED_RECORD.0.get();
    // SAFETY: the cursor mutex is held, which is the discipline `StaticRecord`
    // documents, and `slot` addresses a live static of the right type.
    unsafe { *slot = record };
    slot
}

/// `endutxent`, which closes the database.
///
/// The snapshot is released and the position reset, so a later `getutxent`
/// without an intervening `setutxent` re-reads the file from the start, as it
/// does on glibc.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endutxent() {
    if let Ok(mut cursor) = utmp_cursor().lock() {
        cursor.records = Vec::new();
        cursor.position = 0;
        cursor.opened = false;
    }
}

/// `utmpxname`, which selects the file the entry points above operate on.
///
/// Not in the trap list, and BusyBox as built for Debian does not import it. It
/// is here because it is the only way to point this database at a different file,
/// and a caller that opens `/var/log/wtmp` with it and then walks it with
/// `getutxent` is doing what `last` does. Shipping the readers without it would
/// leave that caller silently reading the wrong file.
///
/// Returns 0 on success and -1 on failure, which is glibc's convention here.
///
/// # Safety
///
/// `path` must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_utmpxname(path: *const c_char) -> c_int {
    // SAFETY: forwarded from this function's contract.
    let Some(path) = (unsafe { borrow(path) }) else {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    };
    let Ok(mut current) = utmp_path().lock() else {
        crate::set_errno(EIO);
        return -1;
    };
    *current = path.to_string();
    0
}

/// Whether two records describe the same login slot.
///
/// This is glibc's `__utmp_equal`, and the rule differs by type. The four
/// process types are matched on `ut_id`, which is the short tag `init` assigns to
/// a line; everything else is matched on `ut_type` alone, because there is only
/// one run level and one boot record in a database.
fn same_slot(existing: &Utmpx, wanted: &Utmpx) -> bool {
    match wanted.ut_type {
        INIT_PROCESS | LOGIN_PROCESS | USER_PROCESS | DEAD_PROCESS => {
            matches!(
                existing.ut_type,
                INIT_PROCESS | LOGIN_PROCESS | USER_PROCESS | DEAD_PROCESS
            ) && existing.ut_id == wanted.ut_id
        }
        RUN_LVL | BOOT_TIME | NEW_TIME | OLD_TIME => existing.ut_type == wanted.ut_type,
        // EMPTY and ACCOUNTING name no slot, so nothing matches them.
        _ => false,
    }
}

/// Ensures the directory holding `path` exists, so a first write can succeed.
///
/// `/var/run` does not exist in a fresh guest tree, and failing the first
/// `pututxline` because of that would be a failure of this layer rather than a
/// true report about the database.
fn ensure_parent(path: &std::path::Path) -> Result<(), i32> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(|error| io_errno(&error))
}

/// `pututxline`, which writes one record into the database.
///
/// Replaces the record occupying the same slot, following [`same_slot`], or
/// appends when there is none. The write is real: the file is created if needed
/// and the record lands on disk, so a later `getutxent` — in this process or
/// another — reads it back.
///
/// Returns a pointer to the record on success and null with errno set on
/// failure. Failure is reported rather than swallowed: a caller that cannot
/// record a login needs to know, and silently discarding would leave `who`
/// permanently disagreeing with reality.
///
/// # Safety
///
/// `record` must point at a readable `struct utmpx`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pututxline(record: *const Utmpx) -> *mut Utmpx {
    if record.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable record.
    let wanted = unsafe { *record };
    // An `EMPTY` record names no slot, so there is nothing to write. glibc
    // reports EINVAL, which distinguishes a malformed record from a database
    // that could not be written.
    if !(RUN_LVL..=ACCOUNTING).contains(&wanted.ut_type) {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    }

    let path = current_utmp_path();
    let host = match host_path(&path) {
        Ok(host) => host,
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    if let Err(error) = ensure_parent(&host) {
        crate::set_errno(error);
        return ptr::null_mut();
    }

    // The whole file is rewritten rather than seeking to one record. The database
    // is a few hundred records at most, and a full rewrite makes the replacement
    // atomic from a reader's point of view without a lock this layer has no way
    // to take across processes.
    let mut records = read_records(&path);
    match records
        .iter()
        .position(|existing| same_slot(existing, &wanted))
    {
        Some(index) => records[index] = wanted,
        None => records.push(wanted),
    }

    let mut bytes = Vec::with_capacity(records.len() * RECORD_SIZE);
    for entry in &records {
        bytes.extend_from_slice(entry.as_bytes());
    }
    if let Err(error) = std::fs::write(&host, &bytes) {
        crate::set_errno(io_errno(&error));
        return ptr::null_mut();
    }

    // The in-process snapshot is now stale, so it is refreshed. Without this a
    // program that writes a record and then walks the database — which is what
    // `login` does — would not see its own write.
    if let Ok(mut cursor) = utmp_cursor().lock()
        && cursor.opened
    {
        cursor.records = records;
    }

    let slot = RETURNED_RECORD.0.get();
    // SAFETY: `slot` addresses a live static of the right type. The write is not
    // under the cursor mutex here because the record being returned is the
    // caller's own, and glibc likewise returns its static from this call.
    unsafe { *slot = wanted };
    slot
}

/// `updwtmpx`, which appends one record to a named wtmp-format file.
///
/// The append is real, into the file the caller names, through the VFS. Nothing
/// is synthesized: the record written is the caller's own, so a guest that logs a
/// session here and then reads the file back with `last` sees exactly what it
/// wrote. That is what makes this safe to implement fully — the honesty problem
/// with utmp is fabricating records, not storing a caller's.
///
/// glibc declares this as returning `void`, so a failure can only be reported
/// through errno. Callers must clear errno first and check it after; there is no
/// return value to test. The record is never silently dropped: on any failure
/// errno holds the real reason and nothing was written.
///
/// # Safety
///
/// `path` must be a null-terminated string and `record` must point at a readable
/// `struct utmpx`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_updwtmpx(path: *const c_char, record: *const Utmpx) {
    // SAFETY: forwarded from this function's contract.
    let Some(path) = (unsafe { borrow(path) }) else {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return;
    };
    if record.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return;
    }
    let host = match host_path(path) {
        Ok(host) => host,
        Err(error) => {
            crate::set_errno(error);
            return;
        }
    };
    if let Err(error) = ensure_parent(&host) {
        crate::set_errno(error);
        return;
    }
    // SAFETY: the caller guarantees a readable record.
    let bytes = unsafe { (*record).as_bytes() };

    use std::io::Write as _;
    let appended = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&host)
        .and_then(|mut file| file.write_all(bytes));
    if let Err(error) = appended {
        crate::set_errno(io_errno(&error));
    }
}

// ---------------------------------------------------------------------------
// Shadow passwords.
//
// This reports absence, which is the decision [`crate::userdb`] already made for
// `getspnam`, `getspent`, `setspent` and `endspent`. Its reasoning applies
// unchanged and is worth restating because it is the whole justification: a
// `struct spwd` exists to carry `sp_pwdp`, and every caller either verifies a
// supplied password against that field or copies it somewhere. No value is safe
// to invent — an empty string asserts the account needs no password, `*` or `!`
// asserts it is locked, and a made-up hash would be compared against real user
// input by `su`. Reporting absence lets the caller decide, and it is what glibc
// reports on a host with no shadow file.
//
// What is new here is the *encoding*. `getspnam` reports "not found" as a null
// return; `getspnam_r` reports it as **0 with a null result pointer**, not as an
// error. That distinction is easy to get wrong and expensive when wrong: a
// caller that receives ENOENT as the return value reports "shadow lookup failed"
// and often aborts, whereas 0-with-null is the ordinary "no such user" it already
// handles by falling back to the passwd entry.
//
// Unlike `userdb`, this section does define `struct spwd`. It has to: the `_r`
// form takes a caller-allocated one, so its layout is part of the interface even
// though this module never fills it in.
// ---------------------------------------------------------------------------

/// The Linux x86_64 `struct spwd`.
///
/// Two pointers then seven longs: offsets 0, 8, 16, 24, 32, 40, 48, 56, 64, so
/// the struct is 72 bytes with 8-byte alignment. Defined for the `_r` signature's
/// sake; nothing in this module writes through such a pointer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Spwd {
    pub sp_namp: *mut c_char,
    pub sp_pwdp: *mut c_char,
    pub sp_lstchg: i64,
    pub sp_min: i64,
    pub sp_max: i64,
    pub sp_warn: i64,
    pub sp_inact: i64,
    pub sp_expire: i64,
    pub sp_flag: u64,
}

/// `getspnam_r`, which always reports that there is no shadow entry.
///
/// Returns **0** with `*result` set to null. That is the POSIX-style encoding of
/// "no such entry": the lookup itself succeeded and found nothing. Returning
/// `ENOENT` would tell the caller the database was unreadable, and BusyBox's
/// `su`, `passwd` and `login` print an error and stop when they see that.
///
/// `entry` and `buffer` are left untouched, since nothing is written into them.
///
/// # Safety
///
/// `name` must be null or null-terminated, `result` must be writable, and
/// `buffer` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getspnam_r(
    name: *const c_char,
    _entry: *mut Spwd,
    _buffer: *mut c_char,
    _length: usize,
    result: *mut *mut Spwd,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    // The name is validated but not consulted: no name has an entry. A caller
    // that passes a null name is still told about it, since that is its bug and
    // not a statement about the database.
    // SAFETY: forwarded from this function's contract.
    if unsafe { borrow(name) }.is_none() {
        // SAFETY: `result` was checked non-null above.
        unsafe { *result = ptr::null_mut() };
        return kinakaze_vfs::EFAULT;
    }
    // SAFETY: `result` was checked non-null above.
    unsafe { *result = ptr::null_mut() };
    // Not an error. See this function's documentation.
    0
}

// ---------------------------------------------------------------------------
// The list of login shells.
//
// Linux reads `/etc/shells`. Three sources are consulted, in order, and the first
// that yields anything wins:
//
// 1. A real `/etc/shells` in the guest tree. If the guest has one, it is the
//    answer by definition, and parsing it is not a substitution at all.
// 2. Probing the conventional shell paths through the VFS and reporting the ones
//    that actually resolve to a file. `/bin/sh` is BusyBox here, so when BusyBox
//    is reachable that is a truthful entry, established by a `stat` rather than
//    assumed.
// 3. `/bin/sh` alone.
//
// The third case needs justifying, because it reports a path that was just
// probed and not found. It is there for consistency with [`crate::userdb`], whose
// `pw_shell` is unconditionally `/bin/sh` — the shell this layer can actually
// exec. `chsh` and `login` validate a user's shell against this list, so a list
// that omitted the shell the passwd entry names would make the one account in the
// database fail its own validation. The claim being made is "this is the shell
// the account is configured with", which is true, rather than "this file exists",
// which the probe already tested. For the same reason `/bin/sh` is always
// included in case 2's result even if the probe did not find it.
// ---------------------------------------------------------------------------

/// The shells probed for, most conventional first.
///
/// All four are shells BusyBox provides or is commonly installed as. Nothing is
/// listed that this layer could not exec.
const SHELL_CANDIDATES: &[&str] = &["/bin/sh", "/bin/ash", "/bin/bash", "/bin/busybox"];

/// Parses `/etc/shells`: one absolute path per line, `#` comments, blanks
/// skipped. A line that is not an absolute path is dropped, because `getusershell`
/// is documented to return pathnames and a caller will try to exec what it gets.
fn parse_shells(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| line.starts_with('/'))
        .map(String::from)
        .collect()
}

/// Whether a guest path resolves to something that exists on the host.
fn guest_file_exists(path: &str) -> bool {
    host_path(path).is_ok_and(|host| host.is_file())
}

/// The shell list, built once. See this section's header for the ordering.
fn shells() -> &'static Vec<CString> {
    static SHELLS: OnceLock<Vec<CString>> = OnceLock::new();
    SHELLS.get_or_init(|| {
        let mut list: Vec<String> = host_path("/etc/shells")
            .ok()
            .and_then(|host| std::fs::read_to_string(host).ok())
            .map(|text| parse_shells(&text))
            .unwrap_or_default();

        if list.is_empty() {
            list = SHELL_CANDIDATES
                .iter()
                .filter(|path| guest_file_exists(path))
                .map(|path| (*path).to_string())
                .collect();
            // Always present, for the reason the section header gives: it is the
            // shell `userdb` reports as `pw_shell`.
            if !list.iter().any(|shell| shell == "/bin/sh") {
                list.insert(0, String::from("/bin/sh"));
            }
        }

        list.into_iter()
            // A path cannot contain a NUL, so this filter drops nothing real; it
            // keeps the conversion total without an unwrap.
            .filter_map(|shell| CString::new(shell).ok())
            .collect()
    })
}

/// The cursor `getusershell` walks and `setusershell` rewinds.
static SHELL_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// `getusershell`, which returns the next shell or null at the end.
///
/// The returned pointer addresses storage this module owns for the life of the
/// process, so unlike glibc — where the buffer is reused by the next call — it
/// stays valid indefinitely. Callers written against glibc copy it immediately
/// and are unaffected by the stronger guarantee.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getusershell() -> *mut c_char {
    let shells = shells();
    // `fetch_add` makes the advance atomic, so two threads walking the list
    // cannot both receive the same entry.
    let index = SHELL_CURSOR.fetch_add(1, Ordering::SeqCst);
    match shells.get(index) {
        Some(shell) => shell.as_ptr().cast_mut(),
        None => {
            // Past the end. The counter is pinned at the length rather than left
            // to climb, so a caller that keeps calling cannot overflow it.
            SHELL_CURSOR.store(shells.len(), Ordering::SeqCst);
            ptr::null_mut()
        }
    }
}

/// `setusershell`, which rewinds the list.
///
/// Not in the trap list — nothing has called it yet — but `getusershell` is
/// useless without it: a second walk over the list would start where the first
/// stopped and return null immediately.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setusershell() {
    SHELL_CURSOR.store(0, Ordering::SeqCst);
}

/// `endusershell`, which closes the enumeration.
///
/// There is no file handle to release, but the position is reset so a later
/// `getusershell` without an intervening `setusershell` starts from the
/// beginning, as it does on glibc.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endusershell() {
    SHELL_CURSOR.store(0, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// The system log.
//
// Windows has a real structural counterpart to syslog: the Event Log, written
// through `ReportEventW`, with severities that Linux priorities map onto. That
// translation is worth doing rather than stubbing, so it is attempted first.
//
// What an *unregistered* source can do was measured rather than assumed.
// Creating the registry key that gives a source a message table needs
// administrator rights, but `RegisterEventSourceW` succeeds for a name that has
// no key, and `ReportEventW` on that handle succeeds too: the record lands in the
// Application log with the correct severity and timestamp, and the Event Viewer
// wraps the text in its "the description for Event ID 0 cannot be found"
// boilerplate before quoting it. That is a cosmetic cost, not a lost message, so
// the Event Log is the normal path here and not a best-effort one.
//
// It can still fail — a low-integrity or heavily sandboxed process cannot open
// the log at all — which is what the stderr fallback in [`deliver`] is for.
//
// One divergence from glibc is deliberate. glibc's `openlog` stores the `ident`
// *pointer* and dereferences it at every later `syslog`, so a caller that frees
// or reuses the string gets garbage in its log. The string is copied here.
// ---------------------------------------------------------------------------

/// Priorities, `LOG_EMERG` through `LOG_DEBUG`.
pub const LOG_EMERG: c_int = 0;
pub const LOG_ALERT: c_int = 1;
pub const LOG_CRIT: c_int = 2;
pub const LOG_ERR: c_int = 3;
pub const LOG_WARNING: c_int = 4;
pub const LOG_NOTICE: c_int = 5;
pub const LOG_INFO: c_int = 6;
pub const LOG_DEBUG: c_int = 7;

/// `openlog` options.
pub const LOG_PID: c_int = 0x01;
pub const LOG_CONS: c_int = 0x02;
pub const LOG_ODELAY: c_int = 0x04;
pub const LOG_NDELAY: c_int = 0x08;
pub const LOG_NOWAIT: c_int = 0x10;
pub const LOG_PERROR: c_int = 0x20;

/// The mask selecting the priority out of a `facility | priority` argument.
const PRIORITY_MASK: c_int = 0x07;
/// The mask selecting the facility.
const FACILITY_MASK: c_int = 0x03f8;

// Windows `ReportEventW` event types. `EVENTLOG_SUCCESS` (0) is deliberately
// absent: Linux has no priority meaning "this went well", so mapping any priority
// onto it would invent a distinction the caller never made.
const EVENTLOG_ERROR_TYPE: u16 = 0x0001;
const EVENTLOG_WARNING_TYPE: u16 = 0x0002;
const EVENTLOG_INFORMATION_TYPE: u16 = 0x0004;

/// Maps a Linux priority onto the Windows event type that means the same thing.
///
/// The three Windows severities are coarser than Linux's eight, so the mapping is
/// lossy in one direction only: `LOG_ERR` and everything more urgent is an error,
/// `LOG_WARNING` is a warning, and `LOG_NOTICE` through `LOG_DEBUG` are
/// informational. That grouping is the one the Event Log viewer's own filters
/// assume, so a `LOG_CRIT` message shows up under "Error" where an administrator
/// will look for it.
///
/// An out-of-range value maps to informational rather than being rejected, because
/// glibc's `syslog` masks the priority and logs it rather than dropping the
/// message.
fn event_type(priority: c_int) -> u16 {
    match priority & PRIORITY_MASK {
        LOG_EMERG | LOG_ALERT | LOG_CRIT | LOG_ERR => EVENTLOG_ERROR_TYPE,
        LOG_WARNING => EVENTLOG_WARNING_TYPE,
        _ => EVENTLOG_INFORMATION_TYPE,
    }
}

/// The `openlog` state, and the Event Log handle when one has been opened.
struct LogState {
    /// The copied `ident`. Empty means "use the program name", as glibc does.
    ident: String,
    options: c_int,
    /// The default facility for a `syslog` call that does not name one.
    facility: c_int,
    /// The Event Log source, opened lazily or by `LOG_NDELAY`.
    source: Option<EventSource>,
    /// Whether opening the source has already been tried and failed. Retrying on
    /// every message would cost a registry lookup per log line on the machines
    /// where it cannot work, which is most of them.
    source_failed: bool,
}

/// An owned `RegisterEventSourceW` handle.
struct EventSource(*mut c_void);

// SAFETY: an Event Log handle is not thread-affine — `ReportEventW` may be called
// on it from any thread — and this one is only ever reached through the mutex
// below. The wrapper exists because a raw pointer is not `Send`.
unsafe impl Send for EventSource {}

impl Drop for EventSource {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from `RegisterEventSourceW` and is closed
            // exactly once, here.
            unsafe { DeregisterEventSource(self.0) };
        }
    }
}

fn log_state() -> &'static Mutex<LogState> {
    static STATE: OnceLock<Mutex<LogState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(LogState {
            ident: String::new(),
            // glibc's default facility when `openlog` was never called.
            facility: LOG_USER,
            options: 0,
            source: None,
            source_failed: false,
        })
    })
}

/// Facilities, as `LOG_*` shifted into place. Only the ones a caller is likely to
/// name are given names; any value inside [`FACILITY_MASK`] is accepted.
pub const LOG_KERN: c_int = 0 << 3;
pub const LOG_USER: c_int = 1 << 3;
pub const LOG_MAIL: c_int = 2 << 3;
pub const LOG_DAEMON: c_int = 3 << 3;
pub const LOG_AUTH: c_int = 4 << 3;
pub const LOG_SYSLOG: c_int = 5 << 3;
pub const LOG_LPR: c_int = 6 << 3;
pub const LOG_NEWS: c_int = 7 << 3;
pub const LOG_UUCP: c_int = 8 << 3;
pub const LOG_CRON: c_int = 9 << 3;
pub const LOG_AUTHPRIV: c_int = 10 << 3;
pub const LOG_FTP: c_int = 11 << 3;
pub const LOG_LOCAL0: c_int = 16 << 3;

/// The program name, used as `ident` when `openlog` supplied none.
///
/// glibc uses `__progname`, which is `argv[0]`'s basename. The Windows command
/// line starts with the hosting loader, so prefer the guest identity published
/// in the shared process table.
fn program_name() -> String {
    let own = crate::process::kinakaze_abi_getpid() as u32;
    if let Some(name) = kinakaze_vfs::job::process_infos()
        .into_iter()
        .find(|info| info.entry.namespace_pid == own)
        .map(|info| info.comm)
        .filter(|name| !name.is_empty())
    {
        return name;
    }
    std::env::args_os()
        .next()
        .map(PathBuf::from)
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| String::from("kinakaze"))
}

/// `openlog`, which sets the identity, options and default facility.
///
/// `LOG_NDELAY` opens the Event Log source immediately, as it opens the socket
/// immediately on Linux, so a caller that wants the connection established before
/// it forks or drops privileges gets that. Without it the source is opened on the
/// first message.
///
/// A facility of 0 leaves the current one in place, which is glibc's behaviour and
/// what lets `openlog(ident, opts, 0)` change only the identity.
///
/// # Safety
///
/// `ident` must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_openlog(
    ident: *const c_char,
    options: c_int,
    facility: c_int,
) {
    // SAFETY: forwarded from this function's contract. The string is copied
    // rather than retained; see this section's header.
    let ident = unsafe { borrow(ident) }.unwrap_or("").to_string();
    let Ok(mut state) = log_state().lock() else {
        return;
    };
    state.ident = ident;
    state.options = options;
    if facility & FACILITY_MASK != 0 {
        state.facility = facility & FACILITY_MASK;
    }
    if options & LOG_NDELAY != 0 {
        let name = if state.ident.is_empty() {
            program_name()
        } else {
            state.ident.clone()
        };
        open_source(&mut state, &name);
    }
}

/// Opens the Event Log source for `name`, remembering a failure.
fn open_source(state: &mut LogState, name: &str) {
    if state.source.is_some() || state.source_failed {
        return;
    }
    let wide_name = wide(name);
    // SAFETY: a null server means the local machine, and `wide_name` is
    // NUL-terminated.
    let handle = unsafe { RegisterEventSourceW(ptr::null(), wide_name.as_ptr()) };
    if handle.is_null() {
        state.source_failed = true;
        return;
    }
    state.source = Some(EventSource(handle));
}

/// `closelog`, which releases the Event Log source and resets the options.
///
/// glibc closes the socket and clears the options, so the next `syslog` behaves as
/// if `openlog` had never been called. The `ident` is cleared for the same reason.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_closelog() {
    let Ok(mut state) = log_state().lock() else {
        return;
    };
    // Dropping the handle deregisters it.
    state.source = None;
    // Cleared so a later `openlog` gets a fresh attempt at the source: the
    // machine's state may have changed, and more usefully the new `ident` may be
    // a source name that does exist.
    state.source_failed = false;
    state.ident = String::new();
    state.options = 0;
    state.facility = LOG_USER;
}

/// A sink that accumulates the formatted message.
struct MessageSink(Vec<u8>);

impl crate::format::Sink for MessageSink {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn written(&self) -> usize {
        self.0.len()
    }
}

/// Expands syslog's `%m` into the current errno's message.
///
/// `%m` is syslog's own conversion and the format engine knows nothing about it,
/// so it is substituted before formatting rather than added to the engine — it
/// consumes no argument, so doing it here cannot disturb the argument order.
///
/// Two details matter. `%%m` is a literal `%m` and must not expand, which is why
/// the scan steps over a doubled `%` as a unit. And any `%` inside the errno
/// message is doubled on the way in, so a message containing one cannot turn into
/// a conversion that eats an argument that was never passed.
///
/// # Safety
///
/// `format` must be a null-terminated string.
unsafe fn expand_errno(format: *const c_char) -> Option<CString> {
    // SAFETY: forwarded from this function's contract.
    let text = unsafe { CStr::from_ptr(format) }.to_bytes();
    if !contains_errno_conversion(text) {
        return None;
    }

    // SAFETY: `strerror` returns a null-terminated static string.
    let message = unsafe { CStr::from_ptr(crate::string::strerror(kinakaze_tls::errno())) };
    let mut escaped = Vec::with_capacity(message.to_bytes().len());
    for byte in message.to_bytes() {
        if *byte == b'%' {
            escaped.push(b'%');
        }
        escaped.push(*byte);
    }

    let mut result = Vec::with_capacity(text.len() + escaped.len());
    let mut index = 0;
    while index < text.len() {
        if text[index] != b'%' {
            result.push(text[index]);
            index += 1;
            continue;
        }
        match text.get(index + 1) {
            Some(b'm') => {
                result.extend_from_slice(&escaped);
                index += 2;
            }
            // A doubled `%` is consumed whole, so `%%m` stays literal.
            Some(next) => {
                result.push(b'%');
                result.push(*next);
                index += 2;
            }
            // A trailing `%` is passed through for the engine to deal with.
            None => {
                result.push(b'%');
                index += 1;
            }
        }
    }
    CString::new(result).ok()
}

/// Whether a format string contains a `%m` that would expand.
///
/// Separate from the rewrite so the common case allocates nothing.
fn contains_errno_conversion(text: &[u8]) -> bool {
    let mut index = 0;
    while index < text.len() {
        if text[index] != b'%' {
            index += 1;
            continue;
        }
        match text.get(index + 1) {
            Some(b'm') => return true,
            Some(_) => index += 2,
            None => return false,
        }
    }
    false
}

/// Builds the line a syslog message becomes.
///
/// The shape is glibc's: `ident[pid]: message` with the brackets present only
/// under `LOG_PID`. The `ident` is the one `openlog` supplied, or the real program
/// name when it supplied none.
fn compose(state: &LogState, message: &str) -> String {
    let ident = if state.ident.is_empty() {
        program_name()
    } else {
        state.ident.clone()
    };
    if state.options & LOG_PID != 0 {
        let pid = crate::process::kinakaze_abi_getpid();
        format!("{ident}[{pid}]: {message}")
    } else {
        format!("{ident}: {message}")
    }
}

/// Writes a composed line to stderr.
///
/// Goes through the libc's own `stderr` rather than Rust's, so the message
/// interleaves correctly with the guest's other output on the same descriptor.
fn write_stderr(line: &str) {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    // SAFETY: `bytes` is a live buffer of exactly this length. Descriptor 2 is
    // stderr in the VFS table, and a write failure is discarded because a logging
    // call reports nothing to its caller.
    let _ = unsafe { crate::kinakaze_write(2, bytes.as_ptr().cast::<c_void>(), bytes.len()) };
}

/// Delivers one composed message, in this order:
///
/// 1. **stderr, if `LOG_PERROR` is set.** The caller asked for it explicitly, so
///    it happens regardless of what else succeeds.
/// 2. **The Windows Event Log.** The real translation, and the path that normally
///    succeeds: `ReportEventW` with the event type [`event_type`] chose and the
///    facility in the category field. The source is opened on first use unless
///    `LOG_NDELAY` already opened it.
/// 3. **stderr, if the Event Log was unavailable.** This is the honest fallback:
///    the alternative is discarding the message, and a log line that vanishes is
///    worse than one on the wrong stream. It is skipped when step 1 already wrote,
///    so a `LOG_PERROR` caller does not see every line twice.
///
/// `LOG_CONS` asks for the console when the log is unreachable, which is what
/// step 3 already does; it is honoured by forcing step 3 even in the case where
/// the Event Log accepted the record, since that is the closest this host has to
/// `/dev/console`.
fn deliver(state: &mut LogState, priority: c_int, message: &str) {
    let line = compose(state, message);
    if crate::fork_trace_enabled() || std::env::var_os("KINAKAZE_REPORT_TRAPS").is_some() {
        eprintln!("kinakaze syslog: [prio={priority}] {line}");
    }

    let mut wrote_stderr = false;
    if state.options & LOG_PERROR != 0 {
        write_stderr(&line);
        wrote_stderr = true;
    }

    let name = if state.ident.is_empty() {
        program_name()
    } else {
        state.ident.clone()
    };
    open_source(state, &name);

    let logged = match &state.source {
        Some(source) => {
            let wide_line = wide(&line);
            let strings = [wide_line.as_ptr()];
            // The facility travels in the category field. It has no Windows
            // meaning, but it is the only place a small integer fits without
            // overwriting something that does, and it keeps the caller's facility
            // recoverable from the record rather than discarded.
            let category = ((priority & FACILITY_MASK) >> 3) as u16;
            // SAFETY: `source.0` came from `RegisterEventSourceW`, `strings`
            // holds exactly the one NUL-terminated string the count declares, and
            // the two optional pointers are null as the documentation allows.
            let ok = unsafe {
                ReportEventW(
                    source.0,
                    event_type(priority),
                    category,
                    // Event id 0: this source has no message table to index, so
                    // any other value would imply a description that does not
                    // exist. The text travels as an insertion string instead.
                    0,
                    ptr::null_mut(),
                    1,
                    0,
                    strings.as_ptr(),
                    ptr::null(),
                )
            };
            ok != 0
        }
        None => false,
    };

    // A record the Event Log rejected is not a record, so stderr gets it. Under
    // LOG_CONS it gets it either way.
    if !wrote_stderr && (!logged || state.options & LOG_CONS != 0) {
        write_stderr(&line);
    }
    // The line every test needs to see. Recording it here rather than in
    // `write_stderr` is what lets a test observe a message that went to the Event
    // Log instead of to stderr.
    #[cfg(test)]
    tests::record_delivery(&line);
}

/// `vsyslog`, the `va_list` form and the one that does the work.
///
/// Not in the trap list, and BusyBox imports the fortified `__syslog_chk` rather
/// than either of these. It is implemented because `openlog` and `closelog` exist
/// to configure something, and shipping the configuration without the call it
/// configures would guarantee a second trap later.
///
/// # Safety
///
/// `format` must be a null-terminated string and `arguments` must be a live
/// `va_list` whose contents match its conversions.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vsyslog(
    priority: c_int,
    format: *const c_char,
    arguments: *mut crate::format::VaList,
) {
    if format.is_null() {
        return;
    }
    // SAFETY: forwarded from this function's contract.
    let expanded = unsafe { expand_errno(format) };
    let effective = expanded.as_ref().map_or(format, |owned| owned.as_ptr());

    let mut sink = MessageSink(Vec::new());
    if arguments.is_null() {
        // No arguments at all. The format string is still emitted, because a
        // caller passing a plain string through `vsyslog` is common and dropping
        // it would lose a real message.
        // SAFETY: `effective` is null-terminated.
        sink.0
            .extend_from_slice(unsafe { CStr::from_ptr(effective) }.to_bytes());
    } else {
        // SAFETY: the caller guarantees a live va_list matching the format.
        unsafe { crate::format::format(&mut sink, effective, &mut *arguments) };
    }
    let message = String::from_utf8_lossy(&sink.0).into_owned();

    let Ok(mut state) = log_state().lock() else {
        return;
    };
    deliver(&mut state, priority, &message);
}

/// Implementation behind the `syslog` assembly thunk.
///
/// # Safety
///
/// `format` must be null-terminated and `arguments` must match it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_syslog_impl(
    priority: c_int,
    format: *const c_char,
    arguments: *mut crate::format::VaList,
) {
    // SAFETY: the thunk passes a live va_list it just constructed.
    unsafe { kinakaze_abi_vsyslog(priority, format, arguments) };
}

// `syslog` is variadic, and Rust cannot declare an `extern "sysv64"` function
// with `...`. The thunk below builds the System V `va_list` its caller's
// arguments imply and calls the implementation above, exactly as
// `crate::variadic` does for `printf` and `crate::fortify` does for the `_chk`
// family. The macro name is distinct from theirs because every `global_asm!` in
// the crate is assembled as one unit and a repeated `.macro` is an error.
//
// `syslog(priority, format, ...)` names two integer arguments, so `gp_offset`
// starts at 16 and the `va_list` is passed third, in rdx.
core::arch::global_asm!(
    r#"
.text

.macro KINAKAZE_SYSLOG_VA_FRAME gp
    push    rbp
    mov     rbp, rsp
    sub     rsp, 208

    mov     [rbp-176], rdi
    mov     [rbp-168], rsi
    mov     [rbp-160], rdx
    mov     [rbp-152], rcx
    mov     [rbp-144], r8
    mov     [rbp-136], r9

    movups  [rbp-128], xmm0
    movups  [rbp-112], xmm1
    movups  [rbp-96], xmm2
    movups  [rbp-80], xmm3
    movups  [rbp-64], xmm4
    movups  [rbp-48], xmm5
    movups  [rbp-32], xmm6
    movups  [rbp-16], xmm7

    mov     dword ptr [rbp-208], \gp
    mov     dword ptr [rbp-204], 48
    lea     rax, [rbp+16]
    mov     [rbp-200], rax
    lea     rax, [rbp-176]
    mov     [rbp-192], rax
    lea     rax, [rbp-208]
.endm

.globl kinakaze_abi_syslog
kinakaze_abi_syslog:
    KINAKAZE_SYSLOG_VA_FRAME 16
    // rdi and rsi already hold `priority` and `format`.
    mov     rdx, rax
    call    kinakaze_syslog_impl
    leave
    ret
"#
);

// A probe used only by the tests below.
//
// Proving the thunk's argument layout is right needs a *variadic* call into it,
// which is the one thing Rust cannot express, so the call is made from assembly.
// The probe takes a priority, a format and one string in a fixed signature Rust
// can declare, and calls `syslog` with the varargs `(text, 42)` — two arguments,
// so both rdx and rcx are exercised and a wrong `gp_offset` shows up as
// misordered output rather than as nothing. The name is not `kinakaze_abi_*`, so
// it is never exported.
#[cfg(test)]
core::arch::global_asm!(
    r#"
.text

// syslog(priority, format, text, 42)
.globl kinakaze_probe_syslog
kinakaze_probe_syslog:
    push    rbp
    mov     rbp, rsp
    // rdi=priority, rsi=format, rdx=text on entry, which is already the
    // position the first variadic argument occupies.
    mov     ecx, 42
    xor     eax, eax            // no vector arguments
    call    kinakaze_abi_syslog
    leave
    ret
"#
);

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;
    use kinakaze_vfs::ENOENT;

    /// Serializes the tests that touch this module's process-global state.
    ///
    /// Cargo runs tests as threads in one process, and the utmp path, the shell
    /// cursor and the log configuration are all process-wide by specification. A
    /// test that changes one of them would otherwise race a test that reads it.
    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The most recent line [`deliver`] produced, for the syslog tests.
    static DELIVERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub(super) fn record_delivery(line: &str) {
        if let Ok(mut lines) = DELIVERED.lock() {
            lines.push(line.to_string());
        }
    }

    fn take_delivered() -> Vec<String> {
        DELIVERED
            .lock()
            .map(|mut lines| core::mem::take(&mut *lines))
            .unwrap_or_default()
    }

    /// Puts the log state into the "Event Log is unavailable" condition.
    ///
    /// This is the state [`open_source`] reaches on a host that refuses to open
    /// the log, so a test using it exercises the real fallback path. Tests take it
    /// deliberately so that running the suite does not append a record to the
    /// machine's Application log on every invocation.
    fn suppress_event_log() {
        if let Ok(mut state) = log_state().lock() {
            state.source = None;
            state.source_failed = true;
        }
    }

    /// Reads a C string an ABI function produced.
    ///
    /// # Safety
    ///
    /// `text` must be a null-terminated string.
    unsafe fn read(text: *const c_char) -> String {
        assert!(!text.is_null(), "a required string field was null");
        // SAFETY: forwarded from this function's contract.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    /// Walks the whole mount table through the non-reentrant interface.
    fn read_table(path: &CStr) -> Vec<(String, String, String, String)> {
        // SAFETY: both arguments are null-terminated literals.
        let stream = unsafe { kinakaze_abi_setmntent(path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null(), "the mount table must open: {path:?}");
        let mut rows = Vec::new();
        loop {
            // SAFETY: `stream` came from `setmntent` and is still open.
            let entry = unsafe { kinakaze_abi_getmntent(stream) };
            if entry.is_null() {
                break;
            }
            // SAFETY: a non-null entry has four null-terminated string fields.
            unsafe {
                rows.push((
                    read((*entry).mnt_fsname),
                    read((*entry).mnt_dir),
                    read((*entry).mnt_type),
                    read((*entry).mnt_opts),
                ));
            }
            // The table is finite; this guards against a cursor that never moves.
            assert!(rows.len() < 128, "the mount table did not terminate");
        }
        // SAFETY: the stream is live and is not used again.
        assert_eq!(unsafe { kinakaze_abi_endmntent(stream) }, 1);
        rows
    }

    #[test]
    fn struct_layouts_match_the_linux_abi() {
        // Guest code reads these offsets directly, so every one is ABI and a
        // wrong value is silent memory corruption rather than a compile error.

        // struct mntent: four pointers then two ints.
        assert_eq!(size_of::<MntEnt>(), 40);
        assert_eq!(align_of::<MntEnt>(), 8);
        assert_eq!(offset_of!(MntEnt, mnt_fsname), 0);
        assert_eq!(offset_of!(MntEnt, mnt_dir), 8);
        assert_eq!(offset_of!(MntEnt, mnt_type), 16);
        assert_eq!(offset_of!(MntEnt, mnt_opts), 24);
        assert_eq!(offset_of!(MntEnt, mnt_freq), 32);
        assert_eq!(offset_of!(MntEnt, mnt_passno), 36);

        // struct utmpx: 384 bytes, and the reason it is 384 is that `ut_tv` and
        // `ut_session` are 32-bit. A 64-bit timeval would make this 392 and shift
        // everything from `ut_tv` onwards.
        assert_eq!(size_of::<Utmpx>(), 384);
        assert_eq!(align_of::<Utmpx>(), 4);
        assert_eq!(offset_of!(Utmpx, ut_type), 0);
        // 2 bytes of padding after the short, before the 4-aligned pid.
        assert_eq!(offset_of!(Utmpx, ut_pid), 4);
        assert_eq!(offset_of!(Utmpx, ut_line), 8);
        assert_eq!(offset_of!(Utmpx, ut_id), 40);
        assert_eq!(offset_of!(Utmpx, ut_user), 44);
        assert_eq!(offset_of!(Utmpx, ut_host), 76);
        assert_eq!(offset_of!(Utmpx, ut_exit), 332);
        assert_eq!(offset_of!(Utmpx, ut_session), 336);
        assert_eq!(offset_of!(Utmpx, ut_tv), 340);
        assert_eq!(offset_of!(Utmpx, ut_addr_v6), 348);
        assert_eq!(offset_of!(Utmpx, __glibc_reserved), 364);
        // The two nested structs, whose widths are the whole reason for the
        // offsets above.
        assert_eq!(size_of::<UtmpExit>(), 4);
        assert_eq!(
            size_of::<UtmpTimeVal>(),
            8,
            "two 32-bit ints, not a timeval"
        );
        // The on-disk stride must equal the struct size, or a file written by one
        // side would be misread by the other.
        assert_eq!(RECORD_SIZE, 384);

        // struct spwd: two pointers then seven longs.
        assert_eq!(size_of::<Spwd>(), 72);
        assert_eq!(align_of::<Spwd>(), 8);
        assert_eq!(offset_of!(Spwd, sp_namp), 0);
        assert_eq!(offset_of!(Spwd, sp_pwdp), 8);
        assert_eq!(offset_of!(Spwd, sp_lstchg), 16);
        assert_eq!(offset_of!(Spwd, sp_min), 24);
        assert_eq!(offset_of!(Spwd, sp_max), 32);
        assert_eq!(offset_of!(Spwd, sp_warn), 40);
        assert_eq!(offset_of!(Spwd, sp_inact), 48);
        assert_eq!(offset_of!(Spwd, sp_expire), 56);
        assert_eq!(offset_of!(Spwd, sp_flag), 64);
    }

    #[test]
    fn the_mount_table_lists_this_machines_real_volumes() {
        let _guard = exclusive();
        let rows = read_table(c"/etc/mtab");
        assert!(!rows.is_empty(), "the table must not be empty");

        // The root and /proc rows are always present.
        assert!(
            rows.iter().any(|(_, directory, _, _)| directory == "/proc"),
            "the synthetic /proc must appear: {rows:?}"
        );

        let mut drive_rows = 0;
        for (device, directory, filesystem, options) in &rows {
            // A path `df` prints must be a path the guest can `stat`, which means
            // it has to survive the VFS translation.
            assert!(
                directory.starts_with('/'),
                "mnt_dir {directory:?} is not an absolute guest path"
            );
            assert!(
                !directory.contains('\\') && !directory.contains(':'),
                "mnt_dir {directory:?} leaked a Windows path shape"
            );
            assert!(
                kinakaze_vfs::resolve_linux_path(directory).is_ok(),
                "mnt_dir {directory:?} does not resolve through the VFS"
            );
            // Whitespace in any field would split it when the row is printed.
            assert!(
                !filesystem.is_empty() && !filesystem.contains(char::is_whitespace),
                "mnt_type {filesystem:?} is not a single token"
            );
            assert!(
                !device.is_empty(),
                "mnt_fsname must name something: {rows:?}"
            );
            // Read-only and read-write are mutually exclusive and one must be
            // stated, because a caller uses `hasmntopt(entry, "ro")` to decide
            // whether it may write.
            assert!(
                options.starts_with("rw") || options.starts_with("ro"),
                "mnt_opts {options:?} states neither ro nor rw"
            );

            // A single-letter mount point is a drive row; check it against the
            // VFS convention that made it.
            if directory.len() == 2 && directory.as_bytes()[1].is_ascii_alphabetic() {
                drive_rows += 1;
                let letter = directory.as_bytes()[1] as char;
                assert!(
                    letter.is_ascii_lowercase(),
                    "drive mount point {directory:?} must be lowercase to match the VFS"
                );
                // The device is the Windows volume spelling, deliberately not a
                // /dev node. See `DriveEntry::device`.
                assert_eq!(
                    device.to_ascii_lowercase(),
                    format!("{letter}:"),
                    "the device must be the Windows volume name"
                );
                assert!(
                    !device.starts_with("/dev"),
                    "a fabricated /dev node would invite fsck to act on it"
                );
                // The filesystem name came from GetVolumeInformationW, so it is a
                // real Windows filesystem rather than a Linux driver name.
                assert!(
                    matches!(
                        filesystem.as_str(),
                        "ntfs" | "fat32" | "fat" | "exfat" | "cdfs" | "udf" | "refs" | "unknown"
                    ),
                    "unexpected filesystem {filesystem:?} for {directory:?}"
                );
            }
        }
        assert!(
            drive_rows > 0,
            "at least one drive must be reported on a Windows host: {rows:?}"
        );

        // Both spellings of the mount table give the same answer, so a guest
        // cannot get two different views of one machine.
        assert_eq!(
            read_table(c"/proc/mounts"),
            rows,
            "/proc/mounts and /etc/mtab must agree"
        );
        assert_eq!(read_table(c"/proc/self/mounts"), rows);
    }

    #[test]
    fn the_root_row_reports_the_directory_the_vfs_publishes() {
        let rows = read_table(c"/etc/mtab");
        let root = rows
            .iter()
            .find(|(_, directory, _, _)| directory == "/")
            .expect("the guest root is a mount point and must be listed");
        // The device is the real host directory, which is the honest answer to
        // "what is mounted here" and is what makes `df` output actionable.
        let expected = kinakaze_vfs::system_root()
            .expect("the test binary has a parent directory")
            .to_string_lossy()
            .into_owned();
        assert_eq!(root.0, expected);
    }

    #[test]
    fn a_drive_is_described_only_when_windows_will_describe_it() {
        let roots = drive_roots().expect("GetLogicalDriveStringsW must work on Windows");
        assert!(!roots.is_empty(), "a Windows host has at least one drive");
        // Every root has the `X:\` shape the VFS convention depends on.
        for root in &roots {
            assert_eq!(root.len(), 3, "unexpected drive root {root:?}");
            assert!(root.as_bytes()[0].is_ascii_alphabetic());
            assert_eq!(&root[1..], ":\\");
        }
        // The drive the test binary lives on is describable, and it is a fixed
        // disk — which is what makes DRIVE_FIXED the ordinary case.
        let here = kinakaze_vfs::system_root().expect("the test binary has a parent");
        let letter = here.to_string_lossy()[..1].to_string();
        let drive = describe_drive(&format!("{letter}:\\"))
            .expect("the drive holding the test binary must be describable");
        assert_eq!(drive.kind, DRIVE_FIXED);
        assert_eq!(drive.device(), format!("{}:", letter.to_uppercase()));
        assert_eq!(
            drive.mount_point().as_deref(),
            Some(format!("/{}", letter.to_lowercase()).as_str())
        );
        // A drive letter that is not mounted has no description to give.
        assert!(
            describe_drive("\u{7f}:\\").is_none(),
            "a nonexistent root must not be described"
        );
    }

    #[test]
    fn hasmntopt_matches_whole_options_only() {
        let options = CString::new("rw,relatime,root=/dev/sda1,nosuid").unwrap();
        let entry = MntEnt {
            mnt_fsname: ptr::null_mut(),
            mnt_dir: ptr::null_mut(),
            mnt_type: ptr::null_mut(),
            mnt_opts: options.as_ptr().cast_mut(),
            mnt_freq: 0,
            mnt_passno: 0,
        };

        // The classic bug: `ro` must not match inside `root`, and must not match
        // inside `relatime` either. A caller that got a hit here would conclude a
        // read-write mount is read-only.
        // SAFETY: the entry is readable and every option is null-terminated.
        assert!(
            unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"ro".as_ptr()) }.is_null(),
            "`ro` must not match a prefix of `root` or `relatime`"
        );

        // Options at the start, the middle and the end all match.
        for (option, expected) in [
            ("rw", "rw,relatime,root=/dev/sda1,nosuid"),
            ("relatime", "relatime,root=/dev/sda1,nosuid"),
            // A match may end at `=`, which is what makes `key=value` findable.
            ("root", "root=/dev/sda1,nosuid"),
            ("nosuid", "nosuid"),
        ] {
            let name = CString::new(option).unwrap();
            // SAFETY: as above.
            let found = unsafe { kinakaze_abi_hasmntopt(&raw const entry, name.as_ptr()) };
            assert!(!found.is_null(), "{option} must be found");
            // The pointer is into the caller's own option list, not a copy.
            // SAFETY: the returned pointer is inside the null-terminated list.
            assert_eq!(unsafe { read(found) }, expected, "for option {option}");
        }

        // A suffix must not match either: `time` sits inside `relatime`.
        // SAFETY: as above.
        assert!(unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"time".as_ptr()) }.is_null());
        // Nor a value that only appears after an `=`.
        // SAFETY: as above.
        assert!(unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"sda1".as_ptr()) }.is_null());
        // An absent option.
        // SAFETY: as above.
        assert!(unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"noexec".as_ptr()) }.is_null());
        // An empty needle would match everywhere, so it matches nowhere.
        // SAFETY: as above.
        assert!(unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"".as_ptr()) }.is_null());

        // A single-option list exercises both boundaries at once.
        let lone = CString::new("ro").unwrap();
        let entry = MntEnt {
            mnt_opts: lone.as_ptr().cast_mut(),
            ..entry
        };
        // SAFETY: as above.
        assert!(!unsafe { kinakaze_abi_hasmntopt(&raw const entry, c"ro".as_ptr()) }.is_null());

        // A null option list is not a match and must not fault.
        let empty = MntEnt::empty();
        // SAFETY: the entry is readable; its option list is null.
        assert!(unsafe { kinakaze_abi_hasmntopt(&raw const empty, c"ro".as_ptr()) }.is_null());
        // SAFETY: a null entry is explicitly allowed.
        assert!(unsafe { kinakaze_abi_hasmntopt(ptr::null(), c"ro".as_ptr()) }.is_null());
    }

    #[test]
    fn getmntent_r_fills_the_callers_buffer_and_reports_erange() {
        // SAFETY: null-terminated literals.
        let stream = unsafe { kinakaze_abi_setmntent(c"/etc/mtab".as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());

        // A buffer far too small must report ERANGE, write nothing, and leave the
        // cursor where it was so the caller can retry.
        let mut entry = MntEnt::empty();
        let mut tiny = [0xAAu8; 4];
        // SAFETY: the stream is live, the entry writable, and only 4 bytes of
        // buffer are offered.
        let result = unsafe {
            kinakaze_abi_getmntent_r(
                stream,
                &raw mut entry,
                tiny.as_mut_ptr().cast::<c_char>(),
                tiny.len() as c_int,
            )
        };
        assert!(result.is_null(), "a tiny buffer must fail");
        assert_eq!(kinakaze_tls::errno(), ERANGE);
        assert!(
            entry.mnt_dir.is_null(),
            "a failed call must not touch the entry"
        );
        assert_eq!(tiny, [0xAA; 4], "a failed call must not write the buffer");

        // The retry with a real buffer gets the row the failed call would have.
        let mut buffer = [0u8; 1024];
        // SAFETY: the stream is live and the buffer is 1024 writable bytes.
        let result = unsafe {
            kinakaze_abi_getmntent_r(
                stream,
                &raw mut entry,
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len() as c_int,
            )
        };
        // The return value is the entry pointer, not an errno: this is the one
        // reentrant function here that does not use the errno-returning
        // convention, and a caller tests it as a pointer.
        assert_eq!(result, &raw mut entry, "getmntent_r returns its entry");
        // SAFETY: the fields point into `buffer`, which is still alive.
        let directory = unsafe { read(entry.mnt_dir) };
        assert_eq!(directory, "/", "the first row is the guest root");
        // Every string must live inside the caller's buffer, not in ours.
        let base = buffer.as_ptr() as usize;
        let span = base..base + buffer.len();
        for field in [
            entry.mnt_fsname,
            entry.mnt_dir,
            entry.mnt_type,
            entry.mnt_opts,
        ] {
            assert!(
                span.contains(&(field as usize)),
                "a string escaped the caller's buffer"
            );
        }

        // A negative length is no space at all, not an enormous one.
        // SAFETY: the stream is live; no buffer is read because the length is
        // treated as zero.
        let result = unsafe {
            kinakaze_abi_getmntent_r(stream, &raw mut entry, buffer.as_mut_ptr().cast(), -1)
        };
        assert!(result.is_null());

        // SAFETY: the stream is live and unused afterwards.
        assert_eq!(unsafe { kinakaze_abi_endmntent(stream) }, 1);
    }

    #[test]
    fn the_synthesized_table_refuses_to_be_written() {
        for path in [c"/etc/mtab", c"/proc/mounts", c"/proc/self/mounts"] {
            // SAFETY: null-terminated literals.
            let stream = unsafe { kinakaze_abi_setmntent(path.as_ptr(), c"w".as_ptr()) };
            assert!(stream.is_null(), "{path:?} must not open for writing");
            assert_eq!(
                kinakaze_tls::errno(),
                EROFS,
                "a derived table has nothing to write to"
            );
        }
    }

    #[test]
    fn a_non_table_path_is_a_real_file_operation() {
        let name = format!("/tmp/kinakaze-mtab-{}.tab", std::process::id());
        let host = kinakaze_vfs::resolve_linux_path(&name).expect("the path must resolve");
        if let Some(parent) = host.parent() {
            std::fs::create_dir_all(parent).expect("the temporary directory must be creatable");
        }
        // A private mtab, which is what `mount -f` writes.
        std::fs::write(
            &host,
            "# a comment\n\n/dev/sda1 / ext4 rw,relatime 0 1\nproc /proc proc rw 0 0\n\
             /dev/sdb1 /mnt/with\\040space vfat ro 0 0\n",
        )
        .expect("the fixture must be writable");

        let rows = read_table(&CString::new(name.clone()).unwrap());
        assert_eq!(rows.len(), 3, "comments and blanks are skipped: {rows:?}");
        assert_eq!(
            rows[0],
            (
                String::from("/dev/sda1"),
                String::from("/"),
                String::from("ext4"),
                String::from("rw,relatime"),
            )
        );
        // The escape glibc uses for a space in a mount point is decoded.
        assert_eq!(rows[2].1, "/mnt/with space");

        // Opening a missing file reports the real reason rather than an empty
        // table, so a caller is not told a file exists when it does not.
        let missing = CString::new(format!("{name}.absent")).unwrap();
        // SAFETY: null-terminated strings.
        let stream = unsafe { kinakaze_abi_setmntent(missing.as_ptr(), c"r".as_ptr()) };
        assert!(stream.is_null());
        assert_eq!(kinakaze_tls::errno(), ENOENT);

        // Write mode creates the file for real.
        let writable = CString::new(format!("{name}.new")).unwrap();
        // SAFETY: null-terminated strings.
        let stream = unsafe { kinakaze_abi_setmntent(writable.as_ptr(), c"w".as_ptr()) };
        assert!(!stream.is_null(), "write mode must create the file");
        // Reading a write stream reports end-of-table, as a write-only FILE would.
        // SAFETY: the stream is live.
        assert!(unsafe { kinakaze_abi_getmntent(stream) }.is_null());
        // SAFETY: the stream is live and unused afterwards.
        assert_eq!(unsafe { kinakaze_abi_endmntent(stream) }, 1);
        let created = kinakaze_vfs::resolve_linux_path(&format!("{name}.new")).unwrap();
        assert!(created.is_file(), "the file must exist on the host");

        let _ = std::fs::remove_file(&host);
        let _ = std::fs::remove_file(&created);
    }

    #[test]
    fn the_returned_entry_survives_endmntent() {
        // BusyBox's `find_mount_point` walks the table, calls `endmntent`, and
        // *then* reads the last entry it received. This test is that sequence.
        // Per-stream storage passes every other test here and fails this one, and
        // the visible symptom was a corrupted `Filesystem` column in `df`.
        // SAFETY: null-terminated literals.
        let stream = unsafe { kinakaze_abi_setmntent(c"/etc/mtab".as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());
        let mut last: *mut MntEnt = ptr::null_mut();
        loop {
            // SAFETY: the stream is live.
            let entry = unsafe { kinakaze_abi_getmntent(stream) };
            if entry.is_null() {
                break;
            }
            last = entry;
        }
        assert!(!last.is_null(), "the table has at least one row");
        // SAFETY: the stream is live and is not used again.
        assert_eq!(unsafe { kinakaze_abi_endmntent(stream) }, 1);

        // The pointer, and every string it names, must still be readable.
        // SAFETY: the entry addresses process-global storage that `endmntent`
        // does not touch, which is the property under test.
        let entry = unsafe { *last };
        // SAFETY: the fields are null-terminated strings owned by this module.
        let device = unsafe { read(entry.mnt_fsname) };
        // SAFETY: as above.
        let directory = unsafe { read(entry.mnt_dir) };
        // SAFETY: as above.
        let filesystem = unsafe { read(entry.mnt_type) };
        assert!(
            !device.is_empty() && !filesystem.is_empty(),
            "the strings must outlive the stream, not dangle"
        );
        assert!(directory.starts_with('/'));
        // whole device string is intact rather than truncated — a dangling read tended to yield a prefix.
        assert!(
            !device.is_empty(),
            "device string is intact: got {device:?}"
        );

        // A second call on a *different* stream overwrites the shared result,
        // which is glibc's documented hazard and is asserted here so the
        // behaviour is deliberate rather than accidental.
        // SAFETY: null-terminated literals.
        let other = unsafe { kinakaze_abi_setmntent(c"/etc/mtab".as_ptr(), c"r".as_ptr()) };
        // SAFETY: the new stream is live.
        let first = unsafe { kinakaze_abi_getmntent(other) };
        assert_eq!(first, last, "both streams publish through one static");
        // SAFETY: the fields are null-terminated.
        assert_eq!(unsafe { read((*first).mnt_dir) }, "/", "now the first row");
        // SAFETY: the stream is live and unused afterwards.
        assert_eq!(unsafe { kinakaze_abi_endmntent(other) }, 1);
    }

    #[test]
    fn setmntent_returns_a_real_file_for_getdelim_and_rewind() {
        // OpenJDK's LinuxFileSystem asks native code to measure a mount-table
        // line with __getdelim, rewinds the same FILE, then walks it with
        // getmntent_r. This exact sequence exposed the old pseudo-FILE stream.
        // SAFETY: null-terminated literals.
        let stream = unsafe { kinakaze_abi_setmntent(c"/etc/mtab".as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());
        let file = stream.cast::<crate::stdio::File>();

        let mut line = ptr::null_mut();
        let mut capacity = 0usize;
        // SAFETY: `file` is a real live FILE and the output slots are writable.
        let length = unsafe {
            crate::fdio::kinakaze_abi___getdelim(
                &raw mut line,
                &raw mut capacity,
                c_int::from(b'\n'),
                file,
            )
        };
        assert!(length > 0, "the first mount-table line must be readable");
        crate::stdio::rewind(file);

        let mut entry = MntEnt::empty();
        let mut buffer = [0u8; 1024];
        // SAFETY: the stream, entry and buffer are all live.
        let result = unsafe {
            kinakaze_abi_getmntent_r(
                stream,
                &raw mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len() as c_int,
            )
        };
        assert_eq!(result, &raw mut entry);
        // SAFETY: the successful call placed a null-terminated string in buffer.
        assert_eq!(unsafe { read(entry.mnt_dir) }, "/");

        // SAFETY: getdelim allocated `line` with this libc allocator.
        unsafe { crate::kinakaze_free(line.cast::<c_void>()) };
        // SAFETY: the stream is live and unused afterwards.
        assert_eq!(unsafe { kinakaze_abi_endmntent(stream) }, 1);
    }

    #[test]
    fn getmntent_accepts_an_ordinary_fopen_stream() {
        let name = format!("/tmp/kinakaze-mtab-fopen-{}.tab", std::process::id());
        let host = kinakaze_vfs::resolve_linux_path(&name).expect("the path must resolve");
        if let Some(parent) = host.parent() {
            std::fs::create_dir_all(parent).expect("the temporary directory must be creatable");
        }
        std::fs::write(&host, "/dev/test /mnt/test ext4 rw 0 0\n")
            .expect("the fixture must be writable");
        let path = CString::new(name).unwrap();
        // SAFETY: both strings are null-terminated.
        let file = unsafe { crate::stdio::fopen(path.as_ptr(), c"r".as_ptr()) };
        assert!(!file.is_null());
        // SAFETY: glibc accepts any live FILE containing mtab-shaped text.
        let entry = unsafe { kinakaze_abi_getmntent(file.cast::<c_void>()) };
        assert!(!entry.is_null());
        // SAFETY: the returned fields are null-terminated process-global data.
        assert_eq!(unsafe { read((*entry).mnt_dir) }, "/mnt/test");
        // SAFETY: `file` is live and unused afterwards.
        assert_eq!(unsafe { kinakaze_abi_endmntent(file.cast()) }, 1);
        let _ = std::fs::remove_file(host);
    }

    #[test]
    fn escapes_and_malformed_lines_are_handled_like_glibc() {
        assert_eq!(unescape("plain"), "plain");
        assert_eq!(unescape(r"with\040space"), "with space");
        assert_eq!(unescape(r"tab\011here"), "tab\there");
        assert_eq!(unescape(r"back\134slash"), r"back\slash");
        // Only the four sequences glibc decodes are decoded; anything else is
        // left exactly as it was found.
        assert_eq!(unescape(r"\101"), r"\101");
        assert_eq!(unescape(r"trailing\"), r"trailing\");
        assert_eq!(unescape(r"\04"), r"\04");

        // A line naming fewer than two fields cannot describe a mount.
        let rows = parse_table("onlyonefield\n/dev/sda1 /\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].directory.to_str().unwrap(), "/");
        // Missing type, options and numbers take glibc's defaults.
        assert_eq!(rows[0].filesystem.to_str().unwrap(), "");
        assert_eq!(rows[0].frequency, 0);
        assert_eq!(rows[0].pass, 0);

        // A mode string that asks to write is recognized in all its spellings.
        for mode in ["w", "a", "r+", "w+", "a+"] {
            assert!(mode_writes(mode), "{mode} writes");
        }
        for mode in ["r", "rb", "rt"] {
            assert!(!mode_writes(mode), "{mode} does not write");
        }
    }

    #[test]
    fn getspnam_r_reports_absence_as_success_with_a_null_result() {
        let mut entry = Spwd {
            sp_namp: ptr::null_mut(),
            sp_pwdp: ptr::null_mut(),
            sp_lstchg: -1,
            sp_min: -1,
            sp_max: -1,
            sp_warn: -1,
            sp_inact: -1,
            sp_expire: -1,
            sp_flag: 0,
        };
        let mut buffer = [0u8; 256];
        // Deliberately not null to start with, so a call that forgot to clear it
        // would be caught.
        let mut result: *mut Spwd = (&raw mut entry).wrapping_add(1);

        for name in [c"root", c"nosuchuser"] {
            // SAFETY: the name is a literal, the entry and result are writable
            // locals, and the buffer is 256 writable bytes.
            let code = unsafe {
                kinakaze_abi_getspnam_r(
                    name.as_ptr(),
                    &raw mut entry,
                    buffer.as_mut_ptr().cast::<c_char>(),
                    buffer.len(),
                    &raw mut result,
                )
            };
            // This is the distinction that matters: 0 means the lookup worked,
            // and the null result means there was no entry. Returning ENOENT
            // would make `su` report a failed shadow lookup and stop, instead of
            // falling back to the passwd entry.
            assert_eq!(code, 0, "absence is not an error, for {name:?}");
            assert!(result.is_null(), "there is no shadow entry to point at");
            assert_ne!(code, ENOENT, "ENOENT would be read as a lookup failure");
        }

        // No hash was fabricated into the caller's struct, which is the whole
        // point of reporting absence. See `crate::userdb`'s shadow section.
        assert!(entry.sp_pwdp.is_null());
        assert!(entry.sp_namp.is_null());
        assert_eq!(buffer[0], 0, "nothing was packed into the buffer");

        // A null result pointer is the caller's bug and is reported as EFAULT,
        // because there is nowhere to write the answer.
        // SAFETY: passing null for `result` is the case under test.
        let code = unsafe {
            kinakaze_abi_getspnam_r(
                c"root".as_ptr(),
                &raw mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                ptr::null_mut(),
            )
        };
        assert_eq!(code, kinakaze_vfs::EFAULT);
    }

    #[test]
    fn the_utmp_iteration_protocol_terminates() {
        let _guard = exclusive();
        // The documented protocol: setutxent, then getutxent until null, then
        // endutxent. On a host where nothing has logged in the first call already
        // returns null, which is the truthful empty database.
        kinakaze_abi_setutxent();
        let mut seen = 0;
        while !kinakaze_abi_getutxent().is_null() {
            seen += 1;
            assert!(seen < 4096, "the iteration did not terminate");
        }
        kinakaze_abi_endutxent();

        // Calling past the end keeps returning null rather than wrapping.
        kinakaze_abi_setutxent();
        while !kinakaze_abi_getutxent().is_null() {}
        assert!(kinakaze_abi_getutxent().is_null());
        assert!(kinakaze_abi_getutxent().is_null());
        kinakaze_abi_endutxent();

        // A second walk starts from the beginning and sees the same count, so the
        // enumeration is repeatable.
        kinakaze_abi_setutxent();
        let mut again = 0;
        while !kinakaze_abi_getutxent().is_null() {
            again += 1;
        }
        kinakaze_abi_endutxent();
        assert_eq!(seen, again);
    }

    #[test]
    fn utmp_writes_reach_the_file_and_read_back() {
        let _guard = exclusive();
        let name = format!("/tmp/kinakaze-utmp-{}.db", std::process::id());
        let host = kinakaze_vfs::resolve_linux_path(&name).expect("the path must resolve");
        let _ = std::fs::remove_file(&host);

        let path = CString::new(name.clone()).unwrap();
        // SAFETY: a null-terminated string.
        assert_eq!(unsafe { kinakaze_abi_utmpxname(path.as_ptr()) }, 0);

        // A record the caller owns. Nothing here is synthesized by this module,
        // which is why writing it is honest.
        let mut record = Utmpx::zeroed();
        record.ut_type = USER_PROCESS;
        record.ut_pid = 4321;
        record.ut_id[..2].copy_from_slice(&[b'p' as c_char, b'1' as c_char]);
        for (slot, byte) in record.ut_user.iter_mut().zip(b"tester") {
            *slot = *byte as c_char;
        }
        record.ut_tv = UtmpTimeVal {
            tv_sec: 1_700_000_000,
            tv_usec: 500,
        };

        // SAFETY: the record is a readable local.
        let written = unsafe { kinakaze_abi_pututxline(&raw const record) };
        assert!(!written.is_null(), "the write must succeed on a real file");
        assert!(host.is_file(), "the record must reach the host filesystem");
        assert_eq!(
            std::fs::metadata(&host).unwrap().len(),
            RECORD_SIZE as u64,
            "exactly one record, at the ABI stride"
        );

        // Reading it back gives the caller's own record, byte for byte.
        kinakaze_abi_setutxent();
        let read_back = kinakaze_abi_getutxent();
        assert!(
            !read_back.is_null(),
            "the record just written must be found"
        );
        // SAFETY: a non-null return addresses this module's static record.
        let read_back = unsafe { *read_back };
        assert_eq!(read_back.ut_type, USER_PROCESS);
        assert_eq!(read_back.ut_pid, 4321);
        assert_eq!(read_back.ut_tv.tv_sec, 1_700_000_000);
        assert_eq!(read_back.ut_tv.tv_usec, 500);
        assert_eq!(read_back.ut_id, record.ut_id);
        assert_eq!(read_back.ut_user, record.ut_user);
        assert!(
            kinakaze_abi_getutxent().is_null(),
            "there is only one record"
        );
        kinakaze_abi_endutxent();

        // A second write to the same slot replaces rather than appends: the id
        // matches, so `same_slot` finds it.
        let mut updated = record;
        updated.ut_type = DEAD_PROCESS;
        // SAFETY: a readable local.
        assert!(!unsafe { kinakaze_abi_pututxline(&raw const updated) }.is_null());
        assert_eq!(
            std::fs::metadata(&host).unwrap().len(),
            RECORD_SIZE as u64,
            "the matching slot was replaced, not appended to"
        );

        // A different id is a different slot and does append.
        let mut other = record;
        other.ut_id[1] = b'2' as c_char;
        // SAFETY: a readable local.
        assert!(!unsafe { kinakaze_abi_pututxline(&raw const other) }.is_null());
        assert_eq!(
            std::fs::metadata(&host).unwrap().len(),
            2 * RECORD_SIZE as u64
        );

        // An EMPTY record names no slot, so it is refused as malformed rather
        // than written somewhere arbitrary.
        let empty = Utmpx::zeroed();
        crate::set_errno(0);
        // SAFETY: a readable local.
        assert!(unsafe { kinakaze_abi_pututxline(&raw const empty) }.is_null());
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // updwtmpx appends to whatever file it is given, which is how `last`
        // gets its history.
        let wtmp = format!("/tmp/kinakaze-wtmp-{}.db", std::process::id());
        let wtmp_host = kinakaze_vfs::resolve_linux_path(&wtmp).unwrap();
        let _ = std::fs::remove_file(&wtmp_host);
        let wtmp_path = CString::new(wtmp.clone()).unwrap();
        crate::set_errno(0);
        // SAFETY: both arguments are live and null-terminated where required.
        unsafe { kinakaze_abi_updwtmpx(wtmp_path.as_ptr(), &raw const record) };
        // The call returns void, so errno is the only channel; it must be clean.
        assert_eq!(kinakaze_tls::errno(), 0, "the append must have succeeded");
        assert_eq!(
            std::fs::metadata(&wtmp_host).unwrap().len(),
            RECORD_SIZE as u64
        );
        // A second append grows the history rather than replacing it.
        // SAFETY: as above.
        unsafe { kinakaze_abi_updwtmpx(wtmp_path.as_ptr(), &raw const record) };
        assert_eq!(
            std::fs::metadata(&wtmp_host).unwrap().len(),
            2 * RECORD_SIZE as u64
        );

        // Restore the default so no later test sees the fixture.
        // SAFETY: a null-terminated literal.
        assert_eq!(
            unsafe { kinakaze_abi_utmpxname(c"/var/run/utmp".as_ptr()) },
            0
        );
        kinakaze_abi_endutxent();
        let _ = std::fs::remove_file(&host);
        let _ = std::fs::remove_file(&wtmp_host);
    }

    #[test]
    fn slot_matching_follows_glibcs_rule() {
        let mut process = Utmpx::zeroed();
        process.ut_type = USER_PROCESS;
        process.ut_id = [b'p' as c_char, b'1' as c_char, 0, 0];

        // A process record matches another process record with the same id, even
        // across the four process types, which is how a login becomes a logout.
        let mut dead = process;
        dead.ut_type = DEAD_PROCESS;
        assert!(same_slot(&process, &dead));
        let mut init = process;
        init.ut_type = INIT_PROCESS;
        assert!(same_slot(&init, &process));
        let mut login = process;
        login.ut_type = LOGIN_PROCESS;
        assert!(same_slot(&login, &process));

        // A different id is a different slot.
        let mut elsewhere = process;
        elsewhere.ut_id = [b'p' as c_char, b'2' as c_char, 0, 0];
        assert!(!same_slot(&elsewhere, &process));

        // The singleton types match on type alone, because a database holds one
        // of each.
        for kind in [RUN_LVL, BOOT_TIME, NEW_TIME, OLD_TIME] {
            let mut a = Utmpx::zeroed();
            a.ut_type = kind;
            let mut b = Utmpx::zeroed();
            b.ut_type = kind;
            b.ut_id = [9, 9, 9, 9];
            assert!(same_slot(&a, &b), "type {kind} matches on type alone");
            // And not against a different type.
            let mut other = Utmpx::zeroed();
            other.ut_type = if kind == RUN_LVL { BOOT_TIME } else { RUN_LVL };
            assert!(!same_slot(&other, &b));
        }

        // EMPTY and ACCOUNTING name no slot at all.
        for kind in [EMPTY, ACCOUNTING] {
            let mut record = Utmpx::zeroed();
            record.ut_type = kind;
            assert!(!same_slot(&record, &record));
        }
    }

    #[test]
    fn a_record_survives_the_byte_round_trip() {
        // The on-disk form is the struct's own bytes, so a record written by this
        // module and read back by it must be identical — otherwise every field
        // after a mis-sized one would be silently shifted.
        let mut record = Utmpx::zeroed();
        record.ut_type = USER_PROCESS;
        record.ut_pid = 0x1234_5678;
        record.ut_session = 0x0BAD_F00D_u32 as i32;
        record.ut_tv = UtmpTimeVal {
            tv_sec: i32::MAX,
            tv_usec: -1,
        };
        record.ut_addr_v6 = [1, 2, 3, 4];
        record.ut_exit = UtmpExit {
            e_termination: 7,
            e_exit: 11,
        };

        let bytes = record.as_bytes().to_vec();
        assert_eq!(bytes.len(), 384);
        let restored = Utmpx::from_bytes(&bytes).expect("a full record must decode");
        assert_eq!(restored.ut_type, record.ut_type);
        assert_eq!(restored.ut_pid, record.ut_pid);
        assert_eq!(restored.ut_session, record.ut_session);
        assert_eq!(restored.ut_tv, record.ut_tv);
        assert_eq!(restored.ut_addr_v6, record.ut_addr_v6);
        assert_eq!(restored.ut_exit, record.ut_exit);

        // A partial record is not a record. glibc ignores a truncated tail, and
        // decoding one would produce a half-initialized entry.
        assert!(Utmpx::from_bytes(&bytes[..383]).is_none());
        assert!(Utmpx::from_bytes(&[]).is_none());
    }

    #[test]
    fn the_shell_list_is_walkable_and_rewindable() {
        let _guard = exclusive();
        kinakaze_abi_setusershell();
        let mut shells = Vec::new();
        loop {
            let shell = kinakaze_abi_getusershell();
            if shell.is_null() {
                break;
            }
            // SAFETY: a non-null return is a null-terminated string owned by this
            // module for the life of the process.
            shells.push(unsafe { read(shell) });
            assert!(shells.len() < 64, "the shell list did not terminate");
        }
        // Checked before `endusershell`, which rewinds: calling past the end
        // keeps returning null rather than wrapping around to the first entry.
        assert!(kinakaze_abi_getusershell().is_null());
        assert!(kinakaze_abi_getusershell().is_null());
        kinakaze_abi_endusershell();

        assert!(!shells.is_empty(), "at least one shell must be reported");
        // Every entry is an absolute path, because a caller execs what it gets.
        for shell in &shells {
            assert!(shell.starts_with('/'), "{shell:?} is not an absolute path");
        }
        // `/bin/sh` is always present, for consistency with `crate::userdb`'s
        // `pw_shell`: `chsh` validates the account's shell against this list, and
        // omitting it would make the one account fail its own validation.
        assert!(
            shells.iter().any(|shell| shell == "/bin/sh"),
            "the shell userdb reports must be listed: {shells:?}"
        );

        // Rewinding gives the same list again, which is what makes a second walk
        // possible — the reason `setusershell` had to ship alongside the getter.
        kinakaze_abi_setusershell();
        let mut again = Vec::new();
        loop {
            let shell = kinakaze_abi_getusershell();
            if shell.is_null() {
                break;
            }
            // SAFETY: as above.
            again.push(unsafe { read(shell) });
        }
        assert_eq!(shells, again);
        // `endusershell` rewinds too, so a walk after it starts from the top.
        kinakaze_abi_endusershell();
        assert!(!kinakaze_abi_getusershell().is_null());
        kinakaze_abi_endusershell();
    }

    #[test]
    fn etc_shells_is_parsed_when_it_exists() {
        // The parser is tested directly, because `shells()` caches for the life of
        // the process and a fixture written now might lose a race with a test that
        // already forced the cache.
        let parsed = parse_shells(
            "# /etc/shells: valid login shells\n\n/bin/sh\n/bin/bash # trailing\n\
             notapath\n  /usr/bin/fish  \n",
        );
        assert_eq!(parsed, vec!["/bin/sh", "/bin/bash", "/usr/bin/fish"]);
        // A line that is not an absolute path is dropped rather than returned,
        // since a caller will try to exec it.
        assert!(parse_shells("bash\nsh\n").is_empty());
    }

    #[test]
    fn the_priority_mapping_is_exhaustive() {
        // Every Linux priority maps onto a Windows event type, and the grouping
        // is the one the Event Viewer's own filters assume.
        for priority in [LOG_EMERG, LOG_ALERT, LOG_CRIT, LOG_ERR] {
            assert_eq!(
                event_type(priority),
                EVENTLOG_ERROR_TYPE,
                "priority {priority} is an error"
            );
        }
        assert_eq!(event_type(LOG_WARNING), EVENTLOG_WARNING_TYPE);
        for priority in [LOG_NOTICE, LOG_INFO, LOG_DEBUG] {
            assert_eq!(
                event_type(priority),
                EVENTLOG_INFORMATION_TYPE,
                "priority {priority} is informational"
            );
        }
        // All eight are covered, so the loop above is exhaustive over the range.
        assert_eq!((LOG_EMERG..=LOG_DEBUG).count(), 8);

        // A facility ORed in must not change the severity: the priority is the
        // low three bits and the facility everything above them.
        for facility in [LOG_KERN, LOG_USER, LOG_DAEMON, LOG_AUTH, LOG_LOCAL0] {
            assert_eq!(event_type(facility | LOG_ERR), EVENTLOG_ERROR_TYPE);
            assert_eq!(event_type(facility | LOG_WARNING), EVENTLOG_WARNING_TYPE);
            assert_eq!(event_type(facility | LOG_INFO), EVENTLOG_INFORMATION_TYPE);
        }
        // The masks do not overlap and together cover the argument.
        assert_eq!(PRIORITY_MASK & FACILITY_MASK, 0);
        assert_eq!(FACILITY_MASK >> 3, 0x7f);
        // An out-of-range value is logged as informational rather than dropped.
        assert_eq!(event_type(999), EVENTLOG_INFORMATION_TYPE);
    }

    #[test]
    fn the_errno_conversion_expands_only_a_bare_percent_m() {
        crate::set_errno(ENOENT);
        // SAFETY: `strerror` returns a null-terminated static string.
        let expected = unsafe { CStr::from_ptr(crate::string::strerror(ENOENT)) }
            .to_string_lossy()
            .into_owned();

        // SAFETY: a null-terminated literal.
        let expanded = unsafe { expand_errno(c"open failed: %m".as_ptr()) }
            .expect("a format containing %m must be rewritten");
        assert_eq!(
            expanded.to_str().unwrap(),
            format!("open failed: {expected}")
        );

        // `%%m` is a literal `%m` and must survive untouched, which is why the
        // scan steps over a doubled percent as a unit.
        // SAFETY: a null-terminated literal.
        assert!(
            unsafe { expand_errno(c"literal %%m here".as_ptr()) }.is_none(),
            "%%m is not a conversion and needs no rewrite"
        );
        // A format with no %m allocates nothing.
        // SAFETY: a null-terminated literal.
        assert!(unsafe { expand_errno(c"plain %s and %d".as_ptr()) }.is_none());
        // A trailing percent is left for the format engine.
        // SAFETY: a null-terminated literal.
        assert!(unsafe { expand_errno(c"trailing %".as_ptr()) }.is_none());

        // The detector agrees with the rewriter on every case above.
        assert!(contains_errno_conversion(b"x %m"));
        assert!(!contains_errno_conversion(b"x %%m"));
        assert!(!contains_errno_conversion(b"x %"));
        assert!(contains_errno_conversion(b"%d %m"));
        // A `%m` after a conversion that itself contains an `m` is still found.
        assert!(contains_errno_conversion(b"%s%m"));
    }

    #[test]
    fn openlog_shapes_the_line_and_closelog_resets_it() {
        let _guard = exclusive();
        let pid = crate::process::kinakaze_abi_getpid();

        // LOG_PID adds the bracketed pid, exactly as glibc formats it.
        // SAFETY: a null-terminated literal.
        unsafe { kinakaze_abi_openlog(c"probe".as_ptr(), LOG_PID, LOG_DAEMON) };
        {
            let state = log_state().lock().unwrap();
            assert_eq!(state.ident, "probe");
            assert_eq!(state.facility, LOG_DAEMON, "the facility is honoured");
            assert_eq!(compose(&state, "hello"), format!("probe[{pid}]: hello"));
        }

        // Without LOG_PID the brackets are absent.
        // SAFETY: a null-terminated literal.
        unsafe { kinakaze_abi_openlog(c"probe".as_ptr(), 0, 0) };
        {
            let state = log_state().lock().unwrap();
            // A zero facility leaves the previous one in place, which is what
            // makes `openlog(ident, opts, 0)` an identity-only change.
            assert_eq!(state.facility, LOG_DAEMON);
            assert_eq!(compose(&state, "hello"), "probe: hello");
        }

        // A null ident falls back to the real program name, not a placeholder.
        // SAFETY: a null ident is explicitly allowed.
        unsafe { kinakaze_abi_openlog(ptr::null(), 0, LOG_USER) };
        {
            let state = log_state().lock().unwrap();
            assert!(state.ident.is_empty());
            let line = compose(&state, "hello");
            assert_eq!(line, format!("{}: hello", program_name()));
            assert!(!program_name().is_empty());
        }

        // closelog returns everything to the pre-openlog state.
        kinakaze_abi_closelog();
        {
            let state = log_state().lock().unwrap();
            assert!(state.ident.is_empty());
            assert_eq!(state.options, 0);
            assert_eq!(state.facility, LOG_USER, "back to the default facility");
            assert!(state.source.is_none(), "the Event Log handle was released");
        }
    }

    #[test]
    fn syslog_delivers_a_formatted_line_through_the_variadic_thunk() {
        let _guard = exclusive();
        unsafe extern "sysv64" {
            /// Calls `syslog(priority, format, text, 42)` from assembly, which is
            /// the only way to make a genuinely variadic call into the thunk.
            fn kinakaze_probe_syslog(priority: c_int, format: *const c_char, text: *const c_char);
        }

        // SAFETY: a null-terminated literal.
        unsafe { kinakaze_abi_openlog(c"probe".as_ptr(), LOG_PID | LOG_PERROR, LOG_USER) };
        // Forced down the stderr fallback, so this test asserts on a deterministic
        // path and does not append a record to the machine's Application log every
        // time the suite runs. `source_failed` is the state the production code
        // genuinely enters when the Event Log cannot be opened, so the path under
        // test is a real one rather than a test-only shortcut.
        suppress_event_log();
        let _ = take_delivered();

        // SAFETY: both pointers are null-terminated literals, and the probe
        // passes exactly the `(const char *, int)` the format consumes.
        unsafe {
            kinakaze_probe_syslog(
                LOG_USER | LOG_ERR,
                c"msg=%s n=%d".as_ptr(),
                c"text".as_ptr(),
            );
        }
        let lines = take_delivered();
        assert_eq!(lines.len(), 1, "one call produces one line: {lines:?}");
        // Both variadic arguments arrived, in order, which is what proves the
        // thunk's gp_offset of 16 is right: a wrong offset would read `format` or
        // `priority` as the first argument.
        assert_eq!(
            lines[0],
            format!(
                "probe[{}]: msg=text n=42",
                crate::process::kinakaze_abi_getpid()
            )
        );

        // `%m` expands through the same path, with the errno current at the call.
        crate::set_errno(ENOENT);
        // SAFETY: `strerror` returns a null-terminated static string.
        let message = unsafe { CStr::from_ptr(crate::string::strerror(ENOENT)) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: as above; the format consumes the string and the int.
        unsafe {
            kinakaze_probe_syslog(LOG_ERR, c"failed %s (%d): %m".as_ptr(), c"open".as_ptr());
        }
        let lines = take_delivered();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].ends_with(&format!("failed open (42): {message}")),
            "%m must expand to the errno text: {:?}",
            lines[0]
        );

        kinakaze_abi_closelog();
    }

    #[test]
    fn vsyslog_without_arguments_still_reports_the_message() {
        let _guard = exclusive();
        // SAFETY: a null-terminated literal.
        unsafe { kinakaze_abi_openlog(c"bare".as_ptr(), LOG_PERROR, LOG_USER) };
        suppress_event_log();
        let _ = take_delivered();
        // A caller passing a plain string and no va_list is common, and dropping
        // the message would lose a real log line.
        // SAFETY: the format is a null-terminated literal and a null va_list is
        // the case under test.
        unsafe { kinakaze_abi_vsyslog(LOG_INFO, c"plain message".as_ptr(), ptr::null_mut()) };
        let lines = take_delivered();
        assert_eq!(lines, vec![String::from("bare: plain message")]);

        // A null format is not a message and must not fault.
        // SAFETY: a null format is explicitly handled.
        unsafe { kinakaze_abi_vsyslog(LOG_INFO, ptr::null(), ptr::null_mut()) };
        assert!(take_delivered().is_empty());
        kinakaze_abi_closelog();
    }

    #[test]
    fn the_event_log_is_reachable_without_privileges() {
        let _guard = exclusive();
        // The claim the module header makes, checked rather than assumed: an
        // unprivileged process can open a source that has no registry key. If this
        // ever fails, the Event Log stops being the normal path and the stderr
        // fallback becomes the only one — which is a change worth learning about
        // from a test rather than from a user's missing logs.
        kinakaze_abi_closelog();
        let mut state = log_state().lock().expect("the log state must be lockable");
        open_source(&mut state, "kinakaze-selftest");
        let opened = state.source.is_some();
        assert!(
            !(state.source_failed && opened),
            "a successful open must not also be recorded as a failure"
        );
        drop(state);
        assert!(
            opened,
            "RegisterEventSourceW must succeed for an unregistered source name"
        );
        // Deliberately no `ReportEventW` here: the handle being open is the fact
        // under test, and writing would put a record in the machine's Application
        // log on every run of the suite.
        kinakaze_abi_closelog();
        let state = log_state().lock().unwrap();
        assert!(state.source.is_none(), "closelog must release the handle");
        assert!(
            !state.source_failed,
            "closelog must clear the failure latch so a later openlog retries"
        );
    }

    #[test]
    fn synthetic_table_paths_are_recognized_exactly() {
        for path in [
            "/etc/mtab",
            "/etc/mnttab",
            "/proc/mounts",
            "/proc/self/mounts",
            // A trailing slash cannot change which file is meant.
            "/proc/mounts/",
        ] {
            assert!(is_synthetic_table(path), "{path} is the mount table");
        }
        // `/etc/fstab` is configuration, not state. Answering it with the live
        // table would make `mount -a` re-mount everything already mounted.
        for path in [
            "/etc/fstab",
            "/proc/self/mountinfo",
            "/tmp/mtab",
            "/etc/mtab.tmp",
            "",
        ] {
            assert!(!is_synthetic_table(path), "{path} is an ordinary file");
        }
    }
}
