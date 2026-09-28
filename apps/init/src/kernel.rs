//! Session-owned kernel sections. No guest code, path walks or I/O data RPCs.
use kinakaze_v2_protocol::{ErrorCode, RpcError, kernel::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::{Foundation::*, System::Memory::*};
mod resources;

type Result<T> = std::result::Result<T, RpcError>;
fn error(code: ErrorCode, message: &str) -> RpcError {
    RpcError::new(code, message)
}
fn open(name: &str) -> Result<OwnedHandle> {
    let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(error(
            ErrorCode::NotFound,
            "shared kernel section is unavailable",
        ));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn lease_alive(domain: u64, key: ObjectKey) -> bool {
    let name: Vec<_> = key.name(domain).encode_utf16().chain(Some(0)).collect();
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if handle.is_null() {
        // Resource pressure or an access failure is not proof of final close.
        return unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND;
    }
    unsafe {
        CloseHandle(handle);
    }
    true
}

struct Object {
    section: OwnedHandle,
    header: MEMORY_MAPPED_VIEW_ADDRESS,
    backing: Option<OwnedHandle>,
    dependencies: BTreeSet<ObjectKey>,
    leases: BTreeSet<ObjectKey>,
}
// The enclosing Kernel mutex serializes management. Shared words are atomic.
unsafe impl Send for Object {}
impl Drop for Object {
    fn drop(&mut self) {
        if !self.header.Value.is_null() {
            unsafe {
                UnmapViewOfFile(self.header);
            }
        }
    }
}
impl Object {
    fn word(&self, index: usize) -> &AtomicU64 {
        // All queried words are aligned inside the mapped, committed header.
        unsafe { &*self.header.Value.cast::<AtomicU64>().add(index) }
    }
    fn open(domain: u64, key: ObjectKey) -> Result<Self> {
        if matches!(key, ObjectKey::CgroupJob(_)) {
            use windows_sys::Win32::System::JobObjects::OpenJobObjectW;
            use windows_sys::Win32::System::SystemServices::JOB_OBJECT_QUERY;
            let name: Vec<_> = key.name(domain).encode_utf16().chain(Some(0)).collect();
            let handle = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(error(ErrorCode::NotFound, "cgroup job is unavailable"));
            }
            return Ok(Self {
                section: unsafe { OwnedHandle::from_raw_handle(handle) },
                header: MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: ptr::null_mut(),
                },
                backing: None,
                dependencies: BTreeSet::new(),
                leases: BTreeSet::new(),
            });
        }
        let section = open(&key.name(domain))?;
        let header = unsafe {
            MapViewOfFile(
                section.as_raw_handle(),
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                match key {
                    ObjectKey::Mount(_) | ObjectKey::Shared(_) => SECTION_SIZE,
                    _ => 72,
                },
            )
        };
        if header.Value.is_null() {
            return Err(error(ErrorCode::Internal, "cannot map kernel header"));
        }
        let object = Self {
            section,
            header,
            backing: None,
            dependencies: BTreeSet::new(),
            leases: BTreeSet::new(),
        };
        let word = |n| object.word(n).load(Ordering::Acquire);
        let valid = match key {
            ObjectKey::Mount(_) | ObjectKey::Shared(_) => {
                word(0) == STORE_MAGIC
                    && word(2) == SECTION_SIZE as u64
                    && word(3) == domain
                    && word(4) == key.id()
            }
            ObjectKey::Time(id) => word(0) == TIME_MAGIC && word(1) == domain && word(2) == id,
            ObjectKey::Bpf => word(0) == BPF_MAGIC,
            ObjectKey::ProcessTable => word(0) == PROCESS_TABLE_MAGIC,
            ObjectKey::CgroupJob(_) => unreachable!(),
            ObjectKey::MountPolicy { .. } => word(0) == POLICY_MAGIC,
            ObjectKey::Sysv { kind, id, .. } => {
                let (magic, offset) = match kind {
                    IpcKind::Memory => (SHM_MAGIC, 68),
                    IpcKind::Semaphore => (SEM_MAGIC, 40),
                };
                word(0) == magic && object.u32_at(offset) == id as u32
            }
        };
        if !valid {
            return Err(error(
                ErrorCode::InvalidRequest,
                "kernel section identity mismatch",
            ));
        }
        Ok(object)
    }
    fn u32_at(&self, offset: usize) -> u32 {
        unsafe {
            &*self
                .header
                .Value
                .cast::<u8>()
                .add(offset)
                .cast::<std::sync::atomic::AtomicU32>()
        }
        .load(Ordering::Acquire)
    }
    fn removed(&self, key: ObjectKey) -> bool {
        match key {
            ObjectKey::Sysv { kind, .. } => {
                self.u32_at(match kind {
                    IpcKind::Memory => 64,
                    IpcKind::Semaphore => 36,
                }) != 0
            }
            _ => false,
        }
    }
    fn externally_referenced(&self) -> bool {
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
        // PUBLIC_OBJECT_BASIC_INFORMATION; HandleCount excludes mapped views.
        // Every worker Store and namespace descriptor owns a section handle.
        let mut info = [0u32; 14];
        let status = unsafe {
            NtQueryObject(
                self.section.as_raw_handle(),
                0,
                info.as_mut_ptr().cast(),
                size_of_val(&info) as u32,
                ptr::null_mut(),
            )
        };
        status < 0 || info[2] > 1 // Keep conservatively if the native query fails.
    }
    fn stage(&self, revision: u64, bytes: &[u8]) -> Result<()> {
        if bytes.len() > BANK_SIZE - 8 {
            return Err(error(ErrorCode::LimitExceeded, "mount table is too large"));
        }
        let bank = unsafe {
            self.header
                .Value
                .cast::<u8>()
                .add(HEADER_SIZE + ((revision as usize + 1) & 1) * BANK_SIZE)
        };
        if unsafe {
            VirtualAlloc(
                bank.cast(),
                8 + bytes.len().next_multiple_of(8),
                MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .is_null()
        {
            return Err(error(
                ErrorCode::Internal,
                "cannot commit mount table pages",
            ));
        }
        for (index, chunk) in bytes.chunks(8).enumerate() {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            unsafe { &*bank.add(8 + index * 8).cast::<AtomicU64>() }
                .store(u64::from_le_bytes(word), Ordering::Release);
        }
        unsafe { &*bank.cast::<AtomicU64>() }.store(bytes.len() as u64, Ordering::Release);
        Ok(())
    }
}

pub struct Kernel {
    domain: u64,
    resources: resources::Resources,
    objects: BTreeMap<ObjectKey, Object>,
    directory_watches: BTreeMap<(u64, u64), kinakaze_v2_host_win::DirectoryWatch>,
}
impl Kernel {
    pub fn new(domain: u64) -> Self {
        Self {
            domain,
            resources: resources::Resources::default(),
            objects: BTreeMap::new(),
            directory_watches: BTreeMap::new(),
        }
    }
    pub fn handle(
        &mut self,
        peer: kinakaze_v2_manager::PeerIdentity,
        command: &KernelCommand,
    ) -> Result<Vec<u64>> {
        if std::env::var_os("KINAKAZE_KERNEL_TRACE").is_some() {
            eprintln!("kernel peer={} {command:?}", peer.host_pid);
        }
        match command {
            KernelCommand::ResourceOwner => return resources::owner(),
            KernelCommand::RetainResources { owner, id, input } => {
                if self.objects.contains_key(&ObjectKey::Shared(*owner)) {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "resource owner must have an independent lifetime",
                    ));
                }
                let input = match input {
                    ResourceInput::Inline(bytes) if bytes.len() <= MAX_INLINE_RESOURCES => {
                        std::borrow::Cow::Borrowed(bytes.as_slice())
                    }
                    ResourceInput::Inline(_) => {
                        return Err(error(
                            ErrorCode::LimitExceeded,
                            "inline resource payload is too large",
                        ));
                    }
                    ResourceInput::Section { source, length } => {
                        std::borrow::Cow::Owned(transfer(peer, *source, *length)?)
                    }
                };
                self.resources
                    .retain(self.domain, peer, *owner, *id, &input)?;
            }
            KernelCommand::Resources { owner, id } => return self.resources.handles(*owner, *id),
            KernelCommand::ResourceSocket { owner, id, index } => {
                return self.resources.socket(peer, *owner, *id, *index);
            }
            KernelCommand::ReleaseResources { owner, id } => self.resources.release(*owner, *id),
            KernelCommand::WatchDirectory { owner, watch, path } => {
                let _owner = open(&ObjectKey::Shared(*owner).name(self.domain))?;
                if *watch == 0 || path.len() > 32768 {
                    return Err(error(ErrorCode::InvalidRequest, "invalid directory watch"));
                }
                if !self.directory_watches.contains_key(&(*owner, *watch)) {
                    if self.directory_watches.len() >= 16384 {
                        self.collect();
                        if self.directory_watches.len() >= 16384 {
                            return Err(error(
                                ErrorCode::LimitExceeded,
                                "directory watch limit reached",
                            ));
                        }
                    }
                    let monitor = kinakaze_v2_host_win::DirectoryWatch::start(
                        self.domain,
                        *watch,
                        std::path::Path::new(path),
                    )
                    .map_err(|e| {
                        error(
                            ErrorCode::Internal,
                            &format!("cannot monitor directory: {e}"),
                        )
                    })?;
                    self.directory_watches.insert((*owner, *watch), monitor);
                }
            }
            KernelCommand::RemoveDirectoryWatch { owner, watch } => {
                self.directory_watches.remove(&(*owner, *watch));
            }
            KernelCommand::Unlease { object, owner } => {
                if let Some(object) = self.objects.get_mut(object) {
                    object.leases.remove(owner);
                }
            }
            KernelCommand::RemoveCgroup { id } => {
                if *id == 0 {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "cannot remove root cgroup",
                    ));
                }
                self.remove(ObjectKey::CgroupJob(*id));
            }
            KernelCommand::RemoveIpc { object } => {
                if !matches!(object, ObjectKey::Sysv { .. }) {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "only IPC objects can be removed",
                    ));
                }
                if let Some(retained) = self.objects.get(object) {
                    if !retained.removed(*object) {
                        return Err(error(ErrorCode::Conflict, "IPC object is still live"));
                    }
                }
                self.remove(*object);
            }
            KernelCommand::Lease { object, owner } => {
                if self.objects.contains_key(owner) || !matches!(owner, ObjectKey::Shared(_)) {
                    return Err(error(
                        ErrorCode::InvalidRequest,
                        "lease must have an independent native lifetime",
                    ));
                }
                let _token = open(&owner.name(self.domain))?;
                self.objects
                    .get_mut(object)
                    .ok_or_else(|| error(ErrorCode::NotFound, "lease target is unavailable"))?
                    .leases
                    .insert(*owner);
            }
            KernelCommand::MountNamespaces => {
                return Ok(self
                    .objects
                    .keys()
                    .filter_map(|key| match key {
                        ObjectKey::Mount(id) => Some(*id),
                        _ => None,
                    })
                    .collect());
            }
            KernelCommand::Retain {
                object,
                tmpfs,
                dependencies,
            } => self.retain(*object, *tmpfs, dependencies)?,
            KernelCommand::PublishMounts {
                topology,
                source,
                length,
                updates,
            } => {
                let input = transfer(peer, *source, *length)?;
                self.publish(*topology, updates, &input)?;
            }
        }
        Ok(Vec::new())
    }
    fn check_dependencies(&self, dependencies: &[ObjectKey]) -> Result<()> {
        if dependencies.len() > 4096 {
            return Err(error(
                ErrorCode::LimitExceeded,
                "too many kernel references",
            ));
        }
        if dependencies
            .iter()
            .any(|key| !self.objects.contains_key(key))
        {
            return Err(error(
                ErrorCode::NotFound,
                "kernel dependency has not been retained",
            ));
        }
        Ok(())
    }
    fn retain(&mut self, key: ObjectKey, tmpfs: bool, dependencies: &[ObjectKey]) -> Result<()> {
        // Hidden namespace filesystems have no mount attachment (id zero).
        let invalid_id = key.id() == 0 && !matches!(key, ObjectKey::MountPolicy { .. });
        if invalid_id || (tmpfs && !matches!(key, ObjectKey::Shared(_))) {
            return Err(error(ErrorCode::InvalidRequest, "invalid kernel object"));
        }
        if !self.objects.contains_key(&key) && self.objects.len() >= 16384 {
            self.collect();
        }
        self.check_dependencies(dependencies)?;
        // Acquire all handles before publishing a reference. Failed requests
        // leave the existing graph untouched, including repeated registrations.
        let backing = if tmpfs && self.objects.get(&key).is_none_or(|o| o.backing.is_none()) {
            Some(open(&format!(
                r"Local\kinakaze.tmpfs.v1.{}.{}",
                self.domain,
                key.id()
            ))?)
        } else {
            None
        };
        if !self.objects.contains_key(&key) {
            if self.objects.len() >= 16384 {
                return Err(error(
                    ErrorCode::LimitExceeded,
                    "kernel object limit reached",
                ));
            }
            let object = Object::open(self.domain, key)?;
            self.objects.insert(key, object);
        }
        let object = self.objects.get_mut(&key).unwrap();
        if backing.is_some() {
            object.backing = backing;
        }
        object.dependencies.extend(dependencies);
        Ok(())
    }
    fn publish(&mut self, topology: u64, updates: &[MountPublication], input: &[u8]) -> Result<()> {
        if updates.is_empty() || updates.len() > 512 {
            return Err(error(
                ErrorCode::InvalidRequest,
                "invalid mount transaction size",
            ));
        }
        let root = self
            .objects
            .get(&ObjectKey::Mount(1))
            .ok_or_else(|| error(ErrorCode::NotFound, "root namespace is unavailable"))?;
        let epoch = root.word(TOPOLOGY_WORD).load(Ordering::SeqCst);
        if epoch != topology || epoch & 1 != 0 || epoch > u64::MAX - 2 {
            return Err(error(ErrorCode::Conflict, "invalid topology publication"));
        }
        let mut seen = BTreeSet::new();
        let mut dependencies = Vec::with_capacity(updates.len());
        let mut cursor = input;
        let mut tables = Vec::with_capacity(updates.len());
        for update in updates {
            if update.expected == u64::MAX || !seen.insert(update.namespace) {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "invalid mount publication",
                ));
            }
            let object = self
                .objects
                .get(&ObjectKey::Mount(update.namespace))
                .ok_or_else(|| error(ErrorCode::NotFound, "mount namespace is unavailable"))?;
            if object.word(1).load(Ordering::SeqCst) != update.expected {
                return Err(error(ErrorCode::Conflict, "stale mount publication"));
            }
            self.check_dependencies(&update.dependencies)?;
            // Allocate before the irreversible publication boundary.
            dependencies.push(update.dependencies.iter().copied().collect::<BTreeSet<_>>());
            let length = u64::from_le_bytes(
                cursor
                    .get(..8)
                    .ok_or_else(|| error(ErrorCode::InvalidRequest, "truncated mount transfer"))?
                    .try_into()
                    .unwrap(),
            );
            if length > (BANK_SIZE - 8) as u64 || length as usize > cursor.len() - 8 {
                return Err(error(
                    ErrorCode::InvalidRequest,
                    "invalid mount transfer length",
                ));
            }
            tables.push(&cursor[8..8 + length as usize]);
            cursor = &cursor[8 + length as usize..];
        }
        if !cursor.is_empty() {
            return Err(error(
                ErrorCode::InvalidRequest,
                "trailing mount transfer data",
            ));
        }
        for (update, bytes) in updates.iter().zip(tables) {
            self.objects[&ObjectKey::Mount(update.namespace)].stage(update.expected, bytes)?;
        }
        root.word(TOPOLOGY_WORD).store(epoch + 1, Ordering::SeqCst);
        for (update, dependencies) in updates.iter().zip(dependencies) {
            let object = self
                .objects
                .get_mut(&ObjectKey::Mount(update.namespace))
                .unwrap();
            object.dependencies = dependencies;
            object.word(1).store(update.expected + 1, Ordering::SeqCst);
        }
        self.objects[&ObjectKey::Mount(1)]
            .word(TOPOLOGY_WORD)
            .store(epoch + 2, Ordering::SeqCst);
        Ok(())
    }
    /// Native handles cover fork, exec and namespace FDs without per-read RPCs.
    /// Graph tracing retains mounted resources and collects unreachable cycles.
    /// Only metadata/namespace sections enter this graph, never pipe/socket ends.
    pub fn collect(&mut self) {
        self.resources.collect(self.domain);
        // One weak token can lease several resources. Probe it only once per
        // sweep; retaining the boolean never extends its native lifetime.
        let mut leases = BTreeMap::new();
        let domain = self.domain;
        let mut alive = |key| {
            *leases
                .entry(key)
                .or_insert_with(|| lease_alive(domain, key))
        };
        self.directory_watches
            .retain(|(owner, _), _| alive(ObjectKey::Shared(*owner)));
        if let Some(ids) = self.cgroup_ids() {
            let stale: Vec<_> = self
                .objects
                .iter()
                .filter_map(|(key, object)| match key {
                    ObjectKey::CgroupJob(id)
                        if !ids.contains(id) && !object.externally_referenced() =>
                    {
                        Some(*key)
                    }
                    _ => None,
                })
                .collect();
            for key in stale {
                self.remove(key);
            }
        }
        // A worker can die after marking IPC_RMID but before its release RPC.
        let removed: Vec<_> = self
            .objects
            .iter()
            .filter_map(|(key, object)| object.removed(*key).then_some(*key))
            .collect();
        for key in removed {
            self.remove(key);
        }
        for object in self.objects.values_mut() {
            object.leases.retain(|key| alive(*key));
        }
        let mut live = BTreeSet::new();
        let mut pending: Vec<_> = self
            .objects
            .iter()
            .filter_map(|(key, object)| {
                (key.is_root() || !object.leases.is_empty()).then_some(*key)
            })
            .collect();
        let trace = |live: &mut BTreeSet<_>, pending: &mut Vec<_>| {
            while let Some(key) = pending.pop() {
                if live.insert(key) {
                    if let Some(object) = self.objects.get(&key) {
                        pending.extend(object.dependencies.iter().copied());
                    }
                }
            }
        };
        trace(&mut live, &mut pending);
        // Mounted graphs are already live. Query native reference counts only
        // for the remaining components, tracing each as soon as it is pinned.
        for (key, object) in &self.objects {
            if !live.contains(key) && object.externally_referenced() {
                pending.push(*key);
                trace(&mut live, &mut pending);
            }
        }
        self.objects.retain(|key, _| {
            let retained = live.contains(key);
            if !retained && std::env::var_os("KINAKAZE_KERNEL_TRACE").is_some() {
                eprintln!("kernel collect {key:?}");
            }
            retained
        });
    }
    fn remove(&mut self, key: ObjectKey) {
        self.objects.remove(&key);
        for object in self.objects.values_mut() {
            object.dependencies.remove(&key);
        }
    }
    fn cgroup_ids(&self) -> Option<BTreeSet<u64>> {
        let catalog = self.objects.get(&ObjectKey::Shared(CGROUP_CATALOG))?;
        let revision = catalog.word(1).load(Ordering::Acquire);
        let bank = unsafe {
            catalog
                .header
                .Value
                .cast::<u8>()
                .add(HEADER_SIZE + (revision as usize & 1) * BANK_SIZE)
        };
        let word =
            |index: usize| unsafe { &*bank.cast::<AtomicU64>().add(index) }.load(Ordering::Acquire);
        let length = word(0);
        if length < 16 || length > (BANK_SIZE - 8) as u64 || word(1) != CGROUP_MAGIC {
            return None;
        }
        let count = word(2);
        if count > 100_000 || count > (length - 16) / 8 {
            return None;
        }
        let ids = (0..count as usize).map(|index| word(index + 3)).collect();
        (revision == catalog.word(1).load(Ordering::Acquire)).then_some(ids)
    }
}

