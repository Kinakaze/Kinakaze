//! Preserving guest `%fs:` accesses and intercepting raw Linux syscalls.
//!
//! Linux x86_64 keeps the thread pointer in the `fs` base, and compiled code reads
//! thread-local data as a fixed displacement from it. The canonical case is the
//! stack-protector canary at `%fs:0x28`, which every `-fstack-protector` function
//! loads on entry — real BusyBox does it at 1469 sites.
//!
//! On Windows the `fs` base is zero, so those reads hit linear address `0x28` and
//! fault. `wrfsbase` appears to fix that, and it does until the thread is
//! descheduled: **Windows restores `fs` from its own thread context on every
//! context switch, and that context has base zero.** This was measured, not
//! assumed — a base written with `wrfsbase` survives straight-line code and is gone
//! after the first `Sleep` or syscall. A short program like `busybox echo` therefore
//! works by luck, while anything that yields faults.
//!
//! `gs` is not an alternative to point elsewhere: Windows uses it for the TEB, and
//! redirecting it would break every Win32 call in the process. But that is also what
//! makes it the solution — the TEB is per-thread and preserved by construction, and
//! it contains `TlsSlots`, 64 words reserved for exactly this kind of use.
//!
//! CFG-proven guest TLS instructions are redirected through permanent out-of-line
//! trampolines. Short instructions consume only complete fallthrough instructions,
//! and only when no independent or direct control-flow entry lands in that tail.
//! This keeps ordinary guest execution independent of Windows exception delivery.

use super::ExecutionError;
use super::Image;
#[path = "code_boundaries.rs"]
mod code_boundaries;
use code_boundaries::CodeBoundaries;

use std::collections::{HashSet, VecDeque};

use iced_x86::{
    ConstantOffsets, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, Register,
};

/// The `fs` segment override prefix.
const PREFIX_FS: u8 = 0x64;
/// The `gs` segment override prefix.
const PREFIX_GS: u8 = 0x65;

/// Offset of `TlsSlots` within the TEB on x86_64.
///
/// Part of the Windows ABI rather than an implementation detail: the slots are what
/// `TlsGetValue` reads, and their location is fixed.
const TEB_TLS_SLOTS: usize = 0x1480;

/// Displacement Linux uses for the stack-protector canary.
const CANARY_DISPLACEMENT: u32 = 0x28;

/// One rewritten site, kept for reporting.
#[derive(Clone, Copy, Debug)]
pub struct PatchedSite {
    pub address: usize,
    pub original_displacement: u32,
    pub new_displacement: u32,
}

/// The recovery actions VEH can take for an FS-relative instruction.
#[derive(Clone, Copy, Debug)]
pub enum HealedThreadPointerSite {
    /// A late/generated TCB self-pointer read was permanently rewritten.
    Patched(PatchedSite),
    /// A register-relative or otherwise non-local FS access now uses a permanent
    /// out-of-line trampoline.
    Trampoline(crate::execution::instruction_trampoline::InstructionTrampoline),
    /// The instruction was safely emulated in-place in the VEH context.
    Emulated,
}

/// What a scan found and changed.
#[derive(Debug, Default)]
pub struct PatchReport {
    pub canary_sites: usize,
    pub other_sites: Vec<PatchedSite>,
    /// Exact decoded `syscall` instructions neutralized during AOT.
    pub syscall_sites: Vec<usize>,
    pub trampoline_sites: Vec<crate::execution::instruction_trampoline::InstructionTrampoline>,
}

/// What AOT emits for a raw Linux `syscall` instruction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AotSyscallPolicy {
    /// Preserve instruction length and execution flow without entering Windows.
    Nop,
    /// Replace the site with `ud2`, producing an immediate attributable trap.
    #[default]
    Trap,
    /// Replace the site with a JMP trampoline to the syscall dispatcher.
    Trampoline(usize),
}

