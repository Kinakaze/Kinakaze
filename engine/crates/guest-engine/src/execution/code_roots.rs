//! Proven indirect function roots from the ELF exception-frame index.
use super::{ExecutionError, Image};
use gimli::{BaseAddresses, EhFrameHdr, LittleEndian, Pointer};
use iced_x86::{Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind, Register};
use std::collections::VecDeque;

/// Relative relocations into the GOT identify address-taken local routines,
/// including stripped musl assembly without unwind records. Read the addend
/// rather than the live slot: static PIE relocates its own GOT after loading.
pub(super) fn got_entries(object: &Image<'_>) -> Result<Vec<usize>, ExecutionError> {
    let elf = object.elf()?;
    let sections = elf.section_headers()?;
    let Some(strings) = sections.get(elf.header().section_name_index as usize) else {
        return Ok(Vec::new());
    };
    let names = elf.slice(strings.offset as usize, strings.size as usize)?;
    let tables: Vec<_> = sections
        .iter()
        .filter(|section| {
            let name = names
                .get(section.name_offset as usize..)
                .and_then(|rest| rest.split(|byte| *byte == 0).next());
            matches!(name, Some(b".got" | b".got.plt")) && section.flags & 2 != 0
        })
        .collect();
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let segments = elf.program_headers()?;
    let mut entries = Vec::new();
    for relocation in elf.relocations()? {
        if relocation.kind != kinakaze_elf::R_X86_64_RELATIVE
            || relocation.addend < 0
            || !tables.iter().any(|section| {
                relocation.offset >= section.virtual_address
                    && relocation.offset - section.virtual_address <= section.size.saturating_sub(8)
                    && section.size >= 8
            })
        {
            continue;
        }
        let target = relocation.addend as u64;
        if segments.iter().any(|segment| {
            segment.kind == kinakaze_elf::PT_LOAD
                && segment.flags & kinakaze_elf::PF_X != 0
                && target >= segment.virtual_address
                && target - segment.virtual_address < segment.file_size
        }) {
            entries.push(object.resolve_address(target)?);
        }
    }
    Ok(entries)
}

pub(super) fn unwind_entries(object: &Image<'_>) -> Result<Vec<usize>, ExecutionError> {
    let mut entries = Vec::new();
    for header in object.elf()?.program_headers()? {
        if header.kind != 0x6474_e550 {
            continue;
        } // PT_GNU_EH_FRAME
        let Some(end) = header.offset.checked_add(header.file_size) else {
            continue;
        };
        let Some(bytes) = usize::try_from(header.offset)
            .ok()
            .zip(usize::try_from(end).ok())
            .and_then(|(start, end)| object.bytes.get(start..end))
        else {
            continue;
        };
        let base = object.resolve_address(header.virtual_address)?;
        entries.extend(header_entries(bytes, base));
    }
    Ok(entries)
}

fn header_entries(bytes: &[u8], base: usize) -> Vec<usize> {
    let bases = BaseAddresses::default().set_eh_frame_hdr(base as u64);
    let Ok(header) = EhFrameHdr::new(bytes, LittleEndian).parse(&bases, 8) else {
        return Vec::new();
    };
    let Some(table) = header.table() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    let mut rows = table.iter(&bases);
    while let Ok(Some((Pointer::Direct(address), _))) = rows.next() {
        if let Ok(address) = usize::try_from(address) {
            entries.push(address);
        }
    }
    entries
}

fn writes(instruction: &Instruction, register: Register) -> bool {
    InstructionInfoFactory::new()
        .info(instruction)
        .used_registers()
        .iter()
        .any(|used| {
            used.register().full_register() == register.full_register()
                && matches!(
                    used.access(),
                    OpAccess::Write
                        | OpAccess::CondWrite
                        | OpAccess::ReadWrite
                        | OpAccess::ReadCondWrite
                )
        })
}

