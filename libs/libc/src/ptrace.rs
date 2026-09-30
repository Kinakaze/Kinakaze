//! Linux tracing using Windows debug events and explicit guest syscall frames.
//!
//! Each attachment has a native debugger thread. Attach, wait, continue and
//! detach all execute on that thread. Running targets block in the native event
//! wait; stopped targets block on the command channel. Neither path polls.
//! A shared control page enables syscall stops without patching guest code or
//! single-stepping the host runtime. Untraced calls only check BeingDebugged.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

use kinakaze_vfs::{EFAULT, EINVAL, EIO, EPERM, ESRCH};
use windows_sys::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::Debug::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Threading::*;

const SYSCALL_EXCEPTION: u32 = 0xe04b_5359;
const SIGNAL_EXCEPTION: u32 = 0xe04b_5347;
const MAGIC: u32 = 0x4b54_5231;
const TRACESYSGOOD: u32 = 1;
const EXITKILL: u32 = 0x0010_0000;
// Event options are deliberately rejected until the exec/fork transactions can
// transfer an attachment to the replacement native owner.
const SUPPORTED_OPTIONS: u32 = TRACESYSGOOD | EXITKILL;
const MODE_CONT: u32 = 0;
const MODE_SYSCALL: u32 = 1;
const MODE_STEP_SYSCALL: u32 = 2;
const TRACEME: u32 = 2;

mod fast;
pub(crate) use fast::debugger_present;

thread_local! { static RAW_DEPTH: Cell<u32> = const { Cell::new(0) }; }

pub(crate) struct RawDispatch(Option<u32>);
impl RawDispatch {
    #[inline]
    pub(crate) fn enter() -> Self {
        if !debugger_present() {
            return Self(None);
        }
        Self(Some(RAW_DEPTH.with(|depth| {
            let previous = depth.get();
            depth.set(previous + 1);
            previous
        })))
    }
}
impl Drop for RawDispatch {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            RAW_DEPTH.with(|depth| depth.set(previous));
        }
    }
}

/// Accelerated libc calls retain their logical syscall identity. Calls made
/// from inside the raw dispatcher bypass this redirection, preventing both
/// recursion and duplicate stops for one Linux operation.
#[inline]
pub(crate) fn fast_syscall(number: i64, arguments: [u64; 6]) -> Option<i64> {
    if !debugger_present() || RAW_DEPTH.with(|depth| depth.get() != 0) {
        return None;
    }
    let control = tracee_control()?;
    if unsafe { &*(control as *const Control) }
        .mode
        .load(Ordering::Acquire)
        == MODE_CONT
    {
        return None;
    }
    let raw = unsafe {
        crate::sysadmin::kinakaze_abi_syscall_raw(
            number,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
            arguments[5],
        )
    };
    Some(if (-4095..0).contains(&raw) {
        crate::set_errno(-raw as i32);
        -1
    } else {
        raw
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Registers {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub eflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub ds: u64,
    pub es: u64,
    pub fs: u64,
    pub gs: u64,
}

#[repr(C)]
struct Control {
    magic: AtomicU32,
    tracer: AtomicU32,
    attached: AtomicU32,
    mode: AtomicU32,
    options: AtomicU32,
    injected: AtomicU32,
}

struct Mapping {
    handle: usize,
    view: usize,
}
impl Mapping {
    fn name(host: u32) -> Vec<u16> {
        format!("Local\\kinakaze.ptrace.{host}")
            .encode_utf16()
            .chain(Some(0))
            .collect()
    }
    fn open(host: u32, create: bool) -> Result<Self, i32> {
        let name = Self::name(host);
        let handle = unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    core::ptr::null(),
                    PAGE_READWRITE,
                    0,
                    4096,
                    name.as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr())
            }
        };
        if handle.is_null() {
            return Err(ESRCH);
        }
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
        if view.Value.is_null() {
            unsafe {
                CloseHandle(handle);
            }
            return Err(EIO);
        }
        Ok(Self {
            handle: handle as usize,
            view: view.Value as usize,
        })
    }
    fn control(&self) -> &Control {
        unsafe { &*(self.view as *const Control) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as _,
            });
            CloseHandle(self.handle as HANDLE);
        }
    }
}

fn copy_memory(process: HANDLE, address: usize, bytes: &mut [u8], write: bool) -> Result<(), i32> {
    let mut copied = 0;
    let ok = unsafe {
        if write && process == GetCurrentProcess() {
            // Local output pointers obey normal guest page permissions. The
            // debugger write primitive can bypass protection and is reserved
            // for writes into the stopped target, such as text breakpoints.
            ReadProcessMemory(
                process,
                bytes.as_ptr().cast(),
                address as _,
                bytes.len(),
                &mut copied,
            )
        } else if write {
            WriteProcessMemory(
                process,
                address as _,
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut copied,
            )
        } else {
            ReadProcessMemory(
                process,
                address as _,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &mut copied,
            )
        }
    };
    if ok == 0 || copied != bytes.len() {
        Err(EFAULT)
    } else {
        Ok(())
    }
}

pub(crate) fn read_value<T: Copy>(address: usize) -> Result<T, i32> {
    let mut result = core::mem::MaybeUninit::<T>::uninit();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(result.as_mut_ptr().cast(), core::mem::size_of::<T>())
    };
    copy_memory(unsafe { GetCurrentProcess() }, address, bytes, false)?;
    Ok(unsafe { result.assume_init() })
}
pub(crate) fn write_value<T: Copy>(address: usize, value: T) -> Result<(), i32> {
    let mut value = value;
    let bytes = unsafe {
        core::slice::from_raw_parts_mut((&mut value as *mut T).cast(), core::mem::size_of::<T>())
    };
    copy_memory(unsafe { GetCurrentProcess() }, address, bytes, true)
}

type Answer = mpsc::SyncSender<Result<i64, i32>>;
struct Command {
    request: i32,
    address: usize,
    data: usize,
    answer: Answer,
}
struct Session {
    host: u32,
    process: usize,
    mapping: Mapping,
    commands: mpsc::Sender<Command>,
    stopped: AtomicU32,
    interrupted: AtomicU32,
    killed: AtomicU32,
    seized: bool,
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process as HANDLE);
        }
    }
}
#[derive(Clone, Copy)]
struct Report {
    pid: i32,
    status: i32,
}
#[derive(Default)]
struct State {
    owner: u32,
    sessions: BTreeMap<i32, Arc<Session>>,
    reports: VecDeque<Report>,
}
fn state() -> &'static (Mutex<State>, Condvar) {
    static STATE: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
    STATE.get_or_init(|| (Mutex::new(State::default()), Condvar::new()))
}
fn lock_state() -> std::sync::MutexGuard<'static, State> {
    let mut state = state()
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = std::process::id();
    if state.owner != host {
        state.sessions.clear();
        state.reports.clear();
        state.owner = host;
    }
    state
}
fn report(pid: i32, status: i32) {
    let mut guard = lock_state();
    // Resuming without collecting a stop must not retain an ever-growing
    // history. Linux exposes a target's current waitable state.
    guard.reports.retain(|event| event.pid != pid);
    guard.reports.push_back(Report { pid, status });
    drop(guard);
    state().1.notify_all();
}

