//! Native capability custody. Payload I/O stays in workers; init owns only
//! references, with weak owner tokens and a graph for queued socket cycles.
use super::*;
use kinakaze_v2_host_win::ProcessHandle;
use kinakaze_v2_manager::PeerIdentity;
use std::sync::OnceLock;
use windows_sys::Win32::Networking::WinSock::*;

#[link(name = "kernelbase.dll", kind = "raw-dylib")]
unsafe extern "system" {
    fn CompareObjectHandles(first: HANDLE, second: HANDLE) -> i32;
}

fn peer_process(peer: PeerIdentity) -> Result<ProcessHandle> {
    let process = ProcessHandle::open(peer.host_pid)
        .map_err(|_| error(ErrorCode::NotFound, "resource publisher exited"))?;
    if process.birth() != peer.birth {
        return Err(error(ErrorCode::Unauthorized, "resource publisher changed"));
    }
    Ok(process)
}

pub(super) fn owner() -> Result<Vec<u64>> {
    let pid = std::process::id();
    let process = ProcessHandle::open(pid)
        .map_err(|_| error(ErrorCode::Internal, "cannot identify native resource owner"))?;
    Ok(vec![u64::from(pid), process.birth()])
}

struct Socket(usize);
impl Drop for Socket {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.0);
        }
    }
}
fn winsock() -> Result<()> {
    static READY: OnceLock<bool> = OnceLock::new();
    if *READY.get_or_init(|| {
        let mut data = unsafe { std::mem::zeroed() };
        unsafe { WSAStartup(0x202, &mut data) == 0 }
    }) {
        Ok(())
    } else {
        Err(error(ErrorCode::Internal, "Winsock initialization failed"))
    }
}
impl Socket {
    fn import(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != size_of::<WSAPROTOCOL_INFOW>() {
            return Err(error(ErrorCode::InvalidRequest, "invalid Winsock recipe"));
        }
        winsock()?;
        let info = unsafe { bytes.as_ptr().cast::<WSAPROTOCOL_INFOW>().read_unaligned() };
        let socket = unsafe {
            WSASocketW(
                FROM_PROTOCOL_INFO,
                FROM_PROTOCOL_INFO,
                FROM_PROTOCOL_INFO,
                &info,
                0,
                WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        if socket == INVALID_SOCKET {
            Err(error(
                ErrorCode::InvalidRequest,
                "cannot import Winsock reference",
            ))
        } else {
            Ok(Self(socket))
        }
    }
    fn export(&self, pid: u32) -> Result<Vec<u64>> {
        let mut info = unsafe { std::mem::zeroed::<WSAPROTOCOL_INFOW>() };
        if unsafe { WSADuplicateSocketW(self.0, pid, &mut info) } != 0 {
            return Err(error(
                ErrorCode::Internal,
                "cannot export Winsock reference",
            ));
        }
        let bytes = unsafe {
            std::slice::from_raw_parts((&raw const info).cast::<u8>(), size_of_val(&info))
        };
        Ok(bytes.iter().map(|b| u64::from(*b)).collect())
    }
}

enum Resource {
    Handle {
        handle: OwnedHandle,
        dependency: u64,
    },
    Socket(Socket),
    Wake {
        _notification: Wake,
    },
}
struct Wake(OwnedHandle);
impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.0.as_raw_handle());
        }
    }
}
struct Bundle {
    fingerprint: [u8; 32],
    publisher: PeerIdentity,
    resources: Vec<Resource>,
}
#[derive(Default)]
pub(super) struct Resources {
    bundles: BTreeMap<(u64, u64), Bundle>,
    count: usize,
}
struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    fn bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.0.len() {
            return Err(error(ErrorCode::InvalidRequest, "short resource payload"));
        }
        let (head, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(head)
    }
    fn word(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
}
impl Resources {
    pub(super) fn retain(
        &mut self,
        domain: u64,
        peer: PeerIdentity,
        owner: u64,
        id: u64,
        bytes: &[u8],
    ) -> Result<()> {
        if owner == 0 || id == 0 {
            return Err(error(ErrorCode::InvalidRequest, "zero resource identity"));
        }
        let _owner = open(&ObjectKey::Shared(owner).name(domain))?;
        let fingerprint = *blake3::hash(bytes).as_bytes();
        if let Some(previous) = self.bundles.get(&(owner, id)) {
            return if previous.publisher == peer && previous.fingerprint == fingerprint {
                Ok(())
            } else {
                Err(error(
                    ErrorCode::Conflict,
                    "resource identity already published",
                ))
            };
        }
        if self.bundles.len() >= 16384 {
            return Err(error(ErrorCode::LimitExceeded, "too many resource bundles"));
        }
        let process = peer_process(peer)?;
        let mut input = Input(bytes);
        if input.word()? != RESOURCE_MAGIC {
            return Err(error(
                ErrorCode::InvalidRequest,
                "invalid resource payload version",
            ));
        }
        let count = usize::try_from(input.word()?)
            .map_err(|_| error(ErrorCode::LimitExceeded, "resource count overflow"))?;
        if count > MAX_NATIVE_RESOURCES || self.count + count > 65536 {
            return Err(error(ErrorCode::LimitExceeded, "too many native resources"));
        }
        let mut resources = Vec::with_capacity(count);
        for _ in 0..count {
            resources.push(match input.word()? {
                0 => {
                    let raw = input.word()?;
                    let dependency = input.word()?;
                    let handle = process
                        .duplicate_object(raw)
                        .map_err(|_| error(ErrorCode::InvalidRequest, "invalid resource handle"))?;
                    if dependency != 0 {
                        let token = open(&ObjectKey::Shared(dependency).name(domain))?;
                        if unsafe {
                            CompareObjectHandles(handle.as_raw_handle(), token.as_raw_handle())
                        } == 0
                        {
                            return Err(error(
                                ErrorCode::InvalidRequest,
                                "resource dependency does not match handle",
                            ));
                        }
                    }
                    Resource::Handle { handle, dependency }
                }
                1 => {
                    let count = usize::try_from(input.word()?)
                        .map_err(|_| error(ErrorCode::InvalidRequest, "invalid recipe length"))?;
                    Resource::Socket(Socket::import(input.bytes(count)?)?)
                }
                2 => Resource::Wake {
                    _notification: Wake(process.duplicate_object(input.word()?).map_err(|_| {
                        error(ErrorCode::InvalidRequest, "invalid resource notification")
                    })?),
                },
                _ => {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "unknown native resource kind",
                    ));
                }
            });
        }
        if !input.0.is_empty() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "trailing resource payload",
            ));
        }
        self.bundles.insert(
            (owner, id),
            Bundle {
                fingerprint,
                publisher: peer,
                resources,
            },
        );
        self.count += count;
        Ok(())
    }
    fn get(&self, owner: u64, id: u64) -> Result<&Bundle> {
        self.bundles
            .get(&(owner, id))
            .ok_or_else(|| error(ErrorCode::NotFound, "resource bundle is unavailable"))
    }
    pub(super) fn handles(&self, owner: u64, id: u64) -> Result<Vec<u64>> {
        Ok(self
            .get(owner, id)?
            .resources
            .iter()
            .map(|r| match r {
                Resource::Handle { handle, .. } => handle.as_raw_handle() as u64,
                Resource::Socket(_) | Resource::Wake { .. } => 0,
            })
            .collect())
    }
    pub(super) fn socket(
        &self,
        peer: PeerIdentity,
        owner: u64,
        id: u64,
        index: usize,
    ) -> Result<Vec<u64>> {
        let _process = peer_process(peer)?;
        match self.get(owner, id)?.resources.get(index) {
            Some(Resource::Socket(socket)) => socket.export(peer.host_pid),
            _ => Err(error(ErrorCode::InvalidRequest, "resource is not a socket")),
        }
    }
    pub(super) fn release(&mut self, owner: u64, id: u64) {
        if let Some(bundle) = self.bundles.remove(&(owner, id)) {
            self.count -= bundle.resources.len();
        }
    }

    pub(super) fn collect(&mut self, domain: u64) {
        let mut owned = BTreeMap::<u64, u32>::new();
        let mut edges = BTreeMap::<u64, BTreeSet<u64>>::new();
        for (&(owner, _), bundle) in &self.bundles {
            edges.entry(owner).or_default();
            for resource in &bundle.resources {
                if let Resource::Handle { dependency, .. } = resource
                    && *dependency != 0
                {
                    *owned.entry(*dependency).or_default() += 1;
                    edges.entry(owner).or_default().insert(*dependency);
                }
            }
        }
        let mut pending = Vec::new();
        for &owner in edges.keys() {
            match open(&ObjectKey::Shared(owner).name(domain)) {
                Ok(token) => {
                    if handle_count(token.as_raw_handle())
                        .is_none_or(|count| count > 1 + owned.get(&owner).copied().unwrap_or(0))
                    {
                        pending.push(owner);
                    }
                }
                Err(_) if lease_alive(domain, ObjectKey::Shared(owner)) => pending.push(owner),
                Err(_) => {}
            }
        }
        let mut live = BTreeSet::new();
        while let Some(owner) = pending.pop() {
            if live.insert(owner)
                && let Some(next) = edges.get(&owner)
            {
                pending.extend(next);
            }
        }
        self.bundles.retain(|(owner, _), bundle| {
            if live.contains(owner) {
                true
            } else {
                self.count -= bundle.resources.len();
                false
            }
        });
    }
}

