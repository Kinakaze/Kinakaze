//! In-place detours for Linux FS-relative instructions.
//!
//! A five-byte near jump replaces one or more whole instructions. The copied
//! block is re-encoded by iced-x86 at its new address, preceded by an FS-base
//! restore from the current thread's direct TEB slot, and followed by an absolute
//! indirect jump back. The same installer is used by AOT and the one-shot VEH
//! fallback, so a recovered site never needs to fault a second time.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, Encoder, FlowControl, Instruction,
    InstructionBlock, InstructionInfoFactory, Register,
};

use super::ExecutionError;

const RUN_SIZE: usize = 64 * 1024;
// Keep every slot power-of-two sized: the full syscall bridge carries nested
// guest context plus Windows fibre stack bounds, and still needs room for a
// relocated instruction and the absolute continuation jump.
const STUB_SIZE: usize = 1024;
const MIN_DETOUR_SIZE: usize = 5;
const MAX_SNAPSHOT: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct InstructionTrampoline {
    #[cfg(test)]
    pub trampoline: usize,
    pub overwritten: usize,
}

/// A writable ELF segment that cannot execute until preparation succeeds.
/// This explicit capability never applies to the VEH/runtime patch path.
pub(super) struct UnpublishedCode {
    start: usize,
    length: usize,
}

impl UnpublishedCode {
    /// The whole range must be writable and unpublished to guest execution.
    pub(super) unsafe fn new(start: usize, length: usize) -> Self {
        Self { start, length }
    }

    fn contains(&self, address: usize, length: usize) -> bool {
        address.checked_sub(self.start).is_some_and(|offset| {
            offset
                .checked_add(length)
                .is_some_and(|end| end <= self.length)
        })
    }

    unsafe fn snapshot(&self, address: usize) -> Result<&[u8], ExecutionError> {
        let offset = address
            .checked_sub(self.start)
            .ok_or(ExecutionError::AddressOverflow)?;
        let length = self
            .length
            .checked_sub(offset)
            .ok_or(ExecutionError::AddressOverflow)?;
        Ok(unsafe { std::slice::from_raw_parts(address as *const u8, length.min(MAX_SNAPSHOT)) })
    }

    /// Publish both the source instructions and their separately allocated stubs.
    pub(super) fn finish(&self) -> Result<(), ExecutionError> {
        #[cfg(windows)]
        if unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                core::ptr::null(),
                0,
            )
        } == 0
        {
            return Err(ExecutionError::ProtectionFailed {
                object: "<prepared instruction cache>".to_owned(),
            });
        }
        Ok(())
    }
}

/// Length of the first valid FS-relative instruction at `address`.
///
/// # Safety
///
/// `address` must name readable executable memory in the current process.
pub unsafe fn first_fs_instruction_len(address: usize) -> Option<usize> {
    if address == 0 {
        return None;
    }
    let snapshot = unsafe { std::slice::from_raw_parts(address as *const u8, 15) };
    let mut decoder = Decoder::with_ip(64, snapshot, address as u64, DecoderOptions::NONE);
    let instruction = decoder.decode();
    (!instruction.is_invalid() && instruction.segment_prefix() == Register::FS)
        .then_some(instruction.len())
}

/// Returns the complete source span required by a five-byte detour beginning at
/// an FS-relative instruction. The caller is responsible for proving that no
/// independent control-flow entry lands in the additional whole instructions.
pub(crate) fn fs_detour_length(code: &[u8], offset: usize, address: usize) -> Option<usize> {
    let bytes = code.get(offset..)?;
    decode_fs_detour(bytes, address).map(|(_, _, overwritten)| overwritten)
}

fn decode_fs_detour(
    bytes: &[u8],
    address: usize,
) -> Option<((Instruction, Register), Vec<Instruction>, usize)> {
    let mut decoder = Decoder::with_ip(64, bytes, address as u64, DecoderOptions::NONE);
    let first = decoder.decode();
    if first.is_invalid() || first.segment_prefix() != Register::FS {
        return None;
    }
    // Validate translation while the object is still pristine. This keeps the
    // planner and installer exact and prevents a partially patched object when
    // an FS addressing form cannot be represented by the trampoline.
    let translated = translate_fs_instruction(first).ok()?;

    let mut trailing_instructions = Vec::new();
    let mut overwritten = first.len();
    while overwritten < MIN_DETOUR_SIZE {
        let instruction = decoder.decode();
        if instruction.is_invalid() || instruction.len() == 0 {
            return None;
        }
        overwritten = overwritten.checked_add(instruction.len())?;
        trailing_instructions.push(instruction);
    }
    Some((translated, trailing_instructions, overwritten))
}

#[derive(Clone, Copy)]
struct Run {
    base: usize,
    used: usize,
}

fn runs() -> &'static Mutex<Vec<Run>> {
    static RUNS: OnceLock<Mutex<Vec<Run>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(Vec::new()))
}

fn syscall_continuations() -> &'static Mutex<HashMap<usize, usize>> {
    static CONTINUATIONS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    CONTINUATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
/// Translates the architectural address immediately following a raw `syscall`
/// to the relocated copy of any instructions consumed by its five-byte detour.
/// Returns-twice operations such as `clone` resume through this address, while
/// ordinary returns reach the same bytes through the dispatcher stub.
pub fn translated_syscall_continuation(logical: usize) -> Option<usize> {
    syscall_continuations()
        .lock()
        .ok()
        .and_then(|continuations| continuations.get(&logical).copied())
}

#[cfg(test)]
/// Installs a permanent trampoline at an FS-relative instruction.
///
/// # Safety
///
/// `address` must be a live instruction pointer in the current process.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install(
    address: usize,
    thread_pointer_teb_offset: usize,
    scratch_teb_offset: usize,
) -> Result<InstructionTrampoline, ExecutionError> {
    unsafe { install_impl(address, thread_pointer_teb_offset, scratch_teb_offset, None) }
}

pub(super) unsafe fn install_unpublished(
    address: usize,
    thread_pointer_teb_offset: usize,
    scratch_teb_offset: usize,
    unpublished: &UnpublishedCode,
) -> Result<InstructionTrampoline, ExecutionError> {
    let result = unsafe {
        install_impl(
            address,
            thread_pointer_teb_offset,
            scratch_teb_offset,
            Some(unpublished),
        )
    };
    result.map_err(|error| ExecutionError::InstructionPatch {
        address,
        detail: format!("TLS: {error:?}"),
    })
}

