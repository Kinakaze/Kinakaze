//! Namespace-visible Unix socket metadata. Sections follow the socket's open
//! description through dup, fork, exec and SCM_RIGHTS; no polling owner exists.
use super::{Namespace, State, UnixSocket};
use crate::EIO;
use crate::fs::object::Object;
use crate::mount::shared::{self, Store};
use std::sync::{Arc, Mutex};

pub(super) struct Record {
    store: Store,
    pub(super) pin: Arc<Object>,
}

static DIRECTORY: Mutex<Option<Arc<Store>>> = Mutex::new(None);
fn directory() -> Result<Arc<Store>, i32> {
    let mut slot = DIRECTORY.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Arc::new(Store::user_object(u64::MAX - 41, true)?));
    }
    Ok(slot.as_ref().unwrap().clone())
}
fn ids(bytes: &[u8]) -> Result<Vec<u64>, i32> {
    if !bytes.len().is_multiple_of(8) {
        return Err(EIO);
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .collect())
}
fn register(id: u64) -> Result<(), i32> {
    directory()?.update(|bytes| {
        let mut list = ids(bytes)?;
        list.retain(|id| Store::user_object(*id, false).is_ok());
        if !list.contains(&id) {
            list.push(id);
        }
        Ok((list.into_iter().flat_map(u64::to_le_bytes).collect(), ()))
    })
}
impl Record {
    pub(super) fn new(network: u64, kind: i32) -> Result<Arc<Self>, i32> {
        let record = Self::from_store(shared::new_object()?)?;
        record.write(network, kind, State::Idle, &super::UnixAddress::unnamed())?;
        register(record.id())?;
        Ok(record)
    }
    fn from_store(store: Store) -> Result<Arc<Self>, i32> {
        let pin = Arc::new(store.pin()?);
        crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
        Ok(Arc::new(Self { store, pin }))
    }
    pub(super) fn restore(id: u64) -> Result<Arc<Self>, i32> {
        // Keep the directory alive in this provider after the creator exits.
        let record = Self::from_store(Store::user_object(id, false)?)?;
        register(id)?;
        Ok(record)
    }
    pub(super) fn id(&self) -> u64 {
        self.store.id()
    }
    pub(super) fn passcred(&self) -> Result<bool, i32> {
        self.store.read_with(|bytes| {
            if bytes.len() < 32 || &bytes[..8] != b"CYUNIX02" {
                return Err(EIO);
            }
            Ok(bytes[28] != 0)
        })
    }
    pub(super) fn set_passcred(&self, enabled: bool) -> Result<(), i32> {
        self.store.update(|bytes| {
            if bytes.len() < 32 || &bytes[..8] != b"CYUNIX02" {
                return Err(EIO);
            }
            let mut bytes = bytes.to_vec();
            bytes[28] = u8::from(enabled);
            Ok((bytes, ()))
        })
    }
    pub(super) fn publish(&self, socket: &UnixSocket) -> Result<(), i32> {
        self.write(
            socket.network,
            socket.socket_type,
            socket.state,
            &socket.local,
        )
    }
    fn write(
        &self,
        network: u64,
        kind: i32,
        state: State,
        address: &super::UnixAddress,
    ) -> Result<(), i32> {
        let flags: u32 = if state == State::Listening {
            0x10000
        } else {
            0
        };
        let state: u32 = match state {
            State::Connected => 3,
            State::Disconnected => 4,
            _ => 1,
        };
        let mut bytes = b"CYUNIX02".to_vec();
        bytes.extend_from_slice(&network.to_le_bytes());
        bytes.extend_from_slice(&(kind as u32).to_le_bytes());
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&state.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        if address.namespace == Namespace::Abstract {
            bytes.push(b'@');
        }
        bytes.extend_from_slice(&address.name);
        self.store.update(|previous| {
            if !previous.is_empty() {
                if previous.len() < 32 || &previous[..8] != b"CYUNIX02" {
                    return Err(EIO);
                }
                bytes[28..32].copy_from_slice(&previous[28..32]);
            }
            Ok((bytes, ()))
        })
    }
}

pub(crate) fn inode(fd: i32) -> Result<u64, i32> {
    Ok(super::snapshot(fd)?.record.id())
}