fn handle_count(handle: HANDLE) -> Option<u32> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryObject(
            handle: HANDLE,
            class: u32,
            info: *mut core::ffi::c_void,
            size: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut info = [0u32; 14];
    (unsafe {
        NtQueryObject(
            handle,
            0,
            info.as_mut_ptr().cast(),
            size_of_val(&info) as u32,
            ptr::null_mut(),
        )
    } >= 0)
        .then_some(info[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    fn domain() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(7000);
        ((std::process::id() as u64) << 32) | NEXT.fetch_add(1, Ordering::Relaxed)
    }
    fn section(domain: u64, id: u64) -> OwnedHandle {
        let name: Vec<_> = ObjectKey::Shared(id)
            .name(domain)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let raw = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                4096,
                name.as_ptr(),
            )
        };
        assert!(!raw.is_null());
        unsafe { OwnedHandle::from_raw_handle(raw) }
    }
    fn peer() -> PeerIdentity {
        let host_pid = std::process::id();
        PeerIdentity {
            host_pid,
            birth: ProcessHandle::open(host_pid).unwrap().birth(),
        }
    }
    fn payload(handles: &[(&OwnedHandle, u64)]) -> Vec<u8> {
        let mut words = vec![RESOURCE_MAGIC, handles.len() as u64];
        for (handle, dependency) in handles {
            words.extend([0, handle.as_raw_handle() as u64, *dependency]);
        }
        words.into_iter().flat_map(u64::to_le_bytes).collect()
    }
    #[test]
    fn publisher_close_replay_release_and_owner_close() {
        let domain = domain();
        let owner = section(domain, 1);
        let source = section(domain, 2);
        let input = payload(&[(&source, 0)]);
        let mut resources = Resources::default();
        resources.retain(domain, peer(), 1, 3, &input).unwrap();
        resources.retain(domain, peer(), 1, 3, &input).unwrap();
        assert_eq!(resources.count, 1);
        drop(source);
        resources.collect(domain);
        assert!(open(&ObjectKey::Shared(2).name(domain)).is_ok());
        resources.release(1, 3);
        resources.release(1, 3);
        assert_eq!(resources.count, 0);
        assert!(open(&ObjectKey::Shared(2).name(domain)).is_err());
        let source = section(domain, 2);
        resources
            .retain(domain, peer(), 1, 4, &payload(&[(&source, 0)]))
            .unwrap();
        drop((source, owner));
        resources.collect(domain);
        assert!(resources.bundles.is_empty());
        assert_eq!(resources.count, 0);
        assert!(open(&ObjectKey::Shared(2).name(domain)).is_err());
    }
    #[test]
    fn malformed_publication_is_atomic_and_identity_cannot_be_replaced() {
        let domain = domain();
        let _owner = section(domain, 1);
        let source = section(domain, 2);
        let mut resources = Resources::default();
        let input = payload(&[(&source, 0), (&source, 0)]);
        for malformed in [
            input[..input.len() - 1].to_vec(),
            [input.clone(), vec![1]].concat(),
            payload(&[(&source, 1)]),
        ] {
            assert!(resources.retain(domain, peer(), 1, 3, &malformed).is_err());
            assert!(resources.bundles.is_empty());
            assert_eq!(resources.count, 0);
            assert_eq!(handle_count(source.as_raw_handle()), Some(1));
        }
        let mut changed = peer();
        changed.birth ^= 1;
        assert!(resources.retain(domain, changed, 1, 3, &input).is_err());
        resources.retain(domain, peer(), 1, 3, &input).unwrap();
        assert_eq!(
            resources
                .retain(domain, peer(), 1, 3, &payload(&[(&source, 0)]))
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(resources.count, 2);
    }
    #[test]
    fn queued_self_reference_is_collected_after_external_close() {
        let domain = domain();
        let owner = section(domain, 1);
        let mut resources = Resources::default();
        resources
            .retain(domain, peer(), 1, 1, &payload(&[(&owner, 1)]))
            .unwrap();
        resources.collect(domain);
        assert_eq!(resources.count, 1);
        drop(owner);
        resources.collect(domain);
        assert_eq!(resources.count, 0);
        assert!(open(&ObjectKey::Shared(1).name(domain)).is_err());
    }
    #[test]
    fn queued_cycle_is_traced_from_external_owner_then_collected() {
        let domain = domain();
        let first = section(domain, 1);
        let second = section(domain, 2);
        let data = section(domain, 3);
        let mut resources = Resources::default();
        resources
            .retain(domain, peer(), 1, 1, &payload(&[(&second, 2)]))
            .unwrap();
        resources
            .retain(domain, peer(), 2, 1, &payload(&[(&first, 1), (&data, 0)]))
            .unwrap();
        drop((second, data));
        resources.collect(domain);
        assert_eq!(resources.count, 3);
        assert!(open(&ObjectKey::Shared(3).name(domain)).is_ok());
        drop(first);
        resources.collect(domain);
        assert_eq!(resources.count, 0);
        for id in 1..=3 {
            assert!(open(&ObjectKey::Shared(id).name(domain)).is_err());
        }
    }
    #[test]
    fn socket_recipes_use_the_provider_and_survive_source_close() {
        let domain = domain();
        let _owner = section(domain, 1);
        winsock().unwrap();
        let source = Socket(unsafe {
            WSASocketW(
                AF_INET as i32,
                SOCK_DGRAM,
                IPPROTO_UDP,
                ptr::null(),
                0,
                WSA_FLAG_OVERLAPPED,
            )
        });
        assert_ne!(source.0, INVALID_SOCKET);
        let recipe: Vec<u8> = source
            .export(std::process::id())
            .unwrap()
            .into_iter()
            .map(|v| v as u8)
            .collect();
        let mut input: Vec<u8> = [RESOURCE_MAGIC, 1, 1, recipe.len() as u64]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect();
        input.extend(recipe);
        let mut resources = Resources::default();
        resources.retain(domain, peer(), 1, 1, &input).unwrap();
        drop(source);
        assert_eq!(resources.handles(1, 1).unwrap(), [0]);
        let recipe: Vec<u8> = resources
            .socket(peer(), 1, 1, 0)
            .unwrap()
            .into_iter()
            .map(|v| v as u8)
            .collect();
        let received = Socket::import(&recipe).unwrap();
        resources.release(1, 1);
        let mut kind = 0i32;
        let mut length = 4;
        assert_eq!(
            unsafe {
                getsockopt(
                    received.0,
                    SOL_SOCKET,
                    SO_TYPE,
                    (&raw mut kind).cast(),
                    &mut length,
                )
            },
            0
        );
        assert_eq!(kind, SOCK_DGRAM);
    }
    #[test]
    fn owner_collection_wakes_blocked_admission() {
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
        let domain = domain();
        let owner = section(domain, 1);
        let event =
            unsafe { OwnedHandle::from_raw_handle(CreateEventW(ptr::null(), 1, 0, ptr::null())) };
        assert!(!event.as_raw_handle().is_null());
        let input: Vec<u8> = [RESOURCE_MAGIC, 1, 2, event.as_raw_handle() as u64]
            .into_iter()
            .flat_map(u64::to_le_bytes)
            .collect();
        let mut resources = Resources::default();
        resources.retain(domain, peer(), 1, 1, &input).unwrap();
        resources.collect(domain);
        assert_eq!(
            unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        drop(owner);
        resources.collect(domain);
        assert_eq!(
            unsafe { WaitForSingleObject(event.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn inline_and_section_publications_share_validation_and_limits() {
        let domain = domain();
        let _owner = section(domain, 1);
        let source = section(domain, 2);
        let transfer = section(domain, 3);
        let bytes = payload(&[(&source, 0)]);
        let view = unsafe {
            MapViewOfFile(
                transfer.as_raw_handle(),
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                bytes.len(),
            )
        };
        assert!(!view.Value.is_null());
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast(), bytes.len());
            UnmapViewOfFile(view);
        }
        let mut kernel = Kernel::new(domain);
        for (id, input) in [
            (1, ResourceInput::Inline(bytes.clone())),
            (
                2,
                ResourceInput::Section {
                    source: transfer.as_raw_handle() as u64,
                    length: bytes.len() as u64,
                },
            ),
        ] {
            kernel
                .handle(
                    peer(),
                    &KernelCommand::RetainResources {
                        owner: 1,
                        id,
                        input,
                    },
                )
                .unwrap();
            let raw = kernel
                .handle(peer(), &KernelCommand::Resources { owner: 1, id })
                .unwrap();
            assert_eq!(raw.len(), 1);
            assert_ne!(
                unsafe { CompareObjectHandles(raw[0] as HANDLE, source.as_raw_handle()) },
                0
            );
            kernel
                .handle(peer(), &KernelCommand::ReleaseResources { owner: 1, id })
                .unwrap();
        }
        assert_eq!(
            kernel
                .handle(
                    peer(),
                    &KernelCommand::RetainResources {
                        owner: 1,
                        id: 3,
                        input: ResourceInput::Inline(vec![0; MAX_INLINE_RESOURCES + 1]),
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::LimitExceeded
        );
        assert!(kernel.resources.bundles.is_empty());
    }
}
