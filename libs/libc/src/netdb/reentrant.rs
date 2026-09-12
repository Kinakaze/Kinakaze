//! Legacy host lookups with caller-owned, bounded backing storage.
use super::*;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyname2(
    name: *const c_char,
    family: c_int,
) -> *mut Hostent {
    if family == AF_INET {
        return unsafe { kinakaze_abi_gethostbyname(name) };
    }
    if family != AF_INET6 {
        set_h_errno(NO_RECOVERY);
        crate::set_errno(97);
        return ptr::null_mut();
    }
    let Some(name) = (unsafe { borrow(name) }) else {
        set_h_errno(HOST_NOT_FOUND);
        return ptr::null_mut();
    };
    if let Ok(address) = name.parse::<std::net::Ipv6Addr>() {
        return publish_hostent(name, family, vec![address.octets().to_vec()]);
    }
    match resolve_addresses(name, family) {
        Ok((canonical, addresses)) => publish_hostent(&canonical, family, addresses),
        Err(error) => {
            set_h_errno(h_errno_from_eai(error));
            ptr::null_mut()
        }
    }
}

pub(super) unsafe fn copy_host(
    h: *const Hostent,
    ret: *mut Hostent,
    buf: *mut c_char,
    buflen: usize,
    result: *mut *mut Hostent,
    h_errnop: *mut c_int,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if ret.is_null() || buf.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    if h.is_null() {
        if !h_errnop.is_null() {
            unsafe {
                *h_errnop = *h_errno_location();
            }
        }
        return 0;
    }
    let h = unsafe { &*h };
    let name = unsafe { CStr::from_ptr(h.h_name) }.to_bytes_with_nul();
    let mut aliases = Vec::new();
    let mut addresses = Vec::new();
    if !h.h_aliases.is_null() {
        let mut p = h.h_aliases;
        while !unsafe { *p }.is_null() {
            aliases.push(unsafe { CStr::from_ptr(*p) }.to_bytes_with_nul());
            p = unsafe { p.add(1) };
        }
    }
    if !h.h_addr_list.is_null() {
        let mut p = h.h_addr_list;
        while !unsafe { *p }.is_null() {
            addresses.push(unsafe {
                std::slice::from_raw_parts((*p).cast::<u8>(), h.h_length as usize)
            });
            p = unsafe { p.add(1) };
        }
    }
    let padding = (buf as usize).wrapping_neg() & (size_of::<usize>() - 1);
    let pointers = (aliases.len() + addresses.len() + 2) * size_of::<usize>();
    let needed = padding
        + pointers
        + name.len()
        + aliases.iter().map(|s| s.len()).sum::<usize>()
        + addresses.iter().map(|s| s.len()).sum::<usize>();
    if needed > buflen {
        if !h_errnop.is_null() {
            unsafe {
                *h_errnop = -1;
            }
        }
        crate::set_errno(kinakaze_vfs::ERANGE);
        return kinakaze_vfs::ERANGE;
    }
    unsafe {
        let alias_table = buf.add(padding).cast::<*mut c_char>();
        let address_table = alias_table.add(aliases.len() + 1);
        let mut at = buf.add(padding + pointers);
        let canonical = at;
        ptr::copy_nonoverlapping(name.as_ptr(), at.cast(), name.len());
        at = at.add(name.len());
        for (n, value) in aliases.iter().enumerate() {
            alias_table.add(n).write(at);
            ptr::copy_nonoverlapping(value.as_ptr(), at.cast(), value.len());
            at = at.add(value.len());
        }
        alias_table.add(aliases.len()).write(ptr::null_mut());
        for (n, value) in addresses.iter().enumerate() {
            address_table.add(n).write(at);
            ptr::copy_nonoverlapping(value.as_ptr(), at.cast(), value.len());
            at = at.add(value.len());
        }
        address_table.add(addresses.len()).write(ptr::null_mut());
        ret.write(Hostent {
            h_name: canonical,
            h_aliases: alias_table,
            h_addrtype: h.h_addrtype,
            h_length: h.h_length,
            h_addr_list: address_table,
        });
        *result = ret;
        if !h_errnop.is_null() {
            *h_errnop = 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyname2_r(
    name: *const c_char,
    family: c_int,
    ret: *mut Hostent,
    buf: *mut c_char,
    buflen: usize,
    result: *mut *mut Hostent,
    h_errnop: *mut c_int,
) -> c_int {
    let h = unsafe { kinakaze_abi_gethostbyname2(name, family) };
    unsafe { copy_host(h, ret, buf, buflen, result, h_errnop) }
}
