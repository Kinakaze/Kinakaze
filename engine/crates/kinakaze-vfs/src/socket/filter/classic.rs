//! Linux classic BPF instruction set, including ancillary loads.
use crate::EINVAL;

pub(crate) const MAX_INSNS: usize = 4096;
pub(crate) const AD_OFF: u32 = (-4096i32) as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct Instruction {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// Offsets are signed Linux skb offsets (including SKF_NET_OFF/SKF_LL_OFF).
/// None means an unavailable header/metadata field or an out-of-bounds load.
/// All multi-byte packet loads are big endian. A missing device on an IFINDEX
/// or HATYPE extension terminates with A, matching Linux's extension conversion.
pub(crate) trait Packet {
    fn len(&self) -> u32;
    fn load(&self, offset: i32, size: usize) -> Option<u32>;
    fn ancillary(&self, offset: u32, accumulator: u32, index: u32) -> Option<u32>;
}

#[derive(Clone, Debug)]
pub(crate) struct Program(Box<[Instruction]>);

impl Program {
    pub(crate) fn new(code: Vec<Instruction>) -> Result<Self, i32> {
        if code.is_empty() || code.len() > MAX_INSNS {
            return Err(EINVAL);
        }
        for (pc, i) in code.iter().enumerate() {
            let remaining = code.len() - pc - 1;
            match i.code {
                // Loads, stores, register transfers, returns.
                0x00 | 0x01 | 0x80 | 0x81 | 0xb1 | 0x07 | 0x87 | 0x06 | 0x16 => {}
                0x20 | 0x28 | 0x30 => {
                    if i.k >= AD_OFF
                        && !matches!(
                            i.k - AD_OFF,
                            0 | 4
                                | 8
                                | 12
                                | 16
                                | 20
                                | 24
                                | 28
                                | 32
                                | 36
                                | 40
                                | 44
                                | 48
                                | 52
                                | 56
                                | 60
                        )
                    {
                        return Err(EINVAL);
                    }
                }
                0x40 | 0x48 | 0x50 => {}
                0x60 | 0x61 | 0x02 | 0x03 if i.k < 16 => {}
                // ALU K/X. Immediate divide/modulo and shifts need bounds.
                0x04 | 0x0c | 0x14 | 0x1c | 0x24 | 0x2c | 0x3c | 0x44 | 0x4c | 0x54 | 0x5c
                | 0x6c | 0x7c | 0x84 | 0x9c | 0xa4 | 0xac => {}
                0x34 | 0x94 if i.k != 0 => {}
                0x64 | 0x74 if i.k < 32 => {}
                0x05 if (i.k as usize) < remaining => {}
                0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d | 0x45 | 0x4d
                    if (i.jt as usize) < remaining && (i.jf as usize) < remaining => {}
                _ => return Err(EINVAL),
            }
        }
        if !matches!(code.last().unwrap().code, 0x06 | 0x16) {
            return Err(EINVAL);
        }
        // Forward edges form a DAG. Intersect initialized scratch words at
        // every target, including paths that skip a store. Match Linux's scan
        // of unreachable instructions as well as its opcode validation.
        let mut masks = vec![u16::MAX; code.len()];
        let mut initialized = 0u16;
        for (pc, i) in code.iter().enumerate() {
            initialized &= masks[pc];
            match i.code {
                0x02 | 0x03 => initialized |= 1 << i.k,
                0x60 | 0x61 if initialized & (1 << i.k) == 0 => return Err(EINVAL),
                0x05 => {
                    masks[pc + 1 + i.k as usize] &= initialized;
                    initialized = u16::MAX;
                }
                0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d | 0x45 | 0x4d => {
                    masks[pc + 1 + i.jt as usize] &= initialized;
                    masks[pc + 1 + i.jf as usize] &= initialized;
                    initialized = u16::MAX;
                }
                _ => {}
            }
        }
        Ok(Self(code.into_boxed_slice()))
    }

    pub(crate) fn instructions(&self) -> &[Instruction] {
        &self.0
    }

    pub(crate) fn run(&self, packet: &impl Packet) -> u32 {
        let (mut a, mut x) = (0u32, 0u32);
        let mut memory = [0u32; 16];
        let mut pc = 0usize;
        loop {
            // Only verified forward targets reach here. No instruction budget
            // or polling is necessary: a program executes at most 4096 steps.
            let i = self.0[pc];
            pc += 1;
            match i.code {
                0x00 => a = i.k,
                0x01 => x = i.k,
                0x80 => a = packet.len(),
                0x81 => x = packet.len(),
                0x60 => a = memory[i.k as usize],
                0x61 => x = memory[i.k as usize],
                0x02 => memory[i.k as usize] = a,
                0x03 => memory[i.k as usize] = x,
                0x07 => x = a,
                0x87 => a = x,
                0x06 => return i.k,
                0x16 => return a,
                0x20 | 0x28 | 0x30 if i.k >= AD_OFF => {
                    a = if i.k - AD_OFF == 40 {
                        a ^ x
                    } else {
                        let Some(value) = packet.ancillary(i.k - AD_OFF, a, x) else {
                            return if matches!(i.k - AD_OFF, 8 | 28) { a } else { 0 };
                        };
                        value
                    };
                }
                0x20 | 0x28 | 0x30 | 0x40 | 0x48 | 0x50 => {
                    let size = match i.code & 0x18 {
                        0 => 4,
                        8 => 2,
                        _ => 1,
                    };
                    let offset = if i.code & 0xe0 == 0x40 {
                        i.k.wrapping_add(x)
                    } else {
                        i.k
                    };
                    let Some(value) = packet.load(offset as i32, size) else {
                        return 0;
                    };
                    a = value;
                }
                0xb1 => {
                    let Some(value) = packet.load(i.k as i32, 1) else {
                        return 0;
                    };
                    x = (value & 15) * 4;
                }
                0x05 => pc += i.k as usize,
                code if code & 7 == 5 => {
                    let rhs = if code & 8 == 0 { i.k } else { x };
                    let take = match code & 0xf0 {
                        0x10 => a == rhs,
                        0x20 => a > rhs,
                        0x30 => a >= rhs,
                        0x40 => a & rhs != 0,
                        _ => unreachable!(),
                    };
                    pc += if take { i.jt } else { i.jf } as usize;
                }
                code => {
                    let rhs = if code & 8 == 0 { i.k } else { x };
                    a = match code & 0xf0 {
                        0x00 => a.wrapping_add(rhs),
                        0x10 => a.wrapping_sub(rhs),
                        0x20 => a.wrapping_mul(rhs),
                        0x30 => {
                            if rhs == 0 {
                                return 0;
                            }
                            a / rhs
                        }
                        0x40 => a | rhs,
                        0x50 => a & rhs,
                        // Linux's classic-to-eBPF conversion uses 32-bit ALU
                        // shifts; the low five bits of a register shift count.
                        0x60 => a.wrapping_shl(rhs),
                        0x70 => a.wrapping_shr(rhs),
                        0x80 => a.wrapping_neg(),
                        0x90 => {
                            if rhs == 0 {
                                return 0;
                            }
                            a % rhs
                        }
                        0xa0 => a ^ rhs,
                        _ => unreachable!(),
                    };
                }
            }
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() % 8 != 0 {
            return Err(EINVAL);
        }
        Self::new(
            bytes
                .chunks_exact(8)
                .map(|b| Instruction {
                    code: u16::from_le_bytes([b[0], b[1]]),
                    jt: b[2],
                    jf: b[3],
                    k: u32::from_le_bytes(b[4..8].try_into().unwrap()),
                })
                .collect(),
        )
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.0.len() * 8);
        for i in self.0.iter() {
            bytes.extend_from_slice(&i.code.to_le_bytes());
            bytes.extend_from_slice(&[i.jt, i.jf]);
            bytes.extend_from_slice(&i.k.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests;