fn target(pid: i32) -> Result<(u32, u64), i32> {
    if pid <= 0 || pid as u32 == kinakaze_vfs::job::process_id() {
        return Err(EPERM);
    }
    if kinakaze_runtime::authority::get().is_some() {
        return kinakaze_runtime::authority::memory_target(pid as u32, true);
    }
    let host = kinakaze_vfs::job::host_pid(pid as u32).ok_or(ESRCH)?;
    let mut ancestor = pid as u32;
    let entries = kinakaze_vfs::job::processes();
    for _ in 0..=entries.len() {
        let entry = entries
            .iter()
            .find(|entry| entry.namespace_pid == ancestor)
            .ok_or(EPERM)?;
        if entry.ppid == kinakaze_vfs::job::process_id() {
            return Ok((host, 0));
        }
        ancestor = entry.ppid;
    }
    Err(EPERM)
}

fn attach(pid: i32, host: u32, birth: u64, seized: bool, options: u32) -> Result<i64, i32> {
    if options & !SUPPORTED_OPTIONS != 0 {
        return Err(EINVAL);
    }
    let mut state = lock_state();
    if state.sessions.contains_key(&pid) {
        return Err(EPERM);
    }
    let process = unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, host) };
    if process.is_null() {
        return Err(EPERM);
    }
    let mut times = [unsafe { core::mem::zeroed() }; 4];
    let valid = unsafe {
        GetProcessTimes(
            process,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } != 0;
    let actual = (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime);
    if !valid || birth != 0 && birth != actual {
        unsafe {
            CloseHandle(process);
        }
        return Err(ESRCH);
    }
    let mapping = match Mapping::open(host, true) {
        Ok(mapping) => mapping,
        Err(error) => {
            unsafe {
                CloseHandle(process);
            }
            return Err(error);
        }
    };
    let control = mapping.control();
    let mut attached = control.attached.load(Ordering::Acquire);
    if attached == 1 {
        let mut debugged = 1;
        if unsafe { CheckRemoteDebuggerPresent(process, &mut debugged) } != 0 && debugged == 0 {
            // An exited tracer is detached by Windows, while a tracee may keep
            // its shared page mapped. Recover that stale relationship.
            let _ = control
                .attached
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
            attached = control.attached.load(Ordering::Acquire);
        }
    }
    if !matches!(attached, 0 | TRACEME)
        || control
            .attached
            .compare_exchange(attached, 3, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        unsafe {
            CloseHandle(process);
        }
        return Err(EPERM);
    }
    let (commands, receiver) = mpsc::channel();
    let (ready, wait_ready) = mpsc::sync_channel(1);
    let session = Arc::new(Session {
        host,
        process: process as usize,
        mapping,
        commands,
        stopped: AtomicU32::new(0),
        interrupted: AtomicU32::new(0),
        killed: AtomicU32::new(0),
        seized,
    });
    session
        .mapping
        .control()
        .tracer
        .store(std::process::id(), Ordering::Release);
    session
        .mapping
        .control()
        .options
        .store(options, Ordering::Release);
    session
        .mapping
        .control()
        .mode
        .store(MODE_CONT, Ordering::Release);
    session
        .mapping
        .control()
        .magic
        .store(MAGIC, Ordering::Release);
    let worker = session.clone();
    std::thread::Builder::new()
        .name(format!("ptrace-{pid}"))
        .spawn(move || {
            debugger(pid, worker, receiver, ready);
        })
        .map_err(|_| {
            session
                .mapping
                .control()
                .attached
                .store(0, Ordering::Release);
            EIO
        })?;
    // Publish before the worker can post a stop. Do not wait while holding the
    // registry lock: a debug event may arrive before the ready notification.
    state.sessions.insert(pid, session);
    drop(state);
    match wait_ready.recv().unwrap_or(Err(EIO)) {
        Ok(()) => Ok(0),
        Err(error) => {
            lock_state().sessions.remove(&pid);
            Err(error)
        }
    }
}