/// Rewrites `%fs:`-relative accesses in one object's executable segments.
///
/// Canary accesses use the real guest TCB, including writes made after fork.
/// `canary_slot_offset` remains part of the legacy cache/report interface.
///
/// Only the exact encodings this understands are touched, and anything else is left
/// alone and reported: silently mis-patching an instruction would corrupt code in a
/// way that surfaces arbitrarily far away.
pub fn retarget_thread_pointer_reads(
    object: &Image<'_>,
    canary_slot_offset: usize,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
    scratch_teb_offset: Option<usize>,
    syscall_policy: AotSyscallPolicy,
) -> Result<(PatchReport, crate::execution::aot_cache::CachedObjectAot), ExecutionError> {
    let mut report = PatchReport::default();
    let mut cached_segments = Vec::new();
    let elf = object.elf()?;
    let segments = elf
        .program_headers()?
        .into_iter()
        .filter(|segment| {
            segment.kind == kinakaze_elf::PT_LOAD
                && segment.flags & kinakaze_elf::PF_X != 0
                && segment.memory_size != 0
        })
        .map(|segment| {
            let start = object.resolve_address(segment.virtual_address)?;
            let length = usize::try_from(segment.memory_size)
                .map_err(|_| ExecutionError::AddressOverflow)?;
            Ok((start, length, segment.virtual_address))
        })
        .collect::<Result<Vec<_>, ExecutionError>>()?;
    let seg_pairs: Vec<(usize, usize)> = segments.iter().map(|(s, l, _)| (*s, *l)).collect();
    let control_flow = protected_instruction_entries(object, &seg_pairs)?;

    let canary_target =
        u32::try_from(canary_slot_offset).map_err(|_| ExecutionError::AddressOverflow)?;

    for (start, length, vaddr) in segments {
        // SAFETY: the range is this object's own mapped executable segment, which is
        // still writable at this point in the link.
        let code = unsafe { std::slice::from_raw_parts_mut(start as *mut u8, length) };

        let (cached_canary, cached_syscalls, cached_thread_pointer) = patch_code(
            code,
            start as u64,
            canary_target,
            thread_pointer_teb_offset,
            host_transition_teb_offset,
            scratch_teb_offset,
            syscall_policy,
            Some(&control_flow.patch_boundaries),
            Some(&control_flow.independent_entries),
            &mut report,
        )?;

        cached_segments.push(crate::execution::aot_cache::CachedSegment {
            virtual_address: vaddr,
            memory_size: length as u64,
            canary_sites: cached_canary,
            syscall_sites: cached_syscalls,
            thread_pointer_sites: cached_thread_pointer,
        });
    }
    Ok((
        report,
        crate::execution::aot_cache::CachedObjectAot {
            segments: cached_segments,
        },
    ))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InstructionPatch {
    pub(crate) prefix_offset: usize,
    pub(crate) displacement_offset: usize,
    pub(crate) displacement_size: usize,
    pub(crate) original_displacement: u64,
    pub(crate) new_displacement: u64,
}

fn patch_code(
    code: &mut [u8],
    ip: u64,
    canary_target: u32,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
    _scratch_teb_offset: Option<usize>,
    syscall_policy: AotSyscallPolicy,
    instruction_boundaries: Option<&[usize]>,
    independent_entries: Option<&HashSet<usize>>,
    report: &mut PatchReport,
) -> Result<
    (
        Vec<crate::execution::aot_cache::CachedCanarySite>,
        Vec<crate::execution::aot_cache::CachedSyscallSite>,
        Vec<crate::execution::aot_cache::CachedThreadPointerSite>,
    ),
    ExecutionError,
> {
    let mut syscalls = Vec::new();
    let mut cached_canary = Vec::new();
    let mut cached_syscalls = Vec::new();
    let mut cached_thread_pointer = Vec::new();
    let mut thread_pointer_sites = Vec::new();
    let mut gs_sites = Vec::new();
    // All AOT source pages are already writable; publish their generated code
    // once after the complete segment has been prepared.
    let unpublished = unsafe {
        super::instruction_trampoline::UnpublishedCode::new(code.as_ptr() as usize, code.len())
    };
    // Decode independently at each CFG-proven patch candidate. All independent
    // entries, including branches after syscalls, were collected by traversal.
    // Starting one decoder
    // at the first byte of an entire PT_LOAD segment is not equivalent: embedded
    // literals can desynchronise x86 decoding and make an immediate byte appear
    // to be a syscall opcode.  Tests that exercise the byte-level planner without
    // an ELF object retain a single linear stream.
    // Retain only instructions that will actually be patched. A large Go
    // executable has millions of ordinary instructions; keeping every iced
    // Instruction here inflated the managed arena by hundreds of megabytes,
    // which every subsequent fork then had to copy, even after this Vec died.
    // All branch targets are still collected before any detour is installed.
    let mut decoded = Vec::new();
    let mut direct_control_flow_entries = HashSet::new();
    let mut record = |offset, instruction: Instruction, constants| {
        if let Some(target) = direct_control_flow_target(&instruction) {
            direct_control_flow_entries.insert(target);
        }
        if instruction.mnemonic() == Mnemonic::Syscall
            || instruction.segment_prefix() == Register::FS
            || super::guest_gs::matches(&instruction)
        {
            decoded.push((offset, instruction, constants));
        }
    };
    if let Some(entries) = instruction_boundaries {
        let segment_start = ip as usize;
        let segment_end = segment_start.saturating_add(code.len());
        let entries = entries
            .iter()
            .copied()
            .filter(|address| *address >= segment_start && *address < segment_end);
        for address in entries {
            let instruction_offset = address - segment_start;
            let mut decoder = Decoder::with_ip(
                64,
                &code[instruction_offset..],
                address as u64,
                DecoderOptions::NONE,
            );
            let instruction = decoder.decode();
            if !instruction.is_invalid() && instruction.len() != 0 {
                let offsets = safe_get_constant_offsets(&decoder, &instruction);
                record(instruction_offset, instruction, offsets);
            }
        }
    } else {
        let mut decoder = Decoder::with_ip(64, code, ip, DecoderOptions::NONE);
        while decoder.can_decode() {
            let instruction_offset = decoder.position();
            let instruction = decoder.decode();
            if !instruction.is_invalid() && instruction.len() != 0 {
                let offsets = safe_get_constant_offsets(&decoder, &instruction);
                record(instruction_offset, instruction, offsets);
            }
        }
    }

    // A syscall detour may need to consume instructions after the two-byte
    // opcode. Every direct control-flow target is therefore an entry boundary,
    // including targets after a syscall.
    for (instruction_offset, instruction, constant_offsets) in decoded {
        let address = ip as usize + instruction_offset;
        if super::guest_gs::matches(&instruction) {
            gs_sites.push((instruction_offset, instruction.len()));
            continue;
        }
        if instruction.mnemonic() == Mnemonic::Syscall {
            syscalls.push((
                instruction_offset,
                instruction.len(),
                syscall_detour_length(code, instruction_offset, address),
            ));
            report.syscall_sites.push(address);
            continue;
        }
        if instruction.segment_prefix() != Register::FS {
            continue;
        }

        let instruction_end = instruction_offset
            .checked_add(instruction.len())
            .ok_or(ExecutionError::AddressOverflow)?;
        let instruction_bytes = code
            .get(instruction_offset..instruction_end)
            .ok_or(ExecutionError::AddressOverflow)?;
        if let Some(patch) = constant_offsets.and_then(|offsets| {
            // AOT uses the canonical guest TCB for all accesses. In particular,
            // a canary must not be redirected to a separate TEB value: programs
            // can also change it through a pointer to the TCB.
            plan_patch(
                &instruction,
                offsets,
                instruction_bytes,
                canary_target,
                None,
            )
        }) {
            apply_patch(&mut code[instruction_offset..instruction_end], patch);
            report.canary_sites += 1;
            cached_canary.push(crate::execution::aot_cache::CachedCanarySite {
                offset: instruction_offset as u32,
                len: instruction.len() as u8,
                prefix_offset: patch.prefix_offset as u8,
                displacement_offset: patch.displacement_offset as u8,
                displacement_size: patch.displacement_size as u8,
                original_displacement: patch.original_displacement as u32,
            });
            continue;
        }

        let site = PatchedSite {
            address,
            original_displacement: instruction.memory_displacement64() as u32,
            new_displacement: instruction.memory_displacement64() as u32,
        };
        // A short FS instruction may consume following whole instructions to
        // make room for E9. Safety against independent entries is checked after
        // every target and syscall address in this segment has been collected.
        if let (Some(overwritten), Some(_), Some(_)) = (
            crate::execution::instruction_trampoline::fs_detour_length(
                code,
                instruction_offset,
                address,
            ),
            thread_pointer_teb_offset,
            _scratch_teb_offset,
        ) {
            thread_pointer_sites.push((instruction_offset, instruction.len(), overwritten, site));
        } else {
            report.other_sites.push(site);
        }
    }
    let mut syscall_addresses = syscalls
        .iter()
        .map(|(instruction_offset, _, _)| ip as usize + *instruction_offset)
        .collect::<HashSet<_>>();
    // No other detour may relocate a virtual GS instruction as native code.
    syscall_addresses.extend(gs_sites.iter().map(|(offset, _)| ip as usize + offset));
    let mut syscall_detour_ranges = Vec::new();
    for (instruction_offset, length, detour_len) in syscalls {
        let address = ip as usize + instruction_offset;
        let detour_is_safe = detour_len.is_some_and(|overwritten| {
            detour_is_safe(
                address,
                length,
                overwritten,
                independent_entries,
                &direct_control_flow_entries,
                &syscall_addresses,
            )
        });
        let use_trampoline =
            matches!(syscall_policy, AotSyscallPolicy::Trampoline(_)) && detour_is_safe;
        cached_syscalls.push(crate::execution::aot_cache::CachedSyscallSite {
            offset: instruction_offset as u32,
            len: length as u8,
            trampoline: use_trampoline,
        });
        match syscall_policy {
            AotSyscallPolicy::Nop => {
                let bytes = &mut code[instruction_offset..instruction_offset + length];
                bytes.fill(0x90);
            }
            AotSyscallPolicy::Trap => {
                let bytes = &mut code[instruction_offset..instruction_offset + length];
                bytes.fill(0x90);
                if bytes.len() >= 2 {
                    bytes[..2].copy_from_slice(&[0x0f, 0x0b]);
                }
            }
            AotSyscallPolicy::Trampoline(dispatcher) => {
                if use_trampoline {
                    let site = unsafe {
                        crate::execution::instruction_trampoline::install_syscall_unpublished(
                            address,
                            dispatcher,
                            thread_pointer_teb_offset,
                            host_transition_teb_offset,
                            &unpublished,
                        )
                    }?;
                    syscall_detour_ranges.push(address..address + site.overwritten);
                    report.trampoline_sites.push(site);
                } else {
                    let bytes = &mut code[instruction_offset..instruction_offset + length];
                    bytes.fill(0x90);
                    if bytes.len() >= 2 {
                        bytes[..2].copy_from_slice(&[0x0f, 0x0b]);
                    }
                }
            }
        }
    }
    if let (Some(thread_pointer_teb_offset), Some(scratch_teb_offset)) =
        (thread_pointer_teb_offset, _scratch_teb_offset)
    {
        // Both CFG-boundary and linear decoding visit addresses in ascending
        // order. A successful detour can therefore overlap only the next sites,
        // never a site beyond the end of the last accepted detour. Searching all
        // earlier ranges made cold preparation quadratic in the TLS site count.
        let mut thread_pointer_detour_end = 0usize;
        for (instruction_offset, instruction_len, planned_overwritten, site) in thread_pointer_sites
        {
            // A syscall trampoline relocates every instruction it consumes. If
            // this FS access is in that relocated tail, the syscall bridge has
            // already restored FS before executing it and the original bytes are
            // now an intentional part of the E9 source range.
            if syscall_detour_ranges
                .iter()
                .any(|range| range.contains(&site.address))
            {
                continue;
            }
            if site.address < thread_pointer_detour_end {
                continue;
            }
            if !detour_is_safe(
                site.address,
                instruction_len,
                planned_overwritten,
                independent_entries,
                &direct_control_flow_entries,
                &syscall_addresses,
            ) {
                report.other_sites.push(site);
                continue;
            }
            let trampoline = unsafe {
                crate::execution::instruction_trampoline::install_unpublished(
                    site.address,
                    thread_pointer_teb_offset,
                    scratch_teb_offset,
                    &unpublished,
                )
            }?;
            if trampoline.overwritten != planned_overwritten {
                return Err(ExecutionError::AddressOverflow);
            }
            thread_pointer_detour_end = site.address + trampoline.overwritten;
            report.trampoline_sites.push(trampoline);
            cached_thread_pointer.push(crate::execution::aot_cache::CachedThreadPointerSite {
                offset: instruction_offset as u32,
                len: instruction_len as u8,
            });
        }
    }
    for (offset, length) in gs_sites {
        let target = host_transition_teb_offset.ok_or(ExecutionError::AddressOverflow)?;
        let site =
            unsafe { super::guest_gs::install(ip as usize + offset, target) }.map_err(|error| {
                ExecutionError::InstructionPatch {
                    address: ip as usize + offset,
                    detail: format!("GS at segment offset {offset:#x}: {error:?}"),
                }
            })?;
        report.trampoline_sites.push(site);
        cached_thread_pointer.push(crate::execution::aot_cache::CachedThreadPointerSite {
            offset: offset as u32,
            len: length as u8 | 0x80,
        });
    }
    unpublished.finish()?;
    Ok((cached_canary, cached_syscalls, cached_thread_pointer))
}

fn detour_is_safe(
    address: usize,
    instruction_len: usize,
    overwritten: usize,
    independent_entries: Option<&HashSet<usize>>,
    direct_control_flow_entries: &HashSet<usize>,
    syscall_addresses: &HashSet<usize>,
) -> bool {
    !independent_entries.is_some_and(|entries| {
        detour_tail_overlaps_entry(address, instruction_len, overwritten, entries)
    }) && !detour_tail_overlaps_entry(
        address,
        instruction_len,
        overwritten,
        direct_control_flow_entries,
    ) && !detour_tail_overlaps_entry(address, instruction_len, overwritten, syscall_addresses)
}

fn detour_tail_overlaps_entry(
    address: usize,
    instruction_len: usize,
    overwritten: usize,
    entries: &HashSet<usize>,
) -> bool {
    let (Some(covered_start), Some(covered_end)) = (
        address.checked_add(instruction_len),
        address.checked_add(overwritten),
    ) else {
        return true;
    };
    let tail_len = covered_end.saturating_sub(covered_start);
    // Detours cover only a few bytes, while a large executable can have hundreds
    // of thousands of protected entries. Scanning the full set for every TLS or
    // syscall site made cold linking quadratic. Query the shorter side without
    // changing the exact half-open interval or relaxing any safety check.
    if tail_len <= entries.len() {
        (covered_start..covered_end).any(|entry| entries.contains(&entry))
    } else {
        entries
            .iter()
            .any(|entry| *entry >= covered_start && *entry < covered_end)
    }
}

fn direct_control_flow_target(instruction: &Instruction) -> Option<usize> {
    match instruction.flow_control() {
        FlowControl::Call if instruction.is_call_near() => {
            Some(instruction.near_branch_target() as usize)
        }
        FlowControl::ConditionalBranch => Some(instruction.near_branch_target() as usize),
        FlowControl::UnconditionalBranch if instruction.is_jmp_short_or_near() => {
            Some(instruction.near_branch_target() as usize)
        }
        FlowControl::XbeginXabortXend => Some(instruction.near_branch_target() as usize),
        _ => None,
    }
}

struct ControlFlowEntries {
    /// Only patch candidates reached at exact instruction boundaries. Ordinary
    /// instructions were already decoded during CFG traversal; decoding them a
    /// second time adds no information to the patch planner.
    patch_boundaries: Vec<usize>,
    /// Function roots and explicit branch destinations. A detour may relocate
    /// ordinary fall-through instructions, but must never cover one of these.
    independent_entries: HashSet<usize>,
}

/// Returns code-island starts separated by an INT3 padding run.
///
/// Linkers use repeated INT3 bytes between independently addressable routines,
/// especially when local symbols have been stripped and a routine is reached
/// only through an indirect call.  Recursive traversal cannot cross such a
/// call, but the padding is an architectural, fail-stop boundary: execution can
/// neither fall through it nor begin in its middle.  Four bytes avoids treating
/// an intentional single trap inside a routine as a new root.
fn trap_padded_code_entries(code: &[u8], base: usize) -> Vec<usize> {
    const MIN_PADDING: usize = 4;
    let mut entries = Vec::new();
    let mut run = 0usize;
    for (offset, byte) in code.iter().copied().enumerate() {
        if byte == 0xcc {
            run += 1;
            continue;
        }
        if run >= MIN_PADDING {
            entries.push(base + offset);
        }
        run = 0;
    }
    entries
}

/// Finds instruction boundaries and independent entries reachable from
/// loader-visible code roots.
fn protected_instruction_entries(
    object: &Image<'_>,
    segments: &[(usize, usize)],
) -> Result<ControlFlowEntries, ExecutionError> {
    let mut queue = VecDeque::new();
    let elf = object.elf()?;
    let entry = elf.header().entry;
    // Stripped indirect-call targets still have authoritative unwind roots.
    // Padding alone misses functions aligned after only one to three INT3s.
    queue.extend(super::code_roots::unwind_entries(object)?);
    if entry != 0 {
        if let Ok(addr) = object.resolve_address(entry) {
            queue.push_back(addr);
        }
    }
    for address in [object.dynamic.init, object.dynamic.fini]
        .into_iter()
        .flatten()
    {
        if let Ok(addr) = object.resolve_address(address) {
            queue.push_back(addr);
        }
    }
    for symbol in elf.symbols()? {
        if symbol.is_defined()
            && matches!(
                symbol.symbol_type(),
                kinakaze_elf::STT_FUNC | kinakaze_elf::STT_GNU_IFUNC
            )
            && symbol.value != 0
            && let Ok(address) = object.resolve_address(symbol.value)
        {
            queue.push_back(address);
        }
    }
    for table in [
        object.dynamic.preinit_array,
        object.dynamic.init_array,
        object.dynamic.fini_array,
    ]
    .into_iter()
    .flatten()
    {
        let Some(size) = table.size else {
            continue;
        };
        let count = usize::try_from(size / 8).map_err(|_| ExecutionError::AddressOverflow)?;
        let table_address = object.resolve_address(table.address)?;
        for index in 0..count {
            let address = unsafe { ((table_address + index * 8) as *const usize).read_unaligned() };
            if address != 0 {
                queue.push_back(address);
            }
        }
    }

    // Stripped objects may expose indirect-call targets only as runtime data.
    // Repeated INT3 linker padding gives those executable islands an exact,
    // language-independent root without byte-pattern guessing inside code.
    for &(segment_start, segment_len) in segments {
        let code = unsafe { std::slice::from_raw_parts(segment_start as *const u8, segment_len) };
        queue.extend(trap_padded_code_entries(code, segment_start));
    }

    trace_control_flow(segments, queue, Some(object))
}

fn trace_control_flow(
    segments: &[(usize, usize)],
    mut queue: VecDeque<usize>,
    object: Option<&Image<'_>>,
) -> Result<ControlFlowEntries, ExecutionError> {
    let mut protected = queue.iter().copied().collect::<HashSet<_>>();
    let mut visited = CodeBoundaries::new(segments)?;
    let mut patch_boundaries = Vec::new();
    while let Some(root) = queue.pop_front() {
        if visited.contains(root) {
            continue;
        }
        let Some(&(segment_start, segment_len)) = segments
            .iter()
            .find(|(start, len)| root >= *start && root < start.saturating_add(*len))
        else {
            continue;
        };
        let segment_end = segment_start.saturating_add(segment_len);
        let slice = unsafe { std::slice::from_raw_parts(root as *const u8, segment_end - root) };
        let mut decoder = Decoder::with_ip(64, slice, root as u64, DecoderOptions::NONE);
        let mut recent = VecDeque::with_capacity(16);

        while decoder.can_decode() {
            let cur_ip = decoder.ip() as usize;
            // Converging branches and symbol roots share the same suffix.
            // Its instructions and outgoing edges were already visited; do
            // not decode that suffix again for every predecessor.
            if !visited.insert(cur_ip) {
                break;
            }
            let instruction = decoder.decode();
            if instruction.is_invalid() || instruction.len() == 0 {
                break;
            }
            if recent.len() == 16 {
                recent.pop_front();
            }
            recent.push_back(instruction);
            if instruction.mnemonic() == Mnemonic::Syscall
                || instruction.segment_prefix() == Register::FS
                || super::guest_gs::matches(&instruction)
            {
                patch_boundaries.push(cur_ip);
            }
            // Linux `syscall` is a call-like operation that normally resumes at
            // the following instruction. iced-x86 classifies it as Interrupt,
            // which is appropriate for a generic decoder but would truncate our
            // Linux CFG and hide branch targets in the return/error path.
            if instruction.mnemonic() == Mnemonic::Syscall {
                continue;
            }
            match instruction.flow_control() {
                FlowControl::Next | FlowControl::IndirectCall => {}
                FlowControl::Call => {
                    if instruction.is_call_near() {
                        let target = instruction.near_branch_target() as usize;
                        protected.insert(target);
                        if !visited.contains(target) {
                            queue.push_back(target);
                        }
                    }
                }
                FlowControl::ConditionalBranch => {
                    let target = instruction.near_branch_target() as usize;
                    protected.insert(target);
                    if !visited.contains(target) {
                        queue.push_back(target);
                    }
                }
                FlowControl::UnconditionalBranch => {
                    if instruction.is_jmp_short_or_near() {
                        let target = instruction.near_branch_target() as usize;
                        protected.insert(target);
                        if !visited.contains(target) {
                            queue.push_back(target);
                        }
                    }
                    break;
                }
                FlowControl::XbeginXabortXend => {
                    let target = instruction.near_branch_target() as usize;
                    protected.insert(target);
                    if !visited.contains(target) {
                        queue.push_back(target);
                    }
                }
                FlowControl::IndirectBranch => {
                    for target in object.into_iter().flat_map(|object| {
                        super::code_roots::relative_switch_entries(
                            object,
                            &recent,
                            segments,
                            |address| visited.contains(address),
                        )
                    }) {
                        protected.insert(target);
                        if !visited.contains(target) {
                            queue.push_back(target);
                        }
                    }
                    break;
                }
                FlowControl::Return | FlowControl::Interrupt | FlowControl::Exception => {
                    break;
                }
            }
        }
    }
    // The overlap planner relies on ascending source order. Each instruction
    // was visited once even when multiple roots converge on the same suffix.
    patch_boundaries.sort_unstable();
    Ok(ControlFlowEntries {
        patch_boundaries,
        independent_entries: protected,
    })
}

/// Number of bytes a five-byte near detour would have to consume beginning at a
/// two-byte syscall. The snapshot is decoded before any site is modified.
fn syscall_detour_length(code: &[u8], offset: usize, address: usize) -> Option<usize> {
    let bytes = code.get(offset..)?;
    let mut decoder = Decoder::with_ip(64, bytes, address as u64, DecoderOptions::NONE);
    let first = decoder.decode();
    if first.is_invalid() || first.mnemonic() != Mnemonic::Syscall {
        return None;
    }
    let mut overwritten = first.len();
    let mut path_terminated = false;
    while overwritten < 5 {
        let instruction = decoder.decode();
        if instruction.is_invalid() || instruction.len() == 0 {
            return None;
        }
        if path_terminated && !matches!(instruction.mnemonic(), Mnemonic::Nop | Mnemonic::Int3) {
            return None;
        }
        overwritten = overwritten.checked_add(instruction.len())?;
        if matches!(
            instruction.flow_control(),
            FlowControl::UnconditionalBranch | FlowControl::IndirectBranch | FlowControl::Return
        ) {
            // Terminal syscall wrappers commonly end in `ret` or a short jump,
            // followed by alignment INT3/NOP bytes. Those bytes are not another
            // code path and may safely supply the remainder of E9's five-byte
            // source window. Any real instruction after the terminator is still
            // rejected, and the caller separately rejects protected entries.
            path_terminated = true;
        } else if overwritten < 5
            && matches!(
                instruction.flow_control(),
                FlowControl::Interrupt | FlowControl::Exception
            )
            && instruction.mnemonic() != Mnemonic::Int3
        {
            return None;
        }
    }
    Some(overwritten)
}

pub(crate) fn plan_patch(
    instruction: &Instruction,
    offsets: ConstantOffsets,
    bytes: &[u8],
    _canary_target: u32,
    thread_pointer_target: Option<u32>,
) -> Option<InstructionPatch> {
    // Only a base-less, index-less segment-relative address can be translated by
    // changing the segment and its constant displacement. Register-based static
    // TLS needs a real guest TLS block and must not silently become TEB-relative.
    if instruction.segment_prefix() != Register::FS
        || instruction.memory_base() != Register::None
        || instruction.memory_index() != Register::None
    {
        return None;
    }

    let original = instruction.memory_displacement64();
    let new = if original == u64::from(CANARY_DISPLACEMENT) {
        // Absolute FS loads/stores, register-relative FS instructions and direct
        // TCB pointer accesses must all observe the same memory. A TEB mirror
        // becomes stale when Chromium resets its guard, or when a dynamically
        // decoded instruction takes the general TLS trampoline path.
        return None;
    } else if original == 0
        && instruction.mnemonic() == Mnemonic::Mov
        && instruction.op0_kind() == iced_x86::OpKind::Register
        && instruction.op1_kind() == iced_x86::OpKind::Memory
        && instruction.memory_size().size() == 8
    {
        // Linux `%fs:0` is the first word of the guest TCB. Both glibc and
        // musl place the TCB self-pointer there, and the process coordinator's
        // direct TEB slot publishes that exact guest pointer. Windows
        // `TEB.Self` is a different object and must never escape to the guest.
        u64::from(thread_pointer_target?)
    } else {
        return None;
    };

    let displacement_size = offsets.displacement_size();
    let displacement_offset = offsets.displacement_offset();
    if !matches!(displacement_size, 1 | 2 | 4 | 8)
        || displacement_offset.checked_add(displacement_size)? > bytes.len()
        || (displacement_size < 8 && new >= (1u64 << (displacement_size * 8)))
    {
        return None;
    }

    Some(InstructionPatch {
        prefix_offset: fs_prefix_offset(bytes)?,
        displacement_offset,
        displacement_size,
        original_displacement: original,
        new_displacement: new,
    })
}

/// Finds an actual FS legacy prefix, stopping before the opcode.
///
/// iced-x86 has already established that the instruction's effective override is
/// FS; restricting this scan to the architectural prefix area prevents an 0x64
/// byte in an immediate or displacement from ever being rewritten.
fn fs_prefix_offset(bytes: &[u8]) -> Option<usize> {
    let mut found = None;
    for (offset, byte) in bytes.iter().copied().enumerate() {
        match byte {
            0x26 | 0x2e | 0x36 | 0x3e | PREFIX_FS | PREFIX_GS | 0x66 | 0x67 | 0xf0 | 0xf2
            | 0xf3 => {
                if byte == PREFIX_FS {
                    found = Some(offset);
                }
            }
            // A REX prefix is the final prefix in a valid encoding.
            0x40..=0x4f => break,
            _ => break,
        }
    }
    found
}

pub(crate) fn apply_patch(bytes: &mut [u8], patch: InstructionPatch) {
    let encoded = patch.new_displacement.to_le_bytes();
    bytes[patch.displacement_offset..patch.displacement_offset + patch.displacement_size]
        .copy_from_slice(&encoded[..patch.displacement_size]);
    // Publish the prefix last. If another thread executes concurrently it sees
    // either the old FS instruction (and faults into VEH) or the complete GS form.
    bytes[patch.prefix_offset] = PREFIX_GS;
}

/// VEH fallback for code that was generated or made executable after AOT linking.
///
#[cfg(windows)]
unsafe fn get_reg_val(
    ctx: &windows_sys::Win32::System::Diagnostics::Debug::CONTEXT,
    reg: Register,
) -> u64 {
    match reg {
        Register::RAX => ctx.Rax,
        Register::RBX => ctx.Rbx,
        Register::RCX => ctx.Rcx,
        Register::RDX => ctx.Rdx,
        Register::RSI => ctx.Rsi,
        Register::RDI => ctx.Rdi,
        Register::RBP => ctx.Rbp,
        Register::RSP => ctx.Rsp,
        Register::R8 => ctx.R8,
        Register::R9 => ctx.R9,
        Register::R10 => ctx.R10,
        Register::R11 => ctx.R11,
        Register::R12 => ctx.R12,
        Register::R13 => ctx.R13,
        Register::R14 => ctx.R14,
        Register::R15 => ctx.R15,
        Register::EAX => ctx.Rax as u32 as u64,
        Register::EBX => ctx.Rbx as u32 as u64,
        Register::ECX => ctx.Rcx as u32 as u64,
        Register::EDX => ctx.Rdx as u32 as u64,
        Register::ESI => ctx.Rsi as u32 as u64,
        Register::EDI => ctx.Rdi as u32 as u64,
        Register::EBP => ctx.Rbp as u32 as u64,
        Register::ESP => ctx.Rsp as u32 as u64,
        Register::R8D => ctx.R8 as u32 as u64,
        Register::R9D => ctx.R9 as u32 as u64,
        Register::R10D => ctx.R10 as u32 as u64,
        Register::R11D => ctx.R11 as u32 as u64,
        Register::R12D => ctx.R12 as u32 as u64,
        Register::R13D => ctx.R13 as u32 as u64,
        Register::R14D => ctx.R14 as u32 as u64,
        Register::R15D => ctx.R15 as u32 as u64,
        Register::AX => ctx.Rax as u16 as u64,
        Register::BX => ctx.Rbx as u16 as u64,
        Register::CX => ctx.Rcx as u16 as u64,
        Register::DX => ctx.Rdx as u16 as u64,
        Register::SI => ctx.Rsi as u16 as u64,
        Register::DI => ctx.Rdi as u16 as u64,
        Register::BP => ctx.Rbp as u16 as u64,
        Register::SP => ctx.Rsp as u16 as u64,
        Register::R8W => ctx.R8 as u16 as u64,
        Register::R9W => ctx.R9 as u16 as u64,
        Register::R10W => ctx.R10 as u16 as u64,
        Register::R11W => ctx.R11 as u16 as u64,
        Register::R12W => ctx.R12 as u16 as u64,
        Register::R13W => ctx.R13 as u16 as u64,
        Register::R14W => ctx.R14 as u16 as u64,
        Register::R15W => ctx.R15 as u16 as u64,
        Register::AL => ctx.Rax as u8 as u64,
        Register::BL => ctx.Rbx as u8 as u64,
        Register::CL => ctx.Rcx as u8 as u64,
        Register::DL => ctx.Rdx as u8 as u64,
        Register::SIL => ctx.Rsi as u8 as u64,
        Register::DIL => ctx.Rdi as u8 as u64,
        Register::BPL => ctx.Rbp as u8 as u64,
        Register::SPL => ctx.Rsp as u8 as u64,
        Register::R8L => ctx.R8 as u8 as u64,
        Register::R9L => ctx.R9 as u8 as u64,
        Register::R10L => ctx.R10 as u8 as u64,
        Register::R11L => ctx.R11 as u8 as u64,
        Register::R12L => ctx.R12 as u8 as u64,
        Register::R13L => ctx.R13 as u8 as u64,
        Register::R14L => ctx.R14 as u8 as u64,
        Register::R15L => ctx.R15 as u8 as u64,
        _ => 0,
    }
}

#[cfg(windows)]
unsafe fn set_reg_val(
    ctx: &mut windows_sys::Win32::System::Diagnostics::Debug::CONTEXT,
    reg: Register,
    val: u64,
) {
    match reg {
        Register::RAX => ctx.Rax = val,
        Register::RBX => ctx.Rbx = val,
        Register::RCX => ctx.Rcx = val,
        Register::RDX => ctx.Rdx = val,
        Register::RSI => ctx.Rsi = val,
        Register::RDI => ctx.Rdi = val,
        Register::RBP => ctx.Rbp = val,
        Register::RSP => ctx.Rsp = val,
        Register::R8 => ctx.R8 = val,
        Register::R9 => ctx.R9 = val,
        Register::R10 => ctx.R10 = val,
        Register::R11 => ctx.R11 = val,
        Register::R12 => ctx.R12 = val,
        Register::R13 => ctx.R13 = val,
        Register::R14 => ctx.R14 = val,
        Register::R15 => ctx.R15 = val,
        Register::EAX => ctx.Rax = val as u32 as u64,
        Register::EBX => ctx.Rbx = val as u32 as u64,
        Register::ECX => ctx.Rcx = val as u32 as u64,
        Register::EDX => ctx.Rdx = val as u32 as u64,
        Register::ESI => ctx.Rsi = val as u32 as u64,
        Register::EDI => ctx.Rdi = val as u32 as u64,
        Register::EBP => ctx.Rbp = val as u32 as u64,
        Register::ESP => ctx.Rsp = val as u32 as u64,
        Register::R8D => ctx.R8 = val as u32 as u64,
        Register::R9D => ctx.R9 = val as u32 as u64,
        Register::R10D => ctx.R10 = val as u32 as u64,
        Register::R11D => ctx.R11 = val as u32 as u64,
        Register::R12D => ctx.R12 = val as u32 as u64,
        Register::R13D => ctx.R13 = val as u32 as u64,
        Register::R14D => ctx.R14 = val as u32 as u64,
        Register::R15D => ctx.R15 = val as u32 as u64,
        Register::AX => ctx.Rax = (ctx.Rax & !0xffff) | (val & 0xffff),
        Register::BX => ctx.Rbx = (ctx.Rbx & !0xffff) | (val & 0xffff),
        Register::CX => ctx.Rcx = (ctx.Rcx & !0xffff) | (val & 0xffff),
        Register::DX => ctx.Rdx = (ctx.Rdx & !0xffff) | (val & 0xffff),
        Register::SI => ctx.Rsi = (ctx.Rsi & !0xffff) | (val & 0xffff),
        Register::DI => ctx.Rdi = (ctx.Rdi & !0xffff) | (val & 0xffff),
        Register::BP => ctx.Rbp = (ctx.Rbp & !0xffff) | (val & 0xffff),
        Register::SP => ctx.Rsp = (ctx.Rsp & !0xffff) | (val & 0xffff),
        Register::R8W => ctx.R8 = (ctx.R8 & !0xffff) | (val & 0xffff),
        Register::R9W => ctx.R9 = (ctx.R9 & !0xffff) | (val & 0xffff),
        Register::R10W => ctx.R10 = (ctx.R10 & !0xffff) | (val & 0xffff),
        Register::R11W => ctx.R11 = (ctx.R11 & !0xffff) | (val & 0xffff),
        Register::R12W => ctx.R12 = (ctx.R12 & !0xffff) | (val & 0xffff),
        Register::R13W => ctx.R13 = (ctx.R13 & !0xffff) | (val & 0xffff),
        Register::R14W => ctx.R14 = (ctx.R14 & !0xffff) | (val & 0xffff),
        Register::R15W => ctx.R15 = (ctx.R15 & !0xffff) | (val & 0xffff),
        Register::AL => ctx.Rax = (ctx.Rax & !0xff) | (val & 0xff),
        Register::BL => ctx.Rbx = (ctx.Rbx & !0xff) | (val & 0xff),
        Register::CL => ctx.Rcx = (ctx.Rcx & !0xff) | (val & 0xff),
        Register::DL => ctx.Rdx = (ctx.Rdx & !0xff) | (val & 0xff),
        Register::SIL => ctx.Rsi = (ctx.Rsi & !0xff) | (val & 0xff),
        Register::DIL => ctx.Rdi = (ctx.Rdi & !0xff) | (val & 0xff),
        Register::BPL => ctx.Rbp = (ctx.Rbp & !0xff) | (val & 0xff),
        Register::SPL => ctx.Rsp = (ctx.Rsp & !0xff) | (val & 0xff),
        Register::R8L => ctx.R8 = (ctx.R8 & !0xff) | (val & 0xff),
        Register::R9L => ctx.R9 = (ctx.R9 & !0xff) | (val & 0xff),
        Register::R10L => ctx.R10 = (ctx.R10 & !0xff) | (val & 0xff),
        Register::R11L => ctx.R11 = (ctx.R11 & !0xff) | (val & 0xff),
        Register::R12L => ctx.R12 = (ctx.R12 & !0xff) | (val & 0xff),
        Register::R13L => ctx.R13 = (ctx.R13 & !0xff) | (val & 0xff),
        Register::R14L => ctx.R14 = (ctx.R14 & !0xff) | (val & 0xff),
        Register::R15L => ctx.R15 = (ctx.R15 & !0xff) | (val & 0xff),
        _ => {}
    }
}

#[cfg(windows)]
unsafe fn emulate_fs_instruction(
    ctx: *mut windows_sys::Win32::System::Diagnostics::Debug::CONTEXT,
    instruction: &Instruction,
    thread_pointer_teb_offset: usize,
) -> bool {
    let ctx_ref = unsafe { &mut *ctx };
    let mut linux_tp: usize = 0;
    unsafe {
        let teb_ptr: usize;
        std::arch::asm!("mov {0}, gs:[0x30]", out(reg) teb_ptr);
        if teb_ptr != 0 {
            linux_tp = *((teb_ptr + thread_pointer_teb_offset) as *const usize);
        }
    }
    if linux_tp == 0 {
        return false;
    }

    let raw_disp = instruction.memory_displacement64();
    let disp: i64 = if instruction.memory_displ_size() <= 4 || raw_disp <= 0xffff_ffff {
        (raw_disp as u32 as i32) as i64
    } else {
        raw_disp as i64
    };
    let base_val = if instruction.memory_base() != Register::None {
        unsafe { get_reg_val(ctx_ref, instruction.memory_base()) }
    } else {
        0
    };
    let index_val = if instruction.memory_index() != Register::None {
        unsafe { get_reg_val(ctx_ref, instruction.memory_index()) }
            .wrapping_mul(instruction.memory_index_scale() as u64)
    } else {
        0
    };
    let target_addr = (linux_tp as u64)
        .wrapping_add(disp as u64)
        .wrapping_add(base_val)
        .wrapping_add(index_val) as usize;

    if std::env::var_os("KINAKAZE_REPORT_TRAPS").is_some() {
        eprintln!(
            "kinakaze: [EMULATE_FS] rip={:#x} tp={:#x} disp={:#x} ({}) target={:#x} mnem={:?} op0={:?} op1={:?}",
            ctx_ref.Rip,
            linux_tp,
            raw_disp,
            disp,
            target_addr,
            instruction.mnemonic(),
            instruction.op0_kind(),
            instruction.op1_kind()
        );
    }

    match instruction.mnemonic() {
        Mnemonic::Mov => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let val = match instruction.op0_register().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), val) };
            } else if instruction.op1_kind() == iced_x86::OpKind::Register {
                let val = unsafe { get_reg_val(ctx_ref, instruction.op1_register()) };
                match instruction.op1_register().size() {
                    1 => unsafe { *(target_addr as *mut u8) = val as u8 },
                    2 => unsafe { *(target_addr as *mut u16) = val as u16 },
                    4 => unsafe { *(target_addr as *mut u32) = val as u32 },
                    8 => unsafe { *(target_addr as *mut u64) = val },
                    _ => return false,
                }
            } else if matches!(
                instruction.op1_kind(),
                iced_x86::OpKind::Immediate8
                    | iced_x86::OpKind::Immediate16
                    | iced_x86::OpKind::Immediate32
                    | iced_x86::OpKind::Immediate64
                    | iced_x86::OpKind::Immediate8to16
                    | iced_x86::OpKind::Immediate8to32
                    | iced_x86::OpKind::Immediate8to64
                    | iced_x86::OpKind::Immediate32to64
            ) {
                let imm = instruction.immediate64();
                match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *mut u8) = imm as u8 },
                    2 => unsafe { *(target_addr as *mut u16) = imm as u16 },
                    4 => unsafe { *(target_addr as *mut u32) = imm as u32 },
                    8 => unsafe { *(target_addr as *mut u64) = imm },
                    _ => return false,
                }
            } else {
                return false;
            }
        }
        Mnemonic::Lea => {
            unsafe { set_reg_val(ctx_ref, instruction.op0_register(), target_addr as u64) };
        }
        Mnemonic::Cmp => {
            let mem_val = match instruction.memory_size().size() {
                1 => unsafe { *(target_addr as *const u8) as u64 },
                2 => unsafe { *(target_addr as *const u16) as u64 },
                4 => unsafe { *(target_addr as *const u32) as u64 },
                8 => unsafe { *(target_addr as *const u64) },
                _ => return false,
            };
            let other_val = if instruction.op0_kind() == iced_x86::OpKind::Register {
                unsafe { get_reg_val(ctx_ref, instruction.op0_register()) }
            } else if instruction.op1_kind() == iced_x86::OpKind::Register {
                unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
            } else {
                instruction.immediate64()
            };
            let (res, overflow) = (other_val as i64).overflowing_sub(mem_val as i64);
            let mut eflags = ctx_ref.EFlags;
            if res == 0 {
                eflags |= 0x40;
            } else {
                eflags &= !0x40;
            }
            if res < 0 {
                eflags |= 0x80;
            } else {
                eflags &= !0x80;
            }
            if overflow {
                eflags |= 0x800;
            } else {
                eflags &= !0x800;
            }
            if other_val < mem_val {
                eflags |= 0x1;
            } else {
                eflags &= !0x1;
            }
            ctx_ref.EFlags = eflags;
        }
        Mnemonic::And => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let mem_val = match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                let reg_val = unsafe { get_reg_val(ctx_ref, instruction.op0_register()) };
                let res = reg_val & mem_val;
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), res) };
            } else {
                let imm = if instruction.op1_kind() == iced_x86::OpKind::Register {
                    unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
                } else {
                    instruction.immediate64()
                };
                match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *mut u8) &= imm as u8 },
                    2 => unsafe { *(target_addr as *mut u16) &= imm as u16 },
                    4 => unsafe { *(target_addr as *mut u32) &= imm as u32 },
                    8 => unsafe { *(target_addr as *mut u64) &= imm },
                    _ => return false,
                }
            }
        }
        Mnemonic::Or => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let mem_val = match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                let reg_val = unsafe { get_reg_val(ctx_ref, instruction.op0_register()) };
                let res = reg_val | mem_val;
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), res) };
            } else {
                let imm = if instruction.op1_kind() == iced_x86::OpKind::Register {
                    unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
                } else {
                    instruction.immediate64()
                };
                match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *mut u8) |= imm as u8 },
                    2 => unsafe { *(target_addr as *mut u16) |= imm as u16 },
                    4 => unsafe { *(target_addr as *mut u32) |= imm as u32 },
                    8 => unsafe { *(target_addr as *mut u64) |= imm },
                    _ => return false,
                }
            }
        }
        Mnemonic::Xor => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let mem_val = match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                let reg_val = unsafe { get_reg_val(ctx_ref, instruction.op0_register()) };
                let res = reg_val ^ mem_val;
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), res) };
            } else {
                let imm = if instruction.op1_kind() == iced_x86::OpKind::Register {
                    unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
                } else {
                    instruction.immediate64()
                };
                match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *mut u8) ^= imm as u8 },
                    2 => unsafe { *(target_addr as *mut u16) ^= imm as u16 },
                    4 => unsafe { *(target_addr as *mut u32) ^= imm as u32 },
                    8 => unsafe { *(target_addr as *mut u64) ^= imm },
                    _ => return false,
                }
            }
        }
        Mnemonic::Add => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let mem_val = match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                let reg_val = unsafe { get_reg_val(ctx_ref, instruction.op0_register()) };
                let res = reg_val.wrapping_add(mem_val);
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), res) };
            } else {
                let imm = if instruction.op1_kind() == iced_x86::OpKind::Register {
                    unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
                } else {
                    instruction.immediate64()
                };
                match instruction.memory_size().size() {
                    1 => unsafe {
                        *(target_addr as *mut u8) =
                            (*(target_addr as *const u8)).wrapping_add(imm as u8)
                    },
                    2 => unsafe {
                        *(target_addr as *mut u16) =
                            (*(target_addr as *const u16)).wrapping_add(imm as u16)
                    },
                    4 => unsafe {
                        *(target_addr as *mut u32) =
                            (*(target_addr as *const u32)).wrapping_add(imm as u32)
                    },
                    8 => unsafe {
                        *(target_addr as *mut u64) =
                            (*(target_addr as *const u64)).wrapping_add(imm)
                    },
                    _ => return false,
                }
            }
        }
        Mnemonic::Sub => {
            if instruction.op0_kind() == iced_x86::OpKind::Register {
                let mem_val = match instruction.memory_size().size() {
                    1 => unsafe { *(target_addr as *const u8) as u64 },
                    2 => unsafe { *(target_addr as *const u16) as u64 },
                    4 => unsafe { *(target_addr as *const u32) as u64 },
                    8 => unsafe { *(target_addr as *const u64) },
                    _ => return false,
                };
                let reg_val = unsafe { get_reg_val(ctx_ref, instruction.op0_register()) };
                let res = reg_val.wrapping_sub(mem_val);
                unsafe { set_reg_val(ctx_ref, instruction.op0_register(), res) };
            } else {
                let imm = if instruction.op1_kind() == iced_x86::OpKind::Register {
                    unsafe { get_reg_val(ctx_ref, instruction.op1_register()) }
                } else {
                    instruction.immediate64()
                };
                match instruction.memory_size().size() {
                    1 => unsafe {
                        *(target_addr as *mut u8) =
                            (*(target_addr as *const u8)).wrapping_sub(imm as u8)
                    },
                    2 => unsafe {
                        *(target_addr as *mut u16) =
                            (*(target_addr as *const u16)).wrapping_sub(imm as u16)
                    },
                    4 => unsafe {
                        *(target_addr as *mut u32) =
                            (*(target_addr as *const u32)).wrapping_sub(imm as u32)
                    },
                    8 => unsafe {
                        *(target_addr as *mut u64) =
                            (*(target_addr as *const u64)).wrapping_sub(imm)
                    },
                    _ => return false,
                }
            }
        }
        _ => return false,
    }

    ctx_ref.Rip += instruction.len() as u64;
    true
}