fn transfer(peer: kinakaze_v2_manager::PeerIdentity, source: u64, length: u64) -> Result<Vec<u8>> {
    if length == 0 || length > 64 * 1024 * 1024 {
        return Err(error(
            ErrorCode::LimitExceeded,
            "invalid mount transfer size",
        ));
    }
    let process = kinakaze_v2_host_win::ProcessHandle::open(peer.host_pid)
        .map_err(|_| error(ErrorCode::NotFound, "mount publisher exited"))?;
    if process.birth() != peer.birth {
        return Err(error(ErrorCode::Unauthorized, "mount publisher changed"));
    }
    let section = process
        .duplicate_object(source)
        .map_err(|_| error(ErrorCode::InvalidRequest, "invalid transfer section"))?;
    let view = unsafe {
        MapViewOfFile(
            section.as_raw_handle(),
            FILE_MAP_READ,
            0,
            0,
            length as usize,
        )
    };
    if view.Value.is_null() {
        return Err(error(
            ErrorCode::InvalidRequest,
            "cannot map transfer section",
        ));
    }
    let mut offset = 0;
    while offset < length as usize {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let address = unsafe { view.Value.cast::<u8>().add(offset) };
        let queried = unsafe { VirtualQuery(address.cast(), &mut info, size_of_val(&info)) };
        if queried == 0
            || info.State != MEM_COMMIT
            || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
        {
            unsafe {
                UnmapViewOfFile(view);
            }
            return Err(error(
                ErrorCode::InvalidRequest,
                "unreadable transfer section",
            ));
        }
        offset += info.RegionSize.min(length as usize - offset);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(view.Value.cast::<u8>(), length as usize) }.to_vec();
    unsafe {
        UnmapViewOfFile(view);
    }
    Ok(bytes)
}

