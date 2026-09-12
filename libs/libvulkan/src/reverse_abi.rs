//! Runtime bridges for callbacks made from the Windows Vulkan loader into ELF code.
//!
//! Vulkan structures can carry function pointers in either direction. The normal
//! exports in this crate translate System V calls into the Windows x64 ABI; a
//! callback stored in one of those structures needs the exact inverse. Rust can
//! express both conventions for a statically known function, but some callbacks
//! (notably `PFN_vkFaultCallbackFunction` and a directly supplied driver's proc
//! address) have no user-data word in which to keep the guest target. This module
//! therefore emits a tiny wrapper whose address itself identifies the target.
//!
//! The emitter is signature-driven rather than Vulkan-command-driven. Vulkan has
//! only the two x86-64 argument classes used here: integer/pointer values and
//! scalar floating-point values. API structures are always passed by pointer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The x86-64 ABI register class of one scalar argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArgumentClass {
    /// Integers, handles, enums, bitmasks and every pointer.
    Integer,
    /// `float` and `double`; both use an SSE register or one eight-byte stack slot.
    Float,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    target: usize,
    arguments: Box<[ArgumentClass]>,
}

fn bridges() -> &'static Mutex<HashMap<Key, usize>> {
    static BRIDGES: OnceLock<Mutex<HashMap<Key, usize>>> = OnceLock::new();
    BRIDGES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Restores the calling thread's ELF TLS base before control enters guest code.
///
/// A Vulkan driver is allowed to make kernel transitions before invoking an
/// application callback, and Windows can reset a user-written FS base across such
/// a transition. The stable copy is kept in a TEB TLS slot by the loader.
pub(crate) extern "system" fn restore_guest_tls() {
    let _ = kinakaze_tls::restore_current_thread_static_tls();
}

/// Returns a permanent Windows-ABI entry point that invokes `target` with the
/// System V x86-64 convention described by `arguments`.
///
/// Equal targets and signatures share one bridge. Published bridges intentionally
/// live until process exit because Vulkan may retain callback pointers for the
/// lifetime of an instance or device.
pub fn for_target(target: usize, arguments: &[ArgumentClass]) -> Option<usize> {
    if target == 0 {
        return None;
    }
    let key = Key {
        target,
        arguments: arguments.into(),
    };
    let mut table = bridges().lock().ok()?;
    if let Some(address) = table.get(&key).copied() {
        return Some(address);
    }
    let code = emit_bridge(target, arguments)?;
    let address = publish(&code)?;
    table.insert(key, address);
    Some(address)
}

// x86-64 register numbers used by ModRM and the B8+rd encodings.
const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8: u8 = 8;
const R9: u8 = 9;

const WINDOWS_INTEGER_ARGUMENTS: [u8; 4] = [RCX, RDX, R8, R9];
const SYSV_INTEGER_ARGUMENTS: [u8; 6] = [RDI, RSI, RDX, RCX, R8, R9];

fn align_frame(size: usize) -> Option<usize> {
    // At a Windows function entry RSP is 8 mod 16. Subtracting a frame that is
    // also 8 mod 16 leaves it aligned for both the helper and guest calls.
    let remainder = size & 15;
    size.checked_add((8usize.wrapping_sub(remainder)) & 15)
}

