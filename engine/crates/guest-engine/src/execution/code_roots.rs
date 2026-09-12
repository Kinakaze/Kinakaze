//! Proven indirect function roots from the ELF exception-frame index.
use super::{ExecutionError, Image};
use gimli::{BaseAddresses, EhFrameHdr, LittleEndian, Pointer};
use iced_x86::{Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind, Register};
use std::collections::VecDeque;

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
            || compare.op0_kind() != OpKind::Register
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
