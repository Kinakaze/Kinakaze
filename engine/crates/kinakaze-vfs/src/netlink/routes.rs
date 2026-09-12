//! FIB queries include kernel routes derived from assigned interface addresses.
//! Derivation uses the same namespace snapshot as explicit routes, so removing
//! or moving an address cannot leave an unrelated process with a stale prefix.
use super::*;
use crate::route_state::{Attribute, Network, Route};

const RTA_PREFSRC: u16 = 7;
const RTA_PRIORITY: u16 = 6;

fn address_size(family: u8) -> Result<usize, i32> {
    match family {
        AF_INET => Ok(4),
        AF_INET6 => Ok(16),
        _ => Err(EINVAL),
    }
}

fn network_prefix(address: &[u8], bits: u8) -> Result<Vec<u8>, i32> {
    if bits as usize > address.len() * 8 {
        return Err(EINVAL);
    }
    Ok(address
        .iter()
        .enumerate()
        .map(|(i, byte)| {
            let kept = (bits as usize).saturating_sub(i * 8).min(8);
            *byte & if kept == 0 { 0 } else { 0xffu8 << (8 - kept) }
        })
        .collect())
}

pub(crate) fn all(state: &Network) -> Result<Vec<Route>, i32> {
    let mut routes = state.routes.clone();
    for address in &state.addresses {
        let size = address_size(address.family)?;
        let Some(local) = route_attribute(&address.attributes, IFA_LOCAL)
            .or_else(|| route_attribute(&address.attributes, IFA_ADDRESS))
        else {
            continue;
        };
        if local.len() != size || address.prefix_len as usize > size * 8 {
            return Err(EINVAL);
        }
        if address.index != 1 && !state.links.iter().any(|l| l.index == address.index) {
            continue;
        }
        if address.family == AF_INET6 && address.flags & (0x40 | 0x08) != 0 {
            continue;
        }
        let mut derived = |destination: Vec<u8>, prefix: u8, table: u8, scope: u8, kind: u8| {
            let route = Route {
                family: address.family,
                dst_len: prefix,
                src_len: 0,
                tos: 0,
                table,
                protocol: 2,
                scope,
                kind,
                flags: 0,
                attributes: vec![
                    Attribute {
                        kind: RTA_DST,
                        value: destination,
                    },
                    Attribute {
                        kind: RTA_OIF,
                        value: address.index.to_ne_bytes().to_vec(),
                    },
                    Attribute {
                        kind: RTA_PREFSRC,
                        value: local.to_vec(),
                    },
                ],
            };
            if !routes.iter().any(|r| {
                r.family == route.family
                    && r.table == table
                    && r.dst_len == prefix
                    && route_attribute(&r.attributes, RTA_DST)
                        == route_attribute(&route.attributes, RTA_DST)
                    && route_attribute(&r.attributes, RTA_OIF)
                        == route_attribute(&route.attributes, RTA_OIF)
            }) {
                routes.push(route);
            }
        };
        derived(local.to_vec(), (size * 8) as u8, 255, 254, 2);
        // IFA_F_NOPREFIXROUTE suppresses only the connected prefix route.
        if address.flags & 0x200 == 0 {
            let peer = route_attribute(&address.attributes, IFA_ADDRESS).unwrap_or(local);
            if peer.len() != size {
                return Err(EINVAL);
            }
            derived(
                network_prefix(peer, address.prefix_len)?,
                address.prefix_len,
                254,
                253,
                1,
            );
        }
    }
    Ok(routes)
}

