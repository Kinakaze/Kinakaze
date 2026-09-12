//! Resolve guest service names before handing numeric ports to address resolution.
use super::super::{
    AI_CANONNAME, AI_NUMERICSERV, AddrInfo, EAI_NONAME, EAI_SERVICE, EAI_SYSTEM, SOCK_DGRAM,
    SOCK_STREAM, kinakaze_abi_freeaddrinfo, kinakaze_abi_getaddrinfo,
};
use core::{
    ffi::{c_char, c_int},
    ptr,
};
use std::ffi::CString;

pub(crate) struct AddressHints {
    pub flags: c_int,
    pub family: c_int,
    pub socktype: c_int,
    pub protocol: c_int,
}

pub(crate) unsafe fn resolve_addresses(
    node: *const c_char,
    name: &[u8],
    hints: AddressHints,
    result: *mut *mut AddrInfo,
) -> c_int {
    if hints.flags & AI_NUMERICSERV != 0 {
        return EAI_NONAME;
    }
    let mut head: *mut AddrInfo = ptr::null_mut();
    let mut tail: *mut AddrInfo = ptr::null_mut();
    // Re-entry always uses a numeric port: depth one, with no host service lookup.
    for (kind, protocol, label) in [
        (SOCK_STREAM, 6, b"tcp".as_slice()),
        (SOCK_DGRAM, 17, b"udp".as_slice()),
    ] {
        if (hints.socktype != 0 && hints.socktype != kind)
            || (hints.protocol != 0 && hints.protocol != protocol)
        {
            continue;
        }
        let service = match super::find_service_by_name(name, Some(label)) {
            Ok(Some(service)) => service,
            Ok(None) => continue,
            Err(error) => {
                unsafe { kinakaze_abi_freeaddrinfo(head) };
                crate::set_errno(error);
                return if error == kinakaze_vfs::ENOENT {
                    EAI_SERVICE
                } else {
                    EAI_SYSTEM
                };
            }
        };
        let numeric = CString::new(service.port.to_string()).unwrap();
        let selected = AddrInfo {
            ai_flags: if head.is_null() {
                hints.flags
            } else {
                hints.flags & !AI_CANONNAME
            },
            ai_family: hints.family,
            ai_socktype: kind,
            ai_protocol: protocol,
            ai_addrlen: 0,
            ai_addr: ptr::null_mut(),
            ai_canonname: ptr::null_mut(),
            ai_next: ptr::null_mut(),
        };
        let mut part = ptr::null_mut();
        let status =
            unsafe { kinakaze_abi_getaddrinfo(node, numeric.as_ptr(), &selected, &mut part) };
        if status != 0 {
            unsafe { kinakaze_abi_freeaddrinfo(head) };
            return status;
        }
        if head.is_null() {
            head = part;
        } else {
            unsafe { (*tail).ai_next = part };
        }
        tail = part;
        while !tail.is_null() && unsafe { !(*tail).ai_next.is_null() } {
            tail = unsafe { (*tail).ai_next };
        }
    }
    if head.is_null() {
        return EAI_SERVICE;
    }
    unsafe { *result = head };
    0
}
