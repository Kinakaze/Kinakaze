//! Linux eBPF program objects and cgroup-device attachments.
//!
//! The Windows host has no kernel eBPF object that can be attached to a Job
//! Object. This module therefore implements the Linux object model explicitly:
//! programs are verified and stored in a session-wide shared section, file
//! descriptors refer to those objects, and cgroup attachments keep programs
//! alive after their loading descriptors close. Device access executes the
//! verified program against `bpf_cgroup_dev_ctx` before the operation proceeds.
//!
//! Only program types for which this layer has a real execution boundary are
//! accepted. Unsupported types and instructions fail; nothing is represented by
//! a `/dev/null` descriptor and no operation reports success without state.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::VecDeque;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};

use crate::{
    EACCES, EBADF, EBUSY, EEXIST, EINVAL, EIO, EMFILE, ENOENT, ENOSPC, EOPNOTSUPP, EPERM, FdFlags,
    FdKind,
};

pub const BPF_PROG_TYPE_CGROUP_SKB: u32 = 8;
pub const BPF_PROG_TYPE_CGROUP_DEVICE: u32 = 15;
pub const BPF_CGROUP_DEVICE: u32 = 6;

pub const BPF_F_ALLOW_OVERRIDE: u32 = 1;
pub const BPF_F_ALLOW_MULTI: u32 = 2;
pub const BPF_F_REPLACE: u32 = 4;

pub const BPF_DEVCG_ACC_MKNOD: u32 = 1;
pub const BPF_DEVCG_ACC_READ: u32 = 2;
pub const BPF_DEVCG_ACC_WRITE: u32 = 4;
pub const BPF_DEVCG_DEV_BLOCK: u32 = 1;
pub const BPF_DEVCG_DEV_CHAR: u32 = 2;

const MAGIC: u64 = 0x4352_5942_5046_3031; // "CRYBPF01"
const HEADER_SIZE: usize = 64;
const HEADER_MAGIC: usize = 0;
const HEADER_NEXT_ID: usize = 8;
const HEADER_PROGRAM_CAPACITY: usize = 12;
const HEADER_ATTACHMENT_CAPACITY: usize = 16;
const HEADER_FD_CAPACITY: usize = 20;
const HEADER_MAX_INSNS: usize = 24;

const PROGRAM_CAPACITY: usize = 128;
const MAX_INSNS: usize = 4096;
const INSN_SIZE: usize = 8;
const PROGRAM_BYTE_CAPACITY: usize = MAX_INSNS * INSN_SIZE;
const PROGRAM_HEADER_SIZE: usize = 128;
const PROGRAM_SIZE: usize = PROGRAM_HEADER_SIZE + PROGRAM_BYTE_CAPACITY;
const PROGRAMS_OFFSET: usize = HEADER_SIZE;

const PROGRAM_STATE: usize = 0;
const PROGRAM_ID: usize = 4;
const PROGRAM_TYPE: usize = 8;
const PROGRAM_EXPECTED_ATTACH: usize = 12;
const PROGRAM_INSN_COUNT: usize = 16;
const PROGRAM_FLAGS: usize = 20;
const PROGRAM_NAME: usize = 24;
const PROGRAM_NAME_CAPACITY: usize = 16;
const PROGRAM_TAG: usize = 40;
const PROGRAM_LOAD_TIME: usize = 48;
const PROGRAM_CREATED_UID: usize = 56;
const PROGRAM_GPL_COMPATIBLE: usize = 60;
const PROGRAM_LICENSE_LENGTH: usize = 64;
const PROGRAM_LICENSE: usize = 68;
const PROGRAM_LICENSE_CAPACITY: usize = 32;
const PROGRAM_BYTECODE: usize = PROGRAM_HEADER_SIZE;
const PROGRAM_FREE: u32 = 0;
const PROGRAM_RESERVED: u32 = 1;
const PROGRAM_LIVE: u32 = 2;

const ATTACHMENT_CAPACITY: usize = 1024;
const ATTACHMENT_PATH_CAPACITY: usize = 512;
const ATTACHMENT_SIZE: usize = 32 + ATTACHMENT_PATH_CAPACITY;
const ATTACHMENTS_OFFSET: usize = PROGRAMS_OFFSET + PROGRAM_CAPACITY * PROGRAM_SIZE;
const ATTACHMENT_STATE: usize = 0;
const ATTACHMENT_PROGRAM_ID: usize = 4;
const ATTACHMENT_TYPE: usize = 8;
const ATTACHMENT_FLAGS: usize = 12;
const ATTACHMENT_PATH_LENGTH: usize = 16;
const ATTACHMENT_PATH: usize = 32;

const FD_CAPACITY: usize = 8192;
const FD_SIZE: usize = 16;
const FDS_OFFSET: usize = ATTACHMENTS_OFFSET + ATTACHMENT_CAPACITY * ATTACHMENT_SIZE;
const FD_STATE: usize = 0;
const FD_PID: usize = 4;
const FD_NUMBER: usize = 8;
const FD_PROGRAM_ID: usize = 12;

const SECTION_SIZE: usize = FDS_OFFSET + FD_CAPACITY * FD_SIZE;