pub(super) fn lookup(state: &Network, query: &Route) -> Result<Route, i32> {
    let size = address_size(query.family)?;
    let destination = route_attribute(&query.attributes, RTA_DST).ok_or(EINVAL)?;
    if destination.len() != size || query.dst_len as usize > size * 8 {
        return Err(EINVAL);
    }
    let oif = route_u32(&query.attributes, RTA_OIF)?;
    let candidates = all(state)?;
    let mut selected: Option<(&Route, (u8, u8, u32))> = None;
    for route in &candidates {
        if route.family != query.family
            || (query.table != 0 && route.table != query.table)
            || (query.table == 0 && !matches!(route.table, 254 | 255))
        {
            continue;
        }
        let interface = route_u32(&route.attributes, RTA_OIF)?;
        if oif.is_some() && oif != interface {
            continue;
        }
        if interface.is_some_and(|index| {
            index != 1
                && state
                    .links
                    .iter()
                    .all(|l| l.index != index || l.flags & IFF_UP == 0)
        }) {
            continue;
        }
        if route.dst_len != 0 {
            let Some(prefix) = route_attribute(&route.attributes, RTA_DST) else {
                continue;
            };
            if prefix.len() != size
                || network_prefix(destination, route.dst_len)?
                    != network_prefix(prefix, route.dst_len)?
            {
                continue;
            }
        }
        let priority = route_u32(&route.attributes, RTA_PRIORITY)?.unwrap_or(0);
        let rank = (
            u8::from(route.table == 255),
            route.dst_len,
            u32::MAX - priority,
        );
        if selected.is_none_or(|(_, previous)| rank > previous) {
            selected = Some((route, rank));
        }
    }
    let mut route = selected.ok_or(crate::ENETUNREACH)?.0.clone();
    if !matches!(route.kind, 1 | 2) {
        return Err(crate::ENETUNREACH);
    }
    route.dst_len = (size * 8) as u8;
    route.attributes.retain(|a| a.kind & 0x3fff != RTA_DST);
    route.attributes.push(Attribute {
        kind: RTA_DST,
        value: destination.to_vec(),
    });
    Ok(route)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(family: u8, ip: &[u8], prefix_len: u8) -> Network {
        let mut state = Network::default();
        state.links.push(crate::route_state::Link {
            index: 77,
            name: "eth0".into(),
            kind: "veth".into(),
            flags: 1,
            mtu: 1500,
            address: vec![],
            broadcast: vec![],
            master: 0,
            peer: 78,
            attributes: vec![],
        });
        state.addresses.push(crate::route_state::Address {
            index: 77,
            family,
            prefix_len,
            flags: 0,
            scope: 0,
            attributes: vec![Attribute {
                kind: IFA_LOCAL,
                value: ip.to_vec(),
            }],
        });
        state
    }
    fn query(family: u8, ip: &[u8]) -> Route {
        Route {
            family,
            dst_len: (ip.len() * 8) as u8,
            src_len: 0,
            tos: 0,
            table: 0,
            protocol: 0,
            scope: 0,
            kind: 0,
            flags: 0,
            attributes: vec![Attribute {
                kind: RTA_DST,
                value: ip.to_vec(),
            }],
        }
    }
    #[test]
    fn gateway_lookup_uses_connected_prefix_and_tracks_address_removal() {
        let mut state = state(AF_INET, &[172, 17, 0, 2], 16);
        let gateway = query(AF_INET, &[172, 17, 0, 1]);
        let route = lookup(&state, &gateway).unwrap();
        assert_eq!((route.table, route.dst_len, route.kind), (254, 32, 1));
        assert_eq!(route_u32(&route.attributes, RTA_OIF).unwrap(), Some(77));
        assert_eq!(
            route_attribute(&route.attributes, RTA_PREFSRC),
            Some(&[172, 17, 0, 2][..])
        );
        assert_eq!(
            lookup(&state, &query(AF_INET, &[172, 18, 0, 1])),
            Err(crate::ENETUNREACH)
        );
        assert_eq!(
            lookup(&state, &query(AF_INET, &[172, 17, 0, 2]))
                .unwrap()
                .kind,
            2
        );
        state.addresses.clear();
        assert_eq!(lookup(&state, &gateway), Err(crate::ENETUNREACH));
    }
    #[test]
    fn ipv6_prefix_and_no_prefix_route_flag_are_respected() {
        let mut local = [0u8; 16];
        local[..4].copy_from_slice(&[0x20, 1, 0xd, 0xb8]);
        local[15] = 2;
        let mut peer = local;
        peer[15] = 1;
        let mut state = state(AF_INET6, &local, 64);
        assert_eq!(
            lookup(&state, &query(AF_INET6, &peer)).unwrap().dst_len,
            128
        );
        state.addresses[0].flags = 0x200;
        assert_eq!(
            lookup(&state, &query(AF_INET6, &peer)),
            Err(crate::ENETUNREACH)
        );
        assert_eq!(lookup(&state, &query(AF_INET6, &local)).unwrap().kind, 2);
    }
    #[test]
    fn more_specific_route_wins_and_down_link_is_not_selected() {
        let mut state = state(AF_INET, &[172, 17, 0, 2], 16);
        let mut specific = all(&state)
            .unwrap()
            .into_iter()
            .find(|r| r.table == 254)
            .unwrap();
        specific.dst_len = 24;
        specific.protocol = 4;
        specific.attributes.retain(|a| a.kind != RTA_DST);
        specific.attributes.push(Attribute {
            kind: RTA_DST,
            value: vec![172, 17, 3, 0],
        });
        state.routes.push(specific);
        assert_eq!(
            lookup(&state, &query(AF_INET, &[172, 17, 3, 1]))
                .unwrap()
                .protocol,
            4
        );
        state.links[0].flags = 0;
        assert_eq!(
            lookup(&state, &query(AF_INET, &[172, 17, 3, 1])),
            Err(crate::ENETUNREACH)
        );
    }
}