/// musl's static-PIE entry jumps through a register loaded by RIP-relative
/// LEA, even though the destination is a fixed local function. Follow that
/// proven target; scanning arbitrary executable bytes would corrupt literals.
pub(super) fn rip_relative_branch_entry(recent: &VecDeque<Instruction>) -> Option<usize> {
    let branch = recent.back()?;
    if !matches!(
        branch.flow_control(),
        iced_x86::FlowControl::IndirectBranch | iced_x86::FlowControl::IndirectCall
    ) || branch.op0_kind() != OpKind::Register
    {
        return None;
    }
    let register = branch.op0_register();
    let source = recent
        .iter()
        .rev()
        .skip(1)
        .take_while(|instruction| instruction.flow_control() == iced_x86::FlowControl::Next)
        .find(|instruction| writes(instruction, register))?;
    (source.mnemonic() == Mnemonic::Lea
        && source.op0_register() == register
        && source.is_ip_rel_memory_operand())
    .then(|| source.ip_rel_memory_address() as usize)
}

/// Follow a formed function pointer across local branches only when its value
/// reaches a register-indirect call. Rust hoists musl readdir into R15 before
/// entering its directory loop; a basic-block-only lookback misses that root.
pub(super) fn rip_relative_invoked_entry(
    instruction: &Instruction,
    segments: &[(usize, usize)],
) -> Option<usize> {
    if instruction.mnemonic() != Mnemonic::Lea
        || !instruction.is_ip_rel_memory_operand()
        || register_bit(instruction.op0_register()) == 0
    {
        return None;
    }
    let target = instruction.ip_rel_memory_address() as usize;
    (segments
        .iter()
        .any(|(start, len)| target >= *start && target - start < *len)
        && callee_invokes_register(
            instruction.next_ip() as usize,
            instruction.op0_register(),
            segments,
        ))
    .then_some(target)
}

/// Follow local callback arguments when a known callee invokes them or
/// registers a recognized TLS cleanup record. PF_X alone is not proof:
/// OpenSSL keeps strings and lookup tables alongside its assembly routines.
pub(super) fn rip_relative_callback_entries(
    recent: &VecDeque<Instruction>,
    segments: &[(usize, usize)],
) -> Vec<usize> {
    use iced_x86::FlowControl;
    let Some(call) = recent.back() else {
        return Vec::new();
    };
    let callee = if call.is_call_near() {
        call.near_branch_target() as usize
    } else if call.flow_control() == FlowControl::IndirectCall {
        let Some(target) = rip_relative_branch_entry(recent) else {
            return Vec::new();
        };
        target
    } else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for (index, source) in recent
        .iter()
        .enumerate()
        .rev()
        .skip(1)
        .take_while(|(_, instruction)| instruction.flow_control() == FlowControl::Next)
    {
        let register = source.op0_register();
        if source.mnemonic() != Mnemonic::Lea
            || !source.is_ip_rel_memory_operand()
            || register_bit(register) == 0
        {
            continue;
        }
        let target = source.ip_rel_memory_address() as usize;
        if !segments
            .iter()
            .any(|(start, len)| target >= *start && target - start < *len)
        {
            continue;
        }
        // musl selects its C or C++ thread entry with LEA/CMOV before clone.
        // Follow both possible full-width pointers through register copies;
        // partial writes and unknown transformations invalidate an alias.
        let mut aliases = register_bit(register);
        let mut info = InstructionInfoFactory::new();
        for instruction in recent.iter().skip(index + 1).take(recent.len() - index - 2) {
            aliases = copied_aliases(instruction, aliases, &mut info);
        }
        if [
            Register::RDI,
            Register::RSI,
            Register::RDX,
            Register::RCX,
            Register::R8,
            Register::R9,
        ]
        .into_iter()
        .any(|argument| {
            aliases & register_bit(argument) != 0
                && (callee_invokes_register(callee, argument, segments)
                    || (argument == Register::RSI && registers_tls_cleanup(callee, segments)))
        }) {
            entries.push(target);
        }
    }
    entries
}

