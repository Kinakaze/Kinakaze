//! Linux interface ioctls use the socket's network namespace and live topology.
use crate::{EADDRNOTAVAIL, EFAULT, EINVAL, ENODEV, EOPNOTSUPP, EPERM};
use core::{ffi::c_void, ptr};

pub const SIOCGIFCONF: u64 = 0x8912;
pub const SIOCGIFFLAGS: u64 = 0x8913;
pub const SIOCSIFFLAGS: u64 = 0x8914;
pub const SIOCGIFADDR: u64 = 0x8915;
pub const SIOCGIFDSTADDR: u64 = 0x8917;
pub const SIOCGIFBRDADDR: u64 = 0x8919;
pub const SIOCGIFNETMASK: u64 = 0x891b;
pub const SIOCGIFMTU: u64 = 0x8921;
pub const SIOCGIFHWADDR: u64 = 0x8927;
pub const SIOCGIFINDEX: u64 = 0x8933;

#[derive(Clone, Copy)]
#[repr(C)]
struct IfReq {
    name: [u8; 16],
    data: [u8; 24],
}
#[derive(Clone, Copy)]
#[repr(C)]
struct IfConf {
    length: i32,
    buffer: *mut u8,
}

fn ipv4(address: &crate::route_state::Address, kind: u16) -> Option<[u8; 4]> {
    address
        .attributes
        .iter()
        .find(|a| a.kind == kind)?
        .value
        .as_slice()
        .try_into()
        .ok()
}
fn local(address: &crate::route_state::Address) -> Result<[u8; 4], i32> {
    ipv4(address, 2)
        .or_else(|| ipv4(address, 1))
        .ok_or(crate::EIO)
}
fn sockaddr(data: &mut [u8; 24], ip: [u8; 4]) {
    data[..16].fill(0);
    data[..2].copy_from_slice(&2u16.to_ne_bytes());
    data[4..8].copy_from_slice(&ip);
}
fn addresses(
    ns: u64,
    interfaces: &[crate::hostnet::Interface],
) -> Result<Vec<crate::route_state::Address>, i32> {
    let mut addresses = crate::route_state::snapshot_for(ns)?.addresses;
    addresses.extend(crate::hostnet::addresses(ns, interfaces)?);
    addresses.retain(|a| a.family == 2);
    Ok(addresses)
}

unsafe fn configuration(ns: u64, argument: *mut c_void) -> Result<(), i32> {
    let mut conf = unsafe { ptr::read_unaligned(argument.cast::<IfConf>()) };
    let links = crate::netlink::network_interfaces(ns)?;
    let addresses = addresses(ns, &links)?;
    let mut count = 0usize;
    for link in links {
        for address in addresses.iter().filter(|a| a.index == link.link.index) {
            let size = core::mem::size_of::<IfReq>();
            if !conf.buffer.is_null() && count + size > conf.length.max(0) as usize {
                break;
            }
            if !conf.buffer.is_null() {
                let mut row = IfReq {
                    name: [0; 16],
                    data: [0; 24],
                };
                let name = link.link.name.as_bytes();
                let length = name.len().min(row.name.len() - 1);
                row.name[..length].copy_from_slice(&name[..length]);
                sockaddr(&mut row.data, local(address)?);
                unsafe { ptr::write_unaligned(conf.buffer.add(count).cast::<IfReq>(), row) };
            }
            count = count.checked_add(size).ok_or(crate::EOVERFLOW)?;
        }
    }
    conf.length = i32::try_from(count).map_err(|_| crate::EOVERFLOW)?;
    unsafe { ptr::write_unaligned(argument.cast::<IfConf>(), conf) };
    Ok(())
}