unsafe fn install_impl(
    address: usize,
    thread_pointer_teb_offset: usize,
    _scratch_teb_offset: usize,
    unpublished: Option<&UnpublishedCode>,
) -> Result<InstructionTrampoline, ExecutionError> {
    use kinakaze_runtime::memory_protection::protect_preserving_copy_on_write as VirtualProtect;
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::PAGE_EXECUTE_READWRITE;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    if address == 0 {
        return Err(ExecutionError::AddressOverflow);
    }
    let process = unsafe { GetCurrentProcess() };
    let snapshot = match unpublished {
        Some(range) => unsafe { range.snapshot(address)? },
        None => unsafe { std::slice::from_raw_parts(address as *const u8, MAX_SNAPSHOT) },
    };
    let ((translated, scratch), trailing_instructions, overwritten) =
        decode_fs_detour(snapshot, address).ok_or(ExecutionError::AddressOverflow)?;
    if unpublished.is_some_and(|range| !range.contains(address, overwritten)) {
        return Err(ExecutionError::AddressOverflow);
    }

    let trampoline = allocate_near(address)?;
    let mut code = Vec::with_capacity(STUB_SIZE);
    // Linux leaf functions own the 128 bytes below RSP. Save our scratch below
    // that red zone, without changing the instruction's incoming condition flags.
    code.extend_from_slice(&[0x48, 0x8d, 0x64, 0x24, 0x80]); // lea rsp,[rsp-128]
    emit_push_gpr(&mut code, scratch)?;
    emit_load_gpr_from_gs(&mut code, scratch, thread_pointer_teb_offset)?;
    let translated_ip = trampoline
        .checked_add(code.len())
        .ok_or(ExecutionError::AddressOverflow)? as u64;
    // Translation accepts only straight-line instructions and replaces the
    // memory base/index, so no branch relaxation or block graph is needed.
    // Give Encoder the existing buffer to avoid a second allocation and copy.
    let mut encoder = Encoder::new(64);
    encoder.set_buffer(code);
    encoder
        .encode(&translated, translated_ip)
        .map_err(|_| ExecutionError::AddressOverflow)?;
    let mut code = encoder.take_buffer();
    emit_pop_gpr(&mut code, scratch)?;
    code.extend_from_slice(&[0x48, 0x8d, 0xa4, 0x24, 0x80, 0, 0, 0]); // lea rsp,[rsp+128]
    if !trailing_instructions.is_empty() {
        let trailing_ip = trampoline
            .checked_add(code.len())
            .ok_or(ExecutionError::AddressOverflow)? as u64;
        let trailing = BlockEncoder::encode(
            64,
            InstructionBlock::new(&trailing_instructions, trailing_ip),
            BlockEncoderOptions::NONE,
        )
        .map_err(|_| ExecutionError::AddressOverflow)?;
        code.extend_from_slice(&trailing.code_buffer);
    }
    emit_absolute_jump(
        &mut code,
        address
            .checked_add(overwritten)
            .ok_or(ExecutionError::AddressOverflow)?,
    );
    if code.len() > STUB_SIZE {
        return Err(ExecutionError::AddressOverflow);
    }
    if std::env::var("KINAKAZE_TRACE_AOT_SITE")
        .ok()
        .and_then(|value| usize::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
        == Some(address)
    {
        eprintln!(
            "kinakaze: AOT site {address:#x} original={:02x?} relocated={:02x?}",
            &snapshot[..overwritten],
            &code,
        );
    }
    // SAFETY: `allocate_near` returned a private RWX slot of exactly STUB_SIZE.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), trampoline as *mut u8, code.len()) };
    finish_slot(trampoline, code.len());
    if unpublished.is_none() {
        unsafe {
            FlushInstructionCache(process, trampoline as *const core::ffi::c_void, code.len())
        };
    }

    let relative = trampoline as i128 - (address + MIN_DETOUR_SIZE) as i128;
    let relative = i32::try_from(relative).map_err(|_| ExecutionError::AddressOverflow)?;
    let mut detour = vec![0x90; overwritten];
    detour[0] = 0xe9;
    detour[1..5].copy_from_slice(&relative.to_le_bytes());

    let mut old_protection = 0u32;
    if unpublished.is_none()
        && unsafe {
            VirtualProtect(
                address as *const core::ffi::c_void,
                overwritten,
                PAGE_EXECUTE_READWRITE,
                &raw mut old_protection,
            )
        } == 0
    {
        return Err(ExecutionError::ProtectionFailed {
            object: "<instruction trampoline source>".to_owned(),
        });
    }
    // Publish the opcode last. A thread racing this patch sees either the old FS
    // prefix and enters VEH, or a complete E9 detour.
    unsafe {
        std::ptr::copy_nonoverlapping(
            detour.as_ptr().add(1),
            (address + 1) as *mut u8,
            detour.len() - 1,
        );
        (address as *mut u8).write_volatile(detour[0]);
        if unpublished.is_none() {
            FlushInstructionCache(process, address as *const core::ffi::c_void, overwritten);
        }
    }
    let mut ignored = 0u32;
    if unpublished.is_none()
        && unsafe {
            VirtualProtect(
                address as *const core::ffi::c_void,
                overwritten,
                old_protection,
                &raw mut ignored,
            )
        } == 0
    {
        return Err(ExecutionError::ProtectionFailed {
            object: "<instruction trampoline source>".to_owned(),
        });
    }

    Ok(InstructionTrampoline {
        #[cfg(test)]
        trampoline,
        overwritten,
    })
}

/// Converts one FS-relative memory instruction into an equivalent ordinary
/// memory instruction whose extra base register holds the Linux thread pointer.
/// The selected register is not read or written by the original instruction.
fn translate_fs_instruction(
    mut instruction: Instruction,
) -> Result<(Instruction, Register), ExecutionError> {
    if instruction.segment_prefix() != Register::FS
        || instruction.flow_control() != FlowControl::Next
    {
        return Err(ExecutionError::AddressOverflow);
    }

    let mut info_factory = InstructionInfoFactory::new();
    let info = info_factory.info(&instruction);
    let candidates = [
        Register::R11,
        Register::R10,
        Register::R9,
        Register::R8,
        Register::RAX,
        Register::RCX,
        Register::RDX,
        Register::RBX,
        Register::R12,
        Register::R13,
        Register::R14,
        Register::R15,
        Register::RSI,
        Register::RDI,
        Register::RBP,
    ];
    let scratch = candidates
        .into_iter()
        .find(|candidate| {
            !info
                .used_registers()
                .iter()
                .any(|used| used.register().full_register() == *candidate)
        })
        .ok_or(ExecutionError::AddressOverflow)?;

    let base = instruction.memory_base();
    let index = instruction.memory_index();
    if base == Register::None {
        instruction.set_memory_base(scratch);
    } else if index == Register::None && !matches!(base, Register::RIP | Register::EIP) {
        instruction.set_memory_index(scratch);
        instruction.set_memory_index_scale(1);
        if base == Register::RSP {
            // The trampoline temporarily pushes the scratch register. Preserve
            // an original RSP-relative effective address across that push.
            instruction
                .set_memory_displacement64(instruction.memory_displacement64().wrapping_add(136));
        }
    } else {
        return Err(ExecutionError::AddressOverflow);
    }
    instruction.set_segment_prefix(Register::None);
    Ok((instruction, scratch))
}

fn gpr_number(register: Register) -> Option<u8> {
    Some(match register {
        Register::RAX => 0,
        Register::RCX => 1,
        Register::RDX => 2,
        Register::RBX => 3,
        Register::RSP => 4,
        Register::RBP => 5,
        Register::RSI => 6,
        Register::RDI => 7,
        Register::R8 => 8,
        Register::R9 => 9,
        Register::R10 => 10,
        Register::R11 => 11,
        Register::R12 => 12,
        Register::R13 => 13,
        Register::R14 => 14,
        Register::R15 => 15,
        _ => return None,
    })
}

fn emit_push_gpr(code: &mut Vec<u8>, register: Register) -> Result<(), ExecutionError> {
    let number = gpr_number(register).ok_or(ExecutionError::AddressOverflow)?;
    if number >= 8 {
        code.push(0x41);
    }
    code.push(0x50 + (number & 7));
    Ok(())
}

fn emit_pop_gpr(code: &mut Vec<u8>, register: Register) -> Result<(), ExecutionError> {
    let number = gpr_number(register).ok_or(ExecutionError::AddressOverflow)?;
    if number >= 8 {
        code.push(0x41);
    }
    code.push(0x58 + (number & 7));
    Ok(())
}

fn emit_load_gpr_from_gs(
    code: &mut Vec<u8>,
    register: Register,
    displacement: usize,
) -> Result<(), ExecutionError> {
    let number = gpr_number(register).ok_or(ExecutionError::AddressOverflow)?;
    let displacement = u32::try_from(displacement).map_err(|_| ExecutionError::AddressOverflow)?;
    code.push(0x65); // GS segment override
    code.push(0x48 | ((number >= 8) as u8) << 2); // REX.W plus R
    code.push(0x8b); // mov r64,r/m64
    code.push(0x04 | ((number & 7) << 3)); // ModRM: reg, [SIB+disp32]
    code.push(0x25); // SIB: no index, no base
    code.extend_from_slice(&displacement.to_le_bytes());
    Ok(())
}

#[cfg(test)]
/// Installs a permanent trampoline for a raw Linux `syscall` instruction.
///
/// Overwrites the 2-byte `syscall` plus any necessary trailing instructions up to >= 5 bytes
/// with an `E9 <rel32>` near jump to a trampoline stub. The stub marshals the Linux syscall
/// arguments (rax, rdi, rsi, rdx, r10, r8, r9) to SysV ABI, calls `syscall_dispatcher`,
/// puts the return code in `rax`, executes the relocated trailing instructions, and jumps back.
///
/// # Safety
///
/// `address` must point at a live `syscall` instruction in the current process.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install_syscall(
    address: usize,
    syscall_dispatcher: usize,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
) -> Result<InstructionTrampoline, ExecutionError> {
    unsafe {
        install_syscall_impl(
            address,
            syscall_dispatcher,
            thread_pointer_teb_offset,
            host_transition_teb_offset,
            None,
        )
    }
}

pub(super) unsafe fn install_syscall_unpublished(
    address: usize,
    syscall_dispatcher: usize,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
    unpublished: &UnpublishedCode,
) -> Result<InstructionTrampoline, ExecutionError> {
    let result = unsafe {
        install_syscall_impl(
            address,
            syscall_dispatcher,
            thread_pointer_teb_offset,
            host_transition_teb_offset,
            Some(unpublished),
        )
    };
    result.map_err(|error| ExecutionError::InstructionPatch {
        address,
        detail: format!("syscall: {error:?}"),
    })
}