/// musl's pthread_cleanup_push does not call its callback immediately: it
/// publishes {function, argument, next} to the thread's cleanup chain. Require
/// the entire registration sequence, including the FS thread pointer and the
/// linked-list update. A data pointer merely stored by a callee is insufficient.
fn registers_tls_cleanup(entry: usize, segments: &[(usize, usize)]) -> bool {
    use iced_x86::{Decoder, DecoderOptions, FlowControl};
    let mut instructions = Vec::new();
    let mut address = entry;
    for _ in 0..10 {
        let Some(&(start, len)) = segments
            .iter()
            .find(|(start, len)| address >= *start && address - start < *len)
        else {
            return false;
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(address as *const u8, (start + len - address).min(15))
        };
        let instruction =
            Decoder::with_ip(64, bytes, address as u64, DecoderOptions::NONE).decode();
        if instruction.is_invalid() {
            return false;
        }
        if instruction.is_jmp_short_or_near() {
            address = instruction.near_branch_target() as usize;
            continue;
        }
        address = instruction.next_ip() as usize;
        instructions.push(instruction);
        match instruction.flow_control() {
            FlowControl::Return => break,
            FlowControl::Next => {}
            _ => return false,
        }
    }
    let [function, argument, thread, head, next, publish, ret] = instructions.as_slice() else {
        return false;
    };
    let memory = |instruction: &Instruction, base, displacement| {
        instruction.mnemonic() == Mnemonic::Mov
            && instruction.memory_base() == base
            && instruction.memory_index() == Register::None
            && instruction.memory_displacement64() == displacement
            && instruction.segment_prefix() == Register::None
    };
    let store = |instruction: &Instruction, base, displacement, source| {
        memory(instruction, base, displacement)
            && instruction.op0_kind() == OpKind::Memory
            && instruction.op1_kind() == OpKind::Register
            && instruction.op1_register() == source
    };
    store(function, Register::RDI, 0, Register::RSI)
        && store(argument, Register::RDI, 8, Register::RDX)
        && thread.mnemonic() == Mnemonic::Mov
        && thread.op0_register() == Register::RAX
        && thread.op1_kind() == OpKind::Memory
        && thread.segment_prefix() == Register::FS
        && thread.memory_base() == Register::None
        && thread.memory_index() == Register::None
        && thread.memory_displacement64() == 0
        && memory(head, Register::RAX, head.memory_displacement64())
        && head.op0_register() == Register::RDX
        && head.op1_kind() == OpKind::Memory
        && store(next, Register::RDI, 16, Register::RDX)
        && store(
            publish,
            Register::RAX,
            head.memory_displacement64(),
            Register::RDI,
        )
        && ret.flow_control() == FlowControl::Return
}

fn register_bit(register: Register) -> u16 {
    let value = register as u32;
    if value >= Register::RAX as u32 && value <= Register::R15 as u32 {
        1 << (value - Register::RAX as u32)
    } else {
        0
    }
}

fn copied_aliases(
    instruction: &Instruction,
    aliases: u16,
    info: &mut InstructionInfoFactory,
) -> u16 {
    let conditional = matches!(
        instruction.mnemonic(),
        Mnemonic::Cmova
            | Mnemonic::Cmovae
            | Mnemonic::Cmovb
            | Mnemonic::Cmovbe
            | Mnemonic::Cmove
            | Mnemonic::Cmovg
            | Mnemonic::Cmovge
            | Mnemonic::Cmovl
            | Mnemonic::Cmovle
            | Mnemonic::Cmovne
            | Mnemonic::Cmovno
            | Mnemonic::Cmovnp
            | Mnemonic::Cmovns
            | Mnemonic::Cmovo
            | Mnemonic::Cmovp
            | Mnemonic::Cmovs
    );
    let destination = register_bit(instruction.op0_register());
    let copy = (instruction.mnemonic() == Mnemonic::Mov || conditional)
        && instruction.op0_kind() == OpKind::Register
        && instruction.op1_kind() == OpKind::Register
        && aliases & register_bit(instruction.op1_register()) != 0;
    let retained = if conditional {
        aliases & destination
    } else {
        0
    };
    let mut result = aliases;
    for used in info.info(instruction).used_registers() {
        if matches!(
            used.access(),
            OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite
        ) {
            result &= !register_bit(used.register().full_register());
        }
    }
    result | retained | if copy { destination } else { 0 }
}