fn from_context(context: &CONTEXT) -> Registers {
    Registers {
        r15: context.R15,
        r14: context.R14,
        r13: context.R13,
        r12: context.R12,
        rbp: context.Rbp,
        rbx: context.Rbx,
        r11: context.R11,
        r10: context.R10,
        r9: context.R9,
        r8: context.R8,
        rax: context.Rax,
        rcx: context.Rcx,
        rdx: context.Rdx,
        rsi: context.Rsi,
        rdi: context.Rdi,
        orig_rax: u64::MAX,
        rip: context.Rip,
        cs: 0x33,
        eflags: u64::from(context.EFlags),
        rsp: context.Rsp,
        ss: 0x2b,
        ..Registers::default()
    }
}
fn to_context(regs: &Registers, context: &mut CONTEXT) -> Result<(), i32> {
    if regs.cs != 0x33 || regs.ss != 0x2b || regs.rip >> 47 != 0 || regs.rsp >> 47 != 0 {
        return Err(EIO);
    }
    context.R15 = regs.r15;
    context.R14 = regs.r14;
    context.R13 = regs.r13;
    context.R12 = regs.r12;
    context.Rbp = regs.rbp;
    context.Rbx = regs.rbx;
    context.R11 = regs.r11;
    context.R10 = regs.r10;
    context.R9 = regs.r9;
    context.R8 = regs.r8;
    context.Rax = regs.rax;
    context.Rcx = regs.rcx;
    context.Rdx = regs.rdx;
    context.Rsi = regs.rsi;
    context.Rdi = regs.rdi;
    context.Rip = regs.rip;
    context.Rsp = regs.rsp;
    context.EFlags = (context.EFlags & !0x0004_0dd5) | (regs.eflags as u32 & 0x0004_0dd5) | 2;
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SyscallInfo {
    pub op: u8,
    pad: [u8; 3],
    pub arch: u32,
    pub instruction_pointer: u64,
    pub stack_pointer: u64,
    pub payload: [u64; 7],
}

struct Stop {
    event: DEBUG_EVENT,
    regs: usize,
    fp: usize,
    fp_xsave: bool,
    info: SyscallInfo,
    signal: i32,
}
// windows-sys models the fields with Rust's eight-byte alignment. The native
// x64 debug API requires the context buffer itself to be 16-byte aligned.
#[repr(C, align(16))]
struct AlignedContext(CONTEXT);
impl Stop {
    fn thread(&self) -> Result<HANDLE, i32> {
        let thread = unsafe {
            OpenThread(
                THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                self.event.dwThreadId,
            )
        };
        if thread.is_null() {
            Err(ESRCH)
        } else {
            Ok(thread)
        }
    }
    fn context(&self) -> Result<CONTEXT, i32> {
        let thread = self.thread()?;
        let mut context = AlignedContext(unsafe { core::mem::zeroed() });
        context.0.ContextFlags = CONTEXT_ALL_AMD64;
        let ok = unsafe { GetThreadContext(thread, &mut context.0) };
        unsafe {
            CloseHandle(thread);
        }
        if ok == 0 { Err(EIO) } else { Ok(context.0) }
    }
    fn set_context(&self, context: &CONTEXT) -> Result<(), i32> {
        let thread = self.thread()?;
        let aligned = AlignedContext(*context);
        let ok = unsafe { SetThreadContext(thread, &aligned.0) };
        unsafe {
            CloseHandle(thread);
        }
        if ok == 0 { Err(EIO) } else { Ok(()) }
    }
    fn registers(&self, session: &Session) -> Result<Registers, i32> {
        if self.regs == 0 {
            return self.context().map(|context| from_context(&context));
        }
        let mut regs = Registers::default();
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                (&mut regs as *mut Registers).cast(),
                core::mem::size_of::<Registers>(),
            )
        };
        copy_memory(session.process as HANDLE, self.regs, bytes, false)?;
        Ok(regs)
    }
    fn set_registers(&self, session: &Session, mut regs: Registers) -> Result<(), i32> {
        if self.regs == 0 {
            let mut context = self.context()?;
            to_context(&regs, &mut context)?;
            return self.set_context(&context);
        }
        if regs.cs != 0x33 || regs.ss != 0x2b || regs.rip >> 47 != 0 || regs.rsp >> 47 != 0 {
            return Err(EIO);
        }
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                (&mut regs as *mut Registers).cast(),
                core::mem::size_of::<Registers>(),
            )
        };
        copy_memory(session.process as HANDLE, self.regs, bytes, true)
    }
    fn fpregs(&self, session: &Session) -> Result<[u8; 512], i32> {
        let mut bytes = [0; 512];
        if self.fp != 0 {
            copy_memory(session.process as HANDLE, self.fp, &mut bytes, false)?;
        } else if self.regs != 0 {
            // A direct native libc call has no saved guest vector frame. Host
            // RaiseException registers must never be presented as guest state.
            return Err(EIO);
        } else {
            let context = self.context()?;
            let source = unsafe {
                core::slice::from_raw_parts(
                    (&context.Anonymous.FltSave as *const XSAVE_FORMAT).cast::<u8>(),
                    512,
                )
            };
            bytes.copy_from_slice(source);
        }
        Ok(bytes)
    }
    fn set_fpregs(&self, session: &Session, mut bytes: [u8; 512]) -> Result<(), i32> {
        let original = self.fpregs(session)?;
        let mask = u32::from_le_bytes(original[28..32].try_into().unwrap());
        let mask = if mask == 0 { 0xffbf } else { mask };
        let mxcsr = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        if mxcsr & !mask != 0 {
            return Err(EIO);
        }
        bytes[28..32].copy_from_slice(&original[28..32]);
        if self.fp != 0 {
            let mut features = [0; 8];
            if self.fp_xsave {
                copy_memory(
                    session.process as HANDLE,
                    self.fp + 512,
                    &mut features,
                    false,
                )?;
            }
            copy_memory(session.process as HANDLE, self.fp, &mut bytes, true)?;
            if self.fp_xsave {
                // XRSTOR must restore newly supplied x87/XMM state even if the
                // guest previously had these components in the initial state.
                features = (u64::from_le_bytes(features) | 3).to_le_bytes();
                copy_memory(
                    session.process as HANDLE,
                    self.fp + 512,
                    &mut features,
                    true,
                )?;
            }
            Ok(())
        } else {
            let mut context = self.context()?;
            let destination = unsafe {
                core::slice::from_raw_parts_mut(
                    (&mut context.Anonymous.FltSave as *mut XSAVE_FORMAT).cast::<u8>(),
                    512,
                )
            };
            destination.copy_from_slice(&bytes);
            context.MxCsr = mxcsr;
            self.set_context(&context)
        }
    }
}

