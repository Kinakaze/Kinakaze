//! Raw Linux `bpf(2)` syscall ABI.
//!
//! Attribute decoding stays in libc because the union contains guest pointers.
//! Object lifetime, verification, cgroup attachment and execution live in the
//! VFS, where all descriptor operations share one model.

use core::ffi::c_void;
use core::mem;
use core::ptr;
use std::sync::OnceLock;

use kinakaze_vfs::{EFAULT, EINVAL, ENOSPC, EOPNOTSUPP};

const E2BIG: i32 = 7;

const BPF_MAP_CREATE: u64 = 0;
const BPF_PROG_LOAD: u64 = 5;
const BPF_PROG_ATTACH: u64 = 8;
const BPF_PROG_DETACH: u64 = 9;
const BPF_PROG_GET_FD_BY_ID: u64 = 13;
const BPF_OBJ_GET_INFO_BY_FD: u64 = 15;
const BPF_PROG_QUERY: u64 = 16;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProgLoadAttr {
    program_type: u32,
    instruction_count: u32,
    instructions: u64,
    license: u64,
    log_level: u32,
    log_size: u32,
    log_buffer: u64,
    kernel_version: u32,
    program_flags: u32,
    program_name: [u8; 16],
    program_ifindex: u32,
    expected_attach_type: u32,
    program_btf_fd: u32,
    function_info_record_size: u32,
    function_info: u64,
    function_info_count: u32,
    line_info_record_size: u32,
    line_info: u64,
    line_info_count: u32,
    attach_btf_id: u32,
    attach_program_fd: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProgAttachAttr {
    target_fd: u32,
    attach_program_fd: u32,
    attach_type: u32,
    attach_flags: u32,
    replace_program_fd: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProgDetachAttr {
    target_fd: u32,
    attach_program_fd: u32,
    attach_type: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct GetFdByIdAttr {
    id: u32,
    open_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct GetInfoAttr {
    fd: u32,
    info_length: u32,
    info: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProgQueryAttr {
    target_fd: u32,
    attach_type: u32,
    query_flags: u32,
    attach_flags: u32,
    program_ids: u64,
    program_count: u32,
    reserved: u32,
}

fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_BPF_TRACE").is_some())
}

fn trace(command: u64, size: usize, result: &Result<i64, i32>) {
    if trace_enabled() {
        eprintln!(
            "kinakaze bpf: pid={} command={} size={} result={result:?}",
            kinakaze_runtime::job::current_pid(),
            command,
            size
        );
    }
}

unsafe fn copy_attr<T: Copy + Default>(
    pointer: *const c_void,
    size: usize,
    minimum: usize,
) -> Result<T, i32> {
    if pointer.is_null() {
        return Err(EFAULT);
    }
    if size < minimum {
        return Err(EINVAL);
    }
    let mut value = T::default();
    let known = mem::size_of::<T>();
    let copied = size.min(known);
    // SAFETY: the syscall caller promises `size` readable bytes. The destination
    // is an initialized local of at least `copied` bytes.
    unsafe {
        ptr::copy_nonoverlapping(pointer.cast::<u8>(), (&raw mut value).cast::<u8>(), copied);
    }
    if size > known {
        // Linux rejects a newer/larger union whose unknown tail is not zero.
        let tail =
            unsafe { core::slice::from_raw_parts(pointer.cast::<u8>().add(known), size - known) };
        if tail.iter().any(|byte| *byte != 0) {
            return Err(E2BIG);
        }
    }
    Ok(value)
}

unsafe fn read_c_string(pointer: u64, limit: usize) -> Result<String, i32> {
    if pointer == 0 {
        return Err(EFAULT);
    }
    let pointer = pointer as usize as *const u8;
    for length in 0..=limit {
        // SAFETY: the bpf syscall ABI requires a readable NUL-terminated string.
        if unsafe { pointer.add(length).read() } == 0 {
            let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
            return core::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| EINVAL);
        }
    }
    Err(EINVAL)
}

unsafe fn write_verifier_log(attr: ProgLoadAttr, message: &str) -> Result<(), i32> {
    if attr.log_level == 0 {
        return Ok(());
    }
    if attr.log_buffer == 0 || attr.log_size == 0 {
        return Err(EFAULT);
    }
    let capacity = attr.log_size as usize;
    let written = message.len().min(capacity.saturating_sub(1));
    let destination = attr.log_buffer as usize as *mut u8;
    // SAFETY: bpf(2) requires the caller's log buffer to be writable for log_size.
    unsafe {
        ptr::copy_nonoverlapping(message.as_ptr(), destination, written);
        destination.add(written).write(0);
    }
    Ok(())
}

unsafe fn prog_load(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let attr: ProgLoadAttr = unsafe { copy_attr(pointer, size, 24) }?;
    if attr.instruction_count == 0 || attr.instructions == 0 {
        return Err(EINVAL);
    }
    let byte_count = (attr.instruction_count as usize)
        .checked_mul(8)
        .ok_or(E2BIG)?;
    // SAFETY: the caller describes instruction_count complete bpf_insn records.
    let instructions = unsafe {
        core::slice::from_raw_parts(attr.instructions as usize as *const u8, byte_count).to_vec()
    };
    let license = unsafe { read_c_string(attr.license, 128) }?;
    let unsupported_metadata = attr.program_ifindex != 0
        || attr.program_btf_fd != 0
        || attr.function_info_record_size != 0
        || attr.function_info != 0
        || attr.function_info_count != 0
        || attr.line_info_record_size != 0
        || attr.line_info != 0
        || attr.line_info_count != 0
        || attr.attach_btf_id != 0
        || attr.attach_program_fd != 0;
    if unsupported_metadata {
        return Err(EOPNOTSUPP);
    }
    let load = kinakaze_vfs::bpf::ProgramLoad {
        program_type: attr.program_type,
        expected_attach_type: attr.expected_attach_type,
        flags: attr.program_flags,
        instructions,
        license,
        name: attr.program_name,
    };
    match kinakaze_vfs::bpf::load_program(load) {
        Ok(fd) => Ok(fd as i64),
        Err(error) => {
            if let Some(log) = error.verifier_log.as_deref() {
                unsafe { write_verifier_log(attr, log) }?;
            }
            Err(error.errno)
        }
    }
}

fn fd(value: u32) -> Result<i32, i32> {
    i32::try_from(value).map_err(|_| kinakaze_vfs::EBADF)
}

unsafe fn prog_attach(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let attr: ProgAttachAttr = unsafe { copy_attr(pointer, size, 16) }?;
    let replace = (attr.replace_program_fd != 0)
        .then(|| fd(attr.replace_program_fd))
        .transpose()?;
    kinakaze_vfs::bpf::attach(
        fd(attr.target_fd)?,
        fd(attr.attach_program_fd)?,
        attr.attach_type,
        attr.attach_flags,
        replace,
    )?;
    Ok(0)
}

unsafe fn prog_detach(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let attr: ProgDetachAttr =
        unsafe { copy_attr(pointer, size, mem::size_of::<ProgDetachAttr>()) }?;
    kinakaze_vfs::bpf::detach(
        fd(attr.target_fd)?,
        fd(attr.attach_program_fd)?,
        attr.attach_type,
    )?;
    Ok(0)
}

unsafe fn get_fd_by_id(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let attr: GetFdByIdAttr = unsafe { copy_attr(pointer, size, 4) }?;
    if attr.open_flags != 0 {
        return Err(EINVAL);
    }
    kinakaze_vfs::bpf::get_fd_by_id(attr.id).map(i64::from)
}

fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    if let Some(destination) = buffer.get_mut(offset..offset + 4) {
        destination.copy_from_slice(&value.to_le_bytes());
    }
}

fn put_u64(buffer: &mut [u8], offset: usize, value: u64) {
    if let Some(destination) = buffer.get_mut(offset..offset + 8) {
        destination.copy_from_slice(&value.to_le_bytes());
    }
}

unsafe fn copy_attr_out<T>(pointer: *mut c_void, size: usize, value: &T) {
    let copied = size.min(mem::size_of::<T>());
    // SAFETY: the syscall caller supplied `size` writable bytes for an in/out
    // bpf_attr.  Linux copies back only the portion present in that user ABI.
    unsafe {
        ptr::copy_nonoverlapping(
            (value as *const T).cast::<u8>(),
            pointer.cast::<u8>(),
            copied,
        );
    }
}

unsafe fn get_info(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let mut attr: GetInfoAttr = unsafe { copy_attr(pointer, size, mem::size_of::<GetInfoAttr>()) }?;
    if attr.info_length != 0 && attr.info == 0 {
        return Err(EFAULT);
    }
    let info = kinakaze_vfs::bpf::program_info(fd(attr.fd)?)?;
    // Linux's bpf_prog_info through the fields used by cilium/ebpf v0.7.0.
    // Later fields remain zero because this implementation has no JIT image,
    // BTF object, maps, or accumulated runtime statistics to report.
    const KNOWN_INFO_SIZE: usize = 232;
    let output_length = (attr.info_length as usize).min(KNOWN_INFO_SIZE);
    let mut output = vec![0u8; output_length];
    put_u32(&mut output, 0, info.program_type);
    put_u32(&mut output, 4, info.id);
    if let Some(destination) = output.get_mut(8..16) {
        destination.copy_from_slice(&info.tag);
    }
    put_u32(&mut output, 20, info.translated_length);
    put_u64(&mut output, 40, info.load_time);
    put_u32(&mut output, 48, info.created_by_uid);
    if let Some(destination) = output.get_mut(64..80) {
        destination.copy_from_slice(&info.name);
    }
    put_u32(&mut output, 84, info.gpl_compatible);
    if !output.is_empty() {
        // SAFETY: the caller provided info_length writable bytes.
        unsafe {
            ptr::copy_nonoverlapping(output.as_ptr(), attr.info as usize as *mut u8, output.len());
        }
    }
    attr.info_length = KNOWN_INFO_SIZE as u32;
    unsafe { copy_attr_out(pointer, size, &attr) };
    Ok(0)
}

unsafe fn prog_query(pointer: *mut c_void, size: usize) -> Result<i64, i32> {
    let mut attr: ProgQueryAttr = unsafe { copy_attr(pointer, size, 28) }?;
    if attr.reserved != 0 {
        return Err(EINVAL);
    }
    let (ids, attach_flags) =
        kinakaze_vfs::bpf::query(fd(attr.target_fd)?, attr.attach_type, attr.query_flags)?;
    let capacity = attr.program_count as usize;
    let copied = capacity.min(ids.len());
    if copied != 0 {
        if attr.program_ids == 0 {
            return Err(EFAULT);
        }
        let destination = attr.program_ids as usize as *mut u32;
        for (index, id) in ids[..copied].iter().enumerate() {
            // SAFETY: the caller advertised capacity u32 entries.
            unsafe { destination.add(index).write_unaligned(*id) };
        }
    }
    attr.program_count = ids.len() as u32;
    attr.attach_flags = attach_flags;
    unsafe { copy_attr_out(pointer, size, &attr) };
    if capacity < ids.len() {
        Err(ENOSPC)
    } else {
        Ok(0)
    }
}

/// Implements the raw Linux syscall return convention: success is non-negative
/// and failure is `-errno` without touching libc's thread-local errno.
///
/// # Safety
///
/// `attribute` must point to `size` bytes readable by the command, including any
/// writable output fields required by that command.
pub unsafe fn syscall(command: u64, attribute: *mut c_void, size: usize) -> i64 {
    let result = match command {
        BPF_MAP_CREATE => Err(EOPNOTSUPP),
        BPF_PROG_LOAD => unsafe { prog_load(attribute, size) },
        BPF_PROG_ATTACH => unsafe { prog_attach(attribute, size) },
        BPF_PROG_DETACH => unsafe { prog_detach(attribute, size) },
        BPF_PROG_GET_FD_BY_ID => unsafe { get_fd_by_id(attribute, size) },
        BPF_OBJ_GET_INFO_BY_FD => unsafe { get_info(attribute, size) },
        BPF_PROG_QUERY => unsafe { prog_query(attribute, size) },
        _ => Err(EINVAL),
    };
    trace(command, size, &result);
    match result {
        Ok(value) => value,
        Err(error) => -i64::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_commands_fail_instead_of_reporting_success() {
        let mut attr = [0u8; 8];
        let result = unsafe { syscall(0xffff, attr.as_mut_ptr().cast(), attr.len()) };
        assert_eq!(result, -i64::from(EINVAL));
    }

    #[test]
    fn nonzero_unknown_attribute_tail_is_rejected() {
        let mut attr = vec![0u8; mem::size_of::<GetFdByIdAttr>() + 1];
        *attr.last_mut().unwrap() = 1;
        let result: Result<GetFdByIdAttr, i32> = unsafe {
            copy_attr(
                attr.as_ptr().cast(),
                attr.len(),
                mem::size_of::<GetFdByIdAttr>(),
            )
        };
        assert_eq!(result.unwrap_err(), E2BIG);
    }

    #[test]
    fn attributes_match_the_linux_uapi_layout_used_by_runc() {
        assert_eq!(mem::size_of::<ProgLoadAttr>(), 120);
        assert_eq!(mem::size_of::<ProgAttachAttr>(), 20);
        assert_eq!(mem::size_of::<ProgDetachAttr>(), 12);
        assert_eq!(mem::size_of::<GetFdByIdAttr>(), 8);
        assert_eq!(mem::size_of::<GetInfoAttr>(), 16);
        assert_eq!(mem::size_of::<ProgQueryAttr>(), 32);
    }

    #[test]
    fn short_output_attribute_does_not_write_past_the_user_size() {
        let mut storage = [0xa5u8; 40];
        let value = ProgQueryAttr {
            program_count: 7,
            ..ProgQueryAttr::default()
        };
        unsafe { copy_attr_out(storage.as_mut_ptr().cast(), 28, &value) };
        assert_eq!(&storage[28..], &[0xa5; 12]);
    }
}
