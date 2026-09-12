//! Cross-process enrollment and crash-safe epoch coordination for native VMAs.
//!
//! This is protocol infrastructure, NOT a claim that a VMA has been converted.
//! The caller must keep an enrollment alive until its native VMA is gone, and
//! must quiesce guest execution before acknowledging PARKED. No production
//! mapping currently enrolls here and FS_IOC_ENABLE_VERITY remains gated.
//!
//! A named pagefile section is kept alive by every participant. Membership
//! pages grow on demand, and each participant pins the page containing its own
//! slot. A vanished page therefore cannot contain a live participant. Entries
//! use PID + creation time + monotonically allocated ticket, not stale counts
//! or transferable LockFileEx ownership. Short state updates use a mutex;
//! epoch ownership uses a DIFFERENT mutex, whose abandonment includes thread
//! death. The latter is never silently interpreted as an abort: persistent EA
//! state must decide recovery after a possibly completed publication.
//!
//! Root/slot publication words are aligned atomics. A process dying while it
//! holds the short mutex cannot leave a half-published phase or membership.
//! Events notify workers; faulting guest threads will use separate per-epoch
//! completion events owned by the mapping-conversion layer, not these events.
//! This follows the existing VFS inode lock's Windows-session scope. Cross-
//! session namespace/security policy and fork handle registration must be
//! resolved before production admission; copying these Rust owners is invalid.

#![allow(dead_code)]

use super::NativeId;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr;
use kinakaze_vfs::{EBUSY, EINVAL, EIO, EOVERFLOW, errno_from_win32};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, GetCurrentProcessId,
    GetCurrentThread, GetCurrentThreadId, GetProcessTimes, GetThreadTimes, INFINITE, OpenEventW,
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    RegisterWaitForSingleObject, ReleaseMutex, ResetEvent, SetEvent, UnregisterWaitEx,
    WT_EXECUTEONLYONCE, WaitForMultipleObjects, WaitForSingleObject,
};

const MAGIC: u64 = 0x3150_414d_5652_5943; // "CYRVMAP1"
const PAGE: usize = 4096;
const SLOTS: usize = PAGE / core::mem::size_of::<Slot>();
const ETIMEDOUT: i32 = 110;
const ESTALE: i32 = 116;

#[repr(C)]
struct Root {
    magic: AtomicU64,
    epoch_phase: AtomicU64,
    next_ticket: AtomicU64,
    pages: AtomicU32,
    owner_pid: AtomicU32,
    owner_tid: AtomicU32,
    owner_birth: AtomicU64,
}

#[repr(C)]
struct Slot {
    // control is published last, after all immutable identity fields.
    control: AtomicU64,
    ticket: AtomicU64,
    born: AtomicU64,
    pid: AtomicU32,
    _reserved: AtomicU32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Phase {
    Idle = 0,
    Preparing = 1,
    Parked = 2,
    Publishing = 3,
    Committed = 4,
    Aborted = 5,
    Recovering = 6,
}

impl Phase {
    fn active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Parked | Self::Publishing | Self::Recovering
        )
    }
    fn decode(word: u64) -> Result<(u64, Self), i32> {
        let phase = match word & 0xff {
            0 => Self::Idle,
            1 => Self::Preparing,
            2 => Self::Parked,
            3 => Self::Publishing,
            4 => Self::Committed,
            5 => Self::Aborted,
            6 => Self::Recovering,
            _ => return Err(EIO),
        };
        Ok((word >> 8, phase))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Kind {
    Unpublished = 1,
    Native = 2,
    Parked = 3,
    Verified = 4,
}

fn slot_word(epoch: u64, kind: Kind) -> u64 {
    epoch << 8 | kind as u64
}

fn slot_state(slot: &Slot) -> Result<Option<(u64, Kind)>, i32> {
    let word = slot.control.load(Ordering::Acquire);
    let kind = match word & 0xff {
        0 if word == 0 => return Ok(None),
        1 => Kind::Unpublished,
        2 => Kind::Native,
        3 => Kind::Parked,
        4 => Kind::Verified,
        _ => return Err(EIO),
    };
    Ok(Some((word >> 8, kind)))
}

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Handle {
    fn new(raw: HANDLE) -> Result<Self, i32> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct View {
    _section: Handle,
    address: *mut c_void,
}
unsafe impl Send for View {}
unsafe impl Sync for View {}
impl View {
    fn open(name: &str) -> Result<Self, i32> {
        let name = wide(name);
        let section = Handle::new(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                PAGE as u32,
                name.as_ptr(),
            )
        })?;
        let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, PAGE) };
        if view.Value.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self {
            _section: section,
            address: view.Value,
        })
    }
    fn root(&self) -> &Root {
        unsafe { &*self.address.cast::<Root>() }
    }
    fn slot(&self, index: usize) -> &Slot {
        assert!(index < SLOTS);
        unsafe { &*self.address.cast::<Slot>().add(index) }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.address,
                },
            )
        };
    }
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}
fn event(name: &str) -> Result<Handle, i32> {
    Handle::new(unsafe { CreateEventW(ptr::null(), 0, 0, wide(name).as_ptr()) })
}
fn mutex(name: &str) -> Result<Handle, i32> {
    Handle::new(unsafe { CreateMutexW(ptr::null(), 0, wide(name).as_ptr()) })
}

/// Ownership is thread-affine even though the underlying HANDLE is shareable.
struct Held<'a> {
    handle: &'a Handle,
    _thread: PhantomData<Rc<()>>,
}
impl<'a> Held<'a> {
    fn acquire(handle: &'a Handle, timeout: u32) -> Result<Self, i32> {
        match unsafe { WaitForSingleObject(handle.0, timeout) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self {
                handle,
                _thread: PhantomData,
            }),
            WAIT_TIMEOUT => Err(ETIMEDOUT),
            _ => Err(EIO),
        }
    }
}
impl Drop for Held<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.handle.0) };
    }
}