static BASE: AtomicUsize = AtomicUsize::new(0);
static SECTION: AtomicUsize = AtomicUsize::new(0);
static GUARD: AtomicUsize = AtomicUsize::new(0);
static SETUP: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub struct ProgramLoad {
    pub program_type: u32,
    pub expected_attach_type: u32,
    pub flags: u32,
    pub instructions: Vec<u8>,
    pub license: String,
    pub name: [u8; PROGRAM_NAME_CAPACITY],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProgramInfo {
    pub program_type: u32,
    pub id: u32,
    pub tag: [u8; 8],
    pub translated_length: u32,
    pub load_time: u64,
    pub created_by_uid: u32,
    pub name: [u8; PROGRAM_NAME_CAPACITY],
    pub gpl_compatible: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BpfError {
    pub errno: i32,
    pub verifier_log: Option<String>,
}

impl BpfError {
    fn errno(errno: i32) -> Self {
        Self {
            errno,
            verifier_log: None,
        }
    }

    fn verifier(message: impl Into<String>) -> Self {
        Self {
            errno: EACCES,
            verifier_log: Some(message.into()),
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(core::iter::once(0)).collect()
}

unsafe fn read_u32(base: *mut u8, offset: usize) -> u32 {
    // SAFETY: callers validate the shared layout and record bounds.
    unsafe { base.add(offset).cast::<u32>().read_unaligned() }
}

unsafe fn write_u32(base: *mut u8, offset: usize, value: u32) {
    // SAFETY: callers validate the shared layout and record bounds.
    unsafe { base.add(offset).cast::<u32>().write_unaligned(value) }
}

unsafe fn read_u64(base: *mut u8, offset: usize) -> u64 {
    // SAFETY: callers validate the shared layout and record bounds.
    unsafe { base.add(offset).cast::<u64>().read_unaligned() }
}

unsafe fn write_u64(base: *mut u8, offset: usize, value: u64) {
    // SAFETY: callers validate the shared layout and record bounds.
    unsafe { base.add(offset).cast::<u64>().write_unaligned(value) }
}

unsafe fn program_slot(base: *mut u8, index: usize) -> *mut u8 {
    // SAFETY: the caller keeps index below PROGRAM_CAPACITY.
    unsafe { base.add(PROGRAMS_OFFSET + index * PROGRAM_SIZE) }
}

unsafe fn attachment_slot(base: *mut u8, index: usize) -> *mut u8 {
    // SAFETY: the caller keeps index below ATTACHMENT_CAPACITY.
    unsafe { base.add(ATTACHMENTS_OFFSET + index * ATTACHMENT_SIZE) }
}

unsafe fn fd_slot(base: *mut u8, index: usize) -> *mut u8 {
    // SAFETY: the caller keeps index below FD_CAPACITY.
    unsafe { base.add(FDS_OFFSET + index * FD_SIZE) }
}

fn acquire(guard: HANDLE) -> Result<(), i32> {
    if guard.is_null() {
        return Err(EIO);
    }
    // SAFETY: the process owns a live handle to the named mutex.
    match unsafe { WaitForSingleObject(guard, INFINITE) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(()),
        _ => Err(EIO),
    }
}

fn release(guard: HANDLE) {
    // SAFETY: the current thread acquired this mutex.
    unsafe { ReleaseMutex(guard) };
}

fn map_registry() -> Result<*mut u8, i32> {
    let cached = BASE.load(Ordering::Acquire);
    if cached != 0 {
        return Ok(cached as *mut u8);
    }
    let _setup = SETUP.lock().map_err(|_| EIO)?;
    let cached = BASE.load(Ordering::Acquire);
    if cached != 0 {
        return Ok(cached as *mut u8);
    }

    let guard_name = wide("Local\\kinakaze.bpf.lock.v1");
    // SAFETY: default security attributes and a live NUL-terminated name.
    let guard = unsafe { CreateMutexW(core::ptr::null(), 0, guard_name.as_ptr()) };
    if guard.is_null() {
        return Err(EIO);
    }
    let section_name = wide("Local\\kinakaze.bpf.v1");
    // SAFETY: creates or opens a pagefile-backed section with this fixed ABI.
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            core::ptr::null(),
            PAGE_READWRITE,
            0,
            SECTION_SIZE as u32,
            section_name.as_ptr(),
        )
    };
    if section.is_null() {
        unsafe { CloseHandle(guard) };
        return Err(EIO);
    }
    // SAFETY: the section was created with SECTION_SIZE bytes.
    let view: MEMORY_MAPPED_VIEW_ADDRESS =
        unsafe { MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE) };
    if view.Value.is_null() {
        unsafe {
            CloseHandle(section);
            CloseHandle(guard);
        }
        return Err(EIO);
    }
    let base = view.Value.cast::<u8>();
    if acquire(guard).is_err() {
        unsafe {
            UnmapViewOfFile(view);
            CloseHandle(section);
            CloseHandle(guard);
        }
        return Err(EIO);
    }
    // SAFETY: the view is writable and the named mutex serializes initialization.
    let compatible = unsafe {
        let magic = read_u64(base, HEADER_MAGIC);
        if magic == 0 {
            write_u32(base, HEADER_NEXT_ID, 1);
            write_u32(base, HEADER_PROGRAM_CAPACITY, PROGRAM_CAPACITY as u32);
            write_u32(base, HEADER_ATTACHMENT_CAPACITY, ATTACHMENT_CAPACITY as u32);
            write_u32(base, HEADER_FD_CAPACITY, FD_CAPACITY as u32);
            write_u32(base, HEADER_MAX_INSNS, MAX_INSNS as u32);
            write_u64(base, HEADER_MAGIC, MAGIC);
        }
        read_u64(base, HEADER_MAGIC) == MAGIC
            && read_u32(base, HEADER_PROGRAM_CAPACITY) == PROGRAM_CAPACITY as u32
            && read_u32(base, HEADER_ATTACHMENT_CAPACITY) == ATTACHMENT_CAPACITY as u32
            && read_u32(base, HEADER_FD_CAPACITY) == FD_CAPACITY as u32
            && read_u32(base, HEADER_MAX_INSNS) == MAX_INSNS as u32
    };
    release(guard);
    if !compatible {
        unsafe {
            UnmapViewOfFile(view);
            CloseHandle(section);
            CloseHandle(guard);
        }
        return Err(EIO);
    }
    SECTION.store(section as usize, Ordering::Release);
    GUARD.store(guard as usize, Ordering::Release);
    BASE.store(base as usize, Ordering::Release);
    Ok(base)
}

fn with_registry<T>(body: impl FnOnce(*mut u8) -> Result<T, i32>) -> Result<T, i32> {
    let base = map_registry()?;
    let guard = GUARD.load(Ordering::Acquire) as HANDLE;
    acquire(guard)?;
    let result = body(base);
    release(guard);
    result
}

unsafe fn find_program(base: *mut u8, id: u32, allow_reserved: bool) -> Option<*mut u8> {
    for index in 0..PROGRAM_CAPACITY {
        let slot = unsafe { program_slot(base, index) };
        let state = unsafe { read_u32(slot, PROGRAM_STATE) };
        if (state == PROGRAM_LIVE || (allow_reserved && state == PROGRAM_RESERVED))
            && unsafe { read_u32(slot, PROGRAM_ID) } == id
        {
            return Some(slot);
        }
    }
    None
}

unsafe fn has_program_reference(base: *mut u8, id: u32) -> bool {
    for index in 0..FD_CAPACITY {
        let slot = unsafe { fd_slot(base, index) };
        if unsafe { read_u32(slot, FD_STATE) } != 0
            && unsafe { read_u32(slot, FD_PROGRAM_ID) } == id
        {
            return true;
        }
    }
    for index in 0..ATTACHMENT_CAPACITY {
        let slot = unsafe { attachment_slot(base, index) };
        if unsafe { read_u32(slot, ATTACHMENT_STATE) } != 0
            && unsafe { read_u32(slot, ATTACHMENT_PROGRAM_ID) } == id
        {
            return true;
        }
    }
    false
}

unsafe fn cleanup_program(base: *mut u8, id: u32) {
    if unsafe { has_program_reference(base, id) } {
        return;
    }
    if let Some(slot) = unsafe { find_program(base, id, true) } {
        // SAFETY: the complete fixed record belongs to the held registry lock.
        unsafe { core::ptr::write_bytes(slot, 0, PROGRAM_SIZE) };
    }
}

unsafe fn add_fd_reference(base: *mut u8, pid: u32, fd: i32, id: u32) -> Result<(), i32> {
    if fd < 0 || unsafe { find_program(base, id, true) }.is_none() {
        return Err(EBADF);
    }
    let mut free = None;
    for index in 0..FD_CAPACITY {
        let slot = unsafe { fd_slot(base, index) };
        if unsafe { read_u32(slot, FD_STATE) } == 0 {
            free.get_or_insert(slot);
            continue;
        }
        if unsafe { read_u32(slot, FD_PID) } == pid
            && unsafe { read_u32(slot, FD_NUMBER) } == fd as u32
        {
            return if unsafe { read_u32(slot, FD_PROGRAM_ID) } == id {
                Ok(())
            } else {
                Err(EBUSY)
            };
        }
    }
    let slot = free.ok_or(EMFILE)?;
    unsafe {
        write_u32(slot, FD_PID, pid);
        write_u32(slot, FD_NUMBER, fd as u32);
        write_u32(slot, FD_PROGRAM_ID, id);
        write_u32(slot, FD_STATE, 1);
    }
    Ok(())
}

fn current_pid() -> u32 {
    crate::job::ensure_registered();
    crate::job::process_id()
}

fn program_id_for_fd(fd: i32) -> Result<u32, i32> {
    if crate::get(fd)?.kind != FdKind::BpfProgram {
        return Err(EBADF);
    }
    let pid = current_pid();
    with_registry(|base| {
        for index in 0..FD_CAPACITY {
            let slot = unsafe { fd_slot(base, index) };
            if unsafe { read_u32(slot, FD_STATE) } != 0
                && unsafe { read_u32(slot, FD_PID) } == pid
                && unsafe { read_u32(slot, FD_NUMBER) } == fd as u32
            {
                return Ok(unsafe { read_u32(slot, FD_PROGRAM_ID) });
            }
        }
        Err(EBADF)
    })
}

#[derive(Clone, Copy)]
struct Insn {
    code: u8,
    dst: u8,
    src: u8,
    offset: i16,
    immediate: i32,
}

fn decode(bytes: &[u8], index: usize) -> Insn {
    let at = index * INSN_SIZE;
    Insn {
        code: bytes[at],
        dst: bytes[at + 1] & 0x0f,
        src: bytes[at + 1] >> 4,
        offset: i16::from_le_bytes([bytes[at + 2], bytes[at + 3]]),
        immediate: i32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()),
    }
}

