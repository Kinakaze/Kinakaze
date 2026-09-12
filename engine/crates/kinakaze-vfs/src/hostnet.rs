//! Read-only observations of Windows interfaces in the initial network namespace.
//! No Windows configuration, persistent cache, or sampler thread is owned here.
pub(crate) mod routing;
use crate::route_state::{Address, Attribute, Link};
use crate::{EEXIST, EIO, EOVERFLOW};
use std::collections::HashSet;
use std::ffi::c_void;
use windows_sys::Win32::NetworkManagement::{IpHelper::*, Ndis::*};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Statistics {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}
impl Statistics {
    fn from_row(row: &MIB_IF_ROW2) -> Result<Self, i32> {
        Ok(Self {
            rx_bytes: row.InOctets,
            rx_packets: row
                .InUcastPkts
                .checked_add(row.InNUcastPkts)
                .ok_or(EOVERFLOW)?,
            rx_errors: row.InErrors,
            rx_dropped: row.InDiscards,
            tx_bytes: row.OutOctets,
            tx_packets: row
                .OutUcastPkts
                .checked_add(row.OutNUcastPkts)
                .ok_or(EOVERFLOW)?,
            tx_errors: row.OutErrors,
            tx_dropped: row.OutDiscards,
        })
    }
    pub fn fields(&self) -> [(&'static str, u64); 8] {
        [
            ("rx_bytes", self.rx_bytes),
            ("rx_packets", self.rx_packets),
            ("rx_errors", self.rx_errors),
            ("rx_dropped", self.rx_dropped),
            ("tx_bytes", self.tx_bytes),
            ("tx_packets", self.tx_packets),
            ("tx_errors", self.tx_errors),
            ("tx_dropped", self.tx_dropped),
        ]
    }
    pub fn rtnl_stats64(&self) -> Vec<u8> {
        // Linux 6.1 rtnl_link_stats64. Windows exposes only these eight counters.
        // Unsupported subcategories are zero, not fabricated from another total:
        // in particular InNUcastPkts includes broadcasts, not just multicast.
        let mut values = [0u64; 25];
        values[..8].copy_from_slice(&[
            self.rx_packets,
            self.tx_packets,
            self.rx_bytes,
            self.tx_bytes,
            self.rx_errors,
            self.tx_errors,
            self.rx_dropped,
            self.tx_dropped,
        ]);
        values.iter().flat_map(|n| n.to_ne_bytes()).collect()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Interface {
    pub link: Link,
    pub hardware_type: u16,
    pub operstate: u8,
    pub host: bool,
    pub stats: Option<Statistics>,
}
impl Interface {
    pub fn operstate_name(&self) -> &'static str {
        match self.operstate {
            1 => "notpresent",
            2 => "down",
            3 => "lowerlayerdown",
            4 => "testing",
            5 => "dormant",
            6 => "up",
            _ => "unknown",
        }
    }
}

/// Uses the SDK allocation and SDK field offsets, including padding before Table.
struct MibAllocation(*const c_void);
impl Drop for MibAllocation {
    fn drop(&mut self) {
        unsafe { FreeMibTable(self.0) };
    }
}

fn from_row(row: &MIB_IF_ROW2) -> Result<Interface, i32> {
    if row.InterfaceIndex == 0 || row.InterfaceIndex > i32::MAX as u32 {
        return Err(EOVERFLOW);
    }
    let (name, hardware_type) = match row.Type {
        24 => ("lo".to_owned(), 772),
        6 | 71 => (format!("eth{}", row.InterfaceIndex), 1),
        23 => (format!("ppp{}", row.InterfaceIndex), 512),
        131 => (format!("tun{}", row.InterfaceIndex), 65534),
        _ => (format!("if{}", row.InterfaceIndex), 65534),
    };
    let mut flags = 0;
    if row.AdminStatus == NET_IF_ADMIN_STATUS_UP {
        flags |= 1;
    }
    if row.OperStatus == IfOperStatusUp {
        flags |= 0x40 | 0x10000;
    }
    if row.Type == 24 {
        flags |= 8;
    }
    match row.AccessType {
        NET_IF_ACCESS_BROADCAST => flags |= 2 | 0x1000,
        NET_IF_ACCESS_POINT_TO_POINT => flags |= 0x10,
        _ => (),
    }
    #[allow(non_upper_case_globals)] // SDK enum constants retain their Windows names.
    let operstate = match row.OperStatus {
        IfOperStatusUp => 6,
        IfOperStatusDown => 2,
        IfOperStatusTesting => 4,
        IfOperStatusDormant => 5,
        IfOperStatusNotPresent => 1,
        IfOperStatusLowerLayerDown => 3,
        _ => 0,
    };
    let address = row
        .PhysicalAddress
        .get(..row.PhysicalAddressLength as usize)
        .ok_or(EIO)?
        .to_vec();
    let address = if hardware_type == 772 && address.is_empty() {
        vec![0; 6]
    } else {
        address
    };
    let broadcast = if flags & 2 != 0 {
        vec![0xff; address.len()]
    } else if hardware_type == 772 {
        vec![0; address.len()]
    } else {
        Vec::new()
    };
    let alias_len = row
        .Alias
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(row.Alias.len());
    let mut alias = String::from_utf16_lossy(&row.Alias[..alias_len]).into_bytes();
    alias.push(0);
    Ok(Interface {
        link: Link {
            index: row.InterfaceIndex,
            name,
            kind: if hardware_type == 772 {
                "loopback".into()
            } else {
                String::new()
            },
            flags,
            mtu: row.Mtu,
            address,
            broadcast,
            master: 0,
            peer: 0,
            attributes: vec![Attribute {
                kind: 20,
                value: alias,
            }],
        },
        hardware_type,
        operstate,
        host: true,
        stats: Some(Statistics::from_row(row)?),
    })
}

/// Indices/names follow the live Windows index, which can change on re-enabling
/// an adapter. Filter stack components are not separate IP-facing interfaces.
pub(crate) fn interfaces(netns: u64) -> Result<Vec<Interface>, i32> {
    if netns != 1 {
        return Ok(Vec::new());
    }
    let mut table = std::ptr::null_mut();
    let result = unsafe { GetIfTable2(&mut table) };
    if result != 0 {
        return Err(crate::errno_from_win32(result));
    }
    if table.is_null() {
        return Err(EIO);
    }
    let _allocation = MibAllocation(table.cast());
    // GetIfTable2 guarantees NumEntries contiguous MIB_IF_ROW2 elements.
    let count = unsafe { (*table).NumEntries as usize };
    if count > isize::MAX as usize / size_of::<MIB_IF_ROW2>() {
        return Err(EOVERFLOW);
    }
    let rows = unsafe {
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>(),
            count,
        )
    };
    let mut out = rows
        .iter()
        .filter(|r| r.InterfaceAndOperStatusFlags._bitfield & 2 == 0)
        .map(from_row)
        .collect::<Result<Vec<_>, _>>()?;
    validate_unique(&out)?;
    out.sort_by_key(|i| (i.hardware_type != 772, i.link.index));
    Ok(out)
}

pub(crate) fn validate_unique(interfaces: &[Interface]) -> Result<(), i32> {
    let mut indices = HashSet::new();
    let mut names = HashSet::new();
    for interface in interfaces {
        if !indices.insert(interface.link.index) || !names.insert(&interface.link.name) {
            return Err(EEXIST);
        }
    }
    Ok(())
}

/// An observation only: never store Windows addresses in our mutable route state.
pub(crate) fn addresses(netns: u64, links: &[Interface]) -> Result<Vec<Address>, i32> {
    if netns != 1 {
        return Ok(Vec::new());
    }
    let mut table = std::ptr::null_mut();
    let result = unsafe { GetUnicastIpAddressTable(AF_UNSPEC, &mut table) };
    if result == windows_sys::Win32::Foundation::ERROR_NOT_FOUND {
        return Ok(Vec::new());
    }
    if result != 0 {
        return Err(crate::errno_from_win32(result));
    }
    if table.is_null() {
        return Err(EIO);
    }
    let _allocation = MibAllocation(table.cast());
    let count = unsafe { (*table).NumEntries as usize };
    if count > isize::MAX as usize / size_of::<MIB_UNICASTIPADDRESS_ROW>() {
        return Err(EOVERFLOW);
    }
    let rows = unsafe {
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).Table).cast::<MIB_UNICASTIPADDRESS_ROW>(),
            count,
        )
    };
    rows.iter()
        .filter_map(|row| {
            let link = links
                .iter()
                .find(|i| i.host && i.link.index == row.InterfaceIndex)?;
            Some(address_from_row(row, link))
        })
        .collect()
}

