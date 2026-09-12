//! Network text views derived at read time, without fixed interfaces or routes.
use crate::hostnet::{Interface, routing};
use crate::route_state::Route;
use crate::{EIO, EOPNOTSUPP};
use std::fmt::Write;

const ROUTE_HEADER: &str =
    "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";
const ARP_HEADER: &str =
    "IP address       HW type     Flags       HW address            Mask     Device\n";

fn attribute(route: &Route, kind: u16) -> Option<&[u8]> {
    route
        .attributes
        .iter()
        .find(|a| a.kind & 0x3fff == kind)
        .map(|a| a.value.as_slice())
}
fn word(bytes: &[u8]) -> Result<u32, i32> {
    Ok(u32::from_ne_bytes(bytes.try_into().map_err(|_| EIO)?))
}
fn value(route: &Route, kind: u16) -> Result<Option<u32>, i32> {
    attribute(route, kind).map(word).transpose()
}
fn metrics(bytes: &[u8]) -> Result<[u32; 3], i32> {
    let mut remaining = bytes;
    let (mut mss, mut window, mut rtt) = (0u32, 0, 0);
    while !remaining.is_empty() {
        if remaining.len() < 4 {
            return Err(EIO);
        }
        let length = u16::from_ne_bytes(remaining[..2].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(remaining[2..4].try_into().unwrap()) & 0x3fff;
        let padded = (length + 3) & !3;
        if length < 4 || padded > remaining.len() {
            return Err(EIO);
        }
        match kind {
            3 => window = word(&remaining[4..length])?, // RTAX_WINDOW
            4 => rtt = word(&remaining[4..length])?,    // RTAX_RTT, in eighths
            8 => mss = word(&remaining[4..length])?,    // RTAX_ADVMSS
            _ => (),
        }
        remaining = &remaining[padded..];
    }
    let mtu = if mss == 0 {
        0
    } else {
        mss.checked_add(40).ok_or(EIO)?
    };
    Ok([mtu, window, rtt >> 3])
}
struct Row<'a> {
    name: &'a str,
    destination: u32,
    gateway: u32,
    prefix: u8,
    metric: u32,
    reject: bool,
    metrics: [u32; 3],
}
fn render(output: &mut String, row: Row<'_>) -> Result<(), i32> {
    if row.prefix > 32 {
        return Err(EIO);
    }
    let mask = u32::MAX
        .checked_shl(32 - u32::from(row.prefix))
        .unwrap_or(0);
    let mask = u32::from_ne_bytes(mask.to_be_bytes());
    let flags = 1
        | if row.gateway != 0 { 2 } else { 0 }
        | if row.prefix == 32 { 4 } else { 0 }
        | if row.reject { 0x200 } else { 0 };
    let [mtu, window, rtt] = row.metrics;
    // Linux fib_route_seq_show also emits zero for the obsolete RefCnt/Use fields.
    writeln!(
        output,
        "{}\t{:08X}\t{:08X}\t{flags:04X}\t0\t0\t{}\t{mask:08X}\t{mtu}\t{window}\t{rtt}",
        row.name,
        row.destination & mask,
        row.gateway,
        row.metric
    )
    .map_err(|_| EIO)
}
fn guest_route(output: &mut String, route: &Route, interfaces: &[Interface]) -> Result<(), i32> {
    let table = value(route, 15)?.unwrap_or(u32::from(route.table));
    if route.family != 2 || table != 254 || matches!(route.kind, 3 | 5) {
        return Ok(());
    }
    if attribute(route, 9).is_some() || attribute(route, 30).is_some() {
        return Err(EOPNOTSUPP); // RTA_MULTIPATH / RTA_NH_ID needs explicit nexthop rendering.
    }
    let index = value(route, 4)?;
    let name = match index {
        Some(index) => interfaces
            .iter()
            .find(|i| i.link.index == index)
            .ok_or(crate::ENODEV)?
            .link
            .name
            .as_str(),
        None => "*", // Reject/blackhole routes have no output device.
    };
    let destination = match value(route, 1)? {
        Some(destination) => destination,
        None if route.dst_len == 0 => 0,
        None => return Err(EIO),
    };
    render(
        output,
        Row {
            name,
            destination,
            gateway: value(route, 5)?.unwrap_or(0),
            prefix: route.dst_len,
            metric: value(route, 6)?.unwrap_or(0),
            reject: matches!(route.kind, 7 | 8),
            metrics: metrics(attribute(route, 8).unwrap_or(&[]))?,
        },
    )
}