fn require_register(mask: u16, register: u8, pc: usize) -> Result<(), BpfError> {
    if register > 10 {
        return Err(BpfError::verifier(format!(
            "invalid register r{register} at instruction {pc}"
        )));
    }
    if mask & (1u16 << register) == 0 {
        return Err(BpfError::verifier(format!(
            "uninitialized register r{register} at instruction {pc}"
        )));
    }
    Ok(())
}

fn jump_target(pc: usize, offset: i16, count: usize) -> Result<usize, BpfError> {
    let target = (pc as isize)
        .checked_add(1)
        .and_then(|value| value.checked_add(offset as isize))
        .ok_or_else(|| BpfError::verifier(format!("jump overflow at instruction {pc}")))?;
    if target < 0 || target as usize >= count {
        return Err(BpfError::verifier(format!(
            "jump out of range at instruction {pc}"
        )));
    }
    if target as usize <= pc {
        return Err(BpfError::verifier(format!(
            "backward jump is not supported at instruction {pc}"
        )));
    }
    Ok(target as usize)
}

fn verify(program_type: u32, bytes: &[u8]) -> Result<(), BpfError> {
    if !matches!(
        program_type,
        BPF_PROG_TYPE_CGROUP_DEVICE | BPF_PROG_TYPE_CGROUP_SKB
    ) {
        return Err(BpfError::errno(EOPNOTSUPP));
    }
    if bytes.is_empty() || !bytes.len().is_multiple_of(INSN_SIZE) {
        return Err(BpfError::errno(EINVAL));
    }
    let count = bytes.len() / INSN_SIZE;
    if count > MAX_INSNS {
        return Err(BpfError::errno(EOPNOTSUPP));
    }
    if decode(bytes, count - 1).code != 0x95 {
        return Err(BpfError::verifier("program does not end with EXIT"));
    }

    let mut incoming = vec![None::<u16>; count];
    incoming[0] = Some((1 << 1) | (1 << 10));
    let mut work = VecDeque::from([0usize]);
    while let Some(pc) = work.pop_front() {
        let mut mask = incoming[pc].unwrap_or(0);
        let insn = decode(bytes, pc);
        if insn.dst > 10 || insn.src > 10 {
            return Err(BpfError::verifier(format!(
                "invalid register encoding at instruction {pc}"
            )));
        }
        let class = insn.code & 0x07;
        let mut successors = [None, None];
        match class {
            0x01 => {
                // LDX MEM. Cgroup-device programs may only read their fixed
                // 12-byte context and may not manufacture another pointer.
                if insn.code & 0xe0 != 0x60 || program_type != BPF_PROG_TYPE_CGROUP_DEVICE {
                    return Err(BpfError::verifier(format!(
                        "unsupported memory load at instruction {pc}"
                    )));
                }
                require_register(mask, insn.src, pc)?;
                if insn.src != 1 {
                    return Err(BpfError::verifier(format!(
                        "context load does not use r1 at instruction {pc}"
                    )));
                }
                let width = match insn.code & 0x18 {
                    0x00 => 4usize,
                    0x08 => 2,
                    0x10 => 1,
                    0x18 => 8,
                    _ => unreachable!(),
                };
                let offset = usize::try_from(insn.offset).map_err(|_| {
                    BpfError::verifier(format!("negative context offset at instruction {pc}"))
                })?;
                if offset.checked_add(width).is_none_or(|end| end > 12) {
                    return Err(BpfError::verifier(format!(
                        "context access out of range at instruction {pc}"
                    )));
                }
                mask |= 1 << insn.dst;
                successors[0] = pc.checked_add(1).filter(|next| *next < count);
            }
            0x04 | 0x07 => {
                let operation = insn.code & 0xf0;
                let source_is_register = insn.code & 0x08 != 0;
                if operation != 0xb0 && operation != 0x80 {
                    require_register(mask, insn.dst, pc)?;
                }
                if source_is_register {
                    require_register(mask, insn.src, pc)?;
                }
                if matches!(operation, 0x30 | 0x90) && !source_is_register && insn.immediate == 0 {
                    return Err(BpfError::verifier(format!(
                        "division by zero at instruction {pc}"
                    )));
                }
                if !matches!(
                    operation,
                    0x00 | 0x10
                        | 0x20
                        | 0x30
                        | 0x40
                        | 0x50
                        | 0x60
                        | 0x70
                        | 0x80
                        | 0x90
                        | 0xa0
                        | 0xb0
                        | 0xc0
                ) {
                    return Err(BpfError::verifier(format!(
                        "unsupported ALU operation at instruction {pc}"
                    )));
                }
                if insn.dst == 10 {
                    return Err(BpfError::verifier(format!(
                        "frame pointer write at instruction {pc}"
                    )));
                }
                mask |= 1 << insn.dst;
                successors[0] = pc.checked_add(1).filter(|next| *next < count);
            }
            0x05 | 0x06 => {
                let operation = insn.code & 0xf0;
                if operation == 0x90 {
                    require_register(mask, 0, pc)?;
                    continue;
                }
                if operation == 0x80 {
                    return Err(BpfError::verifier(format!(
                        "helper calls are not supported at instruction {pc}"
                    )));
                }
                if operation != 0x00 {
                    require_register(mask, insn.dst, pc)?;
                    if insn.code & 0x08 != 0 {
                        require_register(mask, insn.src, pc)?;
                    }
                }
                if !matches!(
                    operation,
                    0x00 | 0x10
                        | 0x20
                        | 0x30
                        | 0x40
                        | 0x50
                        | 0x60
                        | 0x70
                        | 0xa0
                        | 0xb0
                        | 0xc0
                        | 0xd0
                ) {
                    return Err(BpfError::verifier(format!(
                        "unsupported jump operation at instruction {pc}"
                    )));
                }
                successors[0] = Some(jump_target(pc, insn.offset, count)?);
                if operation != 0x00 {
                    successors[1] = pc.checked_add(1).filter(|next| *next < count);
                }
            }
            _ => {
                return Err(BpfError::verifier(format!(
                    "unsupported opcode {:#x} at instruction {pc}",
                    insn.code
                )));
            }
        }
        for successor in successors.into_iter().flatten() {
            match incoming[successor] {
                None => {
                    incoming[successor] = Some(mask);
                    work.push_back(successor);
                }
                Some(previous) => {
                    let merged = previous & mask;
                    if merged != previous {
                        incoming[successor] = Some(merged);
                        work.push_back(successor);
                    }
                }
            }
        }
    }
    if incoming.iter().any(Option::is_none) {
        return Err(BpfError::verifier(
            "program contains unreachable instructions",
        ));
    }
    Ok(())
}

