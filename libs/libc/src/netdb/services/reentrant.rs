//! Caller-owned servent buffers; validate capacity before publishing any field.
use super::*;

unsafe fn store(
    service: &Service,
    result: *mut Servent,
    buffer: *mut c_char,
    capacity: usize,
) -> Result<(), i32> {
    let alignment = core::mem::align_of::<*mut c_char>();
    let padding = (alignment - buffer.addr() % alignment) % alignment;
    let pointers = service.names.aliases().len() + 1;
    let table_bytes = pointers
        .checked_mul(core::mem::size_of::<*mut c_char>())
        .ok_or(kinakaze_vfs::ERANGE)?;
    let strings = std::iter::once(&service.names.name)
        .chain(std::iter::once(&service.protocol))
        .chain(service.names.aliases());
    let needed = strings
        .clone()
        .try_fold(padding + table_bytes, |n, text| {
            n.checked_add(text.as_bytes_with_nul().len())
        })
        .ok_or(kinakaze_vfs::ERANGE)?;
    if needed > capacity {
        return Err(kinakaze_vfs::ERANGE);
    }
    let aliases = unsafe { buffer.cast::<u8>().add(padding).cast::<*mut c_char>() };
    let mut offset = padding + table_bytes;
    let mut output = Servent {
        s_name: ptr::null_mut(),
        s_aliases: aliases,
        s_port: c_int::from(service.port.to_be()),
        s_proto: ptr::null_mut(),
    };
    for (index, text) in strings.enumerate() {
        let target = unsafe { buffer.cast::<u8>().add(offset) };
        unsafe {
            ptr::copy_nonoverlapping(
                text.as_ptr().cast::<u8>(),
                target,
                text.as_bytes_with_nul().len(),
            )
        };
        match index {
            0 => output.s_name = target.cast(),
            1 => output.s_proto = target.cast(),
            _ => unsafe { aliases.add(index - 2).write(target.cast()) },
        }
        offset += text.as_bytes_with_nul().len();
    }
    unsafe {
        aliases.add(pointers - 1).write(ptr::null_mut());
        result.write(output);
    }
    Ok(())
}

unsafe fn lookup_into(
    lookup: impl FnOnce() -> Result<Option<Service>, i32>,
    output: *mut Servent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Servent,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe { *result = ptr::null_mut() };
    if output.is_null() || (buffer.is_null() && capacity != 0) {
        return kinakaze_vfs::EINVAL;
    }
    match lookup() {
        Ok(None) => 0,
        Err(error) => error,
        Ok(Some(service)) => match unsafe { store(&service, output, buffer, capacity) } {
            Ok(()) => {
                unsafe { *result = output };
                0
            }
            Err(error) => error,
        },
    }
}

/// # Safety
/// Arguments follow the Linux getservbyname_r buffer and string contracts.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getservbyname_r(
    name: *const c_char,
    protocol: *const c_char,
    output: *mut Servent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Servent,
) -> c_int {
    unsafe {
        lookup_into(
            || {
                if name.is_null() {
                    return Err(kinakaze_vfs::EINVAL);
                }
                let name = CStr::from_ptr(name).to_bytes();
                let protocol = (!protocol.is_null()).then(|| CStr::from_ptr(protocol).to_bytes());
                find_service_by_name(name, protocol)
            },
            output,
            buffer,
            capacity,
            result,
        )
    }
}

/// # Safety
/// Arguments follow the Linux getservbyport_r buffer and string contracts.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getservbyport_r(
    port: c_int,
    protocol: *const c_char,
    output: *mut Servent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Servent,
) -> c_int {
    unsafe {
        lookup_into(
            || {
                let Ok(port) = u16::try_from(port) else {
                    return Ok(None);
                };
                let protocol = (!protocol.is_null()).then(|| CStr::from_ptr(protocol).to_bytes());
                find_service_by_port(u16::from_be(port), protocol)
            },
            output,
            buffer,
            capacity,
            result,
        )
    }
}