/// Returns the healed site when the faulting instruction was a supported absolute
/// FS access. The caller should return EXCEPTION_CONTINUE_EXECUTION so the CPU
/// retries the now-patched instruction at the same RIP.
///
/// # Safety
///
/// `ctx` must be the non-null pointer to CONTEXT supplied by Windows in an exception.
#[cfg(windows)]
pub fn warmup_decoder() {
    let mut decoder = Decoder::with_ip(64, &[0x90], 0, DecoderOptions::NONE);
    let _ = decoder.decode();
}

pub unsafe fn heal_thread_pointer_instruction(
    ctx: *mut windows_sys::Win32::System::Diagnostics::Debug::CONTEXT,
    canary_slot_offset: usize,
    thread_pointer_teb_offset: usize,
    scratch_teb_offset: usize,
) -> Option<HealedThreadPointerSite> {
    use kinakaze_runtime::memory_protection::protect_preserving_copy_on_write as VirtualProtect;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FlushInstructionCache, ReadProcessMemory,
    };
    use windows_sys::Win32::System::Memory::PAGE_EXECUTE_READWRITE;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    if ctx.is_null() {
        return None;
    }
    let address = unsafe { (*ctx).Rip as usize };
    let process = unsafe { GetCurrentProcess() };
    let mut snapshot = [0u8; 15];
    let mut read = 0usize;
    if unsafe {
        ReadProcessMemory(
            process,
            address as *const core::ffi::c_void,
            snapshot.as_mut_ptr().cast(),
            snapshot.len(),
            &raw mut read,
        )
    } == 0
        || read == 0
    {
        return None;
    }

    let mut decoder = Decoder::with_ip(64, &snapshot[..read], address as u64, DecoderOptions::NONE);
    let instruction = decoder.decode();
    if instruction.is_invalid() || instruction.segment_prefix() != Register::FS {
        return None;
    }
    let offsets = safe_get_constant_offsets(&decoder, &instruction)?;
    let length = instruction.len();
    let patch = plan_patch(
        &instruction,
        offsets,
        &snapshot[..length],
        u32::try_from(canary_slot_offset).ok()?,
        Some(u32::try_from(thread_pointer_teb_offset).ok()?),
    );

    if patch.is_none() {
        let site = unsafe {
            crate::execution::instruction_trampoline::install(
                address,
                thread_pointer_teb_offset,
                scratch_teb_offset,
            )
        };
        if let Ok(site) = site {
            return Some(HealedThreadPointerSite::Trampoline(site));
        }
        if unsafe { emulate_fs_instruction(ctx, &instruction, thread_pointer_teb_offset) } {
            return Some(HealedThreadPointerSite::Emulated);
        }
        return None;
    }
    let patch = patch?;

    let mut old_protection = 0u32;
    if unsafe {
        VirtualProtect(
            address as *const core::ffi::c_void,
            length,
            PAGE_EXECUTE_READWRITE,
            &raw mut old_protection,
        )
    } == 0
    {
        return None;
    }

    // SAFETY: VirtualProtect made precisely this instruction writable.
    let live = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, length) };
    apply_patch(live, patch);
    unsafe { FlushInstructionCache(process, address as *const core::ffi::c_void, length) };

    let mut ignored = 0u32;
    let restored = unsafe {
        VirtualProtect(
            address as *const core::ffi::c_void,
            length,
            old_protection,
            &raw mut ignored,
        )
    };
    if restored == 0 {
        return None;
    }

    Some(HealedThreadPointerSite::Patched(PatchedSite {
        address,
        original_displacement: patch.original_displacement as u32,
        new_displacement: patch.new_displacement as u32,
    }))
}

