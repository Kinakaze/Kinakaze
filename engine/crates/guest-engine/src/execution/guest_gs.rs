//! Virtual Linux GS accesses without changing Windows' TEB register.
//! Each detour covers exactly one instruction. Short instructions use a registered
//! UD2 redirect, so independent entries and the SysV red zone remain intact.
use super::{ExecutionError, instruction_trampoline::InstructionTrampoline};
use iced_x86::{
    Code, Decoder, DecoderOptions, Encoder, FlowControl, Instruction, InstructionInfoFactory,
    MemoryOperand, Mnemonic, Register,
};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

fn redirects() -> &'static Mutex<BTreeMap<usize, usize>> {
    static REDIRECTS: OnceLock<Mutex<BTreeMap<usize, usize>>> = OnceLock::new();
    REDIRECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn redirect(address: usize) -> Option<usize> {
    redirects().lock().ok()?.get(&address).copied()
}

pub(crate) fn snapshot() -> Vec<u8> {
    redirects()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .flat_map(|(address, target)| [address.to_le_bytes(), target.to_le_bytes()].concat())
        .collect()
}

pub(crate) fn restore(bytes: &[u8]) -> bool {
    if bytes.len() % 16 != 0 {
        return false;
    }
    let mut entries = redirects()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.clear();
    for record in bytes.chunks_exact(16) {
        let address = usize::from_le_bytes(record[..8].try_into().unwrap());
        let target = usize::from_le_bytes(record[8..].try_into().unwrap());
        entries.insert(address, target);
    }
    true
}

pub(super) fn matches(instruction: &Instruction) -> bool {
    if matches!(
        instruction.mnemonic(),
        Mnemonic::Rdgsbase | Mnemonic::Wrgsbase
    ) {
        return true;
    }
    if instruction.segment_prefix() != Register::GS {
        return false;
    }
    // Prefixes on LEA/NOP do not perform a segmented memory access.
    InstructionInfoFactory::new()
        .info(instruction)
        .used_memory()
        .iter()
        .any(|memory| memory.segment() == Register::GS)
}

fn memory(base: Register, displacement: i64) -> MemoryOperand {
    MemoryOperand::with_base_displ(base, displacement)
}