fn command(session: &Session, stop: &Stop, command: &Command) -> Result<(i64, bool), i32> {
    let process = session.process as HANDLE;
    let value = match command.request {
        1 | 2 => {
            let mut word = [0u8; 8];
            copy_memory(process, command.address, &mut word, false).map_err(|_| EIO)?;
            i64::from_le_bytes(word)
        }
        4 | 5 => {
            copy_memory(
                process,
                command.address,
                &mut command.data.to_le_bytes(),
                true,
            )
            .map_err(|_| EIO)?;
            unsafe {
                FlushInstructionCache(process, command.address as _, 8);
            }
            0
        }
        3 | 6 => {
            if command.address % 8 != 0 {
                return Err(EIO);
            }
            if command.address < core::mem::size_of::<Registers>() {
                let mut regs = stop.registers(session)?;
                let words = unsafe {
                    core::slice::from_raw_parts_mut((&mut regs as *mut Registers).cast::<u64>(), 27)
                };
                if command.request == 3 {
                    words[command.address / 8] as i64
                } else {
                    words[command.address / 8] = command.data as u64;
                    stop.set_registers(session, regs)?;
                    0
                }
            } else if (848..912).contains(&command.address) {
                let mut context = stop.context()?;
                let index = (command.address - 848) / 8;
                let word = match index {
                    0 => &mut context.Dr0,
                    1 => &mut context.Dr1,
                    2 => &mut context.Dr2,
                    3 => &mut context.Dr3,
                    6 => &mut context.Dr6,
                    7 => &mut context.Dr7,
                    _ => return Err(EIO),
                };
                if command.request == 3 {
                    *word as i64
                } else {
                    *word = command.data as u64;
                    stop.set_context(&context)?;
                    0
                }
            } else {
                return Err(EIO);
            }
        }
        12 => {
            write_value(command.data, stop.registers(session)?)?;
            0
        }
        13 => {
            stop.set_registers(session, read_value(command.data)?)?;
            0
        }
        14 | 15 | 18 | 19 => {
            if matches!(command.request, 14 | 18) {
                write_value(command.data, stop.fpregs(session)?)?;
            } else {
                stop.set_fpregs(session, read_value(command.data)?)?;
            }
            0
        }
        0x4200 => {
            if command.data > u32::MAX as usize || command.data as u32 & !SUPPORTED_OPTIONS != 0 {
                return Err(EINVAL);
            }
            session
                .mapping
                .control()
                .options
                .store(command.data as u32, Ordering::Release);
            unsafe {
                DebugSetProcessKillOnExit(i32::from(command.data as u32 & EXITKILL != 0));
            }
            0
        }
        0x4201 => {
            write_value(command.data, 0u64)?;
            0
        }
        0x4202 => {
            let mut info = [0u8; 128];
            info[..4].copy_from_slice(&stop.signal.to_le_bytes());
            info[8..12]
                .copy_from_slice(&(if stop.info.op != 0 { 0x80i32 } else { 1i32 }).to_le_bytes());
            copy_memory(
                unsafe { GetCurrentProcess() },
                command.data,
                &mut info,
                true,
            )?;
            0
        }
        0x4204 | 0x4205 => {
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct Iovec {
                base: usize,
                len: usize,
            }
            let mut iovec: Iovec = read_value(command.data)?;
            match command.address {
                1 => {
                    let mut regs = stop.registers(session)?;
                    let bytes = unsafe {
                        core::slice::from_raw_parts_mut(
                            (&mut regs as *mut Registers).cast(),
                            core::mem::size_of::<Registers>(),
                        )
                    };
                    if iovec.len % 8 != 0 {
                        return Err(EINVAL);
                    }
                    let length = iovec.len.min(bytes.len());
                    copy_memory(
                        unsafe { GetCurrentProcess() },
                        iovec.base,
                        &mut bytes[..length],
                        command.request == 0x4204,
                    )?;
                    if command.request == 0x4205 {
                        stop.set_registers(session, regs)?;
                    }
                    iovec.len = length;
                }
                2 => {
                    if iovec.len % 8 != 0 {
                        return Err(EINVAL);
                    }
                    let mut bytes = stop.fpregs(session)?;
                    let length = iovec.len.min(bytes.len());
                    if length != 0 {
                        copy_memory(
                            unsafe { GetCurrentProcess() },
                            iovec.base,
                            &mut bytes[..length],
                            command.request == 0x4204,
                        )?;
                    }
                    if command.request == 0x4205 {
                        stop.set_fpregs(session, bytes)?;
                    }
                    iovec.len = length;
                }
                _ => return Err(EINVAL),
            }
            write_value(command.data, iovec)?;
            0
        }
        0x420e => {
            let size = match stop.info.op {
                1 => 80,
                2 => 33,
                _ => 24,
            };
            let mut info = stop.info;
            if info.op != 0 {
                let regs = stop.registers(session)?;
                info.instruction_pointer = regs.rip;
                info.stack_pointer = regs.rsp;
                if info.op == 1 {
                    info.payload = [
                        regs.orig_rax,
                        regs.rdi,
                        regs.rsi,
                        regs.rdx,
                        regs.r10,
                        regs.r8,
                        regs.r9,
                    ];
                } else if info.op == 2 {
                    info.payload[0] = regs.rax;
                    info.payload[1] = u64::from((-4095..0).contains(&(regs.rax as i64)));
                }
            }
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(
                    (&mut info as *mut SyscallInfo).cast(),
                    core::mem::size_of::<SyscallInfo>(),
                )
            };
            let length = command.address.min(size);
            if length != 0 {
                copy_memory(
                    unsafe { GetCurrentProcess() },
                    command.data,
                    &mut bytes[..length],
                    true,
                )?;
            }
            size as i64
        }
        8 => {
            session.killed.store(9, Ordering::Release);
            if unsafe { TerminateProcess(process, 137) } == 0 {
                return Err(EIO);
            }
            return Ok((0, true));
        }
        7 | 9 | 17 | 24 => {
            if command.data >= 65 {
                return Err(EIO);
            }
            session.mapping.control().mode.store(
                match command.request {
                    24 => MODE_SYSCALL,
                    9 if stop.regs != 0 => MODE_STEP_SYSCALL,
                    _ => MODE_CONT,
                },
                Ordering::Release,
            );
            if stop.regs == 0 {
                let mut context = stop.context()?;
                if command.request == 9 {
                    context.EFlags |= 0x100;
                } else {
                    context.EFlags &= !0x100;
                }
                stop.set_context(&context)?;
            }
            if command.data != 0 {
                // Windows exceptions use DBG_EXCEPTION_NOT_HANDLED when the
                // original native signal is redelivered. Other injections go
                // through the existing Linux signal mailbox.
                if stop.regs != 0 {
                    session
                        .mapping
                        .control()
                        .injected
                        .store(command.data as u32, Ordering::Release);
                } else if command.data as i32 != stop.signal {
                    kinakaze_vfs::job::kill(
                        kinakaze_vfs::job::namespace_pid(session.host).ok_or(ESRCH)? as i32,
                        command.data as i32,
                    )?;
                }
            }
            if command.request == 17 {
                session
                    .mapping
                    .control()
                    .attached
                    .store(0, Ordering::Release);
            }
            return Ok((0, true));
        }
        _ => return Err(EIO),
    };
    Ok((value, false))
}