pub(super) fn routes(ns: u64) -> Result<String, i32> {
    let interfaces = crate::netlink::network_interfaces(ns)?;
    let state = crate::route_state::snapshot_for(ns)?;
    let mut output = String::from(ROUTE_HEADER);
    for route in crate::netlink::routes::all(&state)? {
        guest_route(&mut output, &route, &interfaces)?;
    }
    if ns == 1 {
        for route in routing::routes()? {
            // Local/multicast/broadcast host rows do not belong to Linux's main unicast FIB view.
            if route.local || route.destination[0] >= 224 {
                continue;
            }
            let Some(interface) = interfaces
                .iter()
                .find(|i| i.host && i.link.index == route.index)
            else {
                continue;
            };
            render(
                &mut output,
                Row {
                    name: &interface.link.name,
                    destination: u32::from_ne_bytes(route.destination),
                    gateway: u32::from_ne_bytes(route.gateway),
                    prefix: route.prefix,
                    metric: route.metric,
                    reject: false,
                    metrics: [0; 3], // No per-route MSS/window/RTT override in this backend.
                },
            )?;
        }
    }
    Ok(output)
}
pub(super) fn arp(ns: u64) -> Result<String, i32> {
    let mut output = String::from(ARP_HEADER);
    if ns != 1 {
        // Virtual links currently use IP transports; there is no emulated L2 neighbor cache.
        crate::namespaces::pin_network(ns)?;
        return Ok(output);
    }
    let interfaces = crate::netlink::network_interfaces(ns)?;
    for neighbor in routing::neighbors(&interfaces)? {
        let interface = interfaces
            .iter()
            .find(|i| i.host && i.link.index == neighbor.index)
            .ok_or(EIO)?;
        let ip = std::net::Ipv4Addr::from(neighbor.ip);
        let mut mac = String::new();
        for (i, byte) in neighbor.address.iter().enumerate() {
            if i != 0 {
                mac.push(':');
            }
            write!(mac, "{byte:02x}").map_err(|_| EIO)?;
        }
        writeln!(
            output,
            "{ip:<16} 0x{:<10x}0x{:<10x}{mac:<17}     *        {}",
            interface.hardware_type, neighbor.flags, interface.link.name
        )
        .map_err(|_| EIO)?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn route_fields_use_linux_endianness_flags_and_scaled_metrics() {
        let mut output = String::new();
        let nested = [
            8, 0, 8, 0, 0xb4, 5, 0, 0, 8, 0, 3, 0, 0, 16, 0, 0, 8, 0, 4, 0, 80, 0, 0, 0,
        ];
        render(
            &mut output,
            Row {
                name: "veth9",
                destination: u32::from_ne_bytes([10, 7, 6, 88]),
                gateway: u32::from_ne_bytes([10, 7, 6, 1]),
                prefix: 24,
                metric: 23,
                reject: false,
                metrics: metrics(&nested).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(
            output,
            "veth9\t0006070A\t0106070A\t0003\t0\t0\t23\t00FFFFFF\t1500\t4096\t10\n"
        );
        for broken in [
            &nested[..1],
            &nested[..7],
            &[3, 0, 8, 0][..],
            &[5, 0, 8, 0, 1, 0, 0, 0][..],
        ] {
            assert_eq!(metrics(broken), Err(EIO));
        }
    }
    #[test]
    fn local_table_routes_are_not_reported_as_main_routes() {
        let route = Route {
            family: 2,
            dst_len: 32,
            src_len: 0,
            tos: 0,
            table: 255,
            protocol: 2,
            scope: 254,
            kind: 2,
            flags: 0,
            attributes: Vec::new(),
        };
        let mut output = String::new();
        guest_route(&mut output, &route, &[]).unwrap();
        assert!(output.is_empty());
    }
}
