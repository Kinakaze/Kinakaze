//! GNU printf hooks, with real System V varargs and caller-visible FILE output.
use super::{Length, Sink, Spec, VaList};
use crate::stdio::{self, File};
use core::{ffi::c_void, ptr};
pub(super) mod registry;
pub(super) use registry::modifier;

#[repr(C)]
pub struct Info {
    precision: i32,
    width: i32,
    spec: u32,
    flags: u16,
    user: u16,
    pad: u32,
}
pub type Printer = unsafe extern "sysv64" fn(*mut File, *const Info, *const *const c_void) -> i32;
pub type ArgInfo = unsafe extern "sysv64" fn(*const Info, usize, *mut i32, *mut i32) -> i32;
pub type VaReader = unsafe extern "sysv64" fn(*mut c_void, *mut VaList);

fn info(spec: &Spec, analysis: bool) -> Result<Info, i32> {
    let mut flags = 0u16;
    let bits = [
        matches!(
            spec.length,
            Length::LongDouble | Length::LongLong | Length::IntMax
        ),
        spec.length == Length::Short,
        matches!(
            spec.length,
            Length::Long | Length::LongLong | Length::Size | Length::IntMax | Length::PtrDiff
        ),
        spec.alternate,
        spec.space,
        spec.left_align,
        spec.plus,
        spec.group,
        false,
        spec.length == Length::Char,
        false,
        spec.i18n,
    ];
    for (bit, enabled) in bits.into_iter().enumerate() {
        flags |= u16::from(enabled) << bit;
    }
    Ok(Info {
        precision: if analysis && spec.precision_dynamic {
            i32::MIN
        } else {
            spec.precision
                .map(i32::try_from)
                .transpose()
                .map_err(|_| kinakaze_vfs::EOVERFLOW)?
                .unwrap_or(-1)
        },
        width: if analysis && spec.width_dynamic {
            i32::MIN
        } else {
            i32::try_from(spec.width.unwrap_or(0)).map_err(|_| kinakaze_vfs::EOVERFLOW)?
        },
        spec: u32::from(spec.conversion),
        flags,
        user: spec.user,
        pad: u32::from(if spec.zero_pad && !spec.left_align {
            b'0'
        } else {
            b' '
        }),
    })
}

// Guest allocations remain valid if a registered guest callback calls fork.
struct Bytes(*mut u8);
impl Bytes {
    fn new(size: usize) -> Result<Self, i32> {
        let pointer = unsafe { kinakaze_alloc::guest::malloc(size.max(1)) };
        if pointer.is_null() {
            Err(kinakaze_vfs::ENOMEM)
        } else {
            unsafe { ptr::write_bytes(pointer, 0, size.max(1)) };
            Ok(Self(pointer))
        }
    }
}
impl Drop for Bytes {
    fn drop(&mut self) {
        unsafe { kinakaze_alloc::guest::free(self.0) };
    }
}

pub(super) struct Types {
    storage: Bytes,
    pub count: usize,
}
impl Types {
    pub fn kind(&self, i: usize) -> i32 {
        unsafe { self.storage.0.cast::<i32>().add(i).read() }
    }
    fn size(&self, i: usize) -> i32 {
        unsafe { self.storage.0.cast::<i32>().add(self.count + i).read() }
    }
}
unsafe fn describe(handler: registry::Handler, info: &Info) -> Result<Option<Types>, i32> {
    if handler.arginfo == 0 {
        return Ok(None);
    }
    let callback: ArgInfo = unsafe { core::mem::transmute(handler.arginfo) };
    let (mut kind, mut size) = (0, 0);
    // Some existing handlers (including libquadmath) require a writable first
    // slot even when used only to determine the number of arguments.
    let count = unsafe { callback(info, 1, &mut kind, &mut size) };
    if count < 0 {
        return Ok(None);
    }
    let count = count as usize;
    let storage = Bytes::new(count.checked_mul(8).ok_or(kinakaze_vfs::EOVERFLOW)?)?;
    let types = storage.0.cast::<i32>();
    let sizes = unsafe { types.add(count) };
    if count == 1 {
        unsafe {
            types.write(kind);
            sizes.write(size);
        }
    } else if count > 1 && unsafe { callback(info, count, types, sizes) } != count as i32 {
        return Err(kinakaze_vfs::EINVAL);
    }
    Ok(Some(Types { storage, count }))
}
pub(super) unsafe fn argument_types(spec: &Spec) -> Result<Option<Types>, i32> {
    unsafe { describe(registry::handler(spec.conversion)?, &info(spec, true)?) }
}