pub(crate) fn snapshot(network: u64) -> Result<String, i32> {
    use std::fmt::Write;
    let mut output = String::from("Num       RefCount Protocol Flags    Type St Inode Path\n");
    for id in ids(&directory()?.read()?.1)? {
        let Ok(store) = Store::user_object(id, false) else {
            continue;
        };
        let bytes = store.read()?.1;
        if bytes.len() < 32 || &bytes[..8] != b"CYUNIX02" {
            return Err(EIO);
        }
        if u64::from_le_bytes(bytes[8..16].try_into().unwrap()) != network {
            continue;
        }
        let word = |start| u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap());
        // Addresses are bytes; preserve embedded abstract-name NULs as @,
        // matching Linux's text view. Do not expose a native pointer/refcount.
        let path: Vec<u8> = bytes[32..]
            .iter()
            .map(|b| if *b == 0 { b'@' } else { *b })
            .collect();
        let _ = write!(
            output,
            "0000000000000000: 00000000 00000000 {:08X} {:04X} {:02X} {id}",
            word(20),
            word(16),
            word(24)
        );
        if !path.is_empty() {
            let _ = write!(output, " {}", String::from_utf8_lossy(&path));
        }
        output.push('\n');
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(text: &str, id: u64) -> Option<Vec<&str>> {
        text.lines()
            .skip(1)
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .find(|words| words.get(6).is_some_and(|s| s.parse::<u64>() == Ok(id)))
    }
    #[test]
    fn socket_rows_follow_dup_and_final_close() {
        let (left, right) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
        let id = inode(left).unwrap();
        let network = crate::usernet::current().unwrap();
        let text = snapshot(network).unwrap();
        let fields = row(&text, id).unwrap();
        assert_eq!(fields[4], "0001");
        assert_eq!(fields[5], "03");
        assert_eq!(
            crate::procfs::local_fd_link_target(left).unwrap(),
            format!("socket:[{id}]")
        );
        assert!(row(&snapshot(network + 1000000).unwrap(), id).is_none());
        let entry = crate::get(left).unwrap();
        let copy = Object::duplicate(entry.raw as _).unwrap();
        let alias =
            crate::install_duplicate(copy.raw() as usize, entry.kind, entry.flags, entry).unwrap();
        let raw = copy.into_raw() as usize;
        crate::unix::duplicate(left, alias, raw).unwrap();
        crate::close(left).unwrap();
        assert_eq!(inode(alias).unwrap(), id);
        assert!(row(&snapshot(network).unwrap(), id).is_some());
        crate::close(alias).unwrap();
        assert!(row(&snapshot(network).unwrap(), id).is_none());
        crate::close(right).unwrap();
    }
    #[test]
    fn observer_child() {
        let Ok(value) = std::env::var("KINAKAZE_PROCNET_OBSERVER") else {
            return;
        };
        let id: u64 = value.parse().unwrap();
        assert!(row(&snapshot(crate::usernet::current().unwrap()).unwrap(), id).is_some());
    }
    #[test]
    fn listener_publishes_its_abstract_address_and_accept_flag() {
        let fd = crate::unix::socket(crate::socket::SOCK_STREAM, 0).unwrap();
        let id = inode(fd).unwrap();
        let mut address = 1u16.to_ne_bytes().to_vec();
        address.push(0);
        let name = format!("procnet-{}-{id}", std::process::id());
        address.extend_from_slice(name.as_bytes());
        unsafe { crate::unix::bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        crate::unix::listen(fd, 16).unwrap();
        let text = snapshot(crate::usernet::current().unwrap()).unwrap();
        let fields = row(&text, id).unwrap();
        assert_eq!(fields[3], "00010000");
        assert_eq!(fields[5], "01");
        assert_eq!(fields[7], format!("@{name}"));
        crate::close(fd).unwrap();
    }
    #[test]
    fn another_native_process_observes_the_shared_socket_row() {
        use std::os::windows::process::CommandExt;
        let (left, right) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
        let id = inode(left).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unix::procnet::tests::observer_child",
                "--nocapture",
            ])
            .env("KINAKAZE_PROCNET_OBSERVER", id.to_string())
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        crate::close(left).unwrap();
        crate::close(right).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