unsafe fn install_syscall_impl(
    address: usize,
    syscall_dispatcher: usize,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
    unpublished: Option<&UnpublishedCode>,
) -> Result<InstructionTrampoline, ExecutionError> {
    use kinakaze_runtime::memory_protection::protect_preserving_copy_on_write as VirtualProtect;
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::PAGE_EXECUTE_READWRITE;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    if address == 0 {
        return Err(ExecutionError::AddressOverflow);
    }
    let process = unsafe { GetCurrentProcess() };
    let snapshot = match unpublished {
        Some(range) => unsafe { range.snapshot(address)? },
        None => unsafe { std::slice::from_raw_parts(address as *const u8, MAX_SNAPSHOT) },
    };
    let mut decoder = Decoder::with_ip(64, snapshot, address as u64, DecoderOptions::NONE);
    let first = decoder.decode();
    if first.is_invalid() || first.mnemonic() != iced_x86::Mnemonic::Syscall {
        return Err(ExecutionError::AddressOverflow);
    }
    let mut trailing_instructions = Vec::new();
    let mut overwritten = first.len();
    while overwritten < MIN_DETOUR_SIZE {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(ExecutionError::AddressOverflow);
        }
        overwritten = overwritten
            .checked_add(instruction.len())
            .ok_or(ExecutionError::AddressOverflow)?;
        trailing_instructions.push(instruction);
    }

    if unpublished.is_some_and(|range| !range.contains(address, overwritten)) {
        return Err(ExecutionError::AddressOverflow);
    }
    let trampoline = allocate_near(address)?;
    let mut code = Vec::with_capacity(STUB_SIZE);
    let (relocated_return_immediate, frame_return_immediate) = emit_syscall_stub(
        &mut code,
        syscall_dispatcher,
        address
            .checked_add(first.len())
            .ok_or(ExecutionError::AddressOverflow)?,
        thread_pointer_teb_offset,
        host_transition_teb_offset,
    )?;

    let relocated_continuation = trampoline
        .checked_add(code.len())
        .ok_or(ExecutionError::AddressOverflow)?;
    for offset in [relocated_return_immediate, frame_return_immediate] {
        code[offset..offset + 8].copy_from_slice(&(relocated_continuation as u64).to_le_bytes());
    }

    if !trailing_instructions.is_empty() {
        let relocated_ip = relocated_continuation as u64;
        let relocated = BlockEncoder::encode(
            64,
            InstructionBlock::new(&trailing_instructions, relocated_ip),
            BlockEncoderOptions::NONE,
        )
        .map_err(|_| ExecutionError::AddressOverflow)?;
        code.extend_from_slice(&relocated.code_buffer);
    }

    emit_absolute_jump(
        &mut code,
        address
            .checked_add(overwritten)
            .ok_or(ExecutionError::AddressOverflow)?,
    );
    if code.len() > STUB_SIZE {
        return Err(ExecutionError::AddressOverflow);
    }

    // SAFETY: `allocate_near` returned a private RWX slot of exactly STUB_SIZE.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), trampoline as *mut u8, code.len()) };
    finish_slot(trampoline, code.len());
    if unpublished.is_none() {
        unsafe {
            FlushInstructionCache(process, trampoline as *const core::ffi::c_void, code.len())
        };
    }

    if std::env::var_os("KINAKAZE_FORK_TRACE").is_some() {
        eprintln!(
            "kinakaze: syscall trampoline source={address:#x} continuation={:#x} trampoline={trampoline:#x} tp_teb_offset={thread_pointer_teb_offset:?} transition_teb_offset={host_transition_teb_offset:?}",
            address + first.len(),
        );
    }

    let relative = trampoline as i128 - (address + MIN_DETOUR_SIZE) as i128;
    let relative = i32::try_from(relative).map_err(|_| ExecutionError::AddressOverflow)?;
    let mut detour = vec![0x90; overwritten];
    detour[0] = 0xe9;
    detour[1..5].copy_from_slice(&relative.to_le_bytes());

    let mut old_protection = 0u32;
    if unpublished.is_none()
        && unsafe {
            VirtualProtect(
                address as *const core::ffi::c_void,
                overwritten,
                PAGE_EXECUTE_READWRITE,
                &raw mut old_protection,
            )
        } == 0
    {
        return Err(ExecutionError::ProtectionFailed {
            object: "<syscall trampoline source>".to_owned(),
        });
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            detour.as_ptr().add(1),
            (address + 1) as *mut u8,
            detour.len() - 1,
        );
        (address as *mut u8).write_volatile(detour[0]);
        if unpublished.is_none() {
            FlushInstructionCache(process, address as *const core::ffi::c_void, overwritten);
        }
    }
    let mut ignored = 0u32;
    if unpublished.is_none()
        && unsafe {
            VirtualProtect(
                address as *const core::ffi::c_void,
                overwritten,
                old_protection,
                &raw mut ignored,
            )
        } == 0
    {
        return Err(ExecutionError::ProtectionFailed {
            object: "<syscall trampoline source>".to_owned(),
        });
    }

    syscall_continuations()
        .lock()
        .map_err(|_| ExecutionError::AddressOverflow)?
        .insert(address + first.len(), relocated_continuation);

    Ok(InstructionTrampoline {
        #[cfg(test)]
        trampoline,
        overwritten,
    })
}

#[cfg(test)]
#[cfg(not(all(windows, target_arch = "x86_64")))]
pub unsafe fn install_syscall(
    _: usize,
    _: usize,
    _: Option<usize>,
    _: Option<usize>,
) -> Result<InstructionTrampoline, ExecutionError> {
    Err(ExecutionError::MappingFailed {
        object: "<syscall trampoline>".to_owned(),
        len: STUB_SIZE,
    })
}

