//! Classic socket BPF. Immutable verified programs are cached by revision.
//! Packet providers supply the actual protocol view and
//! metadata; the interpreter does not manufacture TCP headers from a byte stream.

pub(crate) mod classic;
mod packet;
use crate::{EFAULT, EINVAL, EIO, ENOENT, ENOTSOCK, EOPNOTSUPP, EPERM, FdKind};
use classic::Program;
pub(crate) use packet::LocalPacket;
use std::sync::Arc;

pub const SO_ATTACH_FILTER: i32 = 26;
pub const SO_DETACH_FILTER: i32 = 27;
pub const SO_GET_FILTER: i32 = SO_ATTACH_FILTER;
pub const SO_LOCK_FILTER: i32 = 44;
pub const SO_DETACH_BPF: i32 = SO_DETACH_FILTER;

#[derive(Clone, Default, Debug)]
pub(crate) struct State {
    pub(crate) program: Option<Program>,
    pub(crate) locked: bool,
}
impl State {
    pub(crate) fn validate_encoding(bytes: &[u8]) -> Result<(), i32> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() < 8
            || bytes.len() > 8 + classic::MAX_INSNS * 8
            || bytes.len() % 8 != 0
            || &bytes[..4] != b"CBF1"
            || bytes[4] > 1
            || bytes[5..8] != [0; 3]
        {
            return Err(EIO);
        }
        Ok(())
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        Self::validate_encoding(bytes)?;
        Ok(Self {
            locked: bytes[4] != 0,
            program: if bytes.len() == 8 {
                None
            } else {
                Some(Program::decode(&bytes[8..]).map_err(|_| EIO)?)
            },
        })
    }
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = b"CBF1\0\0\0\0".to_vec();
        bytes[4] = self.locked as u8;
        if let Some(program) = &self.program {
            bytes.extend_from_slice(&program.encode());
        }
        bytes
    }
    pub(crate) fn run(&self, packet: &impl classic::Packet, cap: usize) -> Option<usize> {
        let Some(program) = &self.program else {
            return Some(packet.len() as usize);
        };
        let result = program.run(packet) as usize;
        (result != 0).then(|| result.max(cap).min(packet.len() as usize))
    }
}

pub(crate) fn snapshot(store: &crate::ofd::Shared) -> Result<Arc<State>, i32> {
    let revision = store.revision();
    let mut cache = store.filter_cache.lock().map_err(|_| EIO)?;
    if let Some((cached, state)) = &*cache {
        if *cached == revision {
            return Ok(state.clone());
        }
    }
    let (revision, bytes) = store.read()?;
    let state = Arc::new(State::decode(bytes.get(16..).ok_or(EIO)?)?);
    *cache = Some((revision, state.clone()));
    Ok(state)
}
pub(crate) fn for_description(description: u64) -> Result<Option<Arc<State>>, i32> {
    crate::ofd::existing(description)?
        .map(|store| snapshot(&store))
        .transpose()
}
fn check_socket(fd: i32) -> Result<crate::FdEntry, i32> {
    let entry = crate::get(fd)?;
    if !matches!(
        entry.kind,
        FdKind::Socket | FdKind::UnixSocket | FdKind::NetlinkSocket
    ) {
        return Err(ENOTSOCK);
    }
    Ok(entry)
}
fn for_entry(entry: crate::FdEntry) -> Result<Option<Arc<State>>, i32> {
    if entry.flags.contains(crate::FdFlags::PACKET_SOCKET) {
        crate::usernet::packet::filter(entry.description_id)
    } else {
        for_description(entry.description_id)
    }
}

// Fault-contained copying of caller-owned memory. RPM probes the destination
// too, without WriteProcessMemory's debugger permission changes.
fn copy(source: *const u8, target: *mut u8, length: usize) -> Result<(), i32> {
    if length == 0 {
        return Ok(());
    }
    if source.is_null()
        || target.is_null()
        || (source as usize).checked_add(length).is_none()
        || (target as usize).checked_add(length).is_none()
    {
        return Err(EFAULT);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReadProcessMemory(
            process: *mut core::ffi::c_void,
            source: *const core::ffi::c_void,
            destination: *mut core::ffi::c_void,
            length: usize,
            copied: *mut usize,
        ) -> i32;
    }
    let mut copied = 0;
    let ok = unsafe {
        ReadProcessMemory(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            source.cast(),
            target.cast(),
            length,
            &mut copied,
        )
    };
    if ok == 0 || copied != length {
        Err(EFAULT)
    } else {
        Ok(())
    }
}

