//! Stable addresses allocated only for our packet sections. Claims are shared
//! across loader processes; live claims cannot alias independently created
//! arenas. A foreign mapping is never relocated or overwritten.
use super::*;
use crate::mount::shared::Store;
const DIRECTORY: u64 = u64::MAX - 43;
const BASE: usize = 0x0000_4800_0000_0000;
const COUNT: usize = 65536;
pub(super) fn offset(address: usize) -> usize {
    address - BASE
}
pub(super) fn section(domain: u64, create: bool) -> Result<crate::fs::object::Object, i32> {
    let name: Vec<u16> = format!("Local\\Kinakaze.Packet.{domain:016x}.Storage")
        .encode_utf16()
        .chain([0])
        .collect();
    let size = (COUNT * ARENA_SIZE) as u64;
    crate::fs::object::Object::owned(unsafe {
        if create {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE | SEC_RESERVE,
                (size >> 32) as u32,
                size as u32,
                name.as_ptr(),
            )
        } else {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr())
        }
    })
}
pub(super) fn addresses() -> Result<Vec<(u64, usize)>, i32> {
    let directory = match Store::user_object(DIRECTORY, false) {
        Ok(directory) => directory,
        Err(crate::ENOENT) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let bytes = directory.read()?.1;
    if bytes.len() % 16 != 0 {
        return Err(EIO);
    }
    bytes
        .chunks_exact(16)
        .map(|row| {
            let id = u64::from_le_bytes(row[..8].try_into().unwrap());
            let slot = u64::from_le_bytes(row[8..].try_into().unwrap()) as usize;
            if slot >= COUNT {
                return Err(EIO);
            }
            Ok((id, BASE + slot * ARENA_SIZE))
        })
        .collect()
}
pub(super) fn address(id: u64) -> Result<Option<usize>, i32> {
    Ok(addresses()?
        .into_iter()
        .find_map(|(owner, address)| (owner == id).then_some(address)))
}

pub(super) fn pin(address: usize) -> Result<Option<Arc<Store>>, i32> {
    if (BASE..BASE + COUNT * ARENA_SIZE).contains(&address) {
        Ok(Some(Arc::new(Store::user_object(DIRECTORY, false)?)))
    } else {
        Ok(None)
    }
}
pub(crate) fn pin_directory() -> Result<crate::fs::object::Object, i32> {
    Store::user_object(DIRECTORY, false)?.pin()
}
impl SharedEndpoint {
    pub(crate) fn create_managed(
        domain: u64,
        id: u64,
        addresses: &[IpCidr],
        ifindex: u32,
        kind: Kind,
    ) -> Result<Arc<Self>, i32> {
        if addresses.is_empty() || addresses.len() > 2 || ifindex == 0 || id == 0 {
            return Err(EINVAL);
        }
        let directory = Store::user_object(DIRECTORY, true)?;
        directory.update(|bytes| {
            if bytes.len() % 16 != 0 {
                return Err(EIO);
            }
            let mut claims = Vec::new();
            let mut occupied = vec![false; COUNT];
            let storage = match section(domain, false) {
                Ok(storage) => Some(storage),
                Err(crate::ENOENT) => None,
                Err(error) => return Err(error),
            };
            let count = bytes.len() / 16;
            let start = (id as usize) % count.max(1);
            for (index, row) in bytes.chunks_exact(16).enumerate() {
                let owner = u64::from_le_bytes(row[..8].try_into().unwrap());
                let slot = u64::from_le_bytes(row[8..].try_into().unwrap()) as usize;
                if slot >= COUNT {
                    return Err(EIO);
                }
                if storage.is_some() {
                    if count >= 64 && (index + count - start) % count < 8 {
                        match Self::open(domain, owner) {
                            Ok(root) if root.collect_empty()? => {
                                let _ = crate::usernet::packet::remove_empty_owner_directory(owner);
                                continue;
                            }
                            Ok(_) => {}
                            Err(ENOENT | crate::EBADF) => continue,
                            Err(error) => return Err(error),
                        }
                    }
                    if occupied[slot] {
                        return Err(EIO);
                    }
                    occupied[slot] = true;
                    claims.extend_from_slice(row);
                    if owner == id {
                        return Err(crate::EEXIST);
                    }
                }
            }
            let slot = occupied
                .iter()
                .position(|used| !used)
                .ok_or(crate::ENOSPC)?;
            let endpoint = Self::open_inner(
                domain,
                id,
                Some((addresses, ifindex, kind)),
                Some(BASE + slot * ARENA_SIZE),
            )?;
            claims.extend_from_slice(&id.to_le_bytes());
            claims.extend_from_slice(&(slot as u64).to_le_bytes());
            Ok((claims, endpoint))
        })
    }
    pub(crate) fn pin_allocation_directory() -> Result<crate::fs::object::Object, i32> {
        pin_directory()
    }
    pub(crate) fn retained_descriptions() -> Result<Vec<(u64, [u8; 120])>, i32> {
        let domain = kinakaze_runtime::authority::domain_id();
        let mut result = Vec::new();
        for (id, _) in addresses()? {
            let root = match Self::open(domain, id) {
                Ok(root) => root,
                Err(ENOENT | crate::EBADF) => continue,
                Err(error) => {
                    diagnostic(id, "open", error);
                    return Err(error);
                }
            };
            match root.descriptions() {
                Ok(descriptions) => result.extend(descriptions),
                Err(crate::EBADF) => {}
                Err(error) => {
                    diagnostic(id, "descriptions", error);
                    return Err(error);
                }
            }
        }
        Ok(result)
    }
}

fn diagnostic(id: u64, stage: &str, error: i32) {
    if let Some(path) = std::env::var_os("KINAKAZE_GATEWAY_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::Path::new(&path).join(format!("packet-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "retained {id} {stage}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_last_view_release_and_reopen_preserves_reserved_address() {
        let store = crate::mount::shared::new_object().unwrap();
        let domain = kinakaze_runtime::authority::domain_id();
        let id = store.id();
        let root = SharedEndpoint::create_managed(
            domain,
            id,
            &[IpCidr::new(
                super::super::super::IpAddress::v4(127, 0, 0, 1),
                8,
            )],
            1,
            Kind::Udp,
        )
        .unwrap();
        let _section = root.pin().unwrap();
        let _directory = pin_directory().unwrap();
        drop(root);
        let workers: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        let root = SharedEndpoint::open(domain, id).unwrap();
                        assert_eq!(root.queued_bytes(), Ok(0));
                        drop(root);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }
    #[test]
    fn closed_udp_storage_is_reused_while_a_live_description_remains_valid() {
        let create = || {
            let fd = crate::socket::socket(2, 2, 0).unwrap();
            crate::usernet::packet::initialize(fd).unwrap();
            fd
        };
        let live = create();
        for _ in 0..192 {
            crate::close(create()).unwrap();
        }
        assert!(addresses().unwrap().len() <= 64);
        assert_eq!(crate::socket::packet_queued_bytes(live), Ok(Some(0)));
        crate::close(live).unwrap();
    }
}
