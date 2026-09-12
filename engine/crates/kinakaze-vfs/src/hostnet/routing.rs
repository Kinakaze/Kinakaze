//! On-demand native IPv4 route and neighbor observations. Never modifies Windows.
use super::{Interface, MibAllocation};
use crate::{EIO, EOVERFLOW, errno_from_win32};
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::*;

pub(crate) struct Route {
    pub index: u32,
    pub destination: [u8; 4],
    pub prefix: u8,
    pub gateway: [u8; 4],
    pub metric: u32,
    pub local: bool,
}
pub(crate) struct Neighbor {
    pub index: u32,
    pub ip: [u8; 4],
    pub address: Vec<u8>,
    pub flags: u32,
}

fn ipv4(address: SOCKADDR_INET) -> Result<[u8; 4], i32> {
    if unsafe { address.si_family } != AF_INET {
        return Err(EIO);
    }
    Ok(unsafe { address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes() })
}
// The SDK allocation owns count initialized rows, including SDK-defined padding.
unsafe fn rows<'a, T>(base: *const T, count: u32) -> Result<&'a [T], i32> {
    if count as usize > isize::MAX as usize / size_of::<T>() {
        return Err(EOVERFLOW);
    }
    Ok(unsafe { std::slice::from_raw_parts(base, count as usize) })
}

pub(crate) fn routes() -> Result<Vec<Route>, i32> {
    let mut table = std::ptr::null_mut();
    let status = unsafe { GetIpForwardTable2(AF_INET, &mut table) };
    if status == windows_sys::Win32::Foundation::ERROR_NOT_FOUND {
        return Ok(Vec::new());
    }
    if status != 0 {
        return Err(errno_from_win32(status));
    }
    if table.is_null() {
        return Err(EIO);
    }
    let _allocation = MibAllocation(table.cast());
    let rows = unsafe {
        rows(
            std::ptr::addr_of!((*table).Table).cast::<MIB_IPFORWARD_ROW2>(),
            (*table).NumEntries,
        )?
    };
    rows.iter()
        .map(|row| {
            if row.DestinationPrefix.PrefixLength > 32 {
                return Err(EIO);
            }
            Ok(Route {
                index: row.InterfaceIndex,
                destination: ipv4(row.DestinationPrefix.Prefix)?,
                prefix: row.DestinationPrefix.PrefixLength,
                gateway: ipv4(row.NextHop)?,
                metric: row.Metric,
                local: row.Loopback,
            })
        })
        .collect()
}

pub(crate) fn neighbors(interfaces: &[Interface]) -> Result<Vec<Neighbor>, i32> {
    let mut table = std::ptr::null_mut();
    let status = unsafe { GetIpNetTable2(AF_INET, &mut table) };
    if status == windows_sys::Win32::Foundation::ERROR_NOT_FOUND {
        return Ok(Vec::new());
    }
    if status != 0 {
        return Err(errno_from_win32(status));
    }
    if table.is_null() {
        return Err(EIO);
    }
    let _allocation = MibAllocation(table.cast());
    let rows = unsafe {
        rows(
            std::ptr::addr_of!((*table).Table).cast::<MIB_IPNET_ROW2>(),
            (*table).NumEntries,
        )?
    };
    let mut output = Vec::new();
    for row in rows {
        let Some(interface) = interfaces
            .iter()
            .find(|i| i.host && i.link.index == row.InterfaceIndex)
        else {
            continue;
        };
        if interface.hardware_type == 772 || interface.link.flags & 0x80 != 0 {
            continue; // Loopback/IFF_NOARP never resolve an IPv4 hardware address.
        }
        let ip = ipv4(row.Address)?;
        // Multicast/broadcast mappings do not undergo ARP resolution (NUD_NOARP).
        if ip[0] >= 224 {
            continue;
        }
        #[allow(non_upper_case_globals)]
        let flags = match row.State {
            NlnsPermanent => 0x2 | 0x4, // ATF_COM | ATF_PERM
            NlnsReachable | NlnsStale | NlnsDelay | NlnsProbe => 0x2,
            NlnsIncomplete | NlnsUnreachable => 0,
            _ => return Err(EIO),
        };
        let address = if row.PhysicalAddressLength == 0 {
            vec![0; interface.link.address.len()]
        } else {
            row.PhysicalAddress
                .get(..row.PhysicalAddressLength as usize)
                .ok_or(EIO)?
                .to_vec()
        };
        if !address.is_empty() && address == interface.link.broadcast {
            continue; // A native subnet broadcast mapping is also NUD_NOARP.
        }
        output.push(Neighbor {
            index: row.InterfaceIndex,
            ip,
            address,
            flags,
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_host_tables_have_valid_interface_address_route_and_neighbor_records() {
        let interfaces = super::super::interfaces(1).expect("host interfaces");
        let addresses = super::super::addresses(1, &interfaces).expect("host addresses");
        assert!(!interfaces.is_empty());
        assert!(!addresses.is_empty());
        let routes = super::routes().expect("host routes");
        assert!(!routes.is_empty());
        let neighbors = super::neighbors(&interfaces).expect("host neighbors");
        for neighbor in neighbors {
            let interface = interfaces
                .iter()
                .find(|i| i.link.index == neighbor.index)
                .unwrap();
            assert_ne!(interface.hardware_type, 772);
            assert!(neighbor.address.is_empty() || neighbor.address != interface.link.broadcast);
        }
    }
}