unsafe fn builtin(kind: i32, arguments: &mut VaList, output: *mut u8) -> Result<(), i32> {
    unsafe {
        if kind & 0x800 != 0 || matches!(kind & 0xff, 3..=5) {
            output.cast::<usize>().write(arguments.next_integer());
        } else {
            match kind & 0xff {
                0 if kind & 0x300 != 0 => output.cast::<i64>().write(arguments.next_integer()),
                0..=2 => output.cast::<i32>().write(arguments.next_integer()),
                6 | 7 if kind & 0x100 == 0 => output.cast::<f64>().write(arguments.next_double()),
                7 => {
                    let input = ((arguments.overflow_arg_area.addr() + 15) & !15) as *const u8;
                    ptr::copy_nonoverlapping(input, output, 16);
                    arguments.overflow_arg_area = input.add(16).cast_mut().cast();
                }
                _ => return Err(kinakaze_vfs::EINVAL),
            }
        }
    }
    Ok(())
}
unsafe extern "sysv64" fn write_sink<S: Sink>(
    context: *mut c_void,
    input: *const u8,
    count: usize,
) -> isize {
    unsafe { (&mut *context.cast::<S>()).write(core::slice::from_raw_parts(input, count)) };
    count as isize
}
unsafe fn call<S: Sink>(
    sink: &mut S,
    printer: Printer,
    info: &Info,
    arguments: *const *const c_void,
) -> i32 {
    if let Some(file) = sink.stream() {
        let result = unsafe { printer(file, info, arguments) };
        if result >= 0 {
            sink.account(result as usize);
        }
        return result;
    }
    // snprintf streams directly to its bounded sink; discarded bytes are
    // counted without allocating an intermediate copy of the entire output.
    let file = unsafe {
        stdio::cookie::kinakaze_abi_fopencookie(
            (sink as *mut S).cast(),
            c"w".as_ptr(),
            stdio::cookie::Functions {
                read: None,
                write: Some(write_sink::<S>),
                seek: None,
                close: None,
            },
        )
    };
    if file.is_null() {
        return -1;
    }
    if stdio::setvbuf(file, ptr::null_mut(), 2, 0) != 0 {
        unsafe { stdio::fclose(file) };
        return -1;
    }
    let result = unsafe { printer(file, info, arguments) };
    let closed = unsafe { stdio::fclose(file) };
    if closed != 0 { -1 } else { result }
}

/// None means the GNU handler declined this conversion, preserving its varargs.
pub(super) unsafe fn emit<S: Sink>(
    sink: &mut S,
    spec: &Spec,
    arguments: &mut VaList,
) -> Result<bool, i32> {
    let handler = registry::handler(spec.conversion)?;
    if handler.printer == 0 {
        return Ok(false);
    }
    let info = info(spec, false)?;
    let Some(types) = (unsafe { describe(handler, &info)? }) else {
        return Ok(false);
    };
    let pointer_bytes = types
        .count
        .checked_mul(8)
        .and_then(|n| n.checked_add(15))
        .ok_or(kinakaze_vfs::EOVERFLOW)?
        & !15;
    let slots_end = pointer_bytes
        .checked_add(types.count.checked_mul(16).ok_or(kinakaze_vfs::EOVERFLOW)?)
        .ok_or(kinakaze_vfs::EOVERFLOW)?;
    let mut size = slots_end;
    for i in 0..types.count {
        if types.kind(i) & 0x800 == 0 && types.kind(i) & 0xff >= 8 {
            let n = usize::try_from(types.size(i)).map_err(|_| kinakaze_vfs::EINVAL)?;
            if n == 0 {
                return Err(kinakaze_vfs::EINVAL);
            }
            size = size
                .checked_add(n)
                .and_then(|n| n.checked_add(15))
                .ok_or(kinakaze_vfs::EOVERFLOW)?
                & !15;
        }
    }
    let values = Bytes::new(size)?;
    let saved = *arguments;
    let pointers = values.0.cast::<*const c_void>();
    let mut payload = slots_end;
    for i in 0..types.count {
        let slot = unsafe { values.0.add(pointer_bytes + i * 16) };
        unsafe { pointers.add(i).write(slot.cast()) };
        let kind = types.kind(i);
        if kind & 0x800 == 0 && kind & 0xff >= 8 {
            let reader = registry::reader(kind & 0xff)?;
            let target = unsafe { values.0.add(payload) };
            unsafe {
                reader(target.cast(), arguments);
                slot.cast::<*const u8>().write(target);
            }
            payload = (payload + types.size(i) as usize + 15) & !15;
        } else {
            unsafe { builtin(kind, arguments, slot)? };
        }
    }
    let printer: Printer = unsafe { core::mem::transmute(handler.printer) };
    let result = unsafe { call(sink, printer, &info, pointers) };
    if result == -2 {
        *arguments = saved;
        Ok(false)
    } else if result < 0 {
        Err(kinakaze_tls::errno())
    } else {
        Ok(true)
    }
}