fn sha1_tag(bytes: &[u8]) -> [u8; 8] {
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x6745_2301u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for block in padded.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(block[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (index, word) in words.into_iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut result = [0u8; 8];
    result[..4].copy_from_slice(&h[0].to_be_bytes());
    result[4..].copy_from_slice(&h[1].to_be_bytes());
    result
}

fn gpl_compatible(license: &str) -> bool {
    matches!(
        license,
        "GPL"
            | "GPL v2"
            | "GPL and additional rights"
            | "Dual BSD/GPL"
            | "Dual MIT/GPL"
            | "Dual MPL/GPL"
    )
}

pub fn load_program(load: ProgramLoad) -> Result<i32, BpfError> {
    if load.instructions.len() / INSN_SIZE > MAX_INSNS
        || load.instructions.len() > PROGRAM_BYTE_CAPACITY
    {
        return Err(BpfError::errno(EOPNOTSUPP));
    }
    if load.license.is_empty() || load.license.len() > PROGRAM_LICENSE_CAPACITY {
        return Err(BpfError::errno(EINVAL));
    }
    if load.flags != 0 {
        return Err(BpfError::errno(EINVAL));
    }
    verify(load.program_type, &load.instructions)?;
    let tag = sha1_tag(&load.instructions);
    let id = with_registry(|base| {
        let mut free = None;
        for index in 0..PROGRAM_CAPACITY {
            let slot = unsafe { program_slot(base, index) };
            if unsafe { read_u32(slot, PROGRAM_STATE) } == PROGRAM_FREE {
                free = Some(slot);
                break;
            }
        }
        let slot = free.ok_or(ENOSPC)?;
        let mut id = unsafe { read_u32(base, HEADER_NEXT_ID) }.max(1);
        for _ in 0..u32::MAX {
            if unsafe { find_program(base, id, true) }.is_none() {
                break;
            }
            id = id.wrapping_add(1).max(1);
        }
        unsafe {
            core::ptr::write_bytes(slot, 0, PROGRAM_SIZE);
            write_u32(slot, PROGRAM_ID, id);
            write_u32(slot, PROGRAM_TYPE, load.program_type);
            write_u32(slot, PROGRAM_EXPECTED_ATTACH, load.expected_attach_type);
            write_u32(
                slot,
                PROGRAM_INSN_COUNT,
                (load.instructions.len() / INSN_SIZE) as u32,
            );
            write_u32(slot, PROGRAM_FLAGS, load.flags);
            core::ptr::copy_nonoverlapping(
                load.name.as_ptr(),
                slot.add(PROGRAM_NAME),
                PROGRAM_NAME_CAPACITY,
            );
            core::ptr::copy_nonoverlapping(tag.as_ptr(), slot.add(PROGRAM_TAG), tag.len());
            write_u64(
                slot,
                PROGRAM_LOAD_TIME,
                GetTickCount64().saturating_mul(1_000_000),
            );
            write_u32(slot, PROGRAM_CREATED_UID, 0);
            write_u32(
                slot,
                PROGRAM_GPL_COMPATIBLE,
                u32::from(gpl_compatible(&load.license)),
            );
            write_u32(slot, PROGRAM_LICENSE_LENGTH, load.license.len() as u32);
            core::ptr::copy_nonoverlapping(
                load.license.as_ptr(),
                slot.add(PROGRAM_LICENSE),
                load.license.len(),
            );
            core::ptr::copy_nonoverlapping(
                load.instructions.as_ptr(),
                slot.add(PROGRAM_BYTECODE),
                load.instructions.len(),
            );
            write_u32(slot, PROGRAM_STATE, PROGRAM_RESERVED);
            write_u32(base, HEADER_NEXT_ID, id.wrapping_add(1).max(1));
        }
        Ok(id)
    })
    .map_err(BpfError::errno)?;

    let pid = current_pid();
    let fd = crate::install_handleless_with(FdKind::BpfProgram, FdFlags::CLOSE_ON_EXEC, |fd| {
        with_registry(|base| unsafe { add_fd_reference(base, pid, fd, id) })
    })
    .map_err(BpfError::errno)?;
    if let Err(error) = with_registry(|base| {
        let slot = unsafe { find_program(base, id, true) }.ok_or(EIO)?;
        unsafe { write_u32(slot, PROGRAM_STATE, PROGRAM_LIVE) };
        Ok(())
    }) {
        let _ = crate::close(fd);
        return Err(BpfError::errno(error));
    }
    Ok(fd)
}

pub fn get_fd_by_id(id: u32) -> Result<i32, i32> {
    with_registry(|base| {
        unsafe { find_program(base, id, false) }
            .map(|_| ())
            .ok_or(ENOENT)
    })?;
    let pid = current_pid();
    crate::install_handleless_with(FdKind::BpfProgram, FdFlags::CLOSE_ON_EXEC, |fd| {
        with_registry(|base| unsafe { add_fd_reference(base, pid, fd, id) })
    })
}

pub fn program_info(fd: i32) -> Result<ProgramInfo, i32> {
    let id = program_id_for_fd(fd)?;
    with_registry(|base| {
        let slot = unsafe { find_program(base, id, false) }.ok_or(EBADF)?;
        let mut name = [0u8; PROGRAM_NAME_CAPACITY];
        let mut tag = [0u8; 8];
        unsafe {
            core::ptr::copy_nonoverlapping(slot.add(PROGRAM_NAME), name.as_mut_ptr(), name.len());
            core::ptr::copy_nonoverlapping(slot.add(PROGRAM_TAG), tag.as_mut_ptr(), tag.len());
        }
        Ok(ProgramInfo {
            program_type: unsafe { read_u32(slot, PROGRAM_TYPE) },
            id,
            tag,
            translated_length: unsafe { read_u32(slot, PROGRAM_INSN_COUNT) } * INSN_SIZE as u32,
            load_time: unsafe { read_u64(slot, PROGRAM_LOAD_TIME) },
            created_by_uid: unsafe { read_u32(slot, PROGRAM_CREATED_UID) },
            name,
            gpl_compatible: unsafe { read_u32(slot, PROGRAM_GPL_COMPATIBLE) },
        })
    })
}

fn cgroup_path_from_fd(fd: i32) -> Result<String, i32> {
    if let Ok(path) = crate::tmpfs::cgroupfs::descriptor_path(fd) {
        return Ok(path);
    }
    let path = crate::synthetic_directory_path(fd).map_err(|_| EBADF)?;
    let clean = path.trim_end_matches('/');
    if clean != "/sys/fs/cgroup" && !clean.starts_with("/sys/fs/cgroup/") {
        return Err(EBADF);
    }
    if crate::cgroup::metadata(clean)?.kind != crate::procfs::ProcKind::Directory {
        return Err(EBADF);
    }
    Ok(clean.to_owned())
}

unsafe fn attachment_path(slot: *mut u8) -> Result<String, i32> {
    let length = unsafe { read_u32(slot, ATTACHMENT_PATH_LENGTH) } as usize;
    if length > ATTACHMENT_PATH_CAPACITY {
        return Err(EIO);
    }
    let bytes = unsafe { core::slice::from_raw_parts(slot.add(ATTACHMENT_PATH), length) };
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| EIO)
}

pub fn attach(
    target_fd: i32,
    program_fd: i32,
    attach_type: u32,
    flags: u32,
    replace_fd: Option<i32>,
) -> Result<(), i32> {
    if attach_type != BPF_CGROUP_DEVICE {
        return Err(EOPNOTSUPP);
    }
    if flags & !(BPF_F_ALLOW_MULTI | BPF_F_REPLACE | BPF_F_ALLOW_OVERRIDE) != 0
        || flags & BPF_F_ALLOW_OVERRIDE != 0
        || flags & BPF_F_REPLACE != 0 && flags & BPF_F_ALLOW_MULTI == 0
    {
        return Err(EINVAL);
    }
    let path = cgroup_path_from_fd(target_fd)?;
    if path.len() > ATTACHMENT_PATH_CAPACITY {
        return Err(EINVAL);
    }
    let id = program_id_for_fd(program_fd)?;
    let replace_id = match replace_fd {
        Some(fd) if flags & BPF_F_REPLACE != 0 => Some(program_id_for_fd(fd)?),
        Some(_) => return Err(EINVAL),
        None if flags & BPF_F_REPLACE != 0 => return Err(EINVAL),
        None => None,
    };
    with_registry(|base| {
        let program = unsafe { find_program(base, id, false) }.ok_or(EBADF)?;
        if unsafe { read_u32(program, PROGRAM_TYPE) } != BPF_PROG_TYPE_CGROUP_DEVICE {
            return Err(EINVAL);
        }
        let mut direct = Vec::new();
        let mut free = None;
        for index in 0..ATTACHMENT_CAPACITY {
            let slot = unsafe { attachment_slot(base, index) };
            if unsafe { read_u32(slot, ATTACHMENT_STATE) } == 0 {
                free.get_or_insert(slot);
                continue;
            }
            if unsafe { read_u32(slot, ATTACHMENT_TYPE) } == attach_type
                && unsafe { attachment_path(slot)? } == path
            {
                direct.push(slot);
            }
        }
        if let Some(replace_id) = replace_id {
            let slot = direct
                .into_iter()
                .find(|slot| unsafe { read_u32(*slot, ATTACHMENT_PROGRAM_ID) } == replace_id)
                .ok_or(ENOENT)?;
            unsafe { write_u32(slot, ATTACHMENT_PROGRAM_ID, id) };
            unsafe { cleanup_program(base, replace_id) };
            return Ok(());
        }
        if flags & BPF_F_ALLOW_MULTI == 0 && !direct.is_empty() {
            return Err(EBUSY);
        }
        if direct
            .iter()
            .any(|slot| unsafe { read_u32(*slot, ATTACHMENT_PROGRAM_ID) } == id)
        {
            return Err(EEXIST);
        }
        let slot = free.ok_or(ENOSPC)?;
        unsafe {
            core::ptr::write_bytes(slot, 0, ATTACHMENT_SIZE);
            write_u32(slot, ATTACHMENT_PROGRAM_ID, id);
            write_u32(slot, ATTACHMENT_TYPE, attach_type);
            write_u32(slot, ATTACHMENT_FLAGS, flags);
            write_u32(slot, ATTACHMENT_PATH_LENGTH, path.len() as u32);
            core::ptr::copy_nonoverlapping(path.as_ptr(), slot.add(ATTACHMENT_PATH), path.len());
            write_u32(slot, ATTACHMENT_STATE, 1);
        }
        Ok(())
    })
}

pub fn detach(target_fd: i32, program_fd: i32, attach_type: u32) -> Result<(), i32> {
    if attach_type != BPF_CGROUP_DEVICE {
        return Err(EOPNOTSUPP);
    }
    let path = cgroup_path_from_fd(target_fd)?;
    let id = program_id_for_fd(program_fd)?;
    with_registry(|base| {
        for index in 0..ATTACHMENT_CAPACITY {
            let slot = unsafe { attachment_slot(base, index) };
            if unsafe { read_u32(slot, ATTACHMENT_STATE) } != 0
                && unsafe { read_u32(slot, ATTACHMENT_PROGRAM_ID) } == id
                && unsafe { read_u32(slot, ATTACHMENT_TYPE) } == attach_type
                && unsafe { attachment_path(slot)? } == path
            {
                unsafe { core::ptr::write_bytes(slot, 0, ATTACHMENT_SIZE) };
                unsafe { cleanup_program(base, id) };
                return Ok(());
            }
        }
        Err(ENOENT)
    })
}

pub fn query(target_fd: i32, attach_type: u32, query_flags: u32) -> Result<(Vec<u32>, u32), i32> {
    if attach_type != BPF_CGROUP_DEVICE {
        return Err(EOPNOTSUPP);
    }
    if query_flags != 0 {
        return Err(EINVAL);
    }
    let path = cgroup_path_from_fd(target_fd)?;
    with_registry(|base| {
        let mut ids = Vec::new();
        let mut common_flags = None;
        for index in 0..ATTACHMENT_CAPACITY {
            let slot = unsafe { attachment_slot(base, index) };
            if unsafe { read_u32(slot, ATTACHMENT_STATE) } != 0
                && unsafe { read_u32(slot, ATTACHMENT_TYPE) } == attach_type
                && unsafe { attachment_path(slot)? } == path
            {
                ids.push(unsafe { read_u32(slot, ATTACHMENT_PROGRAM_ID) });
                let flags = unsafe { read_u32(slot, ATTACHMENT_FLAGS) };
                common_flags = Some(common_flags.map_or(flags, |old| old & flags));
            }
        }
        Ok((ids, common_flags.unwrap_or(0)))
    })
}

pub fn remove_cgroup(path: &str) -> Result<(), i32> {
    let clean = path.trim_end_matches('/');
    with_registry(|base| {
        let mut removed = Vec::new();
        for index in 0..ATTACHMENT_CAPACITY {
            let slot = unsafe { attachment_slot(base, index) };
            if unsafe { read_u32(slot, ATTACHMENT_STATE) } != 0
                && unsafe { attachment_path(slot)? } == clean
            {
                removed.push(unsafe { read_u32(slot, ATTACHMENT_PROGRAM_ID) });
                unsafe { core::ptr::write_bytes(slot, 0, ATTACHMENT_SIZE) };
            }
        }
        for id in removed {
            unsafe { cleanup_program(base, id) };
        }
        Ok(())
    })
}

/// A registry reference outside the guest fd table, retained during SCM export.
pub(crate) struct RightsPin {
    id: u32,
    slot: usize,
    pid: u32,
}
impl RightsPin {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }
}
impl Drop for RightsPin {
    fn drop(&mut self) {
        let _ = with_registry(|base| {
            let slot = unsafe { fd_slot(base, self.slot) };
            if unsafe { read_u32(slot, FD_STATE) } == 2
                && unsafe { read_u32(slot, FD_PID) } == self.pid
                && unsafe { read_u32(slot, FD_PROGRAM_ID) } == self.id
            {
                unsafe {
                    core::ptr::write_bytes(slot, 0, FD_SIZE);
                    cleanup_program(base, self.id);
                }
            }
            Ok(())
        });
    }
}
pub(crate) fn rights_reference(fd: i32) -> Result<RightsPin, i32> {
    let id = program_id_for_fd(fd)?;
    let pid = current_pid();
    with_registry(|base| {
        if unsafe { find_program(base, id, true) }.is_none() {
            return Err(EBADF);
        }
        for index in 0..FD_CAPACITY {
            let slot = unsafe { fd_slot(base, index) };
            if unsafe { read_u32(slot, FD_STATE) } == 0 {
                unsafe {
                    write_u32(slot, FD_PID, pid);
                    write_u32(slot, FD_NUMBER, u32::MAX);
                    write_u32(slot, FD_PROGRAM_ID, id);
                    write_u32(slot, FD_STATE, 2);
                }
                return Ok(RightsPin {
                    id,
                    slot: index,
                    pid,
                });
            }
        }
        Err(EMFILE)
    })
}
pub(crate) fn import_rights(fd: i32, id: u32) -> Result<(), i32> {
    if crate::get(fd)?.kind != FdKind::BpfProgram {
        return Err(EBADF);
    }
    let pid = current_pid();
    with_registry(|base| unsafe { add_fd_reference(base, pid, fd, id) })
}

