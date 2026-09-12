//! Capture the guest call site before entering native code; ld.so owns ELF CFI.
use core::ffi::{CStr, c_char, c_int, c_void};

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_backtrace(
    _buffer: *mut *mut c_void,
    _size: c_int,
) -> c_int {
    core::arch::naked_asm!(
        "sub rsp, 152",
        "mov [rsp], rax", "mov [rsp + 8], rdx", "mov [rsp + 16], rcx",
        "mov [rsp + 24], rbx", "mov [rsp + 32], rsi", "mov [rsp + 40], rdi",
        "mov [rsp + 48], rbp", "lea rax, [rsp + 160]", "mov [rsp + 56], rax",
        "mov [rsp + 64], r8", "mov [rsp + 72], r9", "mov [rsp + 80], r10",
        "mov [rsp + 88], r11", "mov [rsp + 96], r12", "mov [rsp + 104], r13",
        "mov [rsp + 112], r14", "mov [rsp + 120], r15",
        "mov rax, [rsp + 152]", "mov [rsp + 128], rax",
        "mov rdx, rsp", "call {capture}", "add rsp, 152", "ret",
        capture = sym capture,
    );
}

unsafe extern "sysv64" fn capture(
    buffer: *mut *mut c_void,
    size: c_int,
    registers: *const u64,
) -> c_int {
    if size <= 0 || buffer.is_null() {
        return 0;
    }
    let Some(services) = kinakaze_runtime::services::unwind() else {
        return 0;
    };
    unsafe { (services.backtrace)(registers, buffer.cast(), size as usize) as c_int }
}

fn describe(address: *mut c_void) -> String {
    let mut info = kinakaze_runtime::services::AddressInfo {
        dli_fname: core::ptr::null(),
        dli_fbase: core::ptr::null_mut(),
        dli_sname: core::ptr::null(),
        dli_saddr: core::ptr::null_mut(),
    };
    if let Some(services) = kinakaze_runtime::services::unwind()
        && unsafe { (services.address_info)(address, &mut info) } != 0
        && !info.dli_fname.is_null()
    {
        let file = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
        if !info.dli_sname.is_null() {
            let symbol = unsafe { CStr::from_ptr(info.dli_sname) }.to_string_lossy();
            return format!(
                "{file}({symbol}+{:#x}) [{address:p}]",
                (address as usize).saturating_sub(info.dli_saddr as usize)
            );
        }
        return format!(
            "{file}(+{:#x}) [{address:p}]",
            (address as usize).saturating_sub(info.dli_fbase as usize)
        );
    }
    format!("[{address:p}]")
}

/// One guest allocation owns both the pointer vector and all NUL-terminated text.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_backtrace_symbols(
    buffer: *const *mut c_void,
    size: c_int,
) -> *mut *mut c_char {
    if size < 0 || (size > 0 && buffer.is_null()) {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return core::ptr::null_mut();
    }
    let mut text = Vec::with_capacity(size as usize);
    let mut bytes = (size as usize).checked_mul(core::mem::size_of::<usize>());
    for index in 0..size as usize {
        let line = describe(unsafe { *buffer.add(index) });
        bytes = bytes.and_then(|n| n.checked_add(line.len() + 1));
        text.push(line);
    }
    let Some(bytes) = bytes else {
        crate::set_errno(kinakaze_vfs::ENOMEM);
        return core::ptr::null_mut();
    };
    let result = unsafe { crate::c_malloc(bytes) }.cast::<*mut c_char>();
    if result.is_null() {
        return result;
    }
    let mut next = unsafe { result.add(size as usize) }.cast::<u8>();
    for (index, line) in text.iter().enumerate() {
        unsafe {
            result.add(index).write(next.cast());
            core::ptr::copy_nonoverlapping(line.as_ptr(), next, line.len());
            next.add(line.len()).write(0);
            next = next.add(line.len() + 1);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_backtrace_symbols_fd(
    buffer: *const *mut c_void,
    size: c_int,
    fd: c_int,
) {
    if size <= 0 || buffer.is_null() {
        return;
    }
    for index in 0..size as usize {
        let line = describe(unsafe { *buffer.add(index) }) + "\n";
        let mut remaining = line.as_bytes();
        while !remaining.is_empty() {
            let written = unsafe {
                crate::kinakaze_abi_write(fd, remaining.as_ptr().cast(), remaining.len())
            };
            if written <= 0 {
                return;
            }
            remaining = &remaining[written as usize..];
        }
    }
}