fn address_from_row(row: &MIB_UNICASTIPADDRESS_ROW, interface: &Interface) -> Result<Address, i32> {
    let (family, value, link_local) = match unsafe { row.Address.si_family } {
        AF_INET => {
            let bytes = unsafe { row.Address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes() };
            (2, bytes.to_vec(), bytes[..2] == [169, 254])
        }
        AF_INET6 => {
            let bytes = unsafe { row.Address.Ipv6.sin6_addr.u.Byte };
            (
                10,
                bytes.to_vec(),
                bytes[0] == 0xfe && bytes[1] & 0xc0 == 0x80,
            )
        }
        _ => return Err(EIO),
    };
    if row.OnLinkPrefixLength > if family == 2 { 32 } else { 128 } {
        return Err(EIO);
    }
    let mut flags = if row.ValidLifetime == u32::MAX {
        0x80
    } else {
        0
    };
    flags |= match row.DadState {
        1 => 0x40,
        2 => 8,
        3 => 0x20,
        _ => 0,
    };
    let mut label = interface.link.name.as_bytes().to_vec();
    label.push(0);
    let mut attributes = vec![
        Attribute {
            kind: 1,
            value: value.clone(),
        },
        Attribute {
            kind: 2,
            value: value.clone(),
        },
        Attribute {
            kind: 3,
            value: label,
        },
    ];
    if family == 2 && interface.link.flags & 2 != 0 && row.OnLinkPrefixLength < 31 {
        let address = u32::from_be_bytes(value.try_into().map_err(|_| EIO)?);
        let mask = u32::MAX
            .checked_shl(32 - u32::from(row.OnLinkPrefixLength))
            .unwrap_or(0);
        attributes.push(Attribute {
            kind: 4,
            value: (address | !mask).to_be_bytes().to_vec(),
        });
    }
    Ok(Address {
        index: row.InterfaceIndex,
        family,
        prefix_len: row.OnLinkPrefixLength,
        flags,
        scope: if interface.hardware_type == 772 {
            254
        } else if link_local {
            253
        } else {
            0
        },
        attributes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mapping_preserves_type_status_and_64_bit_counters() {
        let mut row = MIB_IF_ROW2::default();
        row.InterfaceIndex = 42;
        row.Type = 71;
        row.AccessType = NET_IF_ACCESS_BROADCAST;
        row.AdminStatus = NET_IF_ADMIN_STATUS_UP;
        row.OperStatus = IfOperStatusDown;
        row.InOctets = 1 << 40;
        row.InUcastPkts = 17;
        row.InNUcastPkts = 3;
        row.OutOctets = 1 << 41;
        row.OutUcastPkts = 11;
        row.OutNUcastPkts = 2;
        let interface = from_row(&row).unwrap();
        assert_eq!(interface.link.name, "eth42");
        assert_eq!(interface.hardware_type, 1);
        assert_eq!(interface.link.flags, 1 | 2 | 0x1000);
        assert_eq!(interface.operstate, 2);
        let stats = interface.stats.unwrap();
        assert_eq!(stats.rx_bytes, 1 << 40);
        assert_eq!((stats.rx_packets, stats.tx_packets), (20, 13));
        let wire = stats.rtnl_stats64();
        assert_eq!(wire.len(), 200);
        assert_eq!(
            u64::from_ne_bytes(wire[16..24].try_into().unwrap()),
            1 << 40
        );
        assert!(wire[64..].iter().all(|b| *b == 0));
        row.Type = 23;
        assert_eq!(from_row(&row).unwrap().hardware_type, 512);
        row.Type = 131;
        assert_eq!(from_row(&row).unwrap().hardware_type, 65534);
    }
    #[test]
    fn overflow_and_invalid_physical_address_do_not_become_truncated_metrics() {
        let mut row = MIB_IF_ROW2::default();
        row.InterfaceIndex = 1;
        row.InUcastPkts = u64::MAX;
        row.InNUcastPkts = 1;
        assert_eq!(from_row(&row).unwrap_err(), EOVERFLOW);
        row.InUcastPkts = 0;
        row.PhysicalAddressLength = 33;
        assert_eq!(from_row(&row).unwrap_err(), EIO);
    }
    #[test]
    fn private_namespace_never_observes_host_counters_or_addresses() {
        assert!(interfaces(12345).unwrap().is_empty());
        assert!(addresses(12345, &[]).unwrap().is_empty());
    }
    #[test]
    fn collisions_are_rejected_instead_of_hiding_an_interface() {
        let mut row = MIB_IF_ROW2::default();
        row.InterfaceIndex = 5;
        let a = from_row(&row).unwrap();
        let mut b = a.clone();
        b.link.name = "another".into();
        assert_eq!(validate_unique(&[a.clone(), b]), Err(EEXIST));
        let mut b = a.clone();
        b.link.index = 6;
        assert_eq!(validate_unique(&[a, b]), Err(EEXIST));
    }
    #[test]
    fn address_translation_preserves_octets_and_linux_family() {
        let mut link = MIB_IF_ROW2::default();
        link.InterfaceIndex = 7;
        link.Type = 6;
        link.AccessType = NET_IF_ACCESS_BROADCAST;
        let link = from_row(&link).unwrap();
        let mut row = MIB_UNICASTIPADDRESS_ROW::default();
        row.InterfaceIndex = 7;
        row.Address.Ipv4.sin_family = AF_INET;
        row.Address.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes([192, 0, 2, 9]);
        row.OnLinkPrefixLength = 24;
        row.ValidLifetime = u32::MAX;
        let address = address_from_row(&row, &link).unwrap();
        assert_eq!(address.family, 2);
        assert_eq!(address.attributes[0].value, [192, 0, 2, 9]);
        assert_eq!(address.attributes[3].value, [192, 0, 2, 255]);
        row.Address.Ipv6.sin6_family = AF_INET6;
        row.Address.Ipv6.sin6_addr.u.Byte = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        row.OnLinkPrefixLength = 64;
        let address = address_from_row(&row, &link).unwrap();
        assert_eq!(address.family, 10);
        assert_eq!(address.scope, 253);
    }
}