fn set_flags(ns: u64, interface: &crate::hostnet::Interface, flags: u32) -> Result<(), i32> {
    if !crate::user_namespace::capable(crate::namespaces::network_owner(ns)?, 12) {
        return Err(EPERM);
    }
    // Linux __dev_change_flags: writable flags, excluding device/status bits.
    const WRITABLE: u32 =
        0x1 | 0x4 | 0x20 | 0x80 | 0x100 | 0x200 | 0x1000 | 0x2000 | 0x4000 | 0x8000;
    if interface.host {
        return if (interface.link.flags ^ flags) & WRITABLE == 0 {
            Ok(())
        } else {
            Err(EOPNOTSUPP)
        };
    }
    crate::route_state::transaction_for(ns, |state| {
        if interface.hardware_type == 772 {
            state.set_loopback_flags(flags, WRITABLE);
        } else {
            let link = state
                .links
                .iter_mut()
                .find(|l| l.index == interface.link.index)
                .ok_or(ENODEV)?;
            link.flags = (link.flags & !WRITABLE) | (flags & WRITABLE);
        }
        Ok::<_, i32>(())
    })?
}

/// # Safety
/// The caller provides the writable Linux ifreq/ifconf structure and its buffer.
pub unsafe fn interface(fd: i32, request: u64, argument: *mut c_void) -> Result<(), i32> {
    crate::get(fd)?;
    let socket = super::endpoint(fd)?.ok_or(crate::ENOTSOCK)?;
    let ns = super::read(&socket)?.ns;
    if argument.is_null() {
        return Err(EFAULT);
    }
    if request == SIOCGIFCONF {
        return unsafe { configuration(ns, argument) };
    }
    let mut row = unsafe { ptr::read_unaligned(argument.cast::<IfReq>()) };
    let end = row.name[..15]
        .iter()
        .position(|b| *b == 0 || *b == b':')
        .unwrap_or(15);
    let name = &row.name[..end];
    let _guard = if request == SIOCSIFFLAGS {
        Some(super::shared::topology_guard()?)
    } else {
        None
    };
    let interfaces = crate::netlink::network_interfaces(ns)?;
    let interface = interfaces
        .iter()
        .find(|i| i.link.name.as_bytes() == name)
        .ok_or(ENODEV)?;
    match request {
        SIOCGIFFLAGS => row.data[..2].copy_from_slice(&(interface.link.flags as u16).to_ne_bytes()),
        SIOCSIFFLAGS => {
            return set_flags(
                ns,
                interface,
                u32::from(u16::from_ne_bytes(row.data[..2].try_into().unwrap())),
            );
        }
        SIOCGIFINDEX => row.data[..4].copy_from_slice(&interface.link.index.to_ne_bytes()),
        SIOCGIFMTU => row.data[..4].copy_from_slice(&interface.link.mtu.to_ne_bytes()),
        SIOCGIFHWADDR => {
            row.data[..16].fill(0);
            row.data[..2].copy_from_slice(&interface.hardware_type.to_ne_bytes());
            let length = interface.link.address.len().min(14);
            row.data[2..2 + length].copy_from_slice(&interface.link.address[..length]);
        }
        SIOCGIFADDR | SIOCGIFDSTADDR | SIOCGIFBRDADDR | SIOCGIFNETMASK => {
            let addresses = addresses(ns, &interfaces)?;
            let address = addresses
                .iter()
                .find(|a| a.index == interface.link.index)
                .ok_or(EADDRNOTAVAIL)?;
            let ip = match request {
                SIOCGIFADDR => local(address)?,
                SIOCGIFDSTADDR => ipv4(address, 1).ok_or(EADDRNOTAVAIL)?,
                // An unconfigured broadcast field is zero, as in Linux in_ifaddr.
                SIOCGIFBRDADDR => ipv4(address, 4).unwrap_or([0; 4]),
                _ => {
                    if address.prefix_len > 32 {
                        return Err(EINVAL);
                    }
                    u32::MAX
                        .checked_shl(32 - u32::from(address.prefix_len))
                        .unwrap_or(0)
                        .to_be_bytes()
                }
            };
            sockaddr(&mut row.data, ip);
        }
        _ => return Err(crate::ENOTTY),
    }
    unsafe { ptr::write_unaligned(argument.cast::<IfReq>(), row) };
    Ok(())
}