/// A bounded, register-only provenance walk. Copies preserve the callback;
/// writes and syscall clobbers discard it. Unknown calls/memory reloads stop
/// proving anything. This covers musl's clone wrapper without guessing code
/// from prologues, names or the contents of executable data.
fn callee_invokes_register(entry: usize, register: Register, segments: &[(usize, usize)]) -> bool {
    use iced_x86::{Decoder, DecoderOptions, FlowControl};
    let mut pending = vec![(entry, register_bit(register))];
    let mut seen = std::collections::HashSet::new();
    let mut info = InstructionInfoFactory::new();
    for _ in 0..128 {
        let Some((address, mut aliases)) = pending.pop() else {
            break;
        };
        if aliases == 0 || !seen.insert((address, aliases)) {
            continue;
        }
        let Some(&(start, len)) = segments
            .iter()
            .find(|(start, len)| address >= *start && address - start < *len)
        else {
            continue;
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(address as *const u8, (start + len - address).min(15))
        };
        let instruction =
            Decoder::with_ip(64, bytes, address as u64, DecoderOptions::NONE).decode();
        if instruction.is_invalid() {
            continue;
        }
        let flow = instruction.flow_control();
        if matches!(
            flow,
            FlowControl::IndirectCall | FlowControl::IndirectBranch
        ) && instruction.op0_kind() == OpKind::Register
            && aliases & register_bit(instruction.op0_register()) != 0
        {
            return true;
        }
        let copy = instruction.mnemonic() == Mnemonic::Mov
            && instruction.op0_kind() == OpKind::Register
            && instruction.op1_kind() == OpKind::Register
            && register_bit(instruction.op0_register()) != 0
            && aliases & register_bit(instruction.op1_register()) != 0;
        for used in info.info(&instruction).used_registers() {
            if matches!(
                used.access(),
                OpAccess::Write
                    | OpAccess::CondWrite
                    | OpAccess::ReadWrite
                    | OpAccess::ReadCondWrite
            ) {
                aliases &= !register_bit(used.register().full_register());
            }
        }
        if copy {
            aliases |= register_bit(instruction.op0_register());
        }
        if instruction.mnemonic() == Mnemonic::Syscall {
            aliases &= !(register_bit(Register::RAX)
                | register_bit(Register::RCX)
                | register_bit(Register::R11));
            pending.push((instruction.next_ip() as usize, aliases));
            continue;
        }
        match flow {
            FlowControl::Next => pending.push((instruction.next_ip() as usize, aliases)),
            FlowControl::ConditionalBranch => {
                pending.push((instruction.near_branch_target() as usize, aliases));
                pending.push((instruction.next_ip() as usize, aliases));
            }
            FlowControl::UnconditionalBranch if instruction.is_jmp_short_or_near() => {
                pending.push((instruction.near_branch_target() as usize, aliases));
            }
            _ => {}
        }
    }
    false
}

/// Recognize a bounded compiler switch, not arbitrary pointer-shaped data:
/// cmp index,N; ja default; lea table(%rip),base; movsxd (base,index,4),target;
/// add base,target; jmp *target. Every table destination must be executable.
pub(super) fn relative_switch_entries(
    object: &Image<'_>,
    recent: &VecDeque<Instruction>,
    segments: &[(usize, usize)],
    is_boundary: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let previous_base = |register: Register, before: usize| {
        // Loop-invariant table bases may precede the branch that queued this
        // block. Only decode boundaries already proven by the CFG traversal.
        let &(segment, _) = segments
            .iter()
            .find(|(start, size)| before >= *start && before - start < *size)?;
        for address in (segment.max(before.saturating_sub(4096))..before).rev() {
            if !is_boundary(address) {
                continue;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(address as *const u8, (before - address).min(15))
            };
            let instruction = iced_x86::Decoder::with_ip(
                64,
                bytes,
                address as u64,
                iced_x86::DecoderOptions::NONE,
            )
            .decode();
            if instruction.is_invalid() {
                continue;
            }
            if writes(&instruction, register) {
                return (instruction.mnemonic() == Mnemonic::Lea
                    && instruction.is_ip_rel_memory_operand())
                .then(|| instruction.ip_rel_memory_address() as usize);
            }
            if instruction.flow_control() == iced_x86::FlowControl::Return {
                break;
            }
        }
        None
    };
    let Some((table, count)) = relative_switch_table(recent, previous_base) else {
        return Vec::new();
    };
    let Ok(elf) = object.elf() else {
        return Vec::new();
    };
    let Ok(headers) = elf.program_headers() else {
        return Vec::new();
    };
    let mut bytes = None;
    for header in headers {
        if header.kind != kinakaze_elf::PT_LOAD {
            continue;
        }
        let Ok(start) = object.resolve_address(header.virtual_address) else {
            continue;
        };
        let Some(delta) = table.checked_sub(start) else {
            continue;
        };
        if (delta as u64).saturating_add(count as u64 * 4) > header.file_size {
            continue;
        }
        let Some(offset) = usize::try_from(header.offset)
            .ok()
            .and_then(|offset| offset.checked_add(delta))
        else {
            continue;
        };
        bytes = offset
            .checked_add(count * 4)
            .and_then(|end| object.bytes.get(offset..end));
        break;
    }
    let Some(bytes) = bytes else {
        return Vec::new();
    };
    let mut targets = Vec::with_capacity(count);
    for word in bytes.chunks_exact(4) {
        let offset = i32::from_le_bytes(word.try_into().unwrap());
        let Some(target) = table.checked_add_signed(offset as isize) else {
            return Vec::new();
        };
        if !segments
            .iter()
            .any(|(start, length)| target >= *start && target - start < *length)
        {
            return Vec::new();
        }
        targets.push(target);
    }
    targets
}

