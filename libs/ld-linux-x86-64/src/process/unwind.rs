//! On-demand DWARF unwinding of mapped ELF images. No sampling or stack scanning.
use super::{dynamic_loader_lock, loaded_linker};
use gimli::UnwindSection;
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EhFrameHdr, LittleEndian, Pointer, Register, RegisterRule,
    UnwindContext,
};

/// Read a saved stack word without faulting on a corrupt CFA or an unmapped stack.
fn word(address: u64) -> Option<u64> {
    let mut value = 0u64;
    let mut read = 0;
    let ok = unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            address as *const _,
            (&raw mut value).cast(),
            8,
            &mut read,
        )
    };
    (ok != 0 && read == 8).then_some(value)
}

fn step(object: &crate::object::MappedObject, pc: u64, registers: &mut [u64; 17]) -> Option<()> {
    let elf = object.elf().ok()?;
    let headers = elf.program_headers().ok()?;
    let header = headers.iter().find(|h| h.kind == 0x6474_e550)?; // PT_GNU_EH_FRAME
    let address = |value: u64| u64::try_from(object.load_bias.checked_add(value.into())?).ok();
    let bytes = |offset: u64, len: u64| {
        object
            .bytes
            .get(usize::try_from(offset).ok()?..usize::try_from(offset.checked_add(len)?).ok()?)
    };
    let bases = BaseAddresses::default().set_eh_frame_hdr(address(header.virtual_address)?);
    let hdr = EhFrameHdr::new(bytes(header.offset, header.file_size)?, LittleEndian)
        .parse(&bases, 8)
        .ok()?;
    let frame_address = match hdr.eh_frame_ptr() {
        Pointer::Direct(value) => value,
        Pointer::Indirect(value) => word(value)?,
    };
    let frame_virtual =
        u64::try_from(i128::from(frame_address).checked_sub(object.load_bias)?).ok()?;
    let segment = headers.iter().find(|h| {
        h.kind == kinakaze_elf::PT_LOAD
            && frame_virtual >= h.virtual_address
            && frame_virtual < h.virtual_address.checked_add(h.file_size).unwrap_or(0)
    })?;
    let delta = frame_virtual.checked_sub(segment.virtual_address)?;
    let frame = EhFrame::new(
        bytes(
            segment.offset.checked_add(delta)?,
            segment.file_size.checked_sub(delta)?,
        )?,
        LittleEndian,
    );
    let bases = bases.set_eh_frame(frame_address);
    let mut context = UnwindContext::new();
    let table = hdr.table()?;
    let row = table
        .unwind_info_for_address(&frame, &bases, &mut context, pc, EhFrame::cie_from_offset)
        .ok()?;
    let cfa = match row.cfa() {
        CfaRule::RegisterAndOffset { register, offset } => registers
            .get(usize::from(register.0))?
            .checked_add_signed(*offset)?,
        // Unsupported expression rules stop at the last proven frame.
        CfaRule::Expression(_) => return None,
    };
    let previous = *registers;
    for index in [3, 6, 12, 13, 14, 15, 16] {
        registers[index] = match row.register(Register(index as u16)) {
            RegisterRule::Undefined => {
                if index == 16 {
                    return None;
                } else {
                    0
                }
            }
            RegisterRule::SameValue => previous[index],
            RegisterRule::Offset(offset) => word(cfa.checked_add_signed(offset)?)?,
            RegisterRule::ValOffset(offset) => cfa.checked_add_signed(offset)?,
            RegisterRule::Register(register) => *previous.get(usize::from(register.0))?,
            RegisterRule::Constant(value) => value,
            _ => return None,
        };
    }
    registers[7] = cfa;
    Some(())
}

pub(super) unsafe extern "sysv64" fn backtrace(
    initial: *const u64,
    output: *mut usize,
    capacity: usize,
) -> usize {
    if initial.is_null() || output.is_null() {
        return 0;
    }
    let _guard = dynamic_loader_lock();
    let Some(linker) = (unsafe { loaded_linker().as_ref() }) else {
        return 0;
    };
    let mut registers = unsafe { initial.cast::<[u64; 17]>().read() };
    let mut count = 0;
    while count < capacity && registers[16] != 0 {
        let pc = registers[16];
        unsafe {
            output.add(count).write(pc as usize);
        }
        count += 1;
        let previous = (pc, registers[7]);
        let Some(owner) = linker.owner_of_address(pc.saturating_sub(1) as usize) else {
            break;
        };
        if step(
            &linker.objects()[owner.0],
            pc.saturating_sub(1),
            &mut registers,
        )
        .is_none()
            || previous == (registers[16], registers[7])
        {
            break;
        }
    }
    count
}