fn debugger(
    pid: i32,
    session: Arc<Session>,
    receiver: mpsc::Receiver<Command>,
    ready: mpsc::SyncSender<Result<(), i32>>,
) {
    if unsafe { DebugActiveProcess(session.host) } == 0 {
        session
            .mapping
            .control()
            .attached
            .store(0, Ordering::Release);
        let _ = ready.send(Err(kinakaze_vfs::errno_from_win32(unsafe {
            GetLastError()
        })));
        return;
    }
    unsafe {
        DebugSetProcessKillOnExit(i32::from(
            session.mapping.control().options.load(Ordering::Acquire) & EXITKILL != 0,
        ));
    }
    session
        .mapping
        .control()
        .attached
        .store(1, Ordering::Release);
    let _ = ready.send(Ok(()));
    let mut initial = true;
    loop {
        let mut event: DEBUG_EVENT = unsafe { core::mem::zeroed() };
        if unsafe { WaitForDebugEventEx(&mut event, INFINITE) } == 0 {
            break;
        }
        let mut disposition = DBG_CONTINUE;
        let mut stop = None;
        match event.dwDebugEventCode {
            CREATE_PROCESS_DEBUG_EVENT => {
                let info = unsafe { event.u.CreateProcessInfo };
                if !info.hFile.is_null() {
                    unsafe {
                        CloseHandle(info.hFile);
                    }
                }
            }
            LOAD_DLL_DEBUG_EVENT => {
                let file = unsafe { event.u.LoadDll.hFile };
                if !file.is_null() {
                    unsafe {
                        CloseHandle(file);
                    }
                }
            }
            EXIT_PROCESS_DEBUG_EVENT => {
                let signal = session.killed.load(Ordering::Acquire);
                report(
                    pid,
                    if signal != 0 {
                        signal as i32
                    } else {
                        ((unsafe { event.u.ExitProcess.dwExitCode } as i32) & 0xff) << 8
                    },
                );
                unsafe {
                    ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE);
                }
                break;
            }
            EXCEPTION_DEBUG_EVENT => {
                let record = unsafe { event.u.Exception.ExceptionRecord };
                let code = record.ExceptionCode as u32;
                if code == SYSCALL_EXCEPTION && record.NumberParameters >= 2 {
                    let mut info = SyscallInfo::default();
                    let bytes = unsafe {
                        core::slice::from_raw_parts_mut(
                            (&mut info as *mut SyscallInfo).cast(),
                            core::mem::size_of::<SyscallInfo>(),
                        )
                    };
                    if copy_memory(
                        session.process as HANDLE,
                        record.ExceptionInformation[1],
                        bytes,
                        false,
                    )
                    .is_ok()
                    {
                        stop = Some(Stop {
                            event,
                            regs: record.ExceptionInformation[0],
                            fp: if record.NumberParameters >= 3 {
                                record.ExceptionInformation[2]
                            } else {
                                0
                            },
                            fp_xsave: record.NumberParameters >= 4
                                && record.ExceptionInformation[3] != 0,
                            info,
                            signal: 5
                                | if session.mapping.control().options.load(Ordering::Acquire)
                                    & TRACESYSGOOD
                                    != 0
                                {
                                    0x80
                                } else {
                                    0
                                },
                        });
                    }
                } else if code == SIGNAL_EXCEPTION && record.NumberParameters >= 1 {
                    stop = Some(Stop {
                        event,
                        regs: 0,
                        fp: 0,
                        fp_xsave: false,
                        info: SyscallInfo::default(),
                        signal: record.ExceptionInformation[0] as i32,
                    });
                } else if code == 0x8000_0003 && initial {
                    initial = false;
                    if !session.seized {
                        stop = Some(Stop {
                            event,
                            regs: 0,
                            fp: 0,
                            fp_xsave: false,
                            info: SyscallInfo::default(),
                            signal: 19,
                        });
                    }
                } else if code == 0x8000_0003 || code == 0x8000_0004 {
                    stop = Some(Stop {
                        event,
                        regs: 0,
                        fp: 0,
                        fp_xsave: false,
                        info: SyscallInfo::default(),
                        signal: 5,
                    });
                } else {
                    // Recoverable runtime faults (FS restoration, UD2 syscall
                    // traps, guarded pages) must reach VEH unchanged.
                    disposition = DBG_EXCEPTION_NOT_HANDLED;
                    if unsafe { event.u.Exception.dwFirstChance } == 0 {
                        let signal = match code {
                            0xc000_0005 => 11,
                            0xc000_0094 | 0xc000_008e => 8,
                            _ => 4,
                        };
                        stop = Some(Stop {
                            event,
                            regs: 0,
                            fp: 0,
                            fp_xsave: false,
                            info: SyscallInfo::default(),
                            signal,
                        });
                    }
                }
            }
            _ => {}
        }
        if let Some(stop) = stop {
            session.stopped.store(1, Ordering::Release);
            let event_kind = if session.interrupted.swap(0, Ordering::AcqRel) != 0 {
                128 << 16
            } else {
                0
            };
            report(pid, event_kind | (stop.signal << 8) | 0x7f);
            loop {
                let Ok(request) = receiver.recv() else {
                    return;
                };
                let result = command(&session, &stop, &request);
                let resume = result.as_ref().is_ok_and(|(_, resume)| *resume);
                let value = result.map(|(value, _)| value);
                // Resume before acknowledging it, so immediate follow-up
                // requests see the new stopped state rather than a stale one.
                if resume {
                    session.stopped.store(0, Ordering::Release);
                    if stop.regs == 0 && request.data as i32 == stop.signal && stop.signal != 19 {
                        disposition = DBG_EXCEPTION_NOT_HANDLED;
                    } else {
                        disposition = DBG_CONTINUE;
                    }
                    unsafe {
                        ContinueDebugEvent(event.dwProcessId, event.dwThreadId, disposition);
                    }
                    if request.request == 17 {
                        let ok = unsafe { DebugActiveProcessStop(session.host) };
                        let mut state = lock_state();
                        state.sessions.remove(&pid);
                        state.reports.retain(|report| report.pid != pid);
                        drop(state);
                        let _ = request.answer.send(if ok == 0 { Err(EIO) } else { value });
                        return;
                    }
                    let _ = request.answer.send(value);
                    break;
                }
                let _ = request.answer.send(value);
            }
        } else {
            unsafe {
                ContinueDebugEvent(event.dwProcessId, event.dwThreadId, disposition);
            }
        }
    }
    session
        .mapping
        .control()
        .attached
        .store(0, Ordering::Release);
    session.stopped.store(0, Ordering::Release);
    lock_state().sessions.remove(&pid);
    state().1.notify_all();
}

/// libc request convention. Raw PEEK has an additional output-pointer copy.
pub fn request(request: i32, pid: i32, address: usize, data: usize) -> Result<i64, i32> {
    match request {
        0 => {
            let mapping = Mapping::open(std::process::id(), true)?;
            let control = mapping.control();
            if control.attached.load(Ordering::Acquire) != 0 {
                return Err(EPERM);
            }
            control
                .tracer
                .store(kinakaze_vfs::job::parent_process_id(), Ordering::Release);
            control.magic.store(MAGIC, Ordering::Release);
            control.attached.store(TRACEME, Ordering::Release);
            tracee_mapping()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace((std::process::id(), mapping));
            return Ok(0);
        }
        16 | 0x4206 => {
            if request == 0x4206 && address != 0 {
                return Err(EINVAL);
            }
            let (host, birth) = target(pid)?;
            return attach(
                pid,
                host,
                birth,
                request == 0x4206,
                if request == 0x4206 {
                    u32::try_from(data).map_err(|_| EINVAL)?
                } else {
                    0
                },
            );
        }
        _ => {}
    }
    let session = lock_state().sessions.get(&pid).cloned().ok_or(ESRCH)?;
    if request == 0x4207 {
        if !session.seized {
            return Err(EIO);
        }
        session.interrupted.store(1, Ordering::Release);
        if session.stopped.load(Ordering::Acquire) == 0
            && unsafe { DebugBreakProcess(session.process as HANDLE) } == 0
        {
            return Err(EIO);
        }
        return Ok(0);
    }
    if request == 8 {
        if session.stopped.load(Ordering::Acquire) == 0 {
            session.killed.store(9, Ordering::Release);
            return if unsafe { TerminateProcess(session.process as HANDLE, 137) } != 0 {
                Ok(0)
            } else {
                Err(EIO)
            };
        }
    }
    if session.stopped.load(Ordering::Acquire) == 0 {
        return Err(ESRCH);
    }
    let (answer, receiver) = mpsc::sync_channel(1);
    session
        .commands
        .send(Command {
            request,
            address,
            data,
            answer,
        })
        .map_err(|_| ESRCH)?;
    receiver.recv().unwrap_or(Err(ESRCH))
}

pub fn raw_request(request_number: i32, pid: i32, address: usize, data: usize) -> i64 {
    let result = request(request_number, pid, address, data).and_then(|word| {
        if matches!(request_number, 1..=3) {
            write_value(data, word)?;
            Ok(0)
        } else {
            Ok(word)
        }
    });
    result.unwrap_or_else(|error| -i64::from(error))
}

fn matches_pid(selector: i32, pid: i32) -> bool {
    if selector > 0 {
        return selector == pid;
    }
    if selector == -1 {
        return true;
    }
    let group = if selector == 0 {
        kinakaze_vfs::job::current_pgid()
    } else {
        selector.saturating_neg()
    };
    kinakaze_vfs::job::getpgid(pid).ok() == Some(group)
}

