//! Routing state owned by network namespace objects.
//!
//! Root and nested namespaces use the same shared publication and lifetime.
//! Namespace references retain the state across fork/exec; a root directory is
//! never a kernel identity or a persistence location for links and routes.
use crate::{EINVAL, EIO, ENOSPC};
const EFBIG: i32 = 27;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attribute {
    pub kind: u16,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Link {
    pub index: u32,
    pub name: String,
    pub kind: String,
    pub flags: u32,
    pub mtu: u32,
    pub address: Vec<u8>,
    pub broadcast: Vec<u8>,
    pub master: u32,
    pub peer: u32,
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Address {
    pub index: u32,
    pub family: u8,
    pub prefix_len: u8,
    pub flags: u32,
    pub scope: u8,
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Route {
    pub family: u8,
    pub dst_len: u8,
    pub src_len: u8,
    pub tos: u8,
    pub table: u8,
    pub protocol: u8,
    pub scope: u8,
    pub kind: u8,
    pub flags: u32,
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Sysctl {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Network {
    pub notifications: crate::netlink::multicast::Journal,
    pub generation: u32,
    pub loopback_flags: u32,
    pub netfilter: Vec<u8>,
    pub next_index: u32,
    pub links: Vec<Link>,
    pub addresses: Vec<Address>,
    pub routes: Vec<Route>,
    /// Network-namespace sysctls whose values differ from their defaults.
    /// Keeping these beside links and routes gives every process the same
    /// kernel-visible state and the same transaction boundary.
    pub sysctls: Vec<Sysctl>,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            notifications: Default::default(),
            generation: 0,
            loopback_flags: 8,
            netfilter: Vec::new(),
            // Low indexes belong to host-backed links. Virtual links begin in
            // a separate range while still following Linux's monotonically
            // increasing, nonzero ifindex contract.
            next_index: 0x10000,
            links: Vec::new(),
            addresses: Vec::new(),
            routes: Vec::new(),
            sysctls: Vec::new(),
        }
    }
}

impl Network {
    /// Change the namespace loopback and materialize its standard IP addresses.
    pub fn set_loopback_flags(&mut self, flags: u32, change: u32) {
        const IFF_LOOPBACK: u32 = 8;
        const IFF_UP: u32 = 1;
        const IFF_RUNNING: u32 = 0x40;
        self.loopback_flags = ((self.loopback_flags & !change) | (flags & change)) | IFF_LOOPBACK;
        if self.loopback_flags & IFF_UP != 0 {
            self.loopback_flags |= IFF_RUNNING;
            for (family, value, prefix) in [
                (2, vec![127, 0, 0, 1], 8),
                (
                    10,
                    {
                        let mut v = vec![0; 16];
                        v[15] = 1;
                        v
                    },
                    128,
                ),
            ] {
                if !self
                    .addresses
                    .iter()
                    .any(|a| a.index == 1 && a.family == family)
                {
                    self.addresses.push(crate::route_state::Address {
                        index: 1,
                        family,
                        prefix_len: prefix,
                        flags: 0x80,
                        scope: 254,
                        attributes: vec![
                            crate::route_state::Attribute {
                                kind: 1,
                                value: value.clone(),
                            },
                            crate::route_state::Attribute { kind: 2, value },
                        ],
                    });
                }
            }
        } else {
            self.loopback_flags &= !IFF_RUNNING;
        }
    }

    pub fn allocate_index(&mut self) -> Result<u32, i32> {
        let start = self.next_index.max(1);
        let mut candidate = start;
        loop {
            if self.links.iter().all(|link| link.index != candidate) {
                self.next_index = candidate.checked_add(1).unwrap_or(1).max(1);
                return Ok(candidate);
            }
            candidate = candidate.checked_add(1).unwrap_or(1).max(1);
            if candidate == start {
                return Err(ENOSPC);
            }
        }
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_blob(out: &mut Vec<u8>, value: &[u8]) -> Result<(), i32> {
    put_u32(out, u32::try_from(value.len()).map_err(|_| EFBIG)?);
    out.extend_from_slice(value);
    Ok(())
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), i32> {
    put_blob(out, value.as_bytes())
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], i32> {
    let end = cursor.checked_add(N).ok_or(EINVAL)?;
    let value = bytes.get(*cursor..end).ok_or(EINVAL)?;
    *cursor = end;
    value.try_into().map_err(|_| EINVAL)
}

fn take_blob(bytes: &[u8], cursor: &mut usize) -> Result<Vec<u8>, i32> {
    let length = u32::from_le_bytes(take(bytes, cursor)?) as usize;
    let end = cursor.checked_add(length).ok_or(EINVAL)?;
    let value = bytes.get(*cursor..end).ok_or(EINVAL)?.to_vec();
    *cursor = end;
    Ok(value)
}

fn take_string(bytes: &[u8], cursor: &mut usize) -> Result<String, i32> {
    String::from_utf8(take_blob(bytes, cursor)?).map_err(|_| EINVAL)
}

fn put_attributes(out: &mut Vec<u8>, attributes: &[Attribute]) -> Result<(), i32> {
    put_u32(out, u32::try_from(attributes.len()).map_err(|_| EFBIG)?);
    for attribute in attributes {
        put_u16(out, attribute.kind);
        put_u16(out, 0);
        put_blob(out, &attribute.value)?;
    }
    Ok(())
}

fn take_attributes(bytes: &[u8], cursor: &mut usize) -> Result<Vec<Attribute>, i32> {
    let count = u32::from_le_bytes(take(bytes, cursor)?) as usize;
    let mut attributes = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = u16::from_le_bytes(take(bytes, cursor)?);
        let _reserved = take::<2>(bytes, cursor)?;
        attributes.push(Attribute {
            kind,
            value: take_blob(bytes, cursor)?,
        });
    }
    Ok(attributes)
}

fn serialize(state: &Network) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    put_u32(
        &mut out,
        u32::try_from(state.links.len()).map_err(|_| EFBIG)?,
    );
    for link in &state.links {
        put_u32(&mut out, link.index);
        put_u32(&mut out, link.flags);
        put_u32(&mut out, link.mtu);
        put_u32(&mut out, link.master);
        put_u32(&mut out, link.peer);
        put_string(&mut out, &link.name)?;
        put_string(&mut out, &link.kind)?;
        put_blob(&mut out, &link.address)?;
        put_blob(&mut out, &link.broadcast)?;
        put_attributes(&mut out, &link.attributes)?;
    }
    put_u32(
        &mut out,
        u32::try_from(state.addresses.len()).map_err(|_| EFBIG)?,
    );
    for address in &state.addresses {
        put_u32(&mut out, address.index);
        out.extend_from_slice(&[address.family, address.prefix_len, address.scope, 0]);
        put_u32(&mut out, address.flags);
        put_attributes(&mut out, &address.attributes)?;
    }
    put_u32(
        &mut out,
        u32::try_from(state.routes.len()).map_err(|_| EFBIG)?,
    );
    for route in &state.routes {
        out.extend_from_slice(&[
            route.family,
            route.dst_len,
            route.src_len,
            route.tos,
            route.table,
            route.protocol,
            route.scope,
            route.kind,
        ]);
        put_u32(&mut out, route.flags);
        put_attributes(&mut out, &route.attributes)?;
    }
    put_u32(
        &mut out,
        u32::try_from(state.sysctls.len()).map_err(|_| EFBIG)?,
    );
    for sysctl in &state.sysctls {
        put_string(&mut out, &sysctl.name)?;
        put_blob(&mut out, &sysctl.value)?;
    }
    put_u32(&mut out, state.loopback_flags);
    put_blob(&mut out, &state.netfilter)?;
    state.notifications.encode(&mut out)?;
    Ok(out)
}

fn deserialize(generation: u32, next_index: u32, bytes: &[u8]) -> Result<Network, i32> {
    let mut cursor = 0usize;
    let link_count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
    let mut links = Vec::with_capacity(link_count);
    for _ in 0..link_count {
        links.push(Link {
            index: u32::from_le_bytes(take(bytes, &mut cursor)?),
            flags: u32::from_le_bytes(take(bytes, &mut cursor)?),
            mtu: u32::from_le_bytes(take(bytes, &mut cursor)?),
            master: u32::from_le_bytes(take(bytes, &mut cursor)?),
            peer: u32::from_le_bytes(take(bytes, &mut cursor)?),
            name: take_string(bytes, &mut cursor)?,
            kind: take_string(bytes, &mut cursor)?,
            address: take_blob(bytes, &mut cursor)?,
            broadcast: take_blob(bytes, &mut cursor)?,
            attributes: take_attributes(bytes, &mut cursor)?,
        });
    }
    let address_count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
    let mut addresses = Vec::with_capacity(address_count);
    for _ in 0..address_count {
        let index = u32::from_le_bytes(take(bytes, &mut cursor)?);
        let fixed = take::<4>(bytes, &mut cursor)?;
        let flags = u32::from_le_bytes(take(bytes, &mut cursor)?);
        addresses.push(Address {
            index,
            family: fixed[0],
            prefix_len: fixed[1],
            scope: fixed[2],
            flags,
            attributes: take_attributes(bytes, &mut cursor)?,
        });
    }
    let route_count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
    let mut routes = Vec::with_capacity(route_count);
    for _ in 0..route_count {
        let fixed = take::<8>(bytes, &mut cursor)?;
        routes.push(Route {
            family: fixed[0],
            dst_len: fixed[1],
            src_len: fixed[2],
            tos: fixed[3],
            table: fixed[4],
            protocol: fixed[5],
            scope: fixed[6],
            kind: fixed[7],
            flags: u32::from_le_bytes(take(bytes, &mut cursor)?),
            attributes: take_attributes(bytes, &mut cursor)?,
        });
    }
    // The sysctl table is an append-only extension of the v1 payload.  Slots
    // written before the extension end after the route table and represent an
    // empty override set; new slots always include the explicit count.
    let mut sysctls = Vec::new();
    if cursor < bytes.len() {
        let sysctl_count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
        sysctls.reserve(sysctl_count);
        for _ in 0..sysctl_count {
            let name = take_string(bytes, &mut cursor)?;
            if name.is_empty() || sysctls.iter().any(|entry: &Sysctl| entry.name == name) {
                return Err(EINVAL);
            }
            sysctls.push(Sysctl {
                name,
                value: take_blob(bytes, &mut cursor)?,
            });
        }
    }
    let loopback_flags = if cursor < bytes.len() {
        u32::from_le_bytes(take(bytes, &mut cursor)?)
    } else {
        8
    };
    let netfilter = if cursor < bytes.len() {
        take_blob(bytes, &mut cursor)?
    } else {
        Vec::new()
    };
    let notifications = crate::netlink::multicast::Journal::decode(&bytes[cursor..])?;
    cursor = bytes.len();
    if cursor != bytes.len() {
        return Err(EINVAL);
    }
    Ok(Network {
        notifications,
        netfilter,
        loopback_flags,
        generation,
        next_index: next_index.max(1),
        links,
        addresses,
        routes,
        sysctls,
    })
}

pub(crate) fn snapshot() -> Result<Network, i32> {
    snapshot_for(crate::usernet::current()?)
}
pub(crate) fn snapshot_for(id: u64) -> Result<Network, i32> {
    decode_namespace(&crate::namespaces::network_data(id)?)
}

pub(crate) fn transaction<T, E>(
    body: impl FnOnce(&mut Network) -> Result<T, E>,
) -> Result<Result<T, E>, i32> {
    transaction_for(crate::usernet::current()?, body)
}
pub(crate) fn transaction_for<T, E>(
    id: u64,
    body: impl FnOnce(&mut Network) -> Result<T, E>,
) -> Result<Result<T, E>, i32> {
    transaction_with_notification(id, body, None)
}

pub(crate) fn broadcast_link(id: u64, source: u32, bytes: Vec<u8>) -> Result<(), i32> {
    transaction_with_notification(id, |_| Ok::<(), i32>(()), Some((source, bytes)))?
}

fn transaction_with_notification<T, E>(
    id: u64,
    body: impl FnOnce(&mut Network) -> Result<T, E>,
    notification: Option<(u32, Vec<u8>)>,
) -> Result<Result<T, E>, i32> {
    let (outcome, wake) = crate::namespaces::network_update(id, |bytes| {
        let mut state = decode_namespace(bytes)?;
        let before = (state.loopback_flags, state.links.clone());
        let journal = std::mem::take(&mut state.notifications);
        let outcome = body(&mut state);
        state.notifications = journal;
        if outcome.is_ok() {
            let wake = crate::netlink::multicast::record(id, before, &mut state, notification)?;
            state.generation = state.generation.wrapping_add(1);
            Ok((encode_namespace(&state)?, (outcome, wake)))
        } else {
            Ok((bytes.to_vec(), (outcome, None)))
        }
    })?;
    if let Some(wake) = wake {
        wake.signal()?;
    }
    Ok(outcome)
}

pub(crate) fn sysctl(name: &str) -> Result<Option<Vec<u8>>, i32> {
    Ok(snapshot()?
        .sysctls
        .into_iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value))
}

pub(crate) fn set_sysctl(name: &str, value: Vec<u8>) -> Result<(), i32> {
    if name.is_empty() {
        return Err(EINVAL);
    }
    transaction(|state| {
        if let Some(entry) = state.sysctls.iter_mut().find(|entry| entry.name == name) {
            entry.value = value;
        } else {
            state.sysctls.push(Sysctl {
                name: name.to_owned(),
                value,
            });
        }
        Ok::<(), i32>(())
    })??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_root_network_lookup_after_chroot_keeps_the_host_topology() {
        const ROOT: &str = "KINAKAZE_ROUTE_ROOT_TEST_ROOT";
        const MARKER: &str = "test.route.domain.marker";
        if let Some(root) = std::env::var_os(ROOT) {
            crate::path::initialize_namespace_root(std::path::PathBuf::from(root)).unwrap();
            crate::path::set_system_root(crate::path::default_system_root().join("jail"));
            assert!(crate::path::system_root().unwrap().ends_with("jail"));
            let network = snapshot_for(1).unwrap();
            assert!(
                network
                    .sysctls
                    .iter()
                    .any(|row| row.name == MARKER && row.value == [73]),
                "chroot must not select a different root network registry"
            );
            return;
        }
        // The parent pins the namespace while its independently hosted reader
        // opens it. No state is expected to survive the last namespace owner.
        let pin = crate::namespaces::pin_network(1).unwrap();
        transaction_for(1, |state| {
            state.sysctls.push(Sysctl {
                name: MARKER.into(),
                value: vec![73],
            });
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-route-root-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("jail")).unwrap();
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "route_state::tests::first_root_network_lookup_after_chroot_keeps_the_host_topology", "--nocapture"])
            .env(ROOT, &root).creation_flags(0x0800_0000).output().unwrap();
        assert!(
            !root.join(".kinakaze").exists(),
            "network state must not create root files"
        );
        std::fs::remove_dir(root.join("jail")).unwrap();
        std::fs::remove_dir(root).unwrap();
        transaction_for(1, |state| {
            state.sysctls.retain(|row| row.name != MARKER);
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        drop(pin);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn network_encoding_round_trips_links_addresses_and_routes() {
        let state = Network {
            notifications: Default::default(),
            loopback_flags: 0x49,
            netfilter: b"namespace-firewall".to_vec(),
            generation: 7,
            next_index: 102,
            links: vec![Link {
                index: 101,
                name: "br-test".into(),
                kind: "bridge".into(),
                flags: 3,
                mtu: 1500,
                address: vec![2, 3, 4, 5, 6, 7],
                broadcast: vec![0xff; 6],
                master: 0,
                peer: 0,
                attributes: vec![Attribute {
                    kind: 18,
                    value: vec![1, 2, 3],
                }],
            }],
            addresses: vec![Address {
                index: 101,
                family: 2,
                prefix_len: 24,
                flags: 0x80,
                scope: 0,
                attributes: vec![Attribute {
                    kind: 1,
                    value: vec![172, 18, 0, 1],
                }],
            }],
            routes: vec![Route {
                family: 2,
                dst_len: 24,
                src_len: 0,
                tos: 0,
                table: 254,
                protocol: 2,
                scope: 253,
                kind: 1,
                flags: 0,
                attributes: vec![Attribute {
                    kind: 4,
                    value: 101u32.to_ne_bytes().to_vec(),
                }],
            }],
            sysctls: vec![Sysctl {
                name: "net.ipv4.ip_forward".into(),
                value: b"1\n".to_vec(),
            }],
        };
        let bytes = serialize(&state).unwrap();
        assert_eq!(deserialize(7, 102, &bytes).unwrap(), state);
    }
}

pub(crate) fn encode_namespace(state: &Network) -> Result<Vec<u8>, i32> {
    let mut bytes = state.generation.to_le_bytes().to_vec();
    bytes.extend_from_slice(&state.next_index.to_le_bytes());
    bytes.extend_from_slice(&serialize(state)?);
    Ok(bytes)
}
fn decode_namespace(bytes: &[u8]) -> Result<Network, i32> {
    if bytes.len() < 8 {
        return Err(EIO);
    }
    deserialize(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        &bytes[8..],
    )
}