fn emit_syscall_stub(
    code: &mut Vec<u8>,
    dispatcher: usize,
    guest_instruction: usize,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
) -> Result<(usize, usize), ExecutionError> {
    use kinakaze_tls::thread_pointer::{
        ACTIVE_RAW_SYSCALL_FRAME_POINTER_OFFSET, ExtendedStateFormat,
        RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET, RAW_SYSCALL_FRAME_PREVIOUS_OFFSET,
        RAW_SYSCALL_FRAME_RAX_OFFSET, RAW_SYSCALL_FRAME_RESULT_OFFSET,
    };

    let extended_state = kinakaze_tls::thread_pointer::extended_state_format();
    let extended_state_size = match extended_state {
        ExtendedStateFormat::Fxsave => 512,
        ExtendedStateFormat::Xsave { size, .. } => size,
    };
    let frame_size = RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET
        .checked_add(extended_state_size)
        .and_then(|size| size.checked_add(63))
        .map(|size| size & !63usize)
        .ok_or(ExecutionError::AddressOverflow)?;
    if frame_size >= kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE {
        return Err(ExecutionError::MappingFailed {
            object: "<raw syscall extended-state frame>".to_owned(),
            len: frame_size,
        });
    }
    let frame_size = u32::try_from(frame_size).map_err(|_| ExecutionError::AddressOverflow)?;
    let active_raw_frame = u32::try_from(ACTIVE_RAW_SYSCALL_FRAME_POINTER_OFFSET)
        .map_err(|_| ExecutionError::AddressOverflow)?;
    let host_stack = u32::try_from(kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET)
        .map_err(|_| ExecutionError::AddressOverflow)?;
    let host_stack_used = u32::try_from(kinakaze_tls::thread_pointer::HOST_CALL_STACK_USED_OFFSET)
        .map_err(|_| ExecutionError::AddressOverflow)?;
    let host_stack_lane = u32::try_from(kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE)
        .map_err(|_| ExecutionError::AddressOverflow)?;
    let host_stack_last_lane = u32::try_from(
        kinakaze_tls::thread_pointer::HOST_CALL_STACK_SIZE
            - kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE,
    )
    .map_err(|_| ExecutionError::AddressOverflow)?;
    let active_guest_stack =
        u32::try_from(kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET)
            .map_err(|_| ExecutionError::AddressOverflow)?;
    let active_guest_instruction =
        u32::try_from(kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET)
            .map_err(|_| ExecutionError::AddressOverflow)?;
    let host_transition_teb_offset = u32::try_from(host_transition_teb_offset.ok_or(
        ExecutionError::InvalidTls {
            object: "<syscall host transition slot>".to_owned(),
        },
    )?)
    .map_err(|_| ExecutionError::AddressOverflow)?;

    // Windows may clear a user-written FS base across a kernel transition. The
    // loader also publishes the guest TCB in a direct TEB slot, so restore it
    // before the first FS-relative access. R11 is a Linux-syscall-clobbered
    // register and needs no scratch preservation here.
    if let Some(offset) = thread_pointer_teb_offset {
        let offset = u32::try_from(offset).map_err(|_| ExecutionError::AddressOverflow)?;
        code.extend_from_slice(&[0x65, 0x4c, 0x8b, 0x1c, 0x25]); // mov r11,gs:[offset]
        code.extend_from_slice(&offset.to_le_bytes());
        code.extend_from_slice(&[0xf3, 0x49, 0x0f, 0xae, 0xd3]); // wrfsbase r11
    }

    // Leave the managed/language-runtime stack before calling arbitrary Rust and
    // Win32 code. No guest memory below RSP is touched until the exact original
    // pointer is restored at the end.
    code.extend_from_slice(&[0x48, 0x89, 0xe1]); // mov rcx,rsp (syscall-clobbered)
    emit_load_host_transition(code, host_transition_teb_offset); // r11 = host state
    code.extend_from_slice(&[0x49, 0x81, 0xbb]); // cmp qword [r11+used],last lane
    code.extend_from_slice(&host_stack_used.to_le_bytes());
    code.extend_from_slice(&host_stack_last_lane.to_le_bytes());
    code.extend_from_slice(&[0x76, 0x02]); // jbe +2
    code.extend_from_slice(&[0x0f, 0x0b]); // ud2: strict nesting overflow
    code.extend_from_slice(&[0x49, 0x8b, 0xa3]); // mov rsp,[r11+host_stack]
    code.extend_from_slice(&host_stack.to_le_bytes());
    code.extend_from_slice(&[0x49, 0x2b, 0xa3]); // sub rsp,[r11+used]
    code.extend_from_slice(&host_stack_used.to_le_bytes());
    code.extend_from_slice(&[0x49, 0x81, 0x83]); // add qword [r11+used],lane
    code.extend_from_slice(&host_stack_used.to_le_bytes());
    code.extend_from_slice(&host_stack_lane.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x83, 0xe4, 0xc0]); // and rsp, -64
    code.extend_from_slice(&[0x48, 0x81, 0xec]); // sub rsp,frame_size
    code.extend_from_slice(&frame_size.to_le_bytes());

    // The seventh SysV argument lives at [rsp] before CALL. The remaining slots
    // retain guest state, the outer raw-frame pointer and the TEB stack limits
    // across the dispatcher.
    code.extend_from_slice(&[0x4c, 0x89, 0x0c, 0x24]); // mov [rsp],r9 (a6)
    code.extend_from_slice(&[0x48, 0x89, 0x4c, 0x24, 0x10]); // guest rsp
    // Preserve an outer syscall's published context, then expose this one.
    emit_load_transition_field(code, host_transition_teb_offset, active_guest_stack);
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x58]); // mov [rsp+88],r11
    code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x10]); // mov rcx,[rsp+16]
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_guest_stack.to_le_bytes());
    emit_load_transition_field(code, host_transition_teb_offset, active_guest_instruction);
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x60]); // mov [rsp+96],r11
    code.extend_from_slice(&[0x48, 0xb9]); // mov rcx, imm64
    code.extend_from_slice(&(guest_instruction as u64).to_le_bytes());
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_guest_instruction.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x89, 0x7c, 0x24, 0x18]); // guest rdi
    code.extend_from_slice(&[0x48, 0x89, 0x74, 0x24, 0x20]); // guest rsi
    code.extend_from_slice(&[0x48, 0x89, 0x54, 0x24, 0x28]); // guest rdx
    code.extend_from_slice(&[0x4c, 0x89, 0x44, 0x24, 0x30]); // guest r8
    code.extend_from_slice(&[0x4c, 0x89, 0x4c, 0x24, 0x38]); // guest r9
    code.extend_from_slice(&[0x4c, 0x89, 0x54, 0x24, 0x40]); // guest r10
    code.extend_from_slice(&[0x48, 0x89, 0x6c, 0x24, 0x48]); // guest rbp
    // A Linux syscall preserves every general-purpose register except
    // RAX/RCX/R11. Save the remaining SysV callee-saved set explicitly: a fork
    // child resumes through a fresh Windows process and cannot rely on an
    // intermediate compiler frame to reconstruct these guest register values.
    code.extend_from_slice(&[0x48, 0x89, 0x5c, 0x24, 0x78]); // guest rbx
    code.extend_from_slice(&[0x4c, 0x89, 0xa4, 0x24, 0x80, 0x00, 0x00, 0x00]); // guest r12
    code.extend_from_slice(&[0x4c, 0x89, 0xac, 0x24, 0x88, 0x00, 0x00, 0x00]); // guest r13
    code.extend_from_slice(&[0x4c, 0x89, 0xb4, 0x24, 0x90, 0x00, 0x00, 0x00]); // guest r14
    code.extend_from_slice(&[0x4c, 0x89, 0xbc, 0x24, 0x98, 0x00, 0x00, 0x00]); // guest r15
    code.extend_from_slice(&[0x48, 0x89, 0x84, 0x24]); // mov [rsp+rax_slot],rax
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_RAX_OFFSET as u32).to_le_bytes());
    emit_load_transition_field(code, host_transition_teb_offset, active_raw_frame);
    code.extend_from_slice(&[0x4c, 0x89, 0x9c, 0x24]); // mov [rsp+previous_slot],r11
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_PREVIOUS_OFFSET as u32).to_le_bytes());
    code.extend_from_slice(&[0x48, 0x89, 0xe1]); // mov rcx,rsp
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_raw_frame.to_le_bytes());
    code.extend_from_slice(&[0x65, 0x4c, 0x8b, 0x1c, 0x25]); // mov r11,gs:[StackBase]
    code.extend_from_slice(&0x08u32.to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x70]); // saved StackBase
    code.extend_from_slice(&[0x65, 0x4c, 0x8b, 0x1c, 0x25]); // mov r11,gs:[StackLimit]
    code.extend_from_slice(&0x10u32.to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x50]); // saved StackLimit
    code.extend_from_slice(&[0x4c, 0x8d, 0x9c, 0x24]); // lea r11,[rsp+frame_size]
    code.extend_from_slice(&frame_size.to_le_bytes());
    code.extend_from_slice(&[0x65, 0x4c, 0x89, 0x1c, 0x25]); // mov gs:[StackBase],r11
    code.extend_from_slice(&0x08u32.to_le_bytes());
    code.extend_from_slice(&[0x45, 0x31, 0xdb]); // xor r11d,r11d
    code.extend_from_slice(&[0x65, 0x4c, 0x89, 0x1c, 0x25]); // mov gs:[StackLimit],r11
    code.extend_from_slice(&0x10u32.to_le_bytes());
    // A frame travels with its trampoline across fork and remapping. A global
    // source-address lookup is insufficient for copied executable mappings.
    code.extend_from_slice(&[0x49, 0xbb]); // mov r11, translated continuation
    let frame_return_immediate = code.len();
    code.extend_from_slice(&0u64.to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x89, 0x9c, 0x24]); // mov [rsp+continuation],r11
    code.extend_from_slice(
        &(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_CONTINUATION_OFFSET as u32).to_le_bytes(),
    );
    emit_save_extended_state(code, extended_state)?;
    // XSAVE consumes EAX/EDX for its component mask. Reload every syscall
    // argument from the fixed record rather than relying on volatile setup
    // registers retaining their entry values.
    code.extend_from_slice(&[0x4c, 0x8b, 0x4c, 0x24, 0x30]); // r9 = guest r8 (a5)
    code.extend_from_slice(&[0x4c, 0x8b, 0x44, 0x24, 0x40]); // r8 = guest r10 (a4)
    code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x28]); // rcx = guest rdx (a3)
    code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x20]); // rdx = guest rsi (a2)
    code.extend_from_slice(&[0x48, 0x8b, 0x74, 0x24, 0x18]); // rsi = guest rdi (a1)
    code.extend_from_slice(&[0x48, 0x8b, 0xbc, 0x24]); // rdi = guest rax (nr)
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_RAX_OFFSET as u32).to_le_bytes());
    code.extend_from_slice(&[0x48, 0xb8]); // mov rax, imm64
    code.extend_from_slice(&(dispatcher as u64).to_le_bytes());
    code.extend_from_slice(&[0xff, 0xd0]); // call rax
    code.extend_from_slice(&[0x48, 0x89, 0x84, 0x24]); // save Linux result
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_RESULT_OFFSET as u32).to_le_bytes());
    // A hardware Linux syscall preserves the complete enabled user xstate. Do
    // this before any guest signal continuation or relocated instruction runs.
    emit_restore_extended_state(code, extended_state)?;
    code.extend_from_slice(&[0x48, 0x8b, 0x84, 0x24]); // reload Linux result
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_RESULT_OFFSET as u32).to_le_bytes());
    // Release this invocation's lane before consulting shared transition
    // fields. A nested syscall has already released only its own lower lane.
    emit_load_host_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&[0x49, 0x81, 0xab]); // sub qword [r11+used],lane
    code.extend_from_slice(&host_stack_used.to_le_bytes());
    code.extend_from_slice(&host_stack_lane.to_le_bytes());
    // A Windows kernel transition made anywhere inside the dispatcher may
    // clear the user-written FS base. Restore it again before touching the
    // per-thread active-context slots. R11 is syscall-clobbered and RAX keeps
    // the dispatcher's Linux return value intact.
    if let Some(offset) = thread_pointer_teb_offset {
        let offset = u32::try_from(offset).map_err(|_| ExecutionError::AddressOverflow)?;
        code.extend_from_slice(&[0x65, 0x4c, 0x8b, 0x1c, 0x25]); // mov r11,gs:[offset]
        code.extend_from_slice(&offset.to_le_bytes());
        code.extend_from_slice(&[0xf3, 0x49, 0x0f, 0xae, 0xd3]); // wrfsbase r11
    }
    // Preserve the context selected by a signal handler before revealing any
    // outer nested syscall context again.  Linux rt_sigreturn permits handlers
    // to rewrite RSP/RIP (language runtimes use this for asynchronous preemption).
    emit_load_transition_field(code, host_transition_teb_offset, active_guest_stack);
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x08]); // selected guest rsp
    emit_load_transition_field(code, host_transition_teb_offset, active_guest_instruction);
    code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x68]); // selected guest rip

    // A Linux signal handler may inject a call by moving RSP down one word,
    // storing the interrupted RIP there, and selecting a different RIP in its
    // ucontext. The logical post-syscall address is part of the guest ABI, but
    // it lies inside the bytes consumed by this detour. Translate that one
    // architecturally identifiable return slot to the relocated continuation;
    // otherwise the injected function returns into the middle of E9's operand.
    //
    // The three-way check is intentionally strict. Ordinary guest stack words
    // are never rewritten: the selected PC must differ, the selected stack must
    // be exactly one word below the interrupted stack, and that word must equal
    // the interrupted PC.
    code.extend_from_slice(&[0x48, 0xb9]); // mov rcx, logical continuation
    code.extend_from_slice(&(guest_instruction as u64).to_le_bytes());
    code.extend_from_slice(&[0x48, 0x39, 0x4c, 0x24, 0x68]); // cmp [rsp+104],rcx
    code.extend_from_slice(&[0x74, 0x00]); // je translation_done
    let normal_rip_jump = code.len() - 1;
    code.extend_from_slice(&[0x4c, 0x8b, 0x54, 0x24, 0x08]); // mov r10,[rsp+8]
    code.extend_from_slice(&[0x4d, 0x8d, 0x5a, 0x08]); // lea r11,[r10+8]
    code.extend_from_slice(&[0x4c, 0x3b, 0x5c, 0x24, 0x10]); // cmp r11,[rsp+16]
    code.extend_from_slice(&[0x75, 0x00]); // jne translation_done
    let stack_shape_jump = code.len() - 1;
    code.extend_from_slice(&[0x49, 0x39, 0x0a]); // cmp [r10],rcx
    code.extend_from_slice(&[0x75, 0x00]); // jne translation_done
    let return_value_jump = code.len() - 1;
    code.extend_from_slice(&[0x49, 0xbb]); // mov r11, relocated continuation
    let relocated_return_immediate = code.len();
    code.extend_from_slice(&0u64.to_le_bytes());
    code.extend_from_slice(&[0x4d, 0x89, 0x1a]); // mov [r10],r11
    let translation_done = code.len();
    for displacement in [normal_rip_jump, stack_shape_jump, return_value_jump] {
        let distance = translation_done
            .checked_sub(displacement + 1)
            .ok_or(ExecutionError::AddressOverflow)?;
        code[displacement] = u8::try_from(distance).map_err(|_| ExecutionError::AddressOverflow)?;
    }

    code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x58]); // prior active rsp
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_guest_stack.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x60]); // prior active rip
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_guest_instruction.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x8b, 0x8c, 0x24]); // previous raw frame
    code.extend_from_slice(&(RAW_SYSCALL_FRAME_PREVIOUS_OFFSET as u32).to_le_bytes());
    emit_store_rcx_to_transition(code, host_transition_teb_offset);
    code.extend_from_slice(&active_raw_frame.to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x8b, 0x5c, 0x24, 0x70]); // saved StackBase
    code.extend_from_slice(&[0x65, 0x4c, 0x89, 0x1c, 0x25]); // mov gs:[StackBase],r11
    code.extend_from_slice(&0x08u32.to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x8b, 0x5c, 0x24, 0x50]); // saved StackLimit
    code.extend_from_slice(&[0x65, 0x4c, 0x89, 0x1c, 0x25]); // mov gs:[StackLimit],r11
    code.extend_from_slice(&0x10u32.to_le_bytes());
    code.extend_from_slice(&[0x48, 0x8b, 0x7c, 0x24, 0x18]); // restore rdi
    code.extend_from_slice(&[0x48, 0x8b, 0x74, 0x24, 0x20]); // restore rsi
    code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x28]); // restore rdx
    code.extend_from_slice(&[0x4c, 0x8b, 0x44, 0x24, 0x30]); // restore r8
    code.extend_from_slice(&[0x4c, 0x8b, 0x4c, 0x24, 0x38]); // restore r9
    code.extend_from_slice(&[0x4c, 0x8b, 0x54, 0x24, 0x40]); // restore r10
    code.extend_from_slice(&[0x48, 0x8b, 0x6c, 0x24, 0x48]); // restore rbp
    code.extend_from_slice(&[0x48, 0x8b, 0x5c, 0x24, 0x78]); // restore rbx
    code.extend_from_slice(&[0x4c, 0x8b, 0xa4, 0x24, 0x80, 0x00, 0x00, 0x00]); // restore r12
    code.extend_from_slice(&[0x4c, 0x8b, 0xac, 0x24, 0x88, 0x00, 0x00, 0x00]); // restore r13
    code.extend_from_slice(&[0x4c, 0x8b, 0xb4, 0x24, 0x90, 0x00, 0x00, 0x00]); // restore r14
    code.extend_from_slice(&[0x4c, 0x8b, 0xbc, 0x24, 0x98, 0x00, 0x00, 0x00]); // restore r15
    if std::env::var("KINAKAZE_BREAK_SYSCALL_RETURN")
        .ok()
        .and_then(|value| usize::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
        .is_some_and(|site| site.checked_add(2) == Some(guest_instruction))
    {
        code.extend_from_slice(&[0x48, 0x85, 0xc0]); // test rax,rax
        code.extend_from_slice(&[0x75, 0x01]); // jne +1
        code.push(0xcc); // int3 on the child/zero return path
    }
    code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x68]); // selected guest rip
    code.extend_from_slice(&[0x4c, 0x8b, 0x5c, 0x24, 0x08]); // selected guest rsp
    code.extend_from_slice(&[0x4c, 0x89, 0xdc]); // mov rsp,r11
    code.extend_from_slice(&[0x49, 0xbb]); // mov r11, expected continuation
    code.extend_from_slice(&(guest_instruction as u64).to_le_bytes());
    code.extend_from_slice(&[0x4c, 0x39, 0xd9]); // cmp rcx,r11
    code.extend_from_slice(&[0x74, 0x02]); // je normal relocated continuation
    code.extend_from_slice(&[0xff, 0xe1]); // jmp rcx (handler-selected continuation)
    Ok((relocated_return_immediate, frame_return_immediate))
}