pub fn duplicate(oldfd: i32, newfd: i32) -> Result<(), i32> {
    let id = program_id_for_fd(oldfd)?;
    let pid = current_pid();
    with_registry(|base| unsafe { add_fd_reference(base, pid, newfd, id) })
}

pub fn close(fd: i32) {
    let pid = current_pid();
    let _ = with_registry(|base| {
        for index in 0..FD_CAPACITY {
            let slot = unsafe { fd_slot(base, index) };
            if unsafe { read_u32(slot, FD_STATE) } != 0
                && unsafe { read_u32(slot, FD_PID) } == pid
                && unsafe { read_u32(slot, FD_NUMBER) } == fd as u32
            {
                let id = unsafe { read_u32(slot, FD_PROGRAM_ID) };
                unsafe { core::ptr::write_bytes(slot, 0, FD_SIZE) };
                unsafe { cleanup_program(base, id) };
                return Ok(());
            }
        }
        Ok(())
    });
}

pub fn serialize_matching(mut keep: impl FnMut(i32) -> bool) -> Result<Vec<u8>, i32> {
    let pid = current_pid();
    with_registry(|base| {
        let mut records = Vec::new();
        for index in 0..FD_CAPACITY {
            let slot = unsafe { fd_slot(base, index) };
            let fd = unsafe { read_u32(slot, FD_NUMBER) } as i32;
            if unsafe { read_u32(slot, FD_STATE) } != 0
                && unsafe { read_u32(slot, FD_PID) } == pid
                && fd >= 0
                && keep(fd)
            {
                records.push((fd, unsafe { read_u32(slot, FD_PROGRAM_ID) }));
            }
        }
        let mut payload = Vec::with_capacity(4 + records.len() * 8);
        payload.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for (fd, id) in records {
            payload.extend_from_slice(&fd.to_le_bytes());
            payload.extend_from_slice(&id.to_le_bytes());
        }
        Ok(payload)
    })
}

