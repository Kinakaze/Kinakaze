//! Packet protocol references owned by guest socket descriptions. Native link
//! handles continue to use the existing Winsock fork/rights handoff; only this
//! program's section is pinned here, without copying its protocol buffers.
use super::stack::shared::SharedEndpoint;
use super::*;
pub(crate) mod gateway;
mod io;
mod lifetime;
mod route;
mod socket;
mod tcp;
pub(crate) fn managed_owners(arena: u64) -> Result<Vec<u64>, i32> {
    lifetime::present(arena)
}
pub(crate) fn remove_empty_owner_directory(arena: u64) -> Result<(), i32> {
    std::fs::remove_dir(lifetime::directory(arena)?).map_err(|_| EIO)
}
pub(crate) use io::prepare;
pub(crate) use io::queued_bytes;
pub(crate) use io::{receive, send};
pub(crate) use socket::{bind, connect, initialize, selected};
pub(crate) use tcp::{accept, listen, shutdown};
pub(crate) unsafe fn get_option(
    fd: i32,
    owner: &Endpoint,
    level: i32,
    name: i32,
    value: *mut u8,
    length: *mut i32,
) -> Result<(), i32> {
    use crate::socket::{SO_ERROR, SO_RCVBUF, SO_SNDBUF, SO_TYPE, SOL_SOCKET};
    if length.is_null() {
        return Err(crate::EFAULT);
    }
    let capacity = unsafe { length.read_unaligned() };
    if capacity < 0 {
        return Err(EINVAL);
    }
    if capacity != 0 && value.is_null() {
        return Err(crate::EFAULT);
    }
    let state = read(owner)?;
    let result: i32 = match (level, name) {
        (SOL_SOCKET, SO_TYPE) => state.kind,
        (SOL_SOCKET, SO_ERROR) => {
            if state.bound {
                owner.packet.as_ref().ok_or(EIO)?.drive(fd, owner)?;
            }
            edit(owner, |state| {
                let error = state.error;
                state.error = 0;
                Ok(error)
            })?
        }
        (SOL_SOCKET, SO_SNDBUF | SO_RCVBUF) => 65536,
        (SOL_SOCKET, 30) => i32::from(state.listening),
        (SOL_SOCKET, 2) => i32::from(state.reuse),
        (SOL_SOCKET, 6) => i32::from(state.broadcast),
        (41, 26) if state.local.family == 10 => i32::from(state.v6only),
        (6, 1) => i32::from(!owner.packet.as_ref().ok_or(EIO)?.endpoint.nagle()?),
        _ => return Err(crate::ENOPROTOOPT),
    };
    let count = (capacity as usize).min(4);
    if count != 0 {
        unsafe { std::ptr::copy_nonoverlapping(result.to_ne_bytes().as_ptr(), value, count) };
    }
    unsafe { length.write_unaligned(count as i32) };
    Ok(())
}
pub(crate) unsafe fn set_option(
    owner: &Endpoint,
    level: i32,
    name: i32,
    value: *const u8,
    length: i32,
) -> Result<(), i32> {
    if length < 4 {
        return Err(EINVAL);
    }
    if value.is_null() {
        return Err(crate::EFAULT);
    }
    let value = unsafe { value.cast::<i32>().read_unaligned() };
    match (level, name) {
        (6, 1) => owner
            .packet
            .as_ref()
            .ok_or(EIO)?
            .endpoint
            .set_nagle(value == 0),
        (1, 2) => edit(owner, |state| {
            state.reuse = value != 0;
            Ok(())
        }),
        (1, 6) => edit(owner, |state| {
            state.broadcast = value != 0;
            Ok(())
        }),
        (41, 26) => edit(owner, |state| {
            if state.local.family != 10 {
                return Err(crate::ENOPROTOOPT);
            }
            if state.bound {
                return Err(EINVAL);
            }
            state.v6only = value != 0;
            Ok(())
        }),
        _ => Err(crate::ENOPROTOOPT),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Reference {
    pub arena: u64,
    pub slot: u32,
    pub generation: u64,
    pub managed: bool,
}
impl Reference {
    fn encode(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.arena.to_le_bytes());
        output.extend_from_slice(&self.slot.to_le_bytes());
        output.extend_from_slice(&u32::from(self.managed).to_le_bytes());
        output.extend_from_slice(&self.generation.to_le_bytes());
    }
    fn decode(bytes: &[u8]) -> Result<Option<Self>, i32> {
        if bytes == [0; 24] {
            return Ok(None);
        }
        let value = Self {
            arena: u64::from_le_bytes(bytes[..8].try_into().map_err(|_| EIO)?),
            slot: u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| EIO)?),
            generation: u64::from_le_bytes(bytes[16..24].try_into().map_err(|_| EIO)?),
            managed: bytes[12] == 1,
        };
        if value.arena == 0
            || bytes[12] > 1
            || bytes[13..16] != [0; 3]
            || (value.slot == u32::MAX) != (value.generation == 0)
        {
            return Err(EIO);
        }
        Ok(Some(value))
    }
}