fn emit_load_host_transition(code: &mut Vec<u8>, teb_offset: u32) {
    code.extend_from_slice(&[0x65, 0x4c, 0x8b, 0x1c, 0x25]); // mov r11,gs:[offset]
    code.extend_from_slice(&teb_offset.to_le_bytes());
}

fn emit_save_extended_state(
    code: &mut Vec<u8>,
    format: kinakaze_tls::thread_pointer::ExtendedStateFormat,
) -> Result<(), ExecutionError> {
    let displacement =
        u32::try_from(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET)
            .map_err(|_| ExecutionError::AddressOverflow)?;
    match format {
        kinakaze_tls::thread_pointer::ExtendedStateFormat::Fxsave => {
            // fxsave64 [rsp+disp32]
            code.extend_from_slice(&[0x48, 0x0f, 0xae, 0x84, 0x24]);
            code.extend_from_slice(&displacement.to_le_bytes());
        }
        kinakaze_tls::thread_pointer::ExtendedStateFormat::Xsave { mask, .. } => {
            code.push(0xb8); // mov eax,mask.low
            code.extend_from_slice(&(mask as u32).to_le_bytes());
            code.push(0xba); // mov edx,mask.high
            code.extend_from_slice(&((mask >> 32) as u32).to_le_bytes());
            // xsave64 [rsp+disp32]
            code.extend_from_slice(&[0x48, 0x0f, 0xae, 0xa4, 0x24]);
            code.extend_from_slice(&displacement.to_le_bytes());
        }
    }
    Ok(())
}

fn emit_restore_extended_state(
    code: &mut Vec<u8>,
    format: kinakaze_tls::thread_pointer::ExtendedStateFormat,
) -> Result<(), ExecutionError> {
    let displacement =
        u32::try_from(kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET)
            .map_err(|_| ExecutionError::AddressOverflow)?;
    match format {
        kinakaze_tls::thread_pointer::ExtendedStateFormat::Fxsave => {
            // fxrstor64 [rsp+disp32]
            code.extend_from_slice(&[0x48, 0x0f, 0xae, 0x8c, 0x24]);
            code.extend_from_slice(&displacement.to_le_bytes());
        }
        kinakaze_tls::thread_pointer::ExtendedStateFormat::Xsave { mask, .. } => {
            code.push(0xb8); // mov eax,mask.low
            code.extend_from_slice(&(mask as u32).to_le_bytes());
            code.push(0xba); // mov edx,mask.high
            code.extend_from_slice(&((mask >> 32) as u32).to_le_bytes());
            // xrstor64 [rsp+disp32]
            code.extend_from_slice(&[0x48, 0x0f, 0xae, 0xac, 0x24]);
            code.extend_from_slice(&displacement.to_le_bytes());
        }
    }
    Ok(())
}