pub fn restore(payload: &[u8]) -> bool {
    let Some(count) = payload
        .get(..4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
    else {
        return false;
    };
    let Some(expected) = (count as usize)
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(4))
    else {
        return false;
    };
    if payload.len() != expected {
        return false;
    }
    let mut records = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let at = 4 + index * 8;
        let fd = i32::from_le_bytes(payload[at..at + 4].try_into().unwrap());
        let id = u32::from_le_bytes(payload[at + 4..at + 8].try_into().unwrap());
        if fd < 0
            || records.iter().any(|(old, _)| *old == fd)
            || crate::get(fd).is_err()
            || crate::get(fd).is_ok_and(|entry| entry.kind != FdKind::BpfProgram)
        {
            return false;
        }
        records.push((fd, id));
    }
    let pid = current_pid();
    with_registry(|base| {
        let mut previous = Vec::new();
        for index in 0..FD_CAPACITY {
            let slot = unsafe { fd_slot(base, index) };
            if unsafe { read_u32(slot, FD_STATE) } != 0 && unsafe { read_u32(slot, FD_PID) } == pid
            {
                previous.push(unsafe { read_u32(slot, FD_PROGRAM_ID) });
                unsafe { core::ptr::write_bytes(slot, 0, FD_SIZE) };
            }
        }
        for (fd, id) in &records {
            unsafe { add_fd_reference(base, pid, *fd, *id) }?;
        }
        for id in previous {
            unsafe { cleanup_program(base, id) };
        }
        Ok(())
    })
    .is_ok()
}