pub(crate) unsafe fn set(fd: i32, name: i32, value: *const u8, length: i32) -> Result<(), i32> {
    let entry = check_socket(fd)?;
    if length < 4 {
        return Err(EINVAL);
    }
    let mut scalar = [0u8; 4];
    copy(value, scalar.as_mut_ptr(), 4)?;
    let program = if name == SO_ATTACH_FILTER {
        if length != 16 {
            return Err(EINVAL);
        }
        let mut header = [0u8; 16];
        copy(value, header.as_mut_ptr(), 16)?;
        if for_entry(entry)?.is_some_and(|s| s.locked) {
            return Err(EPERM);
        }
        let count = u16::from_le_bytes([header[0], header[1]]) as usize;
        if count == 0 || count > classic::MAX_INSNS {
            return Err(EINVAL);
        }
        let address = u64::from_le_bytes(header[8..16].try_into().unwrap()) as *const u8;
        let mut bytes = vec![0; count * 8];
        copy(address, bytes.as_mut_ptr(), bytes.len())?;
        Some(Program::decode(&bytes)?)
    } else {
        None
    };
    if entry.flags.contains(crate::FdFlags::PACKET_SOCKET) {
        return crate::usernet::packet::set_filter(
            fd,
            entry.generation,
            name,
            program,
            i32::from_le_bytes(scalar) != 0,
        );
    }
    // A Winsock byte stream does not expose skb headers or a pre-ACK receive
    // hook. Reject until an enforcing packet backend is installed.
    if name == SO_ATTACH_FILTER && entry.kind == FdKind::Socket {
        return Err(EOPNOTSUPP);
    }
    let store = crate::ofd::promote(fd)?;
    if crate::get(fd)?.generation != entry.generation {
        return Err(crate::EBADF);
    }
    if entry.kind == FdKind::NetlinkSocket {
        crate::netlink::filter_boundary(fd)?;
    }
    store.reserve_update(24 + program.as_ref().map_or(0, |p| p.instructions().len() * 8))?;
    let apply = |publish: &mut dyn FnMut(&[u8]) -> Result<(), i32>| {
        store.update(|bytes| {
            let mut state = State::decode(bytes.get(16..).ok_or(EIO)?)?;
            match name {
                SO_ATTACH_FILTER => {
                    if state.locked {
                        return Err(EPERM);
                    }
                    state.program = program;
                }
                SO_DETACH_FILTER => {
                    if state.locked {
                        return Err(EPERM);
                    }
                    if state.program.take().is_none() {
                        return Err(ENOENT);
                    }
                }
                SO_LOCK_FILTER => {
                    let locked = i32::from_le_bytes(scalar) != 0;
                    if state.locked && !locked {
                        return Err(EPERM);
                    }
                    state.locked = locked;
                }
                _ => return Err(crate::ENOPROTOOPT),
            }
            let mut updated = bytes[..16].to_vec();
            let encoded = state.encode();
            updated.extend_from_slice(&encoded);
            publish(&encoded)?;
            Ok((updated, ()))
        })
    };
    if entry.kind == FdKind::UnixSocket {
        crate::unix::with_filter_publication(fd, &store, apply)
    } else {
        apply(&mut |_| Ok(()))
    }
}

pub(crate) unsafe fn get(fd: i32, name: i32, value: *mut u8, length: *mut i32) -> Result<(), i32> {
    let entry = check_socket(fd)?;
    let mut raw_len = [0u8; 4];
    copy(length.cast(), raw_len.as_mut_ptr(), 4)?;
    let capacity = i32::from_le_bytes(raw_len);
    if capacity < 0 {
        return Err(EINVAL);
    }
    let state = for_entry(entry)?.unwrap_or_default();
    let actual = match name {
        SO_GET_FILTER => {
            let count = state.program.as_ref().map_or(0, |p| p.instructions().len());
            if count > 0 && capacity != 0 {
                if (capacity as usize) < count {
                    return Err(EINVAL);
                }
                let bytes = state.program.as_ref().unwrap().encode();
                copy(bytes.as_ptr(), value, bytes.len())?;
            }
            count as i32
        }
        SO_LOCK_FILTER => {
            let bytes = i32::from(state.locked).to_le_bytes();
            let size = (capacity as usize).min(4);
            copy(bytes.as_ptr(), value, size)?;
            size as i32
        }
        _ => return Err(crate::ENOPROTOOPT),
    };
    copy(actual.to_le_bytes().as_ptr(), length.cast(), 4)
}

#[cfg(test)]
mod tests;