pub(super) struct Binding {
    pub root: Arc<SharedEndpoint>,
    pub endpoint: Arc<SharedEndpoint>,
    driver: Mutex<Option<io::Driver>>,
}
impl Binding {
    pub(super) fn open(reference: Reference) -> Result<Self, i32> {
        let domain = kinakaze_runtime::authority::domain_id();
        let root = SharedEndpoint::open(domain, reference.arena)?;
        let endpoint = if reference.slot == u32::MAX {
            root.clone()
        } else {
            SharedEndpoint::open_connection(
                domain,
                reference.arena,
                reference.slot as usize,
                reference.generation,
            )?
        };
        Ok(Self {
            root,
            endpoint,
            driver: Mutex::new(None),
        })
    }
}

pub(crate) fn description(fd: i32) -> Result<Option<Arc<Endpoint>>, i32> {
    description_at(fd, None)
}
fn description_at(fd: i32, generation: Option<u32>) -> Result<Option<Arc<Endpoint>>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = usize::try_from(fd)
        .ok()
        .and_then(|fd| table.slots.get(fd))
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if generation.is_some_and(|generation| generation != entry.generation) {
        return Err(crate::EBADF);
    }
    if !entry.flags.contains(crate::FdFlags::PACKET_SOCKET) {
        return Ok(None);
    }
    let endpoint = ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned()
        .ok_or(EIO)?;
    if endpoint.packet.is_none() {
        return Err(EIO);
    }
    Ok(Some(endpoint))
}
pub(crate) fn filter(description: u64) -> Result<Option<Arc<crate::socket::filter::State>>, i32> {
    let endpoint = ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .get(&description)
        .cloned();
    endpoint
        .and_then(|endpoint| {
            endpoint
                .packet
                .as_ref()
                .map(|packet| packet.endpoint.clone())
        })
        .map(|endpoint| endpoint.filter_state())
        .transpose()
}
pub(crate) fn set_filter(
    fd: i32,
    generation: u32,
    name: i32,
    program: Option<crate::socket::filter::classic::Program>,
    locked: bool,
) -> Result<(), i32> {
    use crate::socket::filter::{SO_ATTACH_FILTER, SO_DETACH_FILTER, SO_LOCK_FILTER};
    let owner = description_at(fd, Some(generation))?.ok_or(crate::ENOTSOCK)?;
    let endpoint = &owner.packet.as_ref().ok_or(EIO)?.endpoint;
    let snapshot = owner.store.update(|bytes| {
        match name {
            SO_ATTACH_FILTER => endpoint.attach_verified_filter(program.ok_or(EINVAL)?)?,
            SO_DETACH_FILTER => endpoint.detach_filter()?,
            SO_LOCK_FILTER => endpoint.lock_filter(locked)?,
            _ => return Err(crate::ENOPROTOOPT),
        }
        let mut state = State::decode(bytes)?;
        gateway::mirror_filter(&mut state, endpoint.filter_state()?.as_ref());
        let bytes = state.encode();
        Ok((bytes.clone(), bytes))
    })?;
    endpoint.save_description(&snapshot)?;
    if read(&owner)?.listening {
        gateway::activate(&owner)
    } else {
        gateway::kick(fd, &owner)
    }
}

pub(super) fn encode(reference: Option<Reference>, output: &mut Vec<u8>) {
    match reference {
        Some(reference) => reference.encode(output),
        None => output.extend_from_slice(&[0; 24]),
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Option<Reference>, i32> {
    if bytes.len() != 24 {
        return Err(EIO);
    }
    Reference::decode(bytes)
}