fn execute(bytes: &[u8], context: [u32; 3]) -> Result<u64, i32> {
    let count = bytes.len() / INSN_SIZE;
    let mut registers = [0u64; 11];
    registers[1] = 1;
    let mut pc = 0usize;
    while pc < count {
        let insn = decode(bytes, pc);
        let class = insn.code & 0x07;
        match class {
            0x01 => {
                let offset = usize::try_from(insn.offset).map_err(|_| EIO)?;
                let mut raw = [0u8; 12];
                raw[0..4].copy_from_slice(&context[0].to_le_bytes());
                raw[4..8].copy_from_slice(&context[1].to_le_bytes());
                raw[8..12].copy_from_slice(&context[2].to_le_bytes());
                let value = match insn.code & 0x18 {
                    0x00 => u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as u64,
                    0x08 => u16::from_le_bytes(raw[offset..offset + 2].try_into().unwrap()) as u64,
                    0x10 => raw[offset] as u64,
                    0x18 => u64::from_le_bytes(raw[offset..offset + 8].try_into().unwrap()),
                    _ => return Err(EIO),
                };
                registers[insn.dst as usize] = value;
            }
            0x04 | 0x07 => {
                let width32 = class == 0x04;
                let dst = insn.dst as usize;
                let source = if insn.code & 0x08 != 0 {
                    registers[insn.src as usize]
                } else {
                    insn.immediate as i64 as u64
                };
                let current = registers[dst];
                let operation = insn.code & 0xf0;
                let value = if width32 {
                    let left = current as u32;
                    let right = source as u32;
                    (match operation {
                        0x00 => left.wrapping_add(right),
                        0x10 => left.wrapping_sub(right),
                        0x20 => left.wrapping_mul(right),
                        0x30 => {
                            if right == 0 {
                                0
                            } else {
                                left / right
                            }
                        }
                        0x40 => left | right,
                        0x50 => left & right,
                        0x60 => left.wrapping_shl(right & 31),
                        0x70 => left.wrapping_shr(right & 31),
                        0x80 => left.wrapping_neg(),
                        0x90 => {
                            if right == 0 {
                                left
                            } else {
                                left % right
                            }
                        }
                        0xa0 => left ^ right,
                        0xb0 => right,
                        0xc0 => ((left as i32) >> (right & 31)) as u32,
                        _ => return Err(EIO),
                    }) as u64
                } else {
                    match operation {
                        0x00 => current.wrapping_add(source),
                        0x10 => current.wrapping_sub(source),
                        0x20 => current.wrapping_mul(source),
                        0x30 => {
                            if source == 0 {
                                0
                            } else {
                                current / source
                            }
                        }
                        0x40 => current | source,
                        0x50 => current & source,
                        0x60 => current.wrapping_shl((source & 63) as u32),
                        0x70 => current.wrapping_shr((source & 63) as u32),
                        0x80 => current.wrapping_neg(),
                        0x90 => {
                            if source == 0 {
                                current
                            } else {
                                current % source
                            }
                        }
                        0xa0 => current ^ source,
                        0xb0 => source,
                        0xc0 => ((current as i64) >> (source & 63)) as u64,
                        _ => return Err(EIO),
                    }
                };
                registers[dst] = value;
            }
            0x05 | 0x06 => {
                let operation = insn.code & 0xf0;
                if operation == 0x90 {
                    return Ok(registers[0]);
                }
                let width32 = class == 0x06;
                let left = registers[insn.dst as usize];
                let right = if insn.code & 0x08 != 0 {
                    registers[insn.src as usize]
                } else {
                    insn.immediate as i64 as u64
                };
                let taken = match operation {
                    0x00 => true,
                    0x10 => {
                        if width32 {
                            left as u32 == right as u32
                        } else {
                            left == right
                        }
                    }
                    0x20 => {
                        if width32 {
                            left as u32 > right as u32
                        } else {
                            left > right
                        }
                    }
                    0x30 => {
                        if width32 {
                            left as u32 >= right as u32
                        } else {
                            left >= right
                        }
                    }
                    0x40 => {
                        if width32 {
                            left as u32 & right as u32 != 0
                        } else {
                            left & right != 0
                        }
                    }
                    0x50 => {
                        if width32 {
                            left as u32 != right as u32
                        } else {
                            left != right
                        }
                    }
                    0x60 => {
                        if width32 {
                            left as i32 > right as i32
                        } else {
                            left as i64 > right as i64
                        }
                    }
                    0x70 => {
                        if width32 {
                            left as i32 >= right as i32
                        } else {
                            left as i64 >= right as i64
                        }
                    }
                    0xa0 => {
                        if width32 {
                            (left as u32) < right as u32
                        } else {
                            left < right
                        }
                    }
                    0xb0 => {
                        if width32 {
                            (left as u32) <= right as u32
                        } else {
                            left <= right
                        }
                    }
                    0xc0 => {
                        if width32 {
                            (left as i32) < right as i32
                        } else {
                            (left as i64) < right as i64
                        }
                    }
                    0xd0 => {
                        if width32 {
                            (left as i32) <= right as i32
                        } else {
                            (left as i64) <= right as i64
                        }
                    }
                    _ => return Err(EIO),
                };
                if taken {
                    pc = jump_target(pc, insn.offset, count).map_err(|_| EIO)?;
                    continue;
                }
            }
            _ => return Err(EIO),
        }
        pc += 1;
    }
    Err(EIO)
}