fn translated(
    mut original: Instruction,
    transition_offset: usize,
) -> Result<Vec<Instruction>, ExecutionError> {
    let error = || ExecutionError::AddressOverflow;
    if original.is_invalid() || !matches(&original) || original.flow_control() != FlowControl::Next
    {
        return Err(error());
    }
    let mut factory = InstructionInfoFactory::new();
    let info = factory.info(&original);
    // Explicit RSP operands need a separate value-preserving translation. Memory
    // addressing through RSP is handled below, including the temporary save area.
    for operand in 0..original.op_count() {
        if original.op_kind(operand) == iced_x86::OpKind::Register
            && original.op_register(operand).full_register() == Register::RSP
        {
            return Err(error());
        }
    }
    // AH/BH/CH/DH cannot be encoded in any instruction carrying a REX prefix.
    // A rewritten memory base in R8..R15 would force REX even for an 8-bit MOV.
    let legacy_byte = (0..original.op_count()).any(|operand| {
        matches!(
            original.op_register(operand),
            Register::AH | Register::BH | Register::CH | Register::DH
        )
    });
    let mut available = [
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
    ]
    .into_iter()
    .filter(|register| {
        (!legacy_byte
            || matches!(
                register,
                Register::RAX
                    | Register::RCX
                    | Register::RDX
                    | Register::RBX
                    | Register::RSI
                    | Register::RDI
            ))
            && !info
                .used_registers()
                .iter()
                .any(|used| used.register().full_register() == *register)
    });
    let base = available.next().ok_or_else(error)?;
    let offset = available.next().ok_or_else(error)?;
    let mut code = vec![
        Instruction::with2(Code::Lea_r64_m, Register::RSP, memory(Register::RSP, -128))
            .map_err(|_| error())?,
        Instruction::with1(Code::Push_r64, base).map_err(|_| error())?,
        Instruction::with1(Code::Push_r64, offset).map_err(|_| error())?,
    ];
    let mut teb = memory(Register::None, transition_offset as i64);
    teb.segment_prefix = Register::GS;
    code.push(Instruction::with2(Code::Mov_r64_rm64, base, teb).map_err(|_| error())?);
    let gs = memory(
        base,
        kinakaze_tls::thread_pointer::GUEST_GS_BASE_OFFSET as i64,
    );
    match original.mnemonic() {
        Mnemonic::Rdgsbase => {
            let destination = original.op0_register();
            let opcode = if destination.size() == 8 {
                Code::Mov_r64_rm64
            } else {
                Code::Mov_r32_rm32
            };
            code.push(Instruction::with2(opcode, destination, gs).map_err(|_| error())?);
        }
        Mnemonic::Wrgsbase => {
            let source = original.op0_register();
            let value = if source.size() == 4 {
                code.push(
                    Instruction::with2(Code::Mov_r32_rm32, offset.full_register32(), source)
                        .map_err(|_| error())?,
                );
                offset
            } else {
                source
            };
            code.push(Instruction::with2(Code::Mov_rm64_r64, gs, value).map_err(|_| error())?);
        }
        _ => {
            if !(0..original.op_count())
                .any(|operand| original.op_kind(operand) == iced_x86::OpKind::Memory)
            {
                return Err(error());
            }
            let address32 =
                original.memory_base().size() == 4 || original.memory_index().size() == 4;
            let mut displacement = original.memory_displacement64() as i64;
            if original.memory_base().full_register() == Register::RSP {
                displacement = displacement.wrapping_add(144);
            }
            let effective = MemoryOperand::new(
                original.memory_base(),
                original.memory_index(),
                original.memory_index_scale(),
                displacement,
                original.memory_displ_size(),
                false,
                Register::None,
            );
            code.push(
                Instruction::with2(
                    if address32 {
                        Code::Lea_r32_m
                    } else {
                        Code::Lea_r64_m
                    },
                    if address32 {
                        offset.full_register32()
                    } else {
                        offset
                    },
                    effective,
                )
                .map_err(|_| error())?,
            );
            code.push(Instruction::with2(Code::Mov_r64_rm64, base, gs).map_err(|_| error())?);
            code.push(
                Instruction::with2(
                    Code::Lea_r64_m,
                    base,
                    MemoryOperand::new(base, offset, 1, 0, 0, false, Register::None),
                )
                .map_err(|_| error())?,
            );
            original.set_segment_prefix(Register::None);
            original.set_memory_base(base);
            original.set_memory_index(Register::None);
            original.set_memory_displacement64(0);
            original.set_memory_displ_size(0);
            original.set_ip(0);
            code.push(original);
        }
    }
    code.push(Instruction::with1(Code::Pop_r64, offset).map_err(|_| error())?);
    code.push(Instruction::with1(Code::Pop_r64, base).map_err(|_| error())?);
    code.push(
        Instruction::with2(Code::Lea_r64_m, Register::RSP, memory(Register::RSP, 128))
            .map_err(|_| error())?,
    );
    Ok(code)
}