#[derive(Clone, Copy)]
struct Identity {
    pid: u32,
    born: u64,
}
fn process_birth(process: HANDLE) -> Result<u64, i32> {
    let mut times: [FILETIME; 4] = unsafe { core::mem::zeroed() };
    if unsafe {
        GetProcessTimes(
            process,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(u64::from(times[0].dwHighDateTime) << 32 | u64::from(times[0].dwLowDateTime))
}
fn current_thread_birth() -> Result<u64, i32> {
    let mut times: [FILETIME; 4] = unsafe { core::mem::zeroed() };
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(u64::from(times[0].dwHighDateTime) << 32 | u64::from(times[0].dwLowDateTime))
}
impl Identity {
    fn current() -> Result<Self, i32> {
        Ok(Self {
            pid: unsafe { GetCurrentProcessId() },
            born: process_birth(unsafe { GetCurrentProcess() })?,
        })
    }
    /// Access-denied is an error, never evidence that a mapping owner is dead.
    fn live(self) -> Result<Option<Handle>, i32> {
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                self.pid,
            )
        };
        if raw.is_null() {
            let error = unsafe { GetLastError() };
            return if error == ERROR_INVALID_PARAMETER {
                Ok(None)
            } else {
                Err(errno_from_win32(error))
            };
        }
        let handle = Handle(raw);
        if process_birth(raw)? != self.born {
            return Ok(None);
        }
        match unsafe { WaitForSingleObject(raw, 0) } {
            WAIT_TIMEOUT => Ok(Some(handle)),
            WAIT_OBJECT_0 => Ok(None),
            _ => Err(EIO),
        }
    }
}

pub(super) struct Channel {
    name: String,
    root: View,
    state_mutex: Handle,
    epoch_mutex: Handle,
    changed: Handle,
    active: Handle,
}