/// A ptrace stop is waitable regardless of WUNTRACED. WNOWAIT leaves the report
/// intact; attachment to a non-child is also waitable by its tracer.
pub fn wait(selector: i32, options: i32) -> Option<Result<(i32, i32), i32>> {
    // A native debug stop can suspend the target while it owns a shared PID
    // table mutex. Existing sessions must be served without entering that table.
    if let Some(result) = wait_existing(selector, options) {
        return Some(result);
    }
    // A TRACEME child requests its attachment before stopping itself. Discover
    // it through the authoritative child registry, never through host PID scans.
    let own = kinakaze_vfs::job::process_id();
    let candidates: Vec<_> = kinakaze_vfs::job::processes()
        .into_iter()
        .filter(|entry| entry.ppid == own && matches_pid(selector, entry.namespace_pid as i32))
        .collect();
    for child in candidates {
        if lock_state()
            .sessions
            .contains_key(&(child.namespace_pid as i32))
        {
            continue;
        }
        if let Ok(mapping) = Mapping::open(child.pid, false) {
            let control = mapping.control();
            if control.magic.load(Ordering::Acquire) == MAGIC
                && control.attached.load(Ordering::Acquire) == TRACEME
                && control.tracer.load(Ordering::Acquire) == own
            {
                if let Err(error) = attach(child.namespace_pid as i32, child.pid, 0, false, 0) {
                    return Some(Err(error));
                }
                let _ = kinakaze_vfs::job::kill(child.namespace_pid as i32, 18);
            }
        }
    }
    wait_existing(selector, options)
}