fn encode_straight_line(instructions: &[Instruction], ip: u64) -> Result<Vec<u8>, ExecutionError> {
    let mut encoder =
        Encoder::try_with_capacity(64, 128).map_err(|_| ExecutionError::AddressOverflow)?;
    let mut next_ip = ip;
    for instruction in instructions {
        // Generated GS stubs have no branches. Ordinary encoding still adjusts
        // RIP-relative LEA operands and rejects an out-of-range displacement.
        if instruction.flow_control() != FlowControl::Next {
            return Err(ExecutionError::AddressOverflow);
        }
        let length = encoder
            .encode(instruction, next_ip)
            .map_err(|_| ExecutionError::AddressOverflow)?;
        next_ip = next_ip
            .checked_add(length as u64)
            .ok_or(ExecutionError::AddressOverflow)?;
    }
    Ok(encoder.take_buffer())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_x86::{BlockEncoder, BlockEncoderOptions, InstructionBlock};

    #[test]
    fn straight_line_encoding_matches_block_relocation() {
        for bytes in [
            &[0x65, 0x48, 0x8b, 0x00][..],            // base register
            &[0x65, 0x48, 0x8b, 0x44, 0x24, 0xf8],    // RSP-relative
            &[0x65, 0x48, 0x8b, 0x05, 0x70, 0, 0, 0], // RIP-relative
            &[0x65, 0x67, 0x8b, 0x04, 0x88],          // 32-bit base and scaled index
            &[0x65, 0x88, 0x22],                      // high-byte register
            &[0xf3, 0x48, 0x0f, 0xae, 0xc8],          // rdgsbase rax
            &[0xf3, 0x48, 0x0f, 0xae, 0xd8],          // wrgsbase rax
        ] {
            let original = Decoder::with_ip(64, bytes, 0x1000, DecoderOptions::NONE).decode();
            let instructions = translated(original, 0x1480).unwrap();
            for ip in [0x800, 0x2000, 0x7fff0000] {
                let expected = BlockEncoder::encode(
                    64,
                    InstructionBlock::new(&instructions, ip),
                    BlockEncoderOptions::NONE,
                )
                .unwrap();
                assert_eq!(
                    encode_straight_line(&instructions, ip).unwrap(),
                    expected.code_buffer
                );
            }
        }
    }

    #[test]
    fn high_byte_gs_operands_remain_encodable() {
        // Firefox libxul.so contains `mov gs:[rdx],ah`. Keep all four legacy
        // high-byte registers intact when translating loads and stores.
        for opcode in [0x88, 0x8a] {
            for reg in 4..8 {
                let bytes = [0x65, opcode, reg << 3 | 2];
                let original = Decoder::with_ip(64, &bytes, 0x1000, DecoderOptions::NONE).decode();
                let instructions = translated(original, 0x1480).unwrap();
                let encoded = encode_straight_line(&instructions, 0x2000).unwrap();
                let expected =
                    [Register::AH, Register::CH, Register::DH, Register::BH][reg as usize - 4];
                let decoded: Vec<_> = Decoder::with_ip(64, &encoded, 0x2000, DecoderOptions::NONE)
                    .into_iter()
                    .collect();
                assert!(decoded.iter().any(|instruction| {
                    (0..instruction.op_count())
                        .any(|operand| instruction.op_register(operand) == expected)
                }));
                assert!(decoded.iter().all(|instruction| !instruction.is_invalid()));
            }
        }
    }
}

/// Source is writable and cannot execute until the enclosing image is published.
pub(super) unsafe fn install(
    address: usize,
    transition_offset: usize,
) -> Result<InstructionTrampoline, ExecutionError> {
    let bytes = unsafe { std::slice::from_raw_parts(address as *const u8, 15) };
    let original = Decoder::with_ip(64, bytes, address as u64, DecoderOptions::NONE).decode();
    let length = original.len();
    let instructions = translated(original, transition_offset)?;
    let trampoline = super::instruction_trampoline::allocate_near(address)?;
    let mut code = encode_straight_line(&instructions, trampoline as u64)?;
    code.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
    code.extend_from_slice(&(address + length).to_le_bytes());
    if code.len() > 1024 || length < 2 {
        return Err(ExecutionError::AddressOverflow);
    }
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), trampoline as *mut u8, code.len()) };
    super::instruction_trampoline::finish_slot(trampoline, code.len());
    let source = unsafe { std::slice::from_raw_parts_mut(address as *mut u8, length) };
    source.fill(0x90);
    if length >= 5 {
        let relative = i32::try_from(trampoline as i128 - (address + 5) as i128)
            .map_err(|_| ExecutionError::AddressOverflow)?;
        source[0] = 0xe9;
        source[1..5].copy_from_slice(&relative.to_le_bytes());
    } else {
        redirects()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(address, trampoline);
        source[..2].copy_from_slice(&[0x0f, 0x0b]);
    }
    Ok(InstructionTrampoline {
        address,
        trampoline,
        overwritten: length,
    })
}