impl Channel {
    pub(super) fn open(inode: NativeId) -> Result<Arc<Self>, i32> {
        let mut name = format!(r"Local\kinakaze.verity.vma.v1.{:016x}.", inode.volume);
        for byte in inode.id {
            use core::fmt::Write;
            write!(name, "{byte:02x}").map_err(|_| EIO)?;
        }
        let result = Arc::new(Self {
            state_mutex: mutex(&format!("{name}.state"))?,
            epoch_mutex: mutex(&format!("{name}.epoch"))?,
            changed: event(&format!("{name}.changed"))?,
            active: Handle::new(unsafe {
                CreateEventW(ptr::null(), 1, 0, wide(&format!("{name}.active")).as_ptr())
            })?,
            root: View::open(&format!("{name}.root"))?,
            name,
        });
        {
            let _held = Held::acquire(&result.state_mutex, INFINITE)?;
            let root = result.root.root();
            match root.magic.load(Ordering::Acquire) {
                0 => {
                    root.epoch_phase.store(0, Ordering::Relaxed);
                    root.next_ticket.store(1, Ordering::Relaxed);
                    root.pages.store(0, Ordering::Relaxed);
                    root.magic.store(MAGIC, Ordering::Release);
                }
                MAGIC => {}
                _ => return Err(EIO),
            }
        }
        Ok(result)
    }
    fn phase(&self) -> Result<(u64, Phase), i32> {
        Phase::decode(self.root.root().epoch_phase.load(Ordering::Acquire))
    }
    fn owned_by_this_thread(&self) -> Result<bool, i32> {
        let root = self.root.root();
        Ok(
            root.owner_pid.load(Ordering::Relaxed) == unsafe { GetCurrentProcessId() }
                && root.owner_tid.load(Ordering::Relaxed) == unsafe { GetCurrentThreadId() }
                && root.owner_birth.load(Ordering::Relaxed) == current_thread_birth()?,
        )
    }
    fn record_owner(&self) -> Result<(), i32> {
        let birth = current_thread_birth()?;
        let root = self.root.root();
        root.owner_pid
            .store(unsafe { GetCurrentProcessId() }, Ordering::Relaxed);
        root.owner_tid
            .store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        root.owner_birth.store(birth, Ordering::Relaxed);
        Ok(())
    }
    fn page(&self, index: u32) -> Result<View, i32> {
        View::open(&format!("{}.p{index}", self.name))
    }
    fn member_event(&self, ticket: u64) -> String {
        format!("{}.m{ticket:016x}", self.name)
    }
    fn changed(&self) {
        unsafe { SetEvent(self.changed.0) };
    }
    /// Requires state_mutex. Callback is never invoked for a dead incarnation.
    fn scan(
        &self,
        mut visitor: impl FnMut(&Slot, u64, Kind, Handle) -> Result<(), i32>,
    ) -> Result<(), i32> {
        let count = self.root.root().pages.load(Ordering::Acquire);
        for page in 0..count {
            let view = self.page(page)?;
            for index in 0..SLOTS {
                let slot = view.slot(index);
                let Some((epoch, kind)) = slot_state(slot)? else {
                    continue;
                };
                let Some(process) = (Identity {
                    pid: slot.pid.load(Ordering::Relaxed),
                    born: slot.born.load(Ordering::Relaxed),
                })
                .live()?
                else {
                    slot.control.store(0, Ordering::Release);
                    continue;
                };
                visitor(slot, epoch, kind, process)?;
            }
        }
        Ok(())
    }
    fn notify(&self) -> Result<(), i32> {
        self.changed();
        self.scan(|slot, _, _, _| {
            let raw = unsafe {
                OpenEventW(
                    EVENT_MODIFY_STATE,
                    0,
                    wide(&self.member_event(slot.ticket.load(Ordering::Relaxed))).as_ptr(),
                )
            };
            if raw.is_null() {
                let error = unsafe { GetLastError() };
                if error == ERROR_FILE_NOT_FOUND
                    && (Identity {
                        pid: slot.pid.load(Ordering::Relaxed),
                        born: slot.born.load(Ordering::Relaxed),
                    })
                    .live()?
                    .is_none()
                {
                    slot.control.store(0, Ordering::Release);
                    return Ok(());
                }
                return Err(errno_from_win32(error));
            }
            let notification = Handle(raw);
            if unsafe { SetEvent(notification.0) } == 0 {
                return Err(errno_from_win32(unsafe { GetLastError() }));
            }
            Ok(())
        })
    }
    fn set_phase(&self, epoch: u64, phase: Phase) -> Result<(), i32> {
        // A persistent wake precedes publication. Owner death halfway through
        // per-member notification must also wake previously idle workers.
        if phase.active() && unsafe { SetEvent(self.active.0) } == 0 {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        self.root
            .root()
            .epoch_phase
            .store(epoch << 8 | phase as u64, Ordering::Release);
        if !phase.active() {
            unsafe { ResetEvent(self.active.0) };
        }
        self.notify()
    }
    pub(super) fn enroll(self: &Arc<Self>) -> Result<Member, i32> {
        let identity = Identity::current()?;
        let _held = Held::acquire(&self.state_mutex, INFINITE)?;
        // Reclaim by kernel process identity before choosing a slot.
        self.scan(|_, _, _, _| Ok(()))?;
        let root = self.root.root();
        let ticket = root.next_ticket.load(Ordering::Relaxed);
        root.next_ticket
            .store(ticket.checked_add(1).ok_or(EOVERFLOW)?, Ordering::Release);
        let notice = event(&self.member_event(ticket))?;
        let (epoch, _) = self.phase()?;
        let mut page = 0;
        loop {
            let view = self.page(page)?;
            for index in 0..SLOTS {
                let slot = view.slot(index);
                if slot_state(slot)?.is_some() {
                    continue;
                }
                // No reference to unpublished fields escapes state_mutex.
                slot.ticket.store(ticket, Ordering::Relaxed);
                slot.born.store(identity.born, Ordering::Relaxed);
                slot.pid.store(identity.pid, Ordering::Relaxed);
                if page >= root.pages.load(Ordering::Relaxed) {
                    root.pages
                        .store(page.checked_add(1).ok_or(EOVERFLOW)?, Ordering::Release);
                }
                slot.control
                    .store(slot_word(epoch, Kind::Unpublished), Ordering::Release);
                self.changed();
                return Ok(Member {
                    channel: self.clone(),
                    page: view,
                    index,
                    ticket,
                    notice,
                });
            }
            page = page.checked_add(1).ok_or(EOVERFLOW)?;
        }
    }
    /// Acquiring an abandoned epoch never guesses whether durable publication
    /// happened. `is_recovery()` forces the caller to inspect the backend EA.
    pub(super) fn begin(&self, timeout: u32) -> Result<Transaction<'_>, i32> {
        let owner = Held::acquire(&self.epoch_mutex, timeout)
            .map_err(|e| if e == ETIMEDOUT { EBUSY } else { e })?;
        let _state = Held::acquire(&self.state_mutex, INFINITE)?;
        let (epoch, phase) = self.phase()?;
        if phase.active() {
            // Windows mutexes are recursive. A nested begin by the legitimate
            // owner is NOT recovery; PID/TID plus thread creation time avoids
            // confusing a later reused native thread identifier with it.
            if self.owned_by_this_thread()? {
                return Err(EBUSY);
            }
            self.record_owner()?;
            if let Err(error) = self.set_phase(epoch, Phase::Recovering) {
                self.root.root().owner_pid.store(0, Ordering::Release);
                return Err(error);
            }
            return Ok(Transaction {
                channel: self,
                _owner: owner,
                epoch,
                recovery: true,
                resolved: false,
            });
        }
        let mut unfinished = false;
        self.scan(|_, _, kind, _| {
            unfinished |= kind == Kind::Parked;
            Ok(())
        })?;
        if unfinished {
            return Err(EBUSY);
        }
        let epoch = epoch
            .checked_add(1)
            .filter(|v| *v <= u64::MAX >> 8)
            .ok_or(EOVERFLOW)?;
        self.record_owner()?;
        if let Err(error) = self.set_phase(epoch, Phase::Preparing) {
            self.root.root().owner_pid.store(0, Ordering::Release);
            return Err(error);
        }
        Ok(Transaction {
            channel: self,
            _owner: owner,
            epoch,
            recovery: false,
            resolved: false,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Notice {
    AdmitNative(u64),
    Park(u64),
    InstallVerified(u64),
    RestoreNative(u64),
    OwnerLost(u64),
}

/// A member begins UNPUBLISHED. Both fresh mmap and restored fork children must
/// inspect the persistent inode state before admission; an inherited parent
/// ticket or lock is never a valid child enrollment.
pub(super) struct Member {
    channel: Arc<Channel>,
    page: View,
    index: usize,
    ticket: u64,
    notice: Handle,
}
impl Member {
    fn state(&self) -> Result<(u64, Kind), i32> {
        let slot = self.page.slot(self.index);
        if slot.ticket.load(Ordering::Relaxed) != self.ticket {
            return Err(ESTALE);
        }
        slot_state(slot)?.ok_or(ESTALE)
    }
    fn replace(&self, epoch: u64, kind: Kind) {
        self.page
            .slot(self.index)
            .control
            .store(slot_word(epoch, kind), Ordering::Release);
        self.channel.changed();
    }
    pub(super) fn admit_native(&self, epoch: u64) -> Result<(), i32> {
        let _held = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        let (current, phase) = self.channel.phase()?;
        let (_, kind) = self.state()?;
        if current != epoch {
            return Err(ESTALE);
        }
        if !matches!(phase, Phase::Idle | Phase::Aborted) {
            return Err(EBUSY);
        }
        if !matches!(kind, Kind::Unpublished | Kind::Parked) {
            return Err(EINVAL);
        }
        self.replace(epoch, Kind::Native);
        Ok(())
    }
    /// Contract: all native access has been quiesced, not just mprotected, and
    /// all rollback/COW state remains retained until this epoch is resolved.
    pub(super) fn acknowledge_parked(&self, epoch: u64) -> Result<(), i32> {
        let _held = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        let (current, phase) = self.channel.phase()?;
        let (_, kind) = self.state()?;
        if current != epoch {
            return Err(ESTALE);
        }
        if phase != Phase::Preparing || kind != Kind::Native {
            return Err(EINVAL);
        }
        self.replace(epoch, Kind::Parked);
        Ok(())
    }
    pub(super) fn admit_verified(&self, epoch: u64) -> Result<(), i32> {
        let _held = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        let (current, phase) = self.channel.phase()?;
        let (_, kind) = self.state()?;
        if current != epoch {
            return Err(ESTALE);
        }
        if phase != Phase::Committed || !matches!(kind, Kind::Parked | Kind::Unpublished) {
            return Err(EINVAL);
        }
        self.replace(epoch, Kind::Verified);
        Ok(())
    }
    /// Worker wait, not the guest fault handler. No state/mapping mutex is held
    /// while sleeping. A coordinator's thread/process death wakes via its
    /// abandoned epoch mutex, without timeout polling or PID-only guesses.
    pub(super) fn wait(&self, timeout: u32) -> Result<Notice, i32> {
        let deadline = Deadline::new(timeout);
        loop {
            let (epoch, wait_owner, self_owned) = {
                let _held = Held::acquire(&self.channel.state_mutex, INFINITE)?;
                let (epoch, phase) = self.channel.phase()?;
                let (_, kind) = self.state()?;
                let notice = match (kind, phase) {
                    (Kind::Unpublished, Phase::Idle | Phase::Aborted) => {
                        Some(Notice::AdmitNative(epoch))
                    }
                    (Kind::Native, Phase::Preparing) => Some(Notice::Park(epoch)),
                    (Kind::Parked | Kind::Unpublished, Phase::Committed) => {
                        Some(Notice::InstallVerified(epoch))
                    }
                    (Kind::Parked, Phase::Aborted) => Some(Notice::RestoreNative(epoch)),
                    _ => None,
                };
                if let Some(notice) = notice {
                    return Ok(notice);
                }
                let wake =
                    unsafe { WaitForSingleObject(self.channel.active.0, 0) } == WAIT_OBJECT_0;
                (
                    epoch,
                    phase.active() || wake,
                    phase.active() && self.channel.owned_by_this_thread()?,
                )
            };
            let handles = [
                self.notice.0,
                if wait_owner {
                    self.channel.epoch_mutex.0
                } else {
                    self.channel.active.0
                },
            ];
            let waited = unsafe {
                WaitForMultipleObjects(
                    if self_owned { 1 } else { 2 },
                    handles.as_ptr(),
                    0,
                    deadline.remaining(),
                )
            };
            match waited {
                WAIT_OBJECT_0 => {}
                value if !self_owned && !wait_owner && value == WAIT_OBJECT_0 + 1 => {}
                value
                    if !self_owned
                        && wait_owner
                        && (value == WAIT_OBJECT_0 + 1 || value == WAIT_ABANDONED + 1) =>
                {
                    // We own the epoch mutex now; acquire state in the same
                    // order as a coordinator. A completed epoch is rechecked
                    // before reporting owner death.
                    let acquired = Held {
                        handle: &self.channel.epoch_mutex,
                        _thread: PhantomData,
                    };
                    let lost = {
                        let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
                        let (now, phase) = self.channel.phase()?;
                        if !phase.active() {
                            // Terminal publication may have preceded a crash
                            // before ResetEvent. We own the epoch mutex, so a
                            // new transaction cannot race this stale-wake reset.
                            unsafe { ResetEvent(self.channel.active.0) };
                        }
                        now == epoch && phase.active()
                    };
                    drop(acquired);
                    if lost {
                        return Ok(Notice::OwnerLost(epoch));
                    }
                }
                WAIT_TIMEOUT => return Err(ETIMEDOUT),
                _ => return Err(EIO),
            }
        }
    }
}
impl Drop for Member {
    fn drop(&mut self) {
        if let Ok(_held) = Held::acquire(&self.channel.state_mutex, INFINITE)
            && self.page.slot(self.index).ticket.load(Ordering::Relaxed) == self.ticket
        {
            self.page
                .slot(self.index)
                .control
                .store(0, Ordering::Release);
            self.channel.changed();
        }
    }
}

struct Deadline {
    start: Instant,
    milliseconds: u32,
}
impl Deadline {
    fn new(milliseconds: u32) -> Self {
        Self {
            start: Instant::now(),
            milliseconds,
        }
    }
    fn remaining(&self) -> u32 {
        if self.milliseconds == INFINITE {
            return INFINITE;
        }
        self.milliseconds
            .saturating_sub(self.start.elapsed().as_millis().min(u32::MAX as u128) as u32)
    }
}

/// One transient registered wait per live blocking process removes the Windows
/// 64-wait-handle limit. Callback only signals an owned event. Unregistration
/// drains callbacks before either the process or event handle can be closed.
struct DeathWatch {
    registration: HANDLE,
    _process: Handle,
}
unsafe extern "system" fn owner_exited(context: *mut c_void, _: bool) {
    unsafe { SetEvent(context) };
}
impl DeathWatch {
    fn new(process: Handle, changed: HANDLE) -> Result<Self, i32> {
        let mut registration = ptr::null_mut();
        if unsafe {
            RegisterWaitForSingleObject(
                &mut registration,
                process.0,
                Some(owner_exited),
                changed,
                INFINITE,
                WT_EXECUTEONLYONCE,
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self {
            registration,
            _process: process,
        })
    }
}
impl Drop for DeathWatch {
    fn drop(&mut self) {
        unsafe { UnregisterWaitEx(self.registration, INVALID_HANDLE_VALUE) };
    }
}

pub(super) struct Transaction<'a> {
    channel: &'a Channel,
    _owner: Held<'a>,
    epoch: u64,
    recovery: bool,
    resolved: bool,
}
impl Transaction<'_> {
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(super) fn is_recovery(&self) -> bool {
        self.recovery
    }
    pub(super) fn wait_all_parked(&self, timeout: u32) -> Result<(), i32> {
        if self.recovery {
            return Err(EINVAL);
        }
        let deadline = Deadline::new(timeout);
        loop {
            let mut watches = Vec::new();
            {
                let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
                if self.channel.phase()? != (self.epoch, Phase::Preparing) {
                    return Err(ESTALE);
                }
                self.channel.scan(|_, _, kind, process| {
                    if kind == Kind::Native {
                        watches.push(DeathWatch::new(process, self.channel.changed.0)?);
                    }
                    Ok(())
                })?;
                if watches.is_empty() {
                    return self.channel.set_phase(self.epoch, Phase::Parked);
                }
            }
            match unsafe { WaitForSingleObject(self.channel.changed.0, deadline.remaining()) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Err(ETIMEDOUT),
                _ => return Err(EIO),
            }
            drop(watches);
        }
    }
    pub(super) fn publishing(&self) -> Result<(), i32> {
        let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        if self.channel.phase()? != (self.epoch, Phase::Parked) {
            return Err(EINVAL);
        }
        self.channel.set_phase(self.epoch, Phase::Publishing)
    }
    /// Call ONLY after durable VFS commit. An interrupted publication must go
    /// through resolve_recovery after reading the authoritative inode record.
    pub(super) fn committed(mut self) -> Result<(), i32> {
        let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        if self.channel.phase()? != (self.epoch, Phase::Publishing) {
            return Err(EINVAL);
        }
        self.channel.set_phase(self.epoch, Phase::Committed)?;
        self.resolved = true;
        Ok(())
    }
    pub(super) fn abort(mut self) -> Result<(), i32> {
        let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        if !matches!(self.channel.phase()?, (epoch, Phase::Preparing | Phase::Parked) if epoch == self.epoch)
        {
            return Err(EINVAL);
        }
        self.channel.set_phase(self.epoch, Phase::Aborted)?;
        self.resolved = true;
        Ok(())
    }
    pub(super) fn resolve_recovery(mut self, committed: bool) -> Result<(), i32> {
        if !self.recovery {
            return Err(EINVAL);
        }
        let _state = Held::acquire(&self.channel.state_mutex, INFINITE)?;
        if self.channel.phase()? != (self.epoch, Phase::Recovering) {
            return Err(ESTALE);
        }
        if committed {
            let mut bypass = false;
            self.channel.scan(|_, _, kind, _| {
                bypass |= kind == Kind::Native;
                Ok(())
            })?;
            if bypass {
                return Err(EIO);
            }
        }
        self.channel.set_phase(
            self.epoch,
            if committed {
                Phase::Committed
            } else {
                Phase::Aborted
            },
        )?;
        self.resolved = true;
        Ok(())
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if let Ok(_state) = Held::acquire(&self.channel.state_mutex, INFINITE)
            && let Ok((epoch, phase)) = self.channel.phase()
            && epoch == self.epoch
        {
            // A graceful unwind can roll back before publication. Once the
            // backend might have committed, leave an explicit recovery state.
            if !self.resolved && phase.active() {
                let next = if matches!(phase, Phase::Preparing | Phase::Parked) {
                    Phase::Aborted
                } else {
                    Phase::Recovering
                };
                let _ = self.channel.set_phase(epoch, next);
            }
            // A gracefully released unresolved owner must not be confused
            // with recursive acquisition when this SAME thread recovers.
            self.channel
                .root
                .root()
                .owner_pid
                .store(0, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Output, Stdio};
    use std::time::Duration;

    struct Fixture {
        path: PathBuf,
        guest: String,
        fd: i32,
        channel: Arc<Channel>,
    }
    impl Fixture {
        fn new() -> Self {
            let mut nonce = [0u8; 16];
            crate::fdio::random_bytes(&mut nonce).unwrap();
            let path = std::env::temp_dir().join(format!(
                "kinakaze-verity-coord-{}-{:032x}",
                std::process::id(),
                u128::from_le_bytes(nonce)
            ));
            let mut created = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            created.write_all(&[0x69; 8192]).unwrap();
            drop(created);
            let guest = crate::fdio::windows_to_linux(&path);
            let fd = kinakaze_vfs::fs::open(&guest, 0, 0).unwrap();
            let id = super::super::native_id(kinakaze_vfs::get(fd).unwrap().raw as HANDLE).unwrap();
            Self {
                path,
                guest,
                fd,
                channel: Channel::open(id).unwrap(),
            }
        }
        fn helper(&self, mode: &str) -> Helper {
            let ready = event(&format!("{}.test-ready-{mode}", self.channel.name)).unwrap();
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fdio::verity::coordination::tests::verity_coordination_process_helper",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("KINAKAZE_VERITY_COORD_FILE", &self.guest)
                .env("KINAKAZE_VERITY_COORD_MODE", mode)
                .creation_flags(0x0800_0000)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            Helper {
                child: Some(child),
                ready,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = kinakaze_vfs::close(self.fd);
            let _ = std::fs::remove_file(&self.path);
        }
    }
    struct Helper {
        child: Option<Child>,
        ready: Handle,
    }
    impl Helper {
        fn finish(mut self, expected: i32) -> Output {
            let child = self.child.as_mut().unwrap();
            let waited = unsafe { WaitForSingleObject(child.as_raw_handle(), 5_000) };
            assert_eq!(waited, WAIT_OBJECT_0, "native helper exceeded bounded wait");
            let output = self.child.take().unwrap().wait_with_output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }
        fn ready(&self) {
            assert_eq!(
                unsafe { WaitForSingleObject(self.ready.0, 3_000) },
                WAIT_OBJECT_0,
                "helper did not complete admission check"
            );
        }
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    fn wait_members(channel: &Channel, count: usize) {
        let deadline = Deadline::new(3_000);
        loop {
            {
                let _state = Held::acquire(&channel.state_mutex, INFINITE).unwrap();
                let mut found = 0;
                channel
                    .scan(|_, _, _, _| {
                        found += 1;
                        Ok(())
                    })
                    .unwrap();
                if found == count {
                    return;
                }
            }
            assert_eq!(
                unsafe { WaitForSingleObject(channel.changed.0, deadline.remaining()) },
                WAIT_OBJECT_0,
                "membership did not reach {count}"
            );
        }
    }
    fn enabled(fd: i32) -> bool {
        kinakaze_vfs::fs::verity::descriptor(kinakaze_vfs::get(fd).unwrap().raw as HANDLE)
            .unwrap()
            .is_some()
    }

    #[test]
    fn verity_coordination_process_helper() {
        let Ok(path) = std::env::var("KINAKAZE_VERITY_COORD_FILE") else {
            return;
        };
        let mode = std::env::var("KINAKAZE_VERITY_COORD_MODE").unwrap();
        let fd = kinakaze_vfs::fs::open(&path, 0, 0).unwrap();
        let channel = Channel::open(
            super::super::native_id(kinakaze_vfs::get(fd).unwrap().raw as HANDLE).unwrap(),
        )
        .unwrap();
        if mode.starts_with("allocation-") {
            let _held = Held::acquire(&channel.state_mutex, INFINITE).unwrap();
            let root = channel.root.root();
            let page = root.pages.load(Ordering::Acquire);
            let view = channel.page(page).unwrap();
            let ticket = root.next_ticket.fetch_add(1, Ordering::AcqRel);
            let identity = Identity::current().unwrap();
            view.slot(0).ticket.store(ticket, Ordering::Relaxed);
            view.slot(0).pid.store(identity.pid, Ordering::Relaxed);
            view.slot(0).born.store(identity.born, Ordering::Relaxed);
            root.pages.store(page + 1, Ordering::Release);
            if mode == "allocation-published-crash" {
                view.slot(0)
                    .control
                    .store(slot_word(0, Kind::Unpublished), Ordering::Release);
            }
            // Deliberately abandon the short mutex with the next segment
            // partially initialized, without Rust destructors running.
            std::process::exit(23);
        }
        if mode.starts_with("owner-") {
            if mode == "owner-unnotified" {
                let _owner = Held::acquire(&channel.epoch_mutex, 1_000).unwrap();
                let _state = Held::acquire(&channel.state_mutex, 1_000).unwrap();
                channel.record_owner().unwrap();
                unsafe { SetEvent(channel.active.0) };
                channel
                    .root
                    .root()
                    .epoch_phase
                    .store(1 << 8 | Phase::Preparing as u64, Ordering::Release);
                // No per-member event was signalled before abrupt exit.
                std::process::exit(23);
            }
            let transaction = channel.begin(1_000).unwrap();
            transaction.wait_all_parked(3_000).unwrap();
            if mode == "owner-after" {
                transaction.publishing().unwrap();
                kinakaze_vfs::fs::verity::enable(fd, 1, 4096, &[]).unwrap();
                assert!(enabled(fd));
            }
            // before: no EA was published. after: the durable EA committed,
            // but root still says PUBLISHING. Both abandon the owner mutex.
            std::process::exit(23);
        }
        let member = channel.enroll().unwrap();
        if mode == "late" {
            let (epoch, phase) = channel.phase().unwrap();
            assert!(phase.active());
            assert_eq!(member.admit_native(epoch), Err(EBUSY));
            let ready = event(&format!("{}.test-ready-{mode}", channel.name)).unwrap();
            unsafe { SetEvent(ready.0) };
        } else {
            let Notice::AdmitNative(epoch) = member.wait(3_000).unwrap() else {
                panic!("fresh native admission missing")
            };
            member.admit_native(epoch).unwrap();
            let ready = event(&format!("{}.test-ready-{mode}", channel.name)).unwrap();
            unsafe { SetEvent(ready.0) };
            if mode == "dying" {
                let never = event(&format!("{}.never", channel.name)).unwrap();
                assert_eq!(
                    unsafe { WaitForSingleObject(never.0, 4_000) },
                    WAIT_OBJECT_0,
                    "parent did not terminate owned dying helper"
                );
                unreachable!();
            }
            let Notice::Park(epoch) = member.wait(3_000).unwrap() else {
                panic!("park request missing")
            };
            assert_eq!(member.acknowledge_parked(epoch - 1), Err(ESTALE));
            member.acknowledge_parked(epoch).unwrap();
        }
        let Notice::InstallVerified(epoch) = member.wait(3_000).unwrap() else {
            panic!("commit notice missing")
        };
        assert!(enabled(fd), "admission cannot precede persistent commit");
        member.admit_verified(epoch).unwrap();
        println!("COORDINATION_MEMBER_PASS={mode}");
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn verity_coordination_crossprocess_enrollment_epoch_and_late_child() {
        let fixture = Fixture::new();
        let member = fixture.channel.enroll().unwrap();
        member.admit_native(0).unwrap();
        let child = fixture.helper("participant");
        child.ready();
        wait_members(&fixture.channel, 2);
        // Enrollment is initially inhibited; wait until the helper actually
        // admits its native mapping before starting this test transaction.
        loop {
            let _state = Held::acquire(&fixture.channel.state_mutex, INFINITE).unwrap();
            let mut native = 0;
            fixture
                .channel
                .scan(|_, _, kind, _| {
                    native += usize::from(kind == Kind::Native);
                    Ok(())
                })
                .unwrap();
            if native == 2 {
                break;
            }
            drop(_state);
            assert_eq!(
                unsafe { WaitForSingleObject(fixture.channel.changed.0, 3_000) },
                WAIT_OBJECT_0
            );
        }
        let transaction = fixture.channel.begin(1_000).unwrap();
        let epoch = transaction.epoch();
        assert!(!transaction.is_recovery());
        assert!(
            matches!(fixture.channel.begin(0), Err(EBUSY)),
            "recursive mutex ownership is not recovery"
        );
        assert_eq!(member.acknowledge_parked(epoch - 1), Err(ESTALE));
        member.acknowledge_parked(epoch).unwrap();
        transaction.wait_all_parked(3_000).unwrap();
        let late = fixture.helper("late");
        late.ready();
        wait_members(&fixture.channel, 3);
        assert!(!enabled(fixture.fd));
        transaction.publishing().unwrap();
        kinakaze_vfs::fs::verity::enable(fixture.fd, 1, 4096, &[]).unwrap();
        transaction.committed().unwrap();
        assert_eq!(member.wait(1_000), Ok(Notice::InstallVerified(epoch)));
        member.admit_verified(epoch).unwrap();
        assert!(
            String::from_utf8_lossy(&child.finish(0).stdout)
                .contains("COORDINATION_MEMBER_PASS=participant")
        );
        assert!(
            String::from_utf8_lossy(&late.finish(0).stdout)
                .contains("COORDINATION_MEMBER_PASS=late")
        );
    }

    #[test]
    fn verity_coordination_owner_death_recovers_durable_commit_boundary() {
        for mode in ["owner-before", "owner-after", "owner-unnotified"] {
            let fixture = Fixture::new();
            let member = fixture.channel.enroll().unwrap();
            member.admit_native(0).unwrap();
            let child = fixture.helper(mode);
            let Notice::Park(epoch) = member.wait(3_000).unwrap() else {
                panic!("missing park request")
            };
            member.acknowledge_parked(epoch).unwrap();
            let start = Instant::now();
            assert_eq!(member.wait(3_000), Ok(Notice::OwnerLost(epoch)));
            assert!(start.elapsed() < Duration::from_secs(2));
            // A waiter observing owner death has NOT made the VMA readable.
            assert_eq!(member.admit_native(epoch), Err(EBUSY));
            let transaction = fixture.channel.begin(1_000).unwrap();
            assert!(transaction.is_recovery());
            let committed = enabled(fixture.fd);
            assert_eq!(committed, mode == "owner-after");
            transaction.resolve_recovery(committed).unwrap();
            if committed {
                assert_eq!(member.wait(1_000), Ok(Notice::InstallVerified(epoch)));
                member.admit_verified(epoch).unwrap();
            } else {
                assert_eq!(member.wait(1_000), Ok(Notice::RestoreNative(epoch)));
                member.admit_native(epoch).unwrap();
            }
            child.finish(23);
        }
    }

    #[test]
    fn verity_coordination_dead_participant_wakes_barrier_without_timeout_polling() {
        let fixture = Fixture::new();
        let mut child = fixture.helper("dying");
        child.ready();
        wait_members(&fixture.channel, 1);
        loop {
            let _state = Held::acquire(&fixture.channel.state_mutex, INFINITE).unwrap();
            let mut native = false;
            fixture
                .channel
                .scan(|_, _, kind, _| {
                    native |= kind == Kind::Native;
                    Ok(())
                })
                .unwrap();
            if native {
                break;
            }
            drop(_state);
            assert_eq!(
                unsafe { WaitForSingleObject(fixture.channel.changed.0, 3_000) },
                WAIT_OBJECT_0
            );
        }
        let process = child.child.take().unwrap();
        let killer = std::thread::spawn(move || {
            let mut process = process;
            std::thread::sleep(Duration::from_millis(100));
            process.kill().unwrap();
            process.wait_with_output().unwrap()
        });
        let transaction = fixture.channel.begin(1_000).unwrap();
        let started = Instant::now();
        transaction.wait_all_parked(3_000).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        transaction.abort().unwrap();
        assert!(!killer.join().unwrap().status.success());
    }

    #[test]
    fn verity_coordination_growing_segment_owner_death_and_incarnation_aba() {
        for mode in ["allocation-unpublished-crash", "allocation-published-crash"] {
            let fixture = Fixture::new();
            let mut pins = Vec::new();
            for _ in 0..SLOTS {
                pins.push(fixture.channel.enroll().unwrap());
            }
            assert_eq!(fixture.channel.root.root().pages.load(Ordering::Acquire), 1);
            // Pin the next segment to prove recovery sees actual partial data,
            // not just a newly recreated zero-filled kernel section.
            let next = fixture.channel.page(1).unwrap();
            fixture.helper(mode).finish(23);
            assert_eq!(fixture.channel.root.root().pages.load(Ordering::Acquire), 2);
            let abandoned_ticket = next.slot(0).ticket.load(Ordering::Relaxed);
            assert!(abandoned_ticket > 0);
            let recovered = fixture.channel.enroll().unwrap();
            assert_ne!(recovered.ticket, abandoned_ticket);
            assert_eq!(recovered.index, 0);
            assert_eq!(fixture.channel.root.root().pages.load(Ordering::Acquire), 2);
            // Simulate PID reuse with a different creation time. A live PID
            // alone must not preserve the old slot; its old ticket cannot
            // acknowledge or delete the newly enrolled incarnation.
            {
                let _state = Held::acquire(&fixture.channel.state_mutex, INFINITE).unwrap();
                let slot = recovered.page.slot(recovered.index);
                slot.born.fetch_add(1, Ordering::Relaxed);
            }
            let replacement = fixture.channel.enroll().unwrap();
            assert_ne!(replacement.ticket, recovered.ticket);
            assert_eq!(recovered.admit_native(0), Err(ESTALE));
            drop(recovered);
            replacement.admit_native(0).unwrap();
            assert_eq!(
                Identity {
                    pid: unsafe { GetCurrentProcessId() },
                    born: 0
                }
                .live()
                .unwrap()
                .is_none(),
                true
            );
            drop(pins);
        }
    }

    #[test]
    fn verity_coordination_graceful_publication_unwind_is_not_recursive_ownership() {
        let fixture = Fixture::new();
        let member = fixture.channel.enroll().unwrap();
        member.admit_native(0).unwrap();
        let first = fixture.channel.begin(0).unwrap();
        let epoch = first.epoch();
        member.acknowledge_parked(epoch).unwrap();
        first.wait_all_parked(1_000).unwrap();
        first.publishing().unwrap();
        drop(first);
        assert!(!enabled(fixture.fd));
        assert_eq!(member.wait(1_000), Ok(Notice::OwnerLost(epoch)));
        let recovery = fixture.channel.begin(0).unwrap();
        assert!(recovery.is_recovery());
        recovery.resolve_recovery(false).unwrap();
        member.admit_native(epoch).unwrap();
        let second = fixture.channel.begin(0).unwrap();
        assert!(second.epoch() > epoch);
        assert_eq!(member.acknowledge_parked(epoch), Err(ESTALE));
        member.acknowledge_parked(second.epoch()).unwrap();
        second.wait_all_parked(1_000).unwrap();
        let second_epoch = second.epoch();
        second.abort().unwrap();
        member.admit_native(second_epoch).unwrap();
    }

    #[test]
    fn verity_coordination_native_cow_working_set_evidence() {
        use windows_sys::Win32::System::Memory::{
            FILE_MAP_COPY, FILE_MAP_READ, MEM_MAPPED, MEMORY_BASIC_INFORMATION,
            MEMORY_MAPPED_VIEW_ADDRESS, PAGE_NOACCESS, PAGE_WRITECOPY, VirtualLock, VirtualProtect,
            VirtualQuery, VirtualUnlock,
        };
        use windows_sys::Win32::System::ProcessStatus::{
            K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION,
        };
        struct NativeView {
            address: MEMORY_MAPPED_VIEW_ADDRESS,
            locked: bool,
        }
        impl Drop for NativeView {
            fn drop(&mut self) {
                if self.locked {
                    unsafe { VirtualUnlock(self.address.Value, 8192) };
                }
                unsafe { UnmapViewOfFile(self.address) };
            }
        }
        fn query_raw(address: *mut c_void) -> usize {
            let mut info: PSAPI_WORKING_SET_EX_INFORMATION = unsafe { core::mem::zeroed() };
            info.VirtualAddress = address;
            assert_ne!(
                unsafe {
                    K32QueryWorkingSetEx(
                        GetCurrentProcess(),
                        (&raw mut info).cast(),
                        core::mem::size_of_val(&info) as u32,
                    )
                },
                0
            );
            unsafe { info.VirtualAttributes.Flags }
        }
        fn query(address: *mut c_void) -> usize {
            let flags = query_raw(address);
            assert_ne!(
                flags & 1,
                0,
                "only resident Valid pages have meaningful COW state"
            );
            flags
        }
        let fixture = Fixture::new();
        let file = kinakaze_vfs::get(fixture.fd).unwrap().raw as HANDLE;
        let section = Handle::new(unsafe {
            CreateFileMappingW(file, ptr::null(), PAGE_WRITECOPY, 0, 0, ptr::null())
        })
        .unwrap();
        let mut private = NativeView {
            address: unsafe { MapViewOfFile(section.0, FILE_MAP_COPY, 0, 0, 8192) },
            locked: false,
        };
        let mut observer = NativeView {
            address: unsafe { MapViewOfFile(section.0, FILE_MAP_READ, 0, 0, 8192) },
            locked: false,
        };
        assert!(!private.address.Value.is_null() && !observer.address.Value.is_null());
        assert_ne!(unsafe { VirtualLock(private.address.Value, 8192) }, 0);
        private.locked = true;
        assert_ne!(unsafe { VirtualLock(observer.address.Value, 8192) }, 0);
        observer.locked = true;
        const SHARED: usize = 1 << 15;
        let untouched = query(private.address.Value);
        assert_ne!(untouched & SHARED, 0);
        unsafe { private.address.Value.cast::<u8>().write_volatile(0x31) };
        let dirty = query(private.address.Value);
        assert_eq!(dirty & SHARED, 0);
        assert_ne!(
            query(unsafe { private.address.Value.add(4096) }) & SHARED,
            0
        );
        assert_ne!(query(observer.address.Value) & SHARED, 0);
        assert_eq!(
            unsafe { observer.address.Value.cast::<u8>().read_volatile() },
            0x69
        );
        // Bytes equal to the file are still privately dirtied; comparing data
        // against the file is NOT a substitute for COW metadata.
        unsafe { private.address.Value.cast::<u8>().write_volatile(0x69) };
        assert_eq!(query(private.address.Value) & SHARED, 0);
        let mut previous = 0;
        assert_ne!(
            unsafe { VirtualProtect(private.address.Value, 4096, PAGE_NOACCESS, &mut previous) },
            0
        );
        let inaccessible = query_raw(private.address.Value);
        // On this host NOACCESS invalidates the working-set answer despite
        // VirtualLock. Never classify COW from Shared without Valid; the real
        // converter must sample after quiescence but BEFORE revoking access.
        if inaccessible & 1 != 0 {
            assert_eq!(inaccessible & SHARED, 0);
        }
        let mut memory: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        assert_ne!(
            unsafe {
                VirtualQuery(
                    private.address.Value,
                    &mut memory,
                    core::mem::size_of_val(&memory),
                )
            },
            0
        );
        assert_eq!(memory.Type, MEM_MAPPED);
        let mut ignored = 0;
        assert_ne!(
            // A dirtied COW page's effective previous protection can be
            // READWRITE, which the original readonly file section cannot grant
            // anew. Restore the VMA's WRITECOPY contract, not that PTE value.
            unsafe { VirtualProtect(private.address.Value, 4096, PAGE_WRITECOPY, &mut ignored) },
            0,
            "restore COW protection failed {} (effective prior {previous:#x})",
            unsafe { GetLastError() }
        );
        assert_eq!(
            unsafe { private.address.Value.cast::<u8>().read_volatile() },
            0x69
        );
        assert_eq!(query(private.address.Value) & SHARED, 0);
        assert_eq!(std::fs::read(&fixture.path).unwrap(), [0x69; 8192]);
        println!(
            "COW_PROBE clean_flags={untouched:#x} dirty_flags={dirty:#x} noaccess_flags={inaccessible:#x}: equal-byte rewrite stays private; NOACCESS must check Valid; restored access retains private identity"
        );
    }
}