pub fn start_collection(service: &std::sync::Arc<super::Service>) -> io::Result<()> {
    let weak = std::sync::Arc::downgrade(service);
    std::thread::Builder::new()
        .name("kernel-objects".into())
        .spawn(move || {
            while let Some(service) = weak.upgrade() {
                let manager = service.manager.lock().unwrap();
                if service.stopping.load(Ordering::Acquire) {
                    break;
                }
                let (manager, _) = service
                    .changed
                    .wait_timeout_while(manager, std::time::Duration::from_secs(1), |_| {
                        !service.stopping.load(Ordering::Acquire)
                    })
                    .unwrap();
                drop(manager);
                service.kernel.lock().unwrap().collect();
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn domain() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        (u64::from(std::process::id()) << 32) | NEXT.fetch_add(1, Ordering::Relaxed)
    }
    fn create(domain: u64, key: ObjectKey) -> OwnedHandle {
        let name: Vec<_> = key.name(domain).encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE | SEC_RESERVE,
                0,
                SECTION_SIZE as u32,
                name.as_ptr(),
            )
        };
        assert!(!handle.is_null());
        let section = unsafe { OwnedHandle::from_raw_handle(handle) };
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, HEADER_SIZE) };
        assert!(!view.Value.is_null());
        assert!(
            !unsafe { VirtualAlloc(view.Value, HEADER_SIZE, MEM_COMMIT, PAGE_READWRITE) }.is_null()
        );
        for (index, value) in [
            STORE_MAGIC,
            0,
            SECTION_SIZE as u64,
            domain,
            key.id(),
            2,
            1,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            unsafe { &*view.Value.cast::<AtomicU64>().add(index) }.store(value, Ordering::SeqCst);
        }
        unsafe {
            UnmapViewOfFile(view);
        }
        section
    }
    fn table(bytes: &[u8]) -> Vec<u8> {
        [&(bytes.len() as u64).to_le_bytes()[..], bytes].concat()
    }
    fn update(namespace: u64, expected: u64, dependencies: Vec<ObjectKey>) -> MountPublication {
        MountPublication {
            namespace,
            expected,
            dependencies,
        }
    }
    fn native(domain: u64, key: ObjectKey) -> OwnedHandle {
        let section = create(domain, key);
        let view = unsafe { MapViewOfFile(section.as_raw_handle(), FILE_MAP_ALL_ACCESS, 0, 0, 72) };
        assert!(!view.Value.is_null());
        let word = |index: usize, value: u64| unsafe {
            (&*view.Value.cast::<AtomicU64>().add(index)).store(value, Ordering::Release);
        };
        match key {
            ObjectKey::Time(id) => {
                word(0, TIME_MAGIC);
                word(1, domain);
                word(2, id);
            }
            ObjectKey::Bpf => word(0, BPF_MAGIC),
            ObjectKey::ProcessTable => word(0, PROCESS_TABLE_MAGIC),
            ObjectKey::MountPolicy { .. } => word(0, POLICY_MAGIC),
            ObjectKey::Sysv { kind, id, .. } => {
                let (magic, offset) = match kind {
                    IpcKind::Memory => (SHM_MAGIC, 68),
                    IpcKind::Semaphore => (SEM_MAGIC, 40),
                };
                word(0, magic);
                // Clear both removal markers before writing the key.
                unsafe {
                    for offset in [36, 64] {
                        (&*view
                            .Value
                            .cast::<u8>()
                            .add(offset)
                            .cast::<std::sync::atomic::AtomicU32>())
                            .store(0, Ordering::Release);
                    }
                    (&*view
                        .Value
                        .cast::<u8>()
                        .add(offset)
                        .cast::<std::sync::atomic::AtomicU32>())
                        .store(id as u32, Ordering::Release);
                }
            }
            _ => unreachable!(),
        }
        unsafe {
            UnmapViewOfFile(view);
        }
        section
    }
    #[test]
    fn process_table_survives_an_empty_worker_pool() {
        let domain = domain();
        let key = ObjectKey::ProcessTable;
        let creator = native(domain, key);
        let mut kernel = Kernel::new(domain);
        kernel.retain(key, false, &[]).unwrap();
        kernel.objects[&key].word(2).store(12345, Ordering::Release);
        drop(creator);
        kernel.collect();
        let reopened = Object::open(domain, key).unwrap();
        assert_eq!(reopened.word(2).load(Ordering::Acquire), 12345);
        assert!(open(&key.name(domain + 1)).is_err());
        drop(reopened);
        drop(kernel);
        assert!(open(&key.name(domain)).is_err());
    }
    #[test]
    fn ipc_namespace_retains_data_and_recovers_a_crashed_removal() {
        for kind in [IpcKind::Memory, IpcKind::Semaphore] {
            let domain = domain();
            let ipc = ObjectKey::Shared(u64::MAX - 1);
            let object = ObjectKey::Sysv {
                kind,
                namespace: 1,
                id: 42,
            };
            let owner = create(domain, ipc);
            let creator = native(domain, object);
            let mut kernel = Kernel::new(domain);
            kernel.retain(ipc, false, &[]).unwrap();
            kernel.retain(object, false, &[ipc]).unwrap();
            kernel.retain(ipc, false, &[object]).unwrap();
            drop(owner);
            drop(creator);
            kernel.collect();
            assert!(open(&object.name(domain)).is_ok());
            let attachment = open(&object.name(domain)).unwrap();
            let offset = match kind {
                IpcKind::Memory => 64,
                IpcKind::Semaphore => 36,
            };
            unsafe {
                (&*kernel.objects[&object]
                    .header
                    .Value
                    .cast::<u8>()
                    .add(offset)
                    .cast::<std::sync::atomic::AtomicU32>())
                    .store(1, Ordering::Release);
            }
            kernel.collect(); // No RemoveIpc RPC: the writer died first.
            assert!(!kernel.objects.contains_key(&object));
            assert!(!kernel.objects[&ipc].dependencies.contains(&object));
            assert!(open(&object.name(domain)).is_ok()); // Existing attachments survive.
            drop(attachment);
            assert!(open(&object.name(domain)).is_err());
        }
    }
    #[test]
    fn native_registries_are_domain_scoped_and_namespace_fds_pin_their_owner() {
        let domain = domain();
        let mut kernel = Kernel::new(domain);
        let bpf = native(domain, ObjectKey::Bpf);
        let clock = native(domain, ObjectKey::Time(1));
        for key in [ObjectKey::Bpf, ObjectKey::Time(1)] {
            kernel.retain(key, false, &[]).unwrap();
        }
        drop((bpf, clock));
        kernel.collect();
        assert!(open(&ObjectKey::Bpf.name(domain)).is_ok());
        assert!(open(&ObjectKey::Bpf.name(domain + 1)).is_err());
        let owner = ObjectKey::Shared(30);
        let creator = create(domain, owner);
        let descriptor = native(domain, ObjectKey::Time(2));
        kernel.retain(owner, false, &[]).unwrap();
        kernel.retain(ObjectKey::Time(2), false, &[owner]).unwrap();
        drop(creator);
        kernel.collect();
        assert!(open(&owner.name(domain)).is_ok());
        drop(descriptor);
        kernel.collect();
        assert!(!kernel.objects.contains_key(&owner));
        assert!(!kernel.objects.contains_key(&ObjectKey::Time(2)));
        assert!(kernel.objects.contains_key(&ObjectKey::Time(1)));
    }
    #[test]
    fn native_cgroup_accounting_survives_workers_and_releases_on_rmdir() {
        use windows_sys::Win32::System::JobObjects::{CreateJobObjectW, OpenJobObjectW};
        use windows_sys::Win32::System::SystemServices::JOB_OBJECT_QUERY;
        let domain = domain();
        let key = ObjectKey::CgroupJob(7);
        let name: Vec<_> = key.name(domain).encode_utf16().chain(Some(0)).collect();
        let handle = unsafe { CreateJobObjectW(ptr::null(), name.as_ptr()) };
        assert!(!handle.is_null());
        let catalog_key = ObjectKey::Shared(CGROUP_CATALOG);
        let catalog = create(domain, catalog_key);
        let mut kernel = Kernel::new(domain);
        kernel.retain(catalog_key, false, &[]).unwrap();
        let publish = |kernel: &Kernel, revision: u64, ids: &[u64]| {
            let mut bytes = CGROUP_MAGIC.to_le_bytes().to_vec();
            bytes.extend_from_slice(&(ids.len() as u64).to_le_bytes());
            for id in ids {
                bytes.extend_from_slice(&id.to_le_bytes());
            }
            kernel.objects[&catalog_key]
                .stage(revision, &bytes)
                .unwrap();
            kernel.objects[&catalog_key]
                .word(1)
                .store(revision + 1, Ordering::Release);
        };
        publish(&kernel, 0, &[7]);
        kernel.retain(key, false, &[]).unwrap();
        unsafe { CloseHandle(handle) };
        drop(catalog);
        kernel.collect();
        let reopened = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
        assert!(!reopened.is_null());
        unsafe { CloseHandle(reopened) };
        // Simulate a remover dying after publication and before RemoveCgroup.
        publish(&kernel, 1, &[]);
        kernel.collect();
        assert!(unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) }.is_null());
    }
    #[test]
    fn mount_publication_keeps_policy_until_unmount() {
        let domain = domain();
        let key = ObjectKey::MountPolicy {
            namespace: 1,
            id: 0,
        };
        let root = create(domain, ObjectKey::Mount(1));
        let policy = native(domain, key);
        let mut kernel = Kernel::new(domain);
        kernel.retain(ObjectKey::Mount(1), false, &[]).unwrap();
        kernel.retain(key, false, &[]).unwrap();
        kernel
            .publish(0, &[update(1, 0, vec![key])], &table(b"mount"))
            .unwrap();
        drop(policy);
        drop(root);
        kernel.collect();
        assert!(open(&key.name(domain)).is_ok());
        kernel
            .publish(2, &[update(1, 1, vec![])], &table(b"unmount"))
            .unwrap();
        kernel.collect();
        assert!(open(&key.name(domain)).is_err());
    }
    #[test]
    fn mounted_object_survives_creator_then_collects_after_unmount() {
        let domain = domain();
        let root = ObjectKey::Mount(1);
        let volume = ObjectKey::Shared(23);
        let root_creator = create(domain, root);
        let volume_creator = create(domain, volume);
        let mut kernel = Kernel::new(domain);
        kernel.retain(root, false, &[]).unwrap();
        kernel.retain(volume, false, &[]).unwrap();
        kernel
            .publish(0, &[update(1, 0, vec![volume])], &table(b"mount"))
            .unwrap();
        drop(volume_creator);
        drop(root_creator);
        kernel.collect();
        assert!(open(&volume.name(domain)).is_ok());
        kernel
            .publish(2, &[update(1, 1, vec![])], &table(b"unmount"))
            .unwrap();
        kernel.collect();
        assert!(open(&volume.name(domain)).is_err());
        assert!(open(&root.name(domain)).is_ok());
        drop(kernel);
        assert!(open(&root.name(domain)).is_err());
    }
    #[test]
    fn stale_or_malformed_batch_never_publishes_a_partial_topology() {
        let domain = domain();
        let _root = create(domain, ObjectKey::Mount(1));
        let _child = create(domain, ObjectKey::Mount(2));
        let mut kernel = Kernel::new(domain);
        for id in [1, 2] {
            kernel.retain(ObjectKey::Mount(id), false, &[]).unwrap();
        }
        let input = [table(b"first"), table(b"second")].concat();
        assert!(
            kernel
                .publish(0, &[update(1, 0, vec![]), update(2, 9, vec![])], &input)
                .is_err()
        );
        assert_eq!(
            kernel.objects[&ObjectKey::Mount(1)]
                .word(1)
                .load(Ordering::SeqCst),
            0
        );
        assert!(
            kernel
                .publish(
                    0,
                    &[update(1, 0, vec![]), update(2, 0, vec![])],
                    &input[..input.len() - 1]
                )
                .is_err()
        );
        assert_eq!(
            kernel.objects[&ObjectKey::Mount(1)]
                .word(1)
                .load(Ordering::SeqCst),
            0
        );
        kernel
            .publish(0, &[update(1, 0, vec![]), update(2, 0, vec![])], &input)
            .unwrap();
        // Even matching per-namespace versions cannot authorize a computation
        // that observed peers before another topology transaction completed.
        assert!(
            kernel
                .publish(0, &[update(2, 1, vec![])], &table(b"stale peer view"))
                .is_err()
        );
        for id in [1, 2] {
            assert_eq!(
                kernel.objects[&ObjectKey::Mount(id)]
                    .word(1)
                    .load(Ordering::SeqCst),
                1
            );
        }
        assert_eq!(
            kernel.objects[&ObjectKey::Mount(1)]
                .word(TOPOLOGY_WORD)
                .load(Ordering::SeqCst),
            2
        );
    }
    #[test]
    fn namespace_descriptor_pins_graph_and_unreachable_cycles_are_collected() {
        let domain = domain();
        let a = ObjectKey::Mount(2);
        let b = ObjectKey::Shared(3);
        let descriptor = create(domain, a);
        let creator = create(domain, b);
        let mut kernel = Kernel::new(domain);
        kernel.retain(a, false, &[]).unwrap();
        kernel.retain(b, false, &[a]).unwrap();
        kernel.retain(a, false, &[b]).unwrap();
        drop(creator);
        kernel.collect();
        assert_eq!(kernel.objects.len(), 2);
        drop(descriptor);
        kernel.collect();
        assert!(kernel.objects.is_empty());
    }
    #[test]
    fn failed_dependency_does_not_register_an_object() {
        let domain = domain();
        let _creator = create(domain, ObjectKey::Shared(2));
        let mut kernel = Kernel::new(domain);
        assert!(
            kernel
                .retain(ObjectKey::Shared(2), false, &[ObjectKey::Shared(99)])
                .is_err()
        );
        assert!(kernel.objects.is_empty());
    }

    #[test]
    fn open_description_lease_keeps_data_without_pinning_its_last_handle() {
        let domain = domain();
        let data = ObjectKey::Shared(20);
        let fd = ObjectKey::Shared(21);
        let creator = create(domain, data);
        let description = create(domain, fd);
        let mut kernel = Kernel::new(domain);
        kernel.retain(data, false, &[]).unwrap();
        kernel
            .handle(
                kinakaze_v2_manager::PeerIdentity {
                    host_pid: 1,
                    birth: 1,
                },
                &KernelCommand::Lease {
                    object: data,
                    owner: fd,
                },
            )
            .unwrap();
        drop(creator);
        kernel.collect();
        assert!(open(&data.name(domain)).is_ok());
        drop(description);
        assert!(
            open(&fd.name(domain)).is_err(),
            "init must not prevent final close"
        );
        kernel.collect();
        assert!(open(&data.name(domain)).is_err());
    }
}