fn emit_load_transition_field(code: &mut Vec<u8>, teb_offset: u32, field: u32) {
    emit_load_host_transition(code, teb_offset);
    code.extend_from_slice(&[0x4d, 0x8b, 0x9b]); // mov r11,[r11+field]
    code.extend_from_slice(&field.to_le_bytes());
}

fn emit_store_rcx_to_transition(code: &mut Vec<u8>, teb_offset: u32) {
    emit_load_host_transition(code, teb_offset);
    code.extend_from_slice(&[0x49, 0x89, 0x8b]); // mov [r11+field],rcx
}

fn emit_absolute_jump(code: &mut Vec<u8>, target: usize) {
    // jmp qword ptr [rip+0]; <absolute target>
    code.extend_from_slice(&[0xff, 0x25, 0x00, 0x00, 0x00, 0x00]);
    code.extend_from_slice(&(target as u64).to_le_bytes());
}

#[cfg(windows)]
pub(super) fn allocate_near(origin: usize) -> Result<usize, ExecutionError> {
    let mut runs = runs().lock().map_err(|_| ExecutionError::AddressOverflow)?;
    if let Some(run) = runs.iter_mut().rev().find(|run| {
        run.used + STUB_SIZE <= RUN_SIZE && rel32_reachable(origin, run.base + run.used)
    }) {
        let address = run.base + run.used;
        run.used += STUB_SIZE;
        return Ok(address);
    }
    let base = allocate_run_near(origin)?;
    runs.push(Run {
        base,
        used: STUB_SIZE,
    });
    Ok(base)
}

/// Return only the unused tail of our own last reservation. Encoding needs the
/// final address, so reserve the proven maximum first, then keep the actual
/// length. A concurrent reservation makes compaction ineligible; never rewind
/// across another caller's slot. Fork still owns the same complete run mapping.
pub(super) fn finish_slot(address: usize, length: usize) {
    if length == 0 || length > STUB_SIZE {
        return;
    }
    let retained = length.next_multiple_of(64);
    let Ok(mut runs) = runs().lock() else { return };
    if let Some(run) = runs
        .iter_mut()
        .rev()
        .find(|run| address >= run.base && address < run.base + RUN_SIZE)
    {
        let offset = address - run.base;
        if run.used == offset + STUB_SIZE {
            run.used = offset + retained;
        }
    }
}

#[cfg(not(windows))]
pub(super) fn allocate_near(_: usize) -> Result<usize, ExecutionError> {
    Err(ExecutionError::MappingFailed {
        object: "<instruction trampoline run>".to_owned(),
        len: RUN_SIZE,
    })
}

fn rel32_reachable(origin: usize, target: usize) -> bool {
    i32::try_from(target as i128 - (origin + MIN_DETOUR_SIZE) as i128).is_ok()
}

/// Allocation-granularity bounds containing only rel32-reachable bytes.
fn near_allocation_bounds(origin: usize, maximum: usize, radius: usize) -> Option<(usize, usize)> {
    const GRANULARITY: usize = 64 * 1024;
    let next = origin.checked_add(MIN_DETOUR_SIZE)?;
    let low = next.saturating_sub(radius).max(GRANULARITY);
    let low = low.checked_add(GRANULARITY - 1)? & !(GRANULARITY - 1);
    let high = next.saturating_add(radius.checked_sub(1)?).min(maximum);
    let end = high.checked_add(1)? & !(GRANULARITY - 1);
    (end.checked_sub(low)? >= RUN_SIZE).then_some((low, end - 1))
}

#[cfg(windows)]
fn allocate_run_near(origin: usize) -> Result<usize, ExecutionError> {
    use windows_sys::Win32::System::Memory::{
        MEM_ADDRESS_REQUIREMENTS, MEM_COMMIT, MEM_EXTENDED_PARAMETER, MEM_EXTENDED_PARAMETER_0,
        MEM_EXTENDED_PARAMETER_1, MEM_RELEASE, MEM_RESERVE,
        MemExtendedParameterAddressRequirements, PAGE_EXECUTE_READWRITE, VirtualAlloc,
        VirtualAlloc2, VirtualFree,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let _fork_mapping_transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(
        ExecutionError::MappingFailed {
            object: "<near instruction trampoline run>".to_owned(),
            len: RUN_SIZE,
        },
    )?;
    // Ask the kernel to find a free region in the entire rel32 window. Probing
    // every 64 KiB from the instruction repeats thousands of failed syscalls
    // for each run next to a large DSO (and then probes our own earlier runs).
    let mut information = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut information) };
    // Prefer nearby space so relocated RIP-relative operands also stay close
    // to their targets. Widen geometrically when a large image fills the window.
    for radius in [16 << 20, 64 << 20, 256 << 20, 1 << 30, 1usize << 31] {
        let Some((low, high)) = near_allocation_bounds(
            origin,
            information.lpMaximumApplicationAddress as usize,
            radius,
        ) else {
            continue;
        };
        let mut requirements = MEM_ADDRESS_REQUIREMENTS {
            LowestStartingAddress: low as _,
            HighestEndingAddress: high as _,
            Alignment: 0,
        };
        let mut parameter = MEM_EXTENDED_PARAMETER {
            Anonymous1: MEM_EXTENDED_PARAMETER_0 {
                _bitfield: MemExtendedParameterAddressRequirements as u64,
            },
            Anonymous2: MEM_EXTENDED_PARAMETER_1 {
                Pointer: (&raw mut requirements).cast(),
            },
        };
        let block = unsafe {
            VirtualAlloc2(
                GetCurrentProcess(),
                core::ptr::null(),
                RUN_SIZE,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
                &mut parameter,
                1,
            )
        };
        if !block.is_null() {
            if register_run(block as usize) {
                return Ok(block as usize);
            }
            unsafe { VirtualFree(block, 0, MEM_RELEASE) };
            return Err(ExecutionError::MappingFailed {
                object: "<near instruction trampoline registration>".to_owned(),
                len: RUN_SIZE,
            });
        }
    }
    // Retain the exact-address fallback if a constrained allocation fails.
    const GRANULARITY: usize = 64 * 1024;
    const MAX_STEPS: usize = (i32::MAX as usize) / GRANULARITY;
    let center = origin & !(GRANULARITY - 1);
    for step in 1..=MAX_STEPS {
        let distance = step * GRANULARITY;
        let candidates = [center.checked_add(distance), center.checked_sub(distance)];
        for candidate in candidates.into_iter().flatten() {
            if candidate < GRANULARITY || !rel32_reachable(origin, candidate) {
                continue;
            }
            // SAFETY: an exact aligned address is requested; failure just means the
            // region was occupied and scanning continues.
            let block = unsafe {
                VirtualAlloc(
                    candidate as *const core::ffi::c_void,
                    RUN_SIZE,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_EXECUTE_READWRITE,
                )
            };
            if block.is_null() {
                continue;
            }
            if !register_run(block as usize) {
                // SAFETY: the unpublished run is wholly owned here.
                unsafe { VirtualFree(block, 0, MEM_RELEASE) };
                continue;
            }
            return Ok(block as usize);
        }
    }
    Err(ExecutionError::MappingFailed {
        object: "<near instruction trampoline run>".to_owned(),
        len: RUN_SIZE,
    })
}