fn is_ancestor(ancestor: &str, member: &str) -> bool {
    ancestor == member
        || ancestor == "/sys/fs/cgroup" && member.starts_with("/sys/fs/cgroup/")
        || member
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn check_current_device(
    device_type: u32,
    major: u32,
    minor: u32,
    access: u32,
) -> Result<(), i32> {
    if !matches!(device_type, BPF_DEVCG_DEV_BLOCK | BPF_DEVCG_DEV_CHAR)
        || access == 0
        || access & !(BPF_DEVCG_ACC_MKNOD | BPF_DEVCG_ACC_READ | BPF_DEVCG_ACC_WRITE) != 0
    {
        return Err(EINVAL);
    }
    let member = kinakaze_runtime::job::cgroup_path(current_pid())
        .unwrap_or_else(|| "/sys/fs/cgroup".to_string());
    let programs = with_registry(|base| {
        let mut programs = Vec::new();
        for index in 0..ATTACHMENT_CAPACITY {
            let attachment = unsafe { attachment_slot(base, index) };
            if unsafe { read_u32(attachment, ATTACHMENT_STATE) } == 0
                || unsafe { read_u32(attachment, ATTACHMENT_TYPE) } != BPF_CGROUP_DEVICE
            {
                continue;
            }
            let path = unsafe { attachment_path(attachment)? };
            if !is_ancestor(&path, &member) {
                continue;
            }
            let id = unsafe { read_u32(attachment, ATTACHMENT_PROGRAM_ID) };
            let program = unsafe { find_program(base, id, false) }.ok_or(EIO)?;
            let count = unsafe { read_u32(program, PROGRAM_INSN_COUNT) } as usize;
            if count > MAX_INSNS {
                return Err(EIO);
            }
            let bytes = unsafe {
                core::slice::from_raw_parts(program.add(PROGRAM_BYTECODE), count * INSN_SIZE)
            };
            programs.push(bytes.to_vec());
        }
        Ok(programs)
    })?;
    let context = [(device_type & 0xffff) | (access << 16), major, minor];
    for program in programs {
        if execute(&program, context)? == 0 {
            return Err(EPERM);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insn(code: u8, dst: u8, src: u8, offset: i16, immediate: i32) -> [u8; 8] {
        let mut raw = [0u8; 8];
        raw[0] = code;
        raw[1] = dst | (src << 4);
        raw[2..4].copy_from_slice(&offset.to_le_bytes());
        raw[4..8].copy_from_slice(&immediate.to_le_bytes());
        raw
    }

    #[test]
    fn verifier_and_interpreter_execute_a_device_decision() {
        let mut program = Vec::new();
        program.extend_from_slice(&insn(0x61, 2, 1, 0, 0));
        program.extend_from_slice(&insn(0x54, 2, 0, 0, 0xffff));
        program.extend_from_slice(&insn(0x55, 2, 0, 2, BPF_DEVCG_DEV_CHAR as i32));
        program.extend_from_slice(&insn(0xb4, 0, 0, 0, 1));
        program.extend_from_slice(&insn(0x95, 0, 0, 0, 0));
        program.extend_from_slice(&insn(0xb4, 0, 0, 0, 0));
        program.extend_from_slice(&insn(0x95, 0, 0, 0, 0));
        verify(BPF_PROG_TYPE_CGROUP_DEVICE, &program).unwrap();
        assert_eq!(execute(&program, [BPF_DEVCG_DEV_CHAR, 1, 3]).unwrap(), 1);
        assert_eq!(execute(&program, [BPF_DEVCG_DEV_BLOCK, 8, 0]).unwrap(), 0);
    }

    #[test]
    fn verifier_rejects_unknown_instructions_instead_of_accepting_them() {
        let mut program = Vec::new();
        program.extend_from_slice(&insn(0xff, 0, 0, 0, 0));
        program.extend_from_slice(&insn(0x95, 0, 0, 0, 0));
        assert_eq!(
            verify(BPF_PROG_TYPE_CGROUP_DEVICE, &program)
                .unwrap_err()
                .errno,
            EACCES
        );
    }

    #[test]
    fn program_object_attachment_and_device_decision_share_one_lifetime() {
        let path = format!(
            "/sys/fs/cgroup/kinakaze-bpf-test-{}-{}",
            std::process::id(),
            unsafe { GetTickCount64() }
        );
        crate::cgroup::create_directory(&path).unwrap();
        let directory =
            crate::fs::open(&path, crate::fs::O_RDONLY | crate::fs::O_DIRECTORY, 0).unwrap();
        assert!(kinakaze_runtime::job::set_cgroup_path(current_pid(), &path));

        let mut instructions = Vec::new();
        instructions.extend_from_slice(&insn(0xb4, 0, 0, 0, 0));
        instructions.extend_from_slice(&insn(0x95, 0, 0, 0, 0));
        let program = load_program(ProgramLoad {
            program_type: BPF_PROG_TYPE_CGROUP_DEVICE,
            expected_attach_type: 0,
            flags: 0,
            instructions,
            license: "MIT".to_string(),
            name: [0; PROGRAM_NAME_CAPACITY],
        })
        .unwrap();
        let id = program_info(program).unwrap().id;

        attach(
            directory,
            program,
            BPF_CGROUP_DEVICE,
            BPF_F_ALLOW_MULTI,
            None,
        )
        .unwrap();
        assert_eq!(query(directory, BPF_CGROUP_DEVICE, 0).unwrap().0, [id]);
        crate::close(program).unwrap();
        assert_eq!(
            check_current_device(BPF_DEVCG_DEV_CHAR, 1, 3, BPF_DEVCG_ACC_READ),
            Err(EPERM)
        );

        let reopened = get_fd_by_id(id).unwrap();
        detach(directory, reopened, BPF_CGROUP_DEVICE).unwrap();
        assert_eq!(
            check_current_device(BPF_DEVCG_DEV_CHAR, 1, 3, BPF_DEVCG_ACC_READ),
            Ok(())
        );
        crate::close(reopened).unwrap();
        assert_eq!(get_fd_by_id(id), Err(ENOENT));

        assert!(kinakaze_runtime::job::set_cgroup_path(
            current_pid(),
            "/sys/fs/cgroup"
        ));
        crate::close(directory).unwrap();
        crate::cgroup::remove_directory(&path).unwrap();
    }
}