/// The TEB offset for a `TlsAlloc` index.
pub fn teb_slot_offset(index: u32) -> usize {
    TEB_TLS_SLOTS + index as usize * 8
}

/// Recovers the direct Windows TLS index encoded by a fixed TEB slot offset.
///
/// A fork child resumes guest code in a fresh Windows process, so it must
/// republish the parent's per-thread values at the same rewritten offsets.
pub fn teb_slot_index(offset: usize) -> Option<u32> {
    let relative = offset.checked_sub(TEB_TLS_SLOTS)?;
    if relative % 8 != 0 {
        return None;
    }
    let index = u32::try_from(relative / 8).ok()?;
    (index < 64).then_some(index)
}

/// Safely gets constant offsets from an instruction without panicking on malformed instructions.
pub fn safe_get_constant_offsets(
    decoder: &Decoder,
    instruction: &Instruction,
) -> Option<ConstantOffsets> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decoder.get_constant_offsets(instruction)
    }))
    .ok()
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn cfg_candidates_preserve_entries_after_syscalls_and_skip_embedded_opcodes() {
        let code = [
            0xb8u8, 0x64, 0x0f, 0x05, 0x65, // mov eax,imm32: fake FS/syscall/GS
            0x64, 0x48, 0x8b, 0x00, // mov rax,fs:[rax]
            0x90, // ordinary fallthrough consumed by a potential FS detour
            0x0f, 0x05, // syscall must not end traversal
            0x75, 0xfb, // jne to the NOP: independent entry blocks that detour
            0xc3, 0x65, 0x48, 0x8b, 0x00, // unreachable GS bytes after return
        ];
        let start = code.as_ptr() as usize;
        let control = trace_control_flow(
            &[(start, code.len())],
            VecDeque::from([start, start + 5]),
            None,
        )
        .unwrap();
        assert_eq!(control.patch_boundaries, [start + 5, start + 10]);
        assert!(control.independent_entries.contains(&(start + 9)));
        assert!(!detour_is_safe(
            start + 5,
            4,
            5,
            Some(&control.independent_entries),
            &HashSet::new(),
            &HashSet::new(),
        ));
    }

    #[test]
    fn int3_padding_exposes_stripped_indirect_code_islands() {
        let code = [
            0xc3, // preceding routine
            0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, // linker padding
            0xb8, 0x38, 0, 0, 0, 0x0f, 0x05, 0xc3, // indirect-only clone wrapper
        ];
        assert_eq!(trap_padded_code_entries(&code, 0x4000), vec![0x4008]);

        // Isolated traps are executable instructions, not island metadata.
        assert!(trap_padded_code_entries(&[0xcc, 0x90, 0xc3], 0x5000).is_empty());
    }

    #[test]
    fn canary_accesses_are_left_for_the_canonical_guest_tcb_path() {
        // The exact bytes BusyBox faulted on: `movq %fs:0x28, %rax`.
        let mut code = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0x28, 0x00, 0x00, 0x00];
        let target = teb_slot_offset(1) as u32;
        let mut report = PatchReport::default();
        patch_code(
            &mut code,
            0x1000,
            target,
            None,
            None,
            None,
            AotSyscallPolicy::Nop,
            None,
            None,
            &mut report,
        );

        assert_eq!(code[0], PREFIX_FS);
        assert_eq!(&code[1..5], &[0x48, 0x8b, 0x04, 0x25]);
        assert_eq!(&code[5..9], &0x28u32.to_le_bytes());
        assert_eq!(report.canary_sites, 0);
        assert_eq!(report.other_sites.len(), 1);
    }

    #[test]
    fn fs_zero_is_left_as_a_real_guest_tcb_access() {
        // musl's `__errno_location` begins with `movq %fs:0, %rax` and then
        // indexes its TCB. Windows TEB.Self is not a Linux thread pointer.
        let mut code = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0];
        let canary_target = teb_slot_offset(1) as u32;
        let thread_pointer_target = teb_slot_offset(2);
        let mut report = PatchReport::default();
        patch_code(
            &mut code,
            0x1000,
            canary_target,
            Some(thread_pointer_target),
            None,
            None,
            AotSyscallPolicy::Nop,
            None,
            None,
            &mut report,
        )
        .expect("fixed TLS read should patch");

        assert_eq!(code, [0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0]);
        assert_eq!(report.canary_sites, 0);
        assert_eq!(report.other_sites.len(), 1);
    }

    #[test]
    fn an_fs_byte_inside_an_immediate_is_never_patched() {
        // Node's _start contains these two immediates. The former byte scanner
        // started decoding at the 0x64 inside the first value and changed
        // __libc_csu_init from 0x26404a0 to 0x26504a0.
        let mut code = vec![
            0x49, 0xc7, 0xc0, 0x10, 0x05, 0x64, 0x02, 0x48, 0xc7, 0xc1, 0xa0, 0x04, 0x64, 0x02,
        ];
        let original = code.clone();
        let mut report = PatchReport::default();
        patch_code(
            &mut code,
            0xbd12d3,
            teb_slot_offset(1) as u32,
            None,
            None,
            None,
            AotSyscallPolicy::Nop,
            None,
            None,
            &mut report,
        );

        assert_eq!(code, original);
        assert_eq!(report.canary_sites, 0);
        assert!(report.other_sites.is_empty());
    }

    #[test]
    fn the_subtract_form_is_recognised_too() {
        // The epilogue compares with `subq %fs:0x28, %rax`, and missing it would
        // leave a canary check reading address 0x28 while the prologue read the TEB.
        let code = [0x64, 0x48, 0x2b, 0x04, 0x25, 0x28, 0x00, 0x00, 0x00];
        let is_absolute_form = code[1] == 0x48
            && (code[2] == 0x8b || code[2] == 0x2b)
            && code[3] == 0x04
            && code[4] == 0x25;
        assert!(is_absolute_form);
    }

    #[test]
    fn teb_slot_offsets_match_the_windows_layout() {
        // Verified against a live process: TlsAlloc index 1 read back through
        // gs:[0x1488] returns what TlsSetValue stored.
        assert_eq!(teb_slot_offset(0), 0x1480);
        assert_eq!(teb_slot_offset(1), 0x1488);
        assert_eq!(teb_slot_offset(63), 0x1480 + 63 * 8);
        assert_eq!(teb_slot_index(teb_slot_offset(0)), Some(0));
        assert_eq!(teb_slot_index(teb_slot_offset(63)), Some(63));
        assert_eq!(teb_slot_index(0x1478), None);
        assert_eq!(teb_slot_index(0x1481), None);
        assert_eq!(teb_slot_index(teb_slot_offset(64)), None);
    }

    #[test]
    fn terminal_syscall_wrappers_may_use_only_alignment_padding_for_detours() {
        assert_eq!(
            syscall_detour_length(&[0x0f, 0x05, 0xc3, 0xcc, 0xcc], 0, 0x1000),
            Some(5)
        );
        assert_eq!(
            syscall_detour_length(&[0x0f, 0x05, 0xeb, 0x10, 0x90], 0, 0x1000),
            Some(5)
        );
        assert_eq!(
            syscall_detour_length(&[0x0f, 0x05, 0xc3, 0x31, 0xc0], 0, 0x1000),
            None,
            "a real instruction after a terminator belongs to another path"
        );
    }

    #[test]
    fn syscall_detour_never_consumes_a_direct_branch_entry() {
        // This is the important shape used by libc clone wrappers: the second
        // syscall is immediately followed by the shared errno path, while an
        // earlier conditional branch enters that path at syscall+2.
        let code = [
            0x0f, 0x05, // syscall
            0x48, 0x85, 0xc0, // test rax,rax
            0x7c, 0x02, // jl 0x1009
            0x0f, 0x05, // syscall
            0x48, 0xc7, 0xc1, 0xb0, 0xff, 0xff, 0xff, // mov rcx,-0x50
            0xc3,
        ];
        let mut decoder = Decoder::with_ip(64, &code, 0x1000, DecoderOptions::NONE);
        let mut entries = HashSet::new();
        while decoder.can_decode() {
            let instruction = decoder.decode();
            if let Some(target) = direct_control_flow_target(&instruction) {
                entries.insert(target);
            }
        }

        assert!(entries.contains(&0x1009));
        let overwritten = syscall_detour_length(&code, 7, 0x1007).unwrap();
        assert!(detour_tail_overlaps_entry(0x1007, 2, overwritten, &entries));
    }

    #[test]
    fn syscall_detour_may_relocate_ordinary_fallthrough_instructions() {
        // Go's raw vfork wrapper has exactly this shape: syscall; push r12;
        // cmp rax, imm32. The latter two instructions are real CFG boundaries,
        // but neither can be entered independently, so the trampoline must
        // relocate both instead of forcing the returns-twice syscall through
        // Windows exception dispatch.
        let code = [
            0x0f, 0x05, // syscall
            0x41, 0x54, // push r12
            0x48, 0x3d, 0x01, 0xf0, 0xff, 0xff, // cmp rax,-4095
        ];
        let overwritten = syscall_detour_length(&code, 0, 0x1000).unwrap();
        assert_eq!(overwritten, code.len());

        let every_instruction_boundary = HashSet::from([0x1000, 0x1002, 0x1004]);
        assert!(detour_tail_overlaps_entry(
            0x1000,
            2,
            overwritten,
            &every_instruction_boundary,
        ));

        let independent_entries = HashSet::from([0x1000]);
        assert!(detour_is_safe(
            0x1000,
            2,
            overwritten,
            Some(&independent_entries),
            &HashSet::new(),
            &HashSet::from([0x1000]),
        ));
    }

    #[test]
    fn short_fs_detour_requires_an_unshared_fallthrough_tail() {
        let address = 0x2000;
        let instruction_len = 4;
        let overwritten = 9;
        assert!(detour_is_safe(
            address,
            instruction_len,
            overwritten,
            Some(&HashSet::from([address])),
            &HashSet::new(),
            &HashSet::new(),
        ));
        assert!(!detour_is_safe(
            address,
            instruction_len,
            overwritten,
            Some(&HashSet::from([address, address + instruction_len])),
            &HashSet::new(),
            &HashSet::new(),
        ));
        assert!(!detour_is_safe(
            address,
            instruction_len,
            overwritten,
            Some(&HashSet::from([address])),
            &HashSet::from([address + instruction_len]),
            &HashSet::new(),
        ));
    }

    #[test]
    fn detour_entry_membership_matches_interval_scan() {
        let entries: HashSet<_> = (0..5000).map(|index| 0x1000 + index * 13).collect();
        for address in 0x1000..0x1200 {
            for overwritten in [2, 5, 9, 17, 31] {
                let expected = entries
                    .iter()
                    .any(|entry| *entry >= address + 2 && *entry < address + overwritten);
                assert_eq!(
                    detour_tail_overlaps_entry(address, 2, overwritten, &entries),
                    expected
                );
            }
        }
        assert!(!detour_tail_overlaps_entry(0x1000, 2, 2, &entries));
        assert!(detour_tail_overlaps_entry(usize::MAX - 1, 2, 5, &entries));
    }

    #[test]
    #[ignore = "explicit performance comparison; cargo test detour_entry_lookup_cost -- --ignored --nocapture"]
    fn detour_entry_lookup_cost() {
        use std::hint::black_box;
        use std::time::Instant;
        let entries: HashSet<_> = (0..100_000).map(|index| index * 32).collect();
        let start = Instant::now();
        for index in 0..2000 {
            let address = black_box(index * 32);
            black_box(
                entries
                    .iter()
                    .any(|entry| *entry >= address + 2 && *entry < address + 7),
            );
        }
        let scan = start.elapsed();
        let start = Instant::now();
        for index in 0..2000 {
            black_box(detour_tail_overlaps_entry(
                black_box(index * 32),
                2,
                7,
                &entries,
            ));
        }
        eprintln!(
            "detour entry lookup, 100000 roots / 2000 sites: scan={scan:?}, indexed={:?}",
            start.elapsed()
        );
    }

    #[test]
    fn syscall_can_be_nopped_or_turned_into_an_explicit_trap() {
        let original = [0xb8, 0x27, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xc3];
        for (policy, expected) in [
            (AotSyscallPolicy::Nop, [0x90, 0x90]),
            (AotSyscallPolicy::Trap, [0x0f, 0x0b]),
        ] {
            let mut code = original;
            let mut report = PatchReport::default();
            patch_code(
                &mut code,
                0x2000,
                teb_slot_offset(1) as u32,
                None,
                None,
                None,
                policy,
                None,
                None,
                &mut report,
            );
            assert_eq!(&code[5..7], &expected);
            assert_eq!(report.syscall_sites, vec![0x2005]);
        }
    }

    #[test]
    fn syscall_opcode_bytes_inside_an_immediate_are_not_touched() {
        let mut code = [0xb8, 0x0f, 0x05, 0x00, 0x00, 0xc3];
        let original = code;
        let mut report = PatchReport::default();
        patch_code(
            &mut code,
            0x3000,
            teb_slot_offset(1) as u32,
            None,
            None,
            None,
            AotSyscallPolicy::Nop,
            None,
            None,
            &mut report,
        );
        assert_eq!(code, original);
        assert!(report.syscall_sites.is_empty());
    }

    #[test]
    fn compact_scan_keeps_branch_targets_from_unpatched_instructions() {
        // The final jump enters the would-be five-byte syscall detour at +2.
        // Although neither the jump nor the NOPs are patch candidates, their
        // control-flow information must still prevent that detour.
        let mut code = [0x0f, 0x05, 0x90, 0x90, 0x90, 0xeb, 0xfb];
        let mut report = PatchReport::default();
        let (_, sites, _) = patch_code(
            &mut code,
            0x3000,
            teb_slot_offset(1) as u32,
            None,
            None,
            None,
            AotSyscallPolicy::Trampoline(1),
            None,
            None,
            &mut report,
        )
        .unwrap();
        assert_eq!(code, [0x0f, 0x0b, 0x90, 0x90, 0x90, 0xeb, 0xfb]);
        assert_eq!(sites.len(), 1);
        assert!(!sites[0].trampoline);
    }

    #[test]
    fn cfg_boundaries_override_a_desynchronised_segment_decoder() {
        // This is the shape captured from a real fault: bytes before a function
        // make a segment-wide decoder land on 0f 05, while the function's real
        // boundary decodes that 0f as `sub rsp, 0xf`'s immediate byte. Replacing
        // it with E9 changes RSP and transfers control through a displacement.
        let mut code = [
            0xff, 0xff, 0xff, 0x00, 0x01, 0x02, 0x03, 0x08, 0x09, 0x0a, 0x0b, 0x48, 0x83, 0xec,
            0x0f, 0x05, 0x90, 0x90, 0xc3,
        ];
        let original = code;
        let ip = 0x4000u64;
        let mut linear = Decoder::with_ip(64, &code, ip, DecoderOptions::NONE);
        let mut false_syscall = false;
        while linear.can_decode() {
            let instruction = linear.decode();
            false_syscall |= instruction.mnemonic() == Mnemonic::Syscall;
        }
        assert!(
            false_syscall,
            "fixture must reproduce the linear decode error"
        );

        let entries = [0x400b, 0x400f, 0x4010, 0x4011, 0x4012];
        let mut report = PatchReport::default();
        patch_code(
            &mut code,
            ip,
            teb_slot_offset(1) as u32,
            None,
            None,
            None,
            AotSyscallPolicy::Trap,
            Some(&entries),
            None,
            &mut report,
        )
        .expect("CFG-boundary scan should succeed");

        assert_eq!(code, original);
        assert!(report.syscall_sites.is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn emulate_fs_instruction_handles_negative_displacement_and_alu() {
        use iced_x86::{Decoder, DecoderOptions};
        use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT;

        // Allocate a dummy TLS buffer representing static TLS below thread pointer
        let mut tls_buf = [0u8; 64];
        let tp_ptr = (&mut tls_buf[32]) as *mut u8 as usize;
        // Target -8 is at index 24 (32 - 8)
        let target_slot = (&mut tls_buf[24]) as *mut u8 as *mut u64;
        unsafe { *target_slot = 0x1234_5678_9abc_def0 };

        // Set up CONTEXT with Rip and RAX
        let mut ctx: CONTEXT = unsafe { core::mem::zeroed() };
        ctx.Rax = 0;
        ctx.Rip = 0x1000;

        // 1. movq %fs:-8, %rax (64 48 8b 04 25 f8 ff ff ff)
        let bytes_mov_read = [0x64, 0x48, 0x8b, 0x04, 0x25, 0xf8, 0xff, 0xff, 0xff];
        let mut decoder = Decoder::with_ip(64, &bytes_mov_read, 0x1000, DecoderOptions::NONE);
        let inst_mov_read = decoder.decode();

        // Pass thread_pointer directly (0 offset means tp is read from the slot if mock or pointer)
        // Since get_thread_pointer reads from TEB offset, in test we test emulation target addr calculation
        // Let's verify decode and emulation against mock memory:
        let raw_disp = inst_mov_read.memory_displacement64();
        assert_eq!(raw_disp, 0xffff_ffff_ffff_fff8);
        let disp = (raw_disp as u32 as i32) as i64;
        assert_eq!(disp, -8, "sign extension must convert 0xfffffff8 to -8");

        let calc_addr = (tp_ptr as u64).wrapping_add(disp as u64) as usize;
        assert_eq!(calc_addr, target_slot as usize);
    }
}