fn emit_bridge(target: usize, arguments: &[ArgumentClass]) -> Option<Vec<u8>> {
    let mut integer_count = 0usize;
    let mut float_count = 0usize;
    let mut destinations = Vec::with_capacity(arguments.len());
    let mut stack_count = 0usize;
    for class in arguments {
        let destination = match class {
            ArgumentClass::Integer if integer_count < SYSV_INTEGER_ARGUMENTS.len() => {
                let register = SYSV_INTEGER_ARGUMENTS[integer_count];
                integer_count += 1;
                Destination::Integer(register)
            }
            ArgumentClass::Float if float_count < 8 => {
                let register = float_count as u8;
                float_count += 1;
                Destination::Float(register)
            }
            ArgumentClass::Integer => {
                integer_count += 1;
                let slot = stack_count;
                stack_count += 1;
                Destination::Stack(slot)
            }
            ArgumentClass::Float => {
                float_count += 1;
                let slot = stack_count;
                stack_count += 1;
                Destination::Stack(slot)
            }
        };
        destinations.push(destination);
    }

    let outgoing_size = stack_count.checked_mul(8)?;
    let saved_rdi = outgoing_size;
    let saved_rsi = saved_rdi.checked_add(8)?;
    let saved_xmm = saved_rsi.checked_add(8)?;
    let scratch_gpr = saved_xmm.checked_add(10 * 16)?;
    let scratch_xmm = scratch_gpr.checked_add(4 * 8)?;
    let frame = align_frame(scratch_xmm.checked_add(4 * 16)?)?;
    let frame_u32 = u32::try_from(frame).ok()?;

    let mut code = Vec::with_capacity(512 + arguments.len() * 16);
    emit_sub_rsp(&mut code, frame_u32);

    // Windows makes RDI, RSI and XMM6-XMM15 nonvolatile while System V does not.
    // Preserve them around the guest call so the wrapper remains a valid Windows
    // function from its caller's point of view.
    emit_store_gpr(&mut code, RDI, saved_rdi)?;
    emit_store_gpr(&mut code, RSI, saved_rsi)?;
    for xmm in 6..16 {
        emit_store_xmm(&mut code, xmm, saved_xmm + (xmm as usize - 6) * 16)?;
    }

    // Save every positional Windows register before assigning independent SysV
    // integer and SSE sequences. The helper call below is then free to clobber all
    // volatile registers without losing an application argument.
    for (index, register) in WINDOWS_INTEGER_ARGUMENTS.into_iter().enumerate() {
        emit_store_gpr(&mut code, register, scratch_gpr + index * 8)?;
        emit_store_xmm(&mut code, index as u8, scratch_xmm + index * 16)?;
    }

    emit_mov_imm64(&mut code, RAX, restore_guest_tls as *const () as usize);
    code.extend_from_slice(&[0xff, 0xd0]); // call rax

    for (index, destination) in destinations.into_iter().enumerate() {
        let source = if index < 4 {
            match arguments[index] {
                ArgumentClass::Integer => scratch_gpr + index * 8,
                ArgumentClass::Float => scratch_xmm + index * 16,
            }
        } else {
            // Original Windows RSP is `current RSP + frame`. Stack argument 4 is
            // after the return address and the mandatory 32-byte shadow space.
            frame.checked_add(0x28 + (index - 4) * 8)?
        };
        match destination {
            Destination::Integer(register) => emit_load_gpr(&mut code, register, source)?,
            Destination::Float(register) => emit_load_xmm_qword(&mut code, register, source)?,
            Destination::Stack(slot) => {
                emit_load_gpr(&mut code, RAX, source)?;
                emit_store_gpr(&mut code, RAX, slot * 8)?;
            }
        }
    }

    emit_mov_imm64(&mut code, RAX, target);
    code.extend_from_slice(&[0xff, 0xd0]); // call rax

    for xmm in 6..16 {
        emit_load_xmm(&mut code, xmm, saved_xmm + (xmm as usize - 6) * 16)?;
    }
    emit_load_gpr(&mut code, RSI, saved_rsi)?;
    emit_load_gpr(&mut code, RDI, saved_rdi)?;
    emit_add_rsp(&mut code, frame_u32);
    code.push(0xc3); // ret
    Some(code)
}

#[derive(Clone, Copy)]
enum Destination {
    Integer(u8),
    Float(u8),
    Stack(usize),
}

fn displacement(value: usize) -> Option<[u8; 4]> {
    Some(u32::try_from(value).ok()?.to_le_bytes())
}

fn emit_sub_rsp(code: &mut Vec<u8>, amount: u32) {
    code.extend_from_slice(&[0x48, 0x81, 0xec]);
    code.extend_from_slice(&amount.to_le_bytes());
}

fn emit_add_rsp(code: &mut Vec<u8>, amount: u32) {
    code.extend_from_slice(&[0x48, 0x81, 0xc4]);
    code.extend_from_slice(&amount.to_le_bytes());
}

fn emit_mov_imm64(code: &mut Vec<u8>, register: u8, value: usize) {
    code.push(0x48 | u8::from(register >= 8));
    code.push(0xb8 + (register & 7));
    code.extend_from_slice(&(value as u64).to_le_bytes());
}

fn emit_store_gpr(code: &mut Vec<u8>, register: u8, offset: usize) -> Option<()> {
    code.push(0x48 | if register >= 8 { 0x04 } else { 0 });
    code.push(0x89);
    code.push(0x84 | ((register & 7) << 3));
    code.push(0x24);
    code.extend_from_slice(&displacement(offset)?);
    Some(())
}

fn emit_load_gpr(code: &mut Vec<u8>, register: u8, offset: usize) -> Option<()> {
    code.push(0x48 | if register >= 8 { 0x04 } else { 0 });
    code.push(0x8b);
    code.push(0x84 | ((register & 7) << 3));
    code.push(0x24);
    code.extend_from_slice(&displacement(offset)?);
    Some(())
}

fn emit_store_xmm(code: &mut Vec<u8>, register: u8, offset: usize) -> Option<()> {
    code.push(0xf3);
    if register >= 8 {
        code.push(0x44);
    }
    code.extend_from_slice(&[0x0f, 0x7f]);
    code.push(0x84 | ((register & 7) << 3));
    code.push(0x24);
    code.extend_from_slice(&displacement(offset)?);
    Some(())
}