#[cfg(windows)]
fn register_run(base: usize) -> bool {
    kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base,
        len: RUN_SIZE,
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::Ordinary,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: kinakaze_runtime::ForkMappingDomain::HostPrivate,
    })
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn direct_fs_encoding_matches_block_encoder() {
        for bytes in [
            &[0x64, 0x48, 0x8b, 0x04, 0x25, 0x28, 0, 0, 0][..],
            &[0x64, 0x48, 0x2b, 0x04, 0x25, 0x28, 0, 0, 0],
            &[0x64, 0x4d, 0x8b, 0x36],
            &[0x64, 0x4c, 0x89, 0x20],
            &[0x64, 0x48, 0x8b, 0x44, 0x24, 0xf8],
        ] {
            let original = Decoder::with_ip(64, bytes, 0x1000, DecoderOptions::NONE).decode();
            let (translated, _) = translate_fs_instruction(original).unwrap();
            for ip in [0x800, 0x2000, 0x7fff00000000] {
                let expected = BlockEncoder::encode(
                    64,
                    InstructionBlock::new(&[translated], ip),
                    BlockEncoderOptions::NONE,
                )
                .unwrap();
                let mut encoder = Encoder::new(64);
                encoder.encode(&translated, ip).unwrap();
                assert_eq!(encoder.take_buffer(), expected.code_buffer);
            }
        }
    }

    #[test]
    fn constrained_runs_stay_inside_rel32_window() {
        let maximum = 0x7fff_ffff_ffffusize;
        for origin in [0, 0x400000, 0x7fff_1234_5678, maximum - MIN_DETOUR_SIZE] {
            let (low, high) = near_allocation_bounds(origin, maximum, 1usize << 31).unwrap();
            assert_eq!(low % RUN_SIZE, 0);
            assert_eq!((high + 1) % RUN_SIZE, 0);
            assert!(low >= RUN_SIZE && high <= maximum);
            assert!(rel32_reachable(origin, low));
            assert!(rel32_reachable(origin, high));
            assert!(high + 1 - low >= RUN_SIZE);
        }
        assert_eq!(
            near_allocation_bounds(usize::MAX, maximum, 1usize << 31),
            None
        );
        assert_eq!(near_allocation_bounds(0, RUN_SIZE - 1, 1usize << 31), None);
    }

    #[test]
    fn compact_slots_preserve_intervening_reservations() {
        let origin = compact_slots_preserve_intervening_reservations as *const () as usize;
        let first = allocate_near(origin).unwrap();
        finish_slot(first, 47);
        let second = allocate_near(origin).unwrap();
        assert_eq!(second, first + 64);
        let third = allocate_near(origin).unwrap();
        // Completing an older reservation must not overwrite the newer one.
        finish_slot(second, 47);
        finish_slot(third, 100);
        let fourth = allocate_near(origin).unwrap();
        assert_eq!(third, second + STUB_SIZE);
        assert_eq!(fourth, third + 128);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::{TlsAlloc, TlsFree, TlsSetValue};

    #[test]
    fn short_fs_detour_planning_consumes_only_whole_instructions() {
        // mov r14,fs:[r14]; mov rax,[rsp+0x18]
        let code = [0x64, 0x4d, 0x8b, 0x36, 0x48, 0x8b, 0x44, 0x24, 0x18];
        assert_eq!(fs_detour_length(&code, 0, 0x1000), Some(code.len()));
        assert_eq!(
            fs_detour_length(&code[..4], 0, 0x1000),
            None,
            "a short site without a complete following instruction is not patchable"
        );
    }

    #[test]
    fn detour_uses_whole_instructions_and_is_rel32_reachable() {
        // mov [fs:rax],r12; nop; ret
        let block = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            )
        } as *mut u8;
        assert!(!block.is_null());
        let bytes = [0x64, 0x4c, 0x89, 0x20, 0x90, 0xc3];
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len()) };
        let site = unsafe {
            install(
                block as usize,
                crate::execution::segment_patch::teb_slot_offset(1),
                crate::execution::segment_patch::teb_slot_offset(2),
            )
        }
        .unwrap();
        assert_eq!(site.overwritten, 5);
        assert_eq!(unsafe { block.read() }, 0xe9);
        assert!(rel32_reachable(block as usize, site.trampoline));
        unsafe { VirtualFree(block.cast(), 0, MEM_RELEASE) };
    }

    #[test]
    fn translated_fs_access_does_not_depend_on_the_host_fs_base() {
        check_translated_fs_access(false);
    }

    #[test]
    fn unpublished_fs_access_executes_after_batch_publication() {
        check_translated_fs_access(true);
    }

    fn check_translated_fs_access(unpublished: bool) {
        if !kinakaze_tls::thread_pointer::supported() {
            return;
        }
        let teb_slot = unsafe { TlsAlloc() };
        let scratch_slot = unsafe { TlsAlloc() };
        assert!(teb_slot < 64 && scratch_slot < 64);
        let value = Box::new(0x1234_5678_9abc_def0u64);
        let thread_pointer = (&*value as *const u64 as usize) + 8;
        assert_ne!(
            unsafe { TlsSetValue(teb_slot, thread_pointer as *mut core::ffi::c_void) },
            0
        );

        let block = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            )
        } as *mut u8;
        assert!(!block.is_null());
        // mov rax,fs:[-8]; ret
        let bytes = [0x64, 0x48, 0x8b, 0x04, 0x25, 0xf8, 0xff, 0xff, 0xff, 0xc3];
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len()) };
        let thread_offset = crate::execution::segment_patch::teb_slot_offset(teb_slot);
        let scratch_offset = crate::execution::segment_patch::teb_slot_offset(scratch_slot);
        if unpublished {
            let range = unsafe { UnpublishedCode::new(block as usize, bytes.len()) };
            unsafe { install_unpublished(block as usize, thread_offset, scratch_offset, &range) }
                .unwrap();
            range.finish().unwrap();
        } else {
            unsafe { install(block as usize, thread_offset, scratch_offset) }.unwrap();
        }

        let previous_fs: usize;
        unsafe {
            core::arch::asm!("rdfsbase {}", out(reg) previous_fs, options(nostack, preserves_flags));
            core::arch::asm!("wrfsbase {}", in(reg) 0usize, options(nostack, preserves_flags));
        }
        let function: unsafe extern "C" fn() -> u64 = unsafe { core::mem::transmute(block) };
        let observed = unsafe { function() };
        unsafe {
            core::arch::asm!("wrfsbase {}", in(reg) previous_fs, options(nostack, preserves_flags));
        }
        assert_eq!(observed, *value);

        unsafe { VirtualFree(block.cast(), 0, MEM_RELEASE) };
        assert_ne!(unsafe { TlsFree(teb_slot) }, 0);
        assert_ne!(unsafe { TlsFree(scratch_slot) }, 0);
    }

    static DISPATCH_STACK_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static DISPATCH_STACK_POINTER: AtomicUsize = AtomicUsize::new(0);
    static OUTER_DISPATCH_STACK_POINTER: AtomicUsize = AtomicUsize::new(0);
    static NESTED_DISPATCH_STACK_POINTER: AtomicUsize = AtomicUsize::new(0);
    static NESTED_SYSCALL_TARGET: AtomicUsize = AtomicUsize::new(0);
    static NESTED_SYSCALL_RESULT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static INJECTED_LOGICAL_RETURN: AtomicUsize = AtomicUsize::new(0);
    static TEST_TRANSITION_POINTER: AtomicUsize = AtomicUsize::new(0);

    #[unsafe(naked)]
    unsafe extern "sysv64" fn injected_call_target() {
        core::arch::naked_asm!("ret")
    }

    unsafe fn invoke_raw_syscall(target: usize, number: usize) -> i64 {
        let result: i64;
        unsafe {
            core::arch::asm!(
                "push {target}",
                "mov rax, {number}",
                "mov rdi, 1",
                "mov rsi, 1",
                "mov rdx, 1",
                "mov r10, 1",
                "mov r8, 1",
                "mov r9, 1",
                "call qword ptr [rsp]",
                "add rsp, 8",
                target = in(reg) target,
                number = in(reg) number,
                lateout("rax") result,
                out("rcx") _,
                out("rdx") _,
                out("rsi") _,
                out("rdi") _,
                out("r8") _,
                out("r9") _,
                out("r10") _,
                out("r11") _,
            );
        }
        result
    }

    unsafe extern "sysv64" fn mock_syscall_dispatcher(
        number: i64,
        a1: u64,
        a2: u64,
        a3: u64,
        a4: u64,
        a5: u64,
        a6: u64,
    ) -> i64 {
        // A function call may clobber every SysV vector register, while the
        // architectural Linux syscall instruction preserves them. Exercise the
        // bridge with a deliberate clobber instead of relying on this Rust body
        // to happen to allocate an XMM register in a particular build.
        unsafe {
            core::arch::asm!(
                "pcmpeqd xmm15, xmm15",
                out("xmm15") _,
                options(nostack, preserves_flags),
            )
        };
        let stack_limit: usize;
        unsafe {
            core::arch::asm!(
                "mov {}, gs:[0x10]",
                out(reg) stack_limit,
                options(nostack, preserves_flags, readonly),
            )
        };
        DISPATCH_STACK_LIMIT.store(stack_limit, Ordering::SeqCst);
        let stack_pointer: usize;
        unsafe {
            core::arch::asm!(
                "mov {}, rsp",
                out(reg) stack_pointer,
                options(nostack, preserves_flags, readonly),
            )
        };
        DISPATCH_STACK_POINTER.store(stack_pointer, Ordering::SeqCst);
        if number == 100 {
            OUTER_DISPATCH_STACK_POINTER.store(stack_pointer, Ordering::SeqCst);
            let target = NESTED_SYSCALL_TARGET.load(Ordering::SeqCst);
            if target != 0 {
                let nested = unsafe { invoke_raw_syscall(target, 7) };
                NESTED_SYSCALL_RESULT.store(nested as usize, Ordering::SeqCst);
            }
        } else if number == 7 {
            NESTED_DISPATCH_STACK_POINTER.store(stack_pointer, Ordering::SeqCst);
        } else if number == 200 {
            let transition = TEST_TRANSITION_POINTER.load(Ordering::SeqCst);
            let guest_stack = unsafe {
                ((transition + kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET)
                    as *const usize)
                    .read_unaligned()
            };
            let logical_return = unsafe {
                ((transition
                    + kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET)
                    as *const usize)
                    .read_unaligned()
            };
            assert_ne!(guest_stack, 0, "the syscall stub must publish guest RSP");
            assert_ne!(logical_return, 0, "the syscall stub must publish guest RIP");
            unsafe { ((guest_stack - 8) as *mut usize).write(logical_return) };
            unsafe {
                ((transition + kinakaze_tls::thread_pointer::ACTIVE_GUEST_STACK_POINTER_OFFSET)
                    as *mut usize)
                    .write_unaligned(guest_stack - 8);
                ((transition
                    + kinakaze_tls::thread_pointer::ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET)
                    as *mut usize)
                    .write_unaligned(injected_call_target as *const () as usize);
            }
            INJECTED_LOGICAL_RETURN.store(logical_return, Ordering::SeqCst);
        }
        eprintln!(
            "mock_syscall_dispatcher called: nr={number}, a1={a1}, a2={a2}, a3={a3}, a4={a4}, a5={a5}, a6={a6}"
        );
        number + (a1 as i64) + (a2 as i64) + (a3 as i64) + (a4 as i64) + (a5 as i64) + (a6 as i64)
    }

    #[test]
    fn syscall_detour_marshals_and_dispatches_correctly() {
        const ISOLATED_CHILD: &str = "KINAKAZE_SYSCALL_DETOUR_TEST_CHILD";
        if std::env::var_os(ISOLATED_CHILD).is_none() {
            // Installing a Linux FS base freezes the process-wide static TLS
            // layout by design. Run this CPU-state test in its own process so
            // later Linker tests start with the same fresh process lifecycle as
            // a real guest; resetting the layout in place would make already
            // installed thread blocks invalid.
            let status = std::process::Command::new(
                std::env::current_exe().expect("current kinakaze-link test binary"),
            )
            .env(ISOLATED_CHILD, "1")
            .args([
                "--exact",
                "execution::instruction_trampoline::tests::syscall_detour_marshals_and_dispatches_correctly",
                "--nocapture",
            ])
            .status()
            .expect("launch isolated syscall detour test");
            assert!(status.success(), "isolated syscall detour test failed");
            return;
        }

        DISPATCH_STACK_LIMIT.store(usize::MAX, Ordering::SeqCst);
        DISPATCH_STACK_POINTER.store(0, Ordering::SeqCst);
        OUTER_DISPATCH_STACK_POINTER.store(0, Ordering::SeqCst);
        NESTED_DISPATCH_STACK_POINTER.store(0, Ordering::SeqCst);
        NESTED_SYSCALL_TARGET.store(0, Ordering::SeqCst);
        NESTED_SYSCALL_RESULT.store(usize::MAX, Ordering::SeqCst);
        INJECTED_LOGICAL_RETURN.store(0, Ordering::SeqCst);
        TEST_TRANSITION_POINTER.store(0, Ordering::SeqCst);
        let tls_block = kinakaze_tls::thread_pointer::install(&[]).unwrap();
        TEST_TRANSITION_POINTER.store(tls_block.transition_pointer(), Ordering::SeqCst);
        let teb_slot = unsafe { TlsAlloc() };
        let transition_slot = unsafe { TlsAlloc() };
        assert_ne!(teb_slot, u32::MAX);
        assert_ne!(transition_slot, u32::MAX);
        assert!(teb_slot < 64, "test needs a direct TEB TLS slot");
        assert!(transition_slot < 64, "test needs a direct TEB TLS slot");
        assert_ne!(
            unsafe {
                TlsSetValue(
                    teb_slot,
                    tls_block.thread_pointer() as *mut core::ffi::c_void,
                )
            },
            0
        );
        assert_ne!(
            unsafe {
                TlsSetValue(
                    transition_slot,
                    tls_block.transition_pointer() as *mut core::ffi::c_void,
                )
            },
            0
        );
        let host_stack_top = unsafe {
            ((tls_block.transition_pointer()
                + kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET)
                as *const usize)
                .read_unaligned()
        };
        let block = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            )
        } as *mut u8;
        assert!(!block.is_null());
        // syscall (0f 05); add rax, 10 (48 83 c0 0a); ret (c3)
        let bytes = [0x0f, 0x05, 0x48, 0x83, 0xc0, 0x0a, 0xc3];
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len()) };

        let site = unsafe {
            install_syscall(
                block as usize,
                mock_syscall_dispatcher as *const () as usize,
                Some(crate::execution::segment_patch::teb_slot_offset(teb_slot)),
                Some(crate::execution::segment_patch::teb_slot_offset(
                    transition_slot,
                )),
            )
        }
        .unwrap();
        assert_eq!(site.overwritten, 6);
        assert_eq!(unsafe { block.read() }, 0xe9);
        assert!(rel32_reachable(block as usize, site.trampoline));
        tls_block.restore().unwrap();
        NESTED_SYSCALL_TARGET.store(block as usize, Ordering::SeqCst);

        // Call the patched code:
        // Pass rax=100, rdi=1, rsi=2, rdx=3, r10=4, r8=5, r9=6
        let result: i64;
        let stack_limit_before: usize;
        let stack_limit_after: usize;
        let vector_before = [
            0x10u8, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xa9, 0xba, 0xcb, 0xdc, 0xed,
            0xfe, 0x0f,
        ];
        let mut vector_after = [0u8; 16];
        unsafe {
            core::arch::asm!(
                "mov {stack_limit_before}, gs:[0x10]",
                "movdqu xmm15, xmmword ptr [{vector_before}]",
                "push {func}",
                "mov rax, 100",
                "mov rdi, 1",
                "mov rsi, 2",
                "mov rdx, 3",
                "mov r10, 4",
                "mov r8, 5",
                "mov r9, 6",
                "call qword ptr [rsp]",
                "add rsp, 8",
                "movdqu xmmword ptr [{vector_after}], xmm15",
                "mov {stack_limit_after}, gs:[0x10]",
                func = in(reg) block,
                vector_before = in(reg) vector_before.as_ptr(),
                vector_after = in(reg) vector_after.as_mut_ptr(),
                stack_limit_before = out(reg) stack_limit_before,
                stack_limit_after = out(reg) stack_limit_after,
                lateout("rax") result,
                out("rcx") _,
                out("rdx") _,
                out("rsi") _,
                out("rdi") _,
                out("r8") _,
                out("r9") _,
                out("r10") _,
                out("r11") _,
            );
        }
        // Expected: 100 + 1 + 2 + 3 + 4 + 5 + 6 = 121, + 10 (add rax, 10) = 131!
        assert_eq!(result, 131);
        assert_eq!(
            vector_after, vector_before,
            "a Linux syscall must preserve enabled vector state"
        );
        // Nested call: 7 + six one-valued arguments + the relocated add 10.
        assert_eq!(NESTED_SYSCALL_RESULT.load(Ordering::SeqCst), 23);

        // Model the architecture-neutral shape used when a signal handler
        // injects a call: the interrupted PC is pushed as its return address
        // and ucontext selects the injected function. The injected RET must
        // resume through the relocated trailing instructions rather than the
        // bytes occupied by the detour at block+2.
        let injected = unsafe { invoke_raw_syscall(block as usize, 200) };
        assert_eq!(injected, 216);
        let logical_return = INJECTED_LOGICAL_RETURN.load(Ordering::SeqCst);
        assert_eq!(logical_return, block as usize + 2);
        assert!(translated_syscall_continuation(logical_return).is_some());

        assert_eq!(DISPATCH_STACK_LIMIT.load(Ordering::SeqCst), 0);
        let dispatch_rsp = DISPATCH_STACK_POINTER.load(Ordering::SeqCst);
        assert!(dispatch_rsp < host_stack_top);
        assert!(
            dispatch_rsp >= host_stack_top - kinakaze_tls::thread_pointer::HOST_CALL_STACK_SIZE
        );
        let outer_rsp = OUTER_DISPATCH_STACK_POINTER.load(Ordering::SeqCst);
        let nested_rsp = NESTED_DISPATCH_STACK_POINTER.load(Ordering::SeqCst);
        assert!(nested_rsp < outer_rsp);
        assert!(
            outer_rsp - nested_rsp >= kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE / 2,
            "nested dispatcher must use a distinct lower stack lane"
        );
        assert_eq!(stack_limit_after, stack_limit_before);
        let host_stack_used = unsafe {
            ((tls_block.transition_pointer()
                + kinakaze_tls::thread_pointer::HOST_CALL_STACK_USED_OFFSET)
                as *const usize)
                .read_unaligned()
        };
        assert_eq!(host_stack_used, 0, "the syscall lane must be released");

        unsafe { VirtualFree(block.cast(), 0, MEM_RELEASE) };
        TEST_TRANSITION_POINTER.store(0, Ordering::SeqCst);
        assert_ne!(unsafe { TlsFree(teb_slot) }, 0);
        assert_ne!(unsafe { TlsFree(transition_slot) }, 0);
    }
}