/// Non-PIE LLVM output (Bun's Zig code) dispatches through an absolute table:
/// `jmp *table(,index,8)`. An exhaustive enum switch carries no `cmp; ja`
/// guard, so the table is bounded by its index width and ends at the first
/// entry that is not code local to the jump. Adjacent tables of the same
/// function continue that run, but they only contribute real branch targets.
pub(super) fn absolute_switch_entries(
    object: &Image<'_>,
    recent: &VecDeque<Instruction>,
    segments: &[(usize, usize)],
) -> Vec<usize> {
    const LOCAL: usize = 1 << 20;
    let Some(jump) = recent.back() else {
        return Vec::new();
    };
    if jump.mnemonic() != Mnemonic::Jmp
        || jump.op0_kind() != OpKind::Memory
        || jump.memory_base() != Register::None
        || jump.memory_index() == Register::None
        || jump.memory_index_scale() != 8
        || jump.segment_prefix() != Register::None
    {
        return Vec::new();
    }
    let table = jump.memory_displacement64();
    // Absolute tables only exist in objects mapped at their link address.
    if table % 8 != 0 || object.resolve_address(table).ok() != usize::try_from(table).ok() {
        return Vec::new();
    }
    let index = jump.memory_index().full_register();
    let limit = recent
        .iter()
        .rev()
        .skip(1)
        .find(|instruction| writes(instruction, index))
        .map_or(1024, |source| {
            let width = match source.op1_kind() {
                OpKind::Memory => source.memory_size().size(),
                OpKind::Register => source.op1_register().size(),
                _ => 0,
            };
            match source.mnemonic() {
                Mnemonic::Movzx if width == 1 => 256,
                Mnemonic::And
                    if matches!(
                        source.op1_kind(),
                        OpKind::Immediate8to32 | OpKind::Immediate32 | OpKind::Immediate8to64
                    ) && source.immediate(1) < 1024 =>
                {
                    source.immediate(1) as usize + 1
                }
                _ => 1024,
            }
        });
    let Ok(elf) = object.elf() else {
        return Vec::new();
    };
    let Ok(headers) = elf.program_headers() else {
        return Vec::new();
    };
    let Some(bytes) = headers.iter().find_map(|header| {
        if header.kind != kinakaze_elf::PT_LOAD
            || header.flags & kinakaze_elf::PF_X != 0
            || table < header.virtual_address
            || table - header.virtual_address >= header.file_size
        {
            return None;
        }
        let delta = table - header.virtual_address;
        let available = (header.file_size - delta).min(limit as u64 * 8) as usize;
        let offset = usize::try_from(header.offset.checked_add(delta)?).ok()?;
        object.bytes.get(offset..offset.checked_add(available)?)
    }) else {
        return Vec::new();
    };
    let origin = jump.ip() as usize;
    bytes
        .chunks_exact(8)
        .map(|word| u64::from_le_bytes(word.try_into().unwrap()) as usize)
        .take_while(|target| {
            target.abs_diff(origin) < LOCAL
                && segments
                    .iter()
                    .any(|(start, length)| *target >= *start && target - start < *length)
        })
        .collect()
}