fn emit_load_xmm(code: &mut Vec<u8>, register: u8, offset: usize) -> Option<()> {
    code.push(0xf3);
    if register >= 8 {
        code.push(0x44);
    }
    code.extend_from_slice(&[0x0f, 0x6f]);
    code.push(0x84 | ((register & 7) << 3));
    code.push(0x24);
    code.extend_from_slice(&displacement(offset)?);
    Some(())
}

fn emit_load_xmm_qword(code: &mut Vec<u8>, register: u8, offset: usize) -> Option<()> {
    code.push(0xf3);
    if register >= 8 {
        code.push(0x44);
    }
    code.extend_from_slice(&[0x0f, 0x7e]);
    code.push(0x84 | ((register & 7) << 3));
    code.push(0x24);
    code.extend_from_slice(&displacement(offset)?);
    Some(())
}

#[cfg(windows)]
pub(crate) fn publish(code: &[u8]) -> Option<usize> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc,
        VirtualFree, VirtualProtect,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let block = unsafe {
        VirtualAlloc(
            core::ptr::null(),
            code.len(),
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if block.is_null() {
        return None;
    }
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), block.cast(), code.len()) };
    let mut old = 0u32;
    if unsafe { VirtualProtect(block, code.len(), PAGE_EXECUTE_READ, &raw mut old) } == 0 {
        unsafe { VirtualFree(block, 0, MEM_RELEASE) };
        return None;
    }
    if unsafe { FlushInstructionCache(GetCurrentProcess(), block, code.len()) } == 0 {
        unsafe { VirtualFree(block, 0, MEM_RELEASE) };
        return None;
    }
    Some(block as usize)
}

#[cfg(not(windows))]
pub(crate) fn publish(_code: &[u8]) -> Option<usize> {
    None
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use iced_x86::{Decoder, DecoderOptions, Mnemonic};

    unsafe extern "sysv64" fn mixed(
        a: u64,
        b: f64,
        c: u64,
        d: f32,
        e: u64,
        f: f64,
        g: u64,
        h: u64,
        i: u64,
    ) -> u64 {
        a ^ b.to_bits()
            ^ c.rotate_left(3)
            ^ u64::from(d.to_bits()).rotate_left(7)
            ^ e.rotate_left(11)
            ^ f.to_bits().rotate_left(13)
            ^ g.rotate_left(17)
            ^ h.rotate_left(19)
            ^ i.rotate_left(23)
    }

    #[test]
    fn mixed_register_classes_cross_both_abis() {
        let classes = [
            ArgumentClass::Integer,
            ArgumentClass::Float,
            ArgumentClass::Integer,
            ArgumentClass::Float,
            ArgumentClass::Integer,
            ArgumentClass::Float,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
            ArgumentClass::Integer,
        ];
        let bridge = for_target(mixed as *const () as usize, &classes).expect("bridge");
        let call: unsafe extern "system" fn(u64, f64, u64, f32, u64, f64, u64, u64, u64) -> u64 =
            unsafe { core::mem::transmute(bridge) };
        let arguments = (1, 2.5, 3, 4.25, 5, 6.5, 7, 8, 9);
        let expected = unsafe {
            mixed(
                arguments.0,
                arguments.1,
                arguments.2,
                arguments.3,
                arguments.4,
                arguments.5,
                arguments.6,
                arguments.7,
                arguments.8,
            )
        };
        assert_eq!(
            unsafe {
                call(
                    arguments.0,
                    arguments.1,
                    arguments.2,
                    arguments.3,
                    arguments.4,
                    arguments.5,
                    arguments.6,
                    arguments.7,
                    arguments.8,
                )
            },
            expected
        );
        assert_eq!(
            for_target(mixed as *const () as usize, &classes),
            Some(bridge)
        );
    }

    unsafe extern "sysv64" fn nine_integers(
        a: u64,
        b: u64,
        c: u64,
        d: u64,
        e: u64,
        f: u64,
        g: u64,
        h: u64,
        i: u64,
    ) -> u64 {
        a + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8 + i * 9
    }

    #[test]
    fn sysv_stack_arguments_are_repacked() {
        let classes = [ArgumentClass::Integer; 9];
        let bridge = for_target(nine_integers as *const () as usize, &classes).expect("bridge");
        let call: unsafe extern "system" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 =
            unsafe { core::mem::transmute(bridge) };
        assert_eq!(unsafe { call(1, 2, 3, 4, 5, 6, 7, 8, 9) }, 285);
    }

    #[test]
    fn emitted_code_is_writable_then_executable_and_contains_one_guest_call() {
        let code = emit_bridge(0x1234_5678, &[ArgumentClass::Integer]).expect("code");
        let mut decoder = Decoder::with_ip(64, &code, 0, DecoderOptions::NONE);
        let mut calls = 0;
        while decoder.can_decode() {
            calls += usize::from(decoder.decode().mnemonic() == Mnemonic::Call);
        }
        assert_eq!(calls, 2, "TLS restore plus guest target");
    }
}