fn wait_existing(selector: i32, options: i32) -> Option<Result<(i32, i32), i32>> {
    let mut state_guard = lock_state();
    if selector <= 0 && selector != -1 {
        // Native debugging suspends every target thread, including one that
        // owns the shared process-table mutex. Group matching cannot query
        // that table safely until group membership is available independently.
        // Keep unsupported selectors explicit instead of deadlocking a tracer.
        return (!state_guard.sessions.is_empty() || !state_guard.reports.is_empty())
            .then_some(Err(kinakaze_vfs::EOPNOTSUPP));
    }
    loop {
        if let Some(index) = state_guard
            .reports
            .iter()
            .position(|event| matches_pid(selector, event.pid))
        {
            let event = state_guard.reports[index];
            if options & 0x0100_0000 == 0 {
                state_guard.reports.remove(index);
                if event.status & 0xff != 0x7f {
                    state_guard.sessions.remove(&event.pid);
                }
            }
            return Some(Ok((event.pid, event.status)));
        }
        if !state_guard
            .sessions
            .keys()
            .any(|pid| matches_pid(selector, *pid))
        {
            return None;
        }
        if options & 1 != 0 {
            return Some(Ok((0, 0)));
        }
        state_guard = state()
            .1
            .wait(state_guard)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

fn tracee_mapping() -> &'static Mutex<Option<(u32, Mapping)>> {
    static MAPPING: OnceLock<Mutex<Option<(u32, Mapping)>>> = OnceLock::new();
    MAPPING.get_or_init(|| Mutex::new(None))
}

fn tracee_control() -> Option<usize> {
    if !debugger_present() {
        return None;
    }
    // Only traced execution touches the cache mutex. A native diagnostic
    // debugger without a Linux control page never receives synthetic events.
    let mut cache = tracee_mapping()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let own = std::process::id();
    if cache.as_ref().is_none_or(|(host, _)| *host != own) {
        *cache = Mapping::open(own, false).ok().map(|mapping| (own, mapping));
    }
    let (_, mapping) = cache.as_ref()?;
    let control = mapping.control();
    (control.magic.load(Ordering::Acquire) == MAGIC
        && control.attached.load(Ordering::Acquire) == 1)
        .then_some(mapping.view)
}

pub struct SyscallStop {
    control: usize,
    regs: Registers,
    frame: usize,
}
impl SyscallStop {
    pub fn enter(number: &mut i64, arguments: &mut [u64; 6]) -> Option<Self> {
        if !debugger_present() || RAW_DEPTH.with(|depth| depth.get() > 1) {
            return None;
        }
        let control = tracee_control()?;
        let mode = unsafe { &*(control as *const Control) }
            .mode
            .load(Ordering::Acquire);
        if mode == MODE_CONT {
            return None;
        }
        let (stack, instruction) =
            kinakaze_tls::thread_pointer::active_guest_signal_context().unwrap_or((0, 0));
        let frame = kinakaze_tls::thread_pointer::active_raw_syscall_frame_pointer().unwrap_or(0);
        let mut stop = Self {
            control,
            frame,
            regs: Registers {
                orig_rax: *number as u64,
                rax: (-38i64) as u64,
                rdi: arguments[0],
                rsi: arguments[1],
                rdx: arguments[2],
                r10: arguments[3],
                r8: arguments[4],
                r9: arguments[5],
                rip: instruction as u64,
                rsp: stack as u64,
                cs: 0x33,
                ss: 0x2b,
                eflags: 0x202,
                fs_base: crate::process_thread_pointer() as u64,
                ..Registers::default()
            },
        };
        if frame != 0 {
            use kinakaze_tls::thread_pointer::*;
            for (register, offset) in [
                (&mut stop.regs.r15, RAW_SYSCALL_FRAME_R15_OFFSET),
                (&mut stop.regs.r14, RAW_SYSCALL_FRAME_R14_OFFSET),
                (&mut stop.regs.r13, RAW_SYSCALL_FRAME_R13_OFFSET),
                (&mut stop.regs.r12, RAW_SYSCALL_FRAME_R12_OFFSET),
                (&mut stop.regs.rbp, RAW_SYSCALL_FRAME_RBP_OFFSET),
                (&mut stop.regs.rbx, RAW_SYSCALL_FRAME_RBX_OFFSET),
            ] {
                *register = unsafe { ((frame + offset) as *const u64).read_unaligned() };
            }
        }
        if mode == MODE_SYSCALL {
            stop.emit(1, 0);
            *number = stop.regs.orig_rax as i64;
            *arguments = [
                stop.regs.rdi,
                stop.regs.rsi,
                stop.regs.rdx,
                stop.regs.r10,
                stop.regs.r8,
                stop.regs.r9,
            ];
            stop.apply_frame();
        }
        Some(stop)
    }
    fn emit(&mut self, op: u8, result: i64) {
        let mut info = SyscallInfo {
            op,
            arch: 0xc000_003e,
            instruction_pointer: self.regs.rip,
            stack_pointer: self.regs.rsp,
            ..SyscallInfo::default()
        };
        if op == 1 {
            info.payload = [
                self.regs.orig_rax,
                self.regs.rdi,
                self.regs.rsi,
                self.regs.rdx,
                self.regs.r10,
                self.regs.r8,
                self.regs.r9,
            ];
        } else {
            info.payload[0] = result as u64;
            info.payload[1] = u64::from((-4095..0).contains(&result));
        }
        let fp = if self.frame == 0 {
            0
        } else {
            self.frame + kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET
        };
        let xsave = fp != 0
            && matches!(
                kinakaze_tls::thread_pointer::extended_state_format(),
                kinakaze_tls::thread_pointer::ExtendedStateFormat::Xsave { .. }
            );
        let parameters = [
            (&mut self.regs as *mut Registers) as usize,
            (&info as *const SyscallInfo) as usize,
            fp,
            usize::from(xsave),
        ];
        unsafe {
            RaiseException(
                SYSCALL_EXCEPTION,
                0,
                parameters.len() as u32,
                parameters.as_ptr(),
            );
        }
        let signal = unsafe { &*(self.control as *const Control) }
            .injected
            .swap(0, Ordering::AcqRel);
        if signal != 0 {
            let _ = kinakaze_vfs::signal::raise_signal(signal as i32);
        }
    }
    fn apply_frame(&self) {
        if self.frame == 0 {
            return;
        }
        let mut gregs = [0u64; 23];
        for (index, value) in [
            (0, self.regs.r8),
            (1, self.regs.r9),
            (2, self.regs.r10),
            (4, self.regs.r12),
            (5, self.regs.r13),
            (6, self.regs.r14),
            (7, self.regs.r15),
            (8, self.regs.rdi),
            (9, self.regs.rsi),
            (10, self.regs.rbp),
            (11, self.regs.rbx),
            (12, self.regs.rdx),
            (13, self.regs.rax),
            (15, self.regs.rsp),
            (16, self.regs.rip),
        ] {
            gregs[index] = value;
        }
        kinakaze_tls::thread_pointer::update_active_guest_general_registers(&gregs);
        kinakaze_tls::thread_pointer::update_active_guest_signal_context(
            self.regs.rsp as usize,
            self.regs.rip as usize,
        );
    }
    pub fn exit(mut self, result: i64) -> i64 {
        let control = unsafe { &*(self.control as *const Control) };
        if control.attached.load(Ordering::Acquire) != 1
            || control.mode.load(Ordering::Acquire) == MODE_CONT
        {
            return result;
        }
        self.regs.rax = result as u64;
        self.emit(2, result);
        self.apply_frame();
        self.regs.rax as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static DEBUG_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    #[ignore = "release benchmark; run with --release --ignored --exact"]
    fn benchmark_untraced_syscalls() {
        use std::hint::black_box;
        assert!(!debugger_present());
        fn measure(name: &str, operation: impl Fn() -> i64) {
            for _ in 0..1024 {
                black_box(operation());
            }
            let mut samples = [0f64; 5];
            for sample in &mut samples {
                let started = std::time::Instant::now();
                for _ in 0..200_000 {
                    black_box(operation());
                }
                *sample = started.elapsed().as_secs_f64() * 1e9 / 200_000f64;
            }
            samples.sort_by(f64::total_cmp);
            println!("BENCH {{\"case\":\"{name}\",\"median_ns\":{}}}", samples[2]);
        }
        measure("libc_getpid", || {
            i64::from(crate::process::kinakaze_abi_getpid())
        });
        measure("raw_getpid", || unsafe {
            crate::sysadmin::kinakaze_abi_syscall_raw(39, 0, 0, 0, 0, 0, 0)
        });
        measure("raw_gettid", || unsafe {
            crate::sysadmin::kinakaze_abi_syscall_raw(186, 0, 0, 0, 0, 0, 0)
        });
        measure("raw_unknown", || unsafe {
            crate::sysadmin::kinakaze_abi_syscall_raw(999999, 0, 0, 0, 0, 0, 0)
        });
    }
    #[test]
    fn linux_register_and_syscall_info_layouts() {
        assert_eq!(core::mem::size_of::<Registers>(), 216);
        assert_eq!(core::mem::offset_of!(Registers, orig_rax), 120);
        assert_eq!(core::mem::offset_of!(SyscallInfo, payload), 24);
        assert_eq!(core::mem::size_of::<SyscallInfo>(), 80);
    }
    #[test]
    fn local_guest_copies_contain_invalid_pointer_faults() {
        assert_eq!(read_value::<u64>(1), Err(EFAULT));
        assert_eq!(write_value(1, 12u64), Err(EFAULT));
        let value = u64::MAX;
        assert_eq!(read_value::<u64>(&value as *const _ as usize), Ok(value));
    }

    #[test]
    fn uncollected_stops_keep_only_the_current_waitable_state() {
        let _guard = DEBUG_TEST_LOCK.lock().unwrap();
        let pid = i32::MAX;
        for _ in 0..10_000 {
            report(pid, (5 << 8) | 0x7f);
        }
        assert_eq!(
            lock_state()
                .reports
                .iter()
                .filter(|event| event.pid == pid)
                .count(),
            1
        );
        report(pid, 23 << 8);
        assert_eq!(wait_existing(0, 1), Some(Err(kinakaze_vfs::EOPNOTSUPP)));
        assert_eq!(wait_existing(-1234, 1), Some(Err(kinakaze_vfs::EOPNOTSUPP)));
        assert_eq!(wait_existing(pid, 0x0100_0000), Some(Ok((pid, 23 << 8))));
        assert_eq!(wait_existing(pid, 0), Some(Ok((pid, 23 << 8))));
        assert!(wait_existing(pid, 1).is_none());
    }

    #[test]
    fn native_debug_child() {
        use std::io::Write;
        let Ok(name) = std::env::var("KINAKAZE_PTRACE_FIXTURE") else {
            return;
        };
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let event = unsafe { OpenEventW(0x0010_0000, 0, name.as_ptr()) };
        assert!(!event.is_null());
        let marker = Box::new(0x0123_4567_89ab_cdefu64);
        println!("PTR {:x}", (&*marker as *const u64) as usize);
        std::io::stdout().flush().unwrap();
        unsafe {
            WaitForSingleObject(event, INFINITE);
            CloseHandle(event);
        }
        let first = crate::process::kinakaze_abi_getpid() as i64;
        let second =
            unsafe { crate::sysadmin::kinakaze_abi_syscall_raw(1, u64::MAX, 0, 0, 0, 0, 0) };
        let third =
            unsafe { crate::sysadmin::kinakaze_abi_syscall_raw(0, u64::MAX, 0, 1, 0, 0, 0) };
        let fourth = unsafe { crate::sysadmin::kinakaze_abi_syscall_raw(999999, 0, 0, 0, 0, 0, 0) };
        unsafe {
            ExitProcess(
                if first == 1234
                    && second == 1234
                    && third == 1234
                    && fourth == 1234
                    && *marker == u64::MAX
                {
                    23
                } else {
                    91
                },
            );
        }
    }

    struct NativeFixture {
        child: std::process::Child,
        event: HANDLE,
        pointer: usize,
    }
    impl NativeFixture {
        fn new(label: &str) -> Self {
            use std::io::{BufRead, BufReader};
            let name = format!("Local\\kinakaze.ptrace.{label}.{}", std::process::id());
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let event = unsafe { CreateEventW(core::ptr::null(), 1, 0, wide.as_ptr()) };
            assert!(!event.is_null());
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ptrace::tests::native_debug_child",
                    "--nocapture",
                ])
                .env("KINAKAZE_PTRACE_FIXTURE", name)
                .stdout(std::process::Stdio::piped())
                .spawn();
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    unsafe {
                        CloseHandle(event);
                    }
                    panic!("spawn fixture: {error}")
                }
            };
            let mut fixture = Self {
                child,
                event,
                pointer: 0,
            };
            let mut output = BufReader::new(fixture.child.stdout.take().unwrap());
            loop {
                let mut line = String::new();
                assert!(output.read_line(&mut line).unwrap() > 0);
                if let Some(address) = line.trim().strip_prefix("PTR ") {
                    fixture.pointer = usize::from_str_radix(address, 16).unwrap();
                    return fixture;
                }
            }
        }
        fn pid(&self) -> i32 {
            self.child.id() as i32
        }
    }
    impl Drop for NativeFixture {
        fn drop(&mut self) {
            // A test assertion or timeout must never leave a stopped child or
            // a named event alive after its parent test has unwound.
            let attached = lock_state().sessions.contains_key(&self.pid());
            if attached {
                let _ = request(8, self.pid(), 0, 0);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
            unsafe {
                CloseHandle(self.event);
            }
        }
    }

    #[test]
    fn native_debugger_stops_memory_registers_syscalls_and_exit() {
        let _guard = DEBUG_TEST_LOCK.lock().unwrap();
        let mut fixture = NativeFixture::new("syscalls");
        let pointer = fixture.pointer;
        // Tests exercise the private native backend directly. Guest-facing
        // attach always passes through manager/ancestry authorization first.
        let pid = fixture.pid();
        attach(pid, fixture.child.id(), 0, false, 0).unwrap();
        assert_eq!(wait(pid, 0), Some(Ok((pid, (19 << 8) | 0x7f))));
        let mut fp = [0u8; 512];
        assert_eq!(request(14, pid, 0, fp.as_mut_ptr() as usize), Ok(0));
        assert_eq!(request(15, pid, 0, fp.as_ptr() as usize), Ok(0));
        assert_eq!(request(1, pid, pointer, 0), Ok(0x0123_4567_89ab_cdef));
        assert_eq!(request(5, pid, pointer, usize::MAX), Ok(0));
        crate::set_errno(77);
        assert_eq!(crate::process::kinakaze_abi_ptrace(1, pid, pointer, 0), -1);
        assert_eq!(
            kinakaze_tls::errno(),
            0,
            "a successful PEEK of -1 clears errno"
        );
        let mut word = 0i64;
        assert_eq!(
            raw_request(1, pid, pointer, &mut word as *mut _ as usize),
            0
        );
        assert_eq!(word, -1);
        assert_eq!(raw_request(1, pid, pointer, 1), -i64::from(EFAULT));
        assert_eq!(request(0x4200, pid, 0, 1), Ok(0));
        assert_eq!(request(0x4200, pid, 0, 2), Err(EINVAL));
        assert_eq!(request(24, pid, 0, 0), Ok(0));
        unsafe {
            SetEvent(fixture.event);
        }
        for expected in [39u64, 1, 0, 999999] {
            assert_eq!(wait(pid, 0), Some(Ok((pid, (0x85 << 8) | 0x7f))));
            let mut info = SyscallInfo::default();
            assert_eq!(
                request(0x420e, pid, 80, &mut info as *mut _ as usize),
                Ok(80)
            );
            assert_eq!(
                (info.op, info.arch, info.payload[0]),
                (1, 0xc000_003e, expected)
            );
            let mut regs = Registers::default();
            assert_eq!(request(12, pid, 0, &mut regs as *mut _ as usize), Ok(0));
            assert_eq!(regs.orig_rax, expected);
            assert_eq!(
                request(14, pid, 0, fp.as_mut_ptr() as usize),
                Err(EIO),
                "direct native ABI calls have no guest vector frame"
            );
            // Turn even the invalid write into getpid before dispatch.
            if expected == 1 {
                regs.orig_rax = 39;
            }
            assert_eq!(request(13, pid, 0, &regs as *const _ as usize), Ok(0));
            assert_eq!(request(24, pid, 0, 0), Ok(0));
            assert_eq!(wait(pid, 0), Some(Ok((pid, (0x85 << 8) | 0x7f))));
            assert_eq!(
                request(0x420e, pid, 80, &mut info as *mut _ as usize),
                Ok(33)
            );
            assert_eq!(info.op, 2);
            assert_eq!(
                info.payload[1] & 0xff,
                u64::from(expected == 0 || expected == 999999)
            );
            assert_eq!(request(12, pid, 0, &mut regs as *mut _ as usize), Ok(0));
            regs.rax = 1234;
            assert_eq!(request(13, pid, 0, &regs as *const _ as usize), Ok(0));
            assert_eq!(request(24, pid, 0, 0), Ok(0));
        }
        assert_eq!(wait(pid, 0), Some(Ok((pid, 23 << 8))));
        assert_eq!(fixture.child.wait().unwrap().code(), Some(23));
        assert_eq!(request(12, pid, 0, 0), Err(ESRCH));
    }

    #[test]
    fn native_attach_detach_churn_releases_handles_and_reports() {
        let _guard = DEBUG_TEST_LOCK.lock().unwrap();
        let fixture = NativeFixture::new("churn");
        let pid = fixture.pid();
        let cycle = || {
            attach(pid, pid as u32, 0, false, 0).unwrap();
            assert!(wait(pid, 0).unwrap().unwrap().1 & 0xff == 0x7f);
            assert_eq!(request(17, pid, 0, 0), Ok(0));
        };
        cycle(); // initialize process-table and other bounded caches first
        let count = || {
            let mut handles = 0;
            assert_ne!(
                unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) },
                0
            );
            handles
        };
        let before = count();
        for _ in 0..64 {
            cycle();
        }
        let after = count();
        assert!(
            after <= before + 2,
            "native handles grew from {before} to {after}"
        );
        assert!(lock_state().sessions.is_empty());
        assert!(lock_state().reports.is_empty());
    }

    #[test]
    fn seize_interrupt_single_step_detach_and_stopped_kill() {
        let _guard = DEBUG_TEST_LOCK.lock().unwrap();
        let fixture = NativeFixture::new("seize");
        let pid = fixture.pid();
        attach(pid, pid as u32, 0, true, 0).unwrap();
        assert_eq!(wait_existing(pid, 1), Some(Ok((0, 0))));
        assert_eq!(request(0x4207, pid, 0, 0), Ok(0));
        let stop = (128 << 16) | (5 << 8) | 0x7f;
        assert_eq!(wait(pid, 0x0100_0000), Some(Ok((pid, stop))));
        assert_eq!(wait(pid, 0), Some(Ok((pid, stop))));
        assert_eq!(request(9, pid, 0, 0), Ok(0));
        assert_eq!(wait(pid, 0), Some(Ok((pid, (5 << 8) | 0x7f))));
        assert_eq!(request(17, pid, 0, 0), Ok(0));
        assert_eq!(request(12, pid, 0, 0), Err(ESRCH));
        drop(fixture);

        let mut fixture = NativeFixture::new("kill");
        let pid = fixture.pid();
        attach(pid, pid as u32, 0, false, 0).unwrap();
        assert_eq!(wait(pid, 0), Some(Ok((pid, (19 << 8) | 0x7f))));
        assert_eq!(request(8, pid, 0, 0), Ok(0));
        assert_eq!(wait(pid, 0), Some(Ok((pid, 9))));
        assert_eq!(fixture.child.wait().unwrap().code(), Some(137));
        assert!(lock_state().sessions.is_empty());
        assert!(lock_state().reports.is_empty());
    }
}