fn relative_switch_table(
    recent: &VecDeque<Instruction>,
    previous_base: impl Fn(Register, usize) -> Option<usize>,
) -> Option<(usize, usize)> {
    let length = recent.len();
    if length < 5 {
        return None;
    }
    let jump = &recent[length - 1];
    if jump.mnemonic() != Mnemonic::Jmp || jump.op0_kind() != OpKind::Register {
        return None;
    }
    let target = jump.op0_register();
    // Register restores and other unrelated instructions may occur between
    // loading the table entry, adding its base, and the indirect jump.
    let add_index = (0..length - 1)
        .rev()
        .find(|i| writes(&recent[*i], target))?;
    let add = &recent[add_index];
    let load_index = (0..add_index).rev().find(|i| writes(&recent[*i], target))?;
    let load = &recent[load_index];
    if load_index < 2
        || add.mnemonic() != Mnemonic::Add
        || add.op1_kind() != OpKind::Register
        || load.mnemonic() != Mnemonic::Movsxd
        || load.op1_kind() != OpKind::Memory
        || load.memory_index_scale() != 4
        || load.memory_displacement64() != 0
    {
        return None;
    }
    let base = load.memory_base();
    let index = load.memory_index();
    if base == Register::None
        || index == Register::None
        || base.full_register() == target.full_register()
        || add.op0_register() != target
        || load.op0_register() != target
        || add.op1_register() != base
    {
        return None;
    }
    if (load_index + 1..add_index).any(|i| writes(&recent[i], base))
        || (load_index + 1..length - 1)
            .any(|i| recent[i].flow_control() != iced_x86::FlowControl::Next)
    {
        return None;
    }
    let table = if let Some(lea_index) = (0..load_index).rev().find(|i| writes(&recent[*i], base)) {
        let lea = &recent[lea_index];
        if lea.mnemonic() != Mnemonic::Lea || !lea.is_ip_rel_memory_operand() {
            return None;
        }
        lea.ip_rel_memory_address() as usize
    } else {
        previous_base(base, load.ip() as usize)?
    };
    for i in (0..load_index - 1).rev() {
        let compare = &recent[i];
        let branch = &recent[i + 1];
        if compare.mnemonic() != Mnemonic::Cmp
            || !matches!(compare.op0_kind(), OpKind::Register | OpKind::Memory)
            || !matches!(
                compare.op1_kind(),
                OpKind::Immediate8
                    | OpKind::Immediate32
                    | OpKind::Immediate8to32
                    | OpKind::Immediate8to64
                    | OpKind::Immediate32to64
            )
            || !matches!(branch.mnemonic(), Mnemonic::Ja | Mnemonic::Jae)
        {
            continue;
        }
        let bound = compare.immediate(1);
        let count = bound.checked_add(u64::from(branch.mnemonic() == Mnemonic::Ja))?;
        if count == 0 || count > 65536 {
            return None;
        }
        let destination = branch.near_branch_target();
        if destination >= branch.next_ip() && destination <= jump.ip() {
            return None;
        }
        // GCC's musl posix_spawn dispatch compares the action stored in memory
        // and loads that same word only after the bounds check. Require the
        // adjacent load, equal width/address, and a full index definition.
        if compare.op0_kind() == OpKind::Memory {
            if i + 3 != load_index {
                continue;
            }
            let read = &recent[i + 2];
            if read.mnemonic() == Mnemonic::Mov
                && read.op0_kind() == OpKind::Register
                && read.op1_kind() == OpKind::Memory
                && read.op0_register().full_register() == index.full_register()
                && read.op0_register().size() >= 4
                && read.memory_size() == compare.memory_size()
                && read.memory_base() == compare.memory_base()
                && read.memory_index() == compare.memory_index()
                && read.memory_index_scale() == compare.memory_index_scale()
                && read.memory_displacement64() == compare.memory_displacement64()
                && read.memory_segment() == compare.memory_segment()
            {
                return Some((table, count as usize));
            }
            continue;
        }
        // Clang can copy the checked index into another register before the
        // table load (Firefox's wasm2c XML tokenizer does this). Follow only
        // full-register MOVs backwards; arithmetic and partial writes cannot
        // establish the same bounded value.
        let mut checked = index.full_register();
        let mut copied = false;
        let mut zero_extended = false;
        for instruction in recent.range(i + 2..load_index).rev() {
            if !writes(instruction, checked) {
                continue;
            }
            if instruction.mnemonic() != Mnemonic::Mov
                || instruction.op0_kind() != OpKind::Register
                || instruction.op1_kind() != OpKind::Register
                || instruction.op0_register().size() < 4
                || instruction.op0_register().size() != instruction.op1_register().size()
                || instruction.op0_register().full_register() != checked
            {
                return None;
            }
            copied = true;
            zero_extended |= instruction.op0_register().size() == 4;
            checked = instruction.op1_register().full_register();
        }
        if compare.op0_register().full_register() != checked {
            continue;
        }
        if copied && compare.op0_register().size() < 8 && !zero_extended {
            // A 32-bit comparison does not bound the source's high half.
            return None;
        }
        return Some((table, count as usize));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_pie_got_roots_use_relocation_addends_and_exclude_data() {
        let mut bytes = vec![0u8; 768];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[40..48].copy_from_slice(&176u64.to_le_bytes());
        for (at, value) in [(54, 56u16), (56, 2), (58, 64), (60, 4), (62, 1)] {
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (at, flags, offset, size) in [(64, 5u32, 640u64, 16u64), (120, 6, 576, 32)] {
            bytes[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
            for (field, value) in [(8, offset), (16, offset), (32, size), (40, size)] {
                bytes[at + field..at + field + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        bytes[432..438].copy_from_slice(b"\0.got\0");
        for (at, name, kind, flags, offset, size, stride) in [
            (240, 0u32, 3u32, 0u64, 432u64, 6u64, 0u64),
            (304, 1, 1, 2, 576, 32, 8),
            (368, 0, 4, 2, 480, 72, 24),
        ] {
            bytes[at..at + 4].copy_from_slice(&name.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&kind.to_le_bytes());
            for (field, value) in [
                (8, flags),
                (16, offset),
                (24, offset),
                (32, size),
                (56, stride),
            ] {
                bytes[at + field..at + field + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        for (at, slot, target) in [(480, 576u64, 640u64), (504, 584, 608), (528, 608, 648)] {
            bytes[at..at + 8].copy_from_slice(&slot.to_le_bytes());
            bytes[at + 8..at + 16].copy_from_slice(&8u64.to_le_bytes());
            bytes[at + 16..at + 24].copy_from_slice(&target.to_le_bytes());
        }
        let view = kinakaze_runtime::execution::CodeImage {
            bytes: bytes.as_ptr(),
            length: bytes.len(),
            base: 0x10000,
            mapped_length: bytes.len(),
            load_bias: 0x10000,
            content_hash: core::ptr::null(),
        };
        let object = Image {
            bytes: &bytes,
            dynamic: Default::default(),
            view: &view,
        };
        // The GOT itself is still zero: no self-relocation has run yet.
        assert_eq!(got_entries(&object).unwrap(), [0x10000 + 640]);
    }

    #[test]
    fn stripped_functions_use_signed_data_relative_unwind_roots() {
        // An exception index can identify a routine preceded by just one INT3,
        // which the code-island padding heuristic deliberately does not accept.
        let mut bytes = vec![1, 0x1b, 0x03, 0x3b];
        bytes.extend_from_slice(&0x100i32.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for word in [-0x201i32, 0x120, 0x300, 0x160] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(header_entries(&bytes, 0x1000), vec![0xdff, 0x1300]);
        assert!(header_entries(&bytes[..3], 0x1000).is_empty());
    }

    #[test]
    fn relative_switch_requires_a_bound_and_unmodified_index() {
        // cmp eax,2; ja outside; lea [rip+0x10],r14; movsxd [r14+rax*4],rax;
        // add r14,rax; jmp rax -- the form used by Firefox's wasm sandbox.
        let bytes = [
            0x83, 0xf8, 2, 0x77, 0x30, 0x4c, 0x8d, 0x35, 0x10, 0, 0, 0, 0x49, 0x63, 0x04, 0x86,
            0x4c, 0x01, 0xf0, 0xff, 0xe0,
        ];
        let mut recent: VecDeque<_> =
            iced_x86::Decoder::with_ip(64, &bytes, 0x1000, iced_x86::DecoderOptions::NONE)
                .into_iter()
                .collect();
        assert_eq!(
            relative_switch_table(&recent, |_, _| None),
            Some((0x101c, 3))
        );
        recent.remove(0);
        assert_eq!(relative_switch_table(&recent, |_, _| None), None);
    }

    #[test]
    fn relative_switch_accepts_a_checked_memory_index_only_from_the_same_word() {
        // cmp dword [r8+16],5; ja default; mov eax,[r8+16];
        // movsxd rax,[r9+rax*4]; add rax,r9; jmp rax.
        let mut bytes = vec![
            0x41, 0x83, 0x78, 0x10, 5, 0x77, 0x40, 0x41, 0x8b, 0x40, 0x10, 0x49, 0x63, 0x04, 0x81,
            0x4c, 0x01, 0xc8, 0xff, 0xe0,
        ];
        let decode = |bytes: &[u8]| {
            iced_x86::Decoder::with_ip(64, bytes, 0x1000, iced_x86::DecoderOptions::NONE)
                .into_iter()
                .collect()
        };
        let base = |register, _| (register == Register::R9).then_some(0x3000);
        assert_eq!(
            relative_switch_table(&decode(&bytes), base),
            Some((0x3000, 6))
        );
        bytes[10] = 0x14;
        assert_eq!(relative_switch_table(&decode(&bytes), base), None);
        bytes[10] = 0x10;
        bytes.insert(7, 0x66); // A partial index load cannot establish the bound.
        assert_eq!(relative_switch_table(&decode(&bytes), base), None);
    }

    #[test]
    fn relative_switch_follows_a_copied_index_without_accepting_partial_writes() {
        for copy in [
            vec![0x45, 0x89, 0xc8],
            vec![0x4d, 0x89, 0xc8],
            vec![0x66, 0x45, 0x89, 0xc8],
            vec![0x45, 0x01, 0xc8],
        ] {
            // cmp r9d,8; ja default; mov r8d,r9d; lea table,r11;
            // movsxd r8,[r11+r8*4]; add r8,r11; jmp r8.
            let compare = if copy == [0x4d, 0x89, 0xc8] {
                0x49
            } else {
                0x41
            };
            let mut bytes = vec![compare, 0x83, 0xf9, 8, 0x77, 0x40];
            bytes.extend_from_slice(&copy);
            bytes.extend_from_slice(&[
                0x4c, 0x8d, 0x1d, 0x10, 0, 0, 0, 0x4f, 0x63, 0x04, 0x83, 0x4d, 0x01, 0xd8, 0x41,
                0xff, 0xe0,
            ]);
            let recent: VecDeque<_> =
                iced_x86::Decoder::with_ip(64, &bytes, 0x1000, iced_x86::DecoderOptions::NONE)
                    .into_iter()
                    .collect();
            let expected = if copy == [0x45, 0x89, 0xc8] || copy == [0x4d, 0x89, 0xc8] {
                Some((recent[3].ip_rel_memory_address() as usize, 9))
            } else {
                None
            };
            assert_eq!(
                relative_switch_table(&recent, |_, _| None),
                expected,
                "{copy:x?}"
            );
            if copy == [0x4d, 0x89, 0xc8] {
                bytes[0] = 0x41;
                let recent =
                    iced_x86::Decoder::with_ip(64, &bytes, 0x1000, iced_x86::DecoderOptions::NONE)
                        .into_iter()
                        .collect();
                assert_eq!(relative_switch_table(&recent, |_, _| None), None);
            }
        }
    }
}
