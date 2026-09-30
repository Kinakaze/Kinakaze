//! Init owns unused native bootstraps and uncommitted fork candidates. Workers
//! are consumed once; guest memory and logical identities are never recycled.
use super::Service;
use kinakaze_v2_host_win::{ProcessHandle, RemoteTransfer};
use kinakaze_v2_manager::PeerIdentity;
use kinakaze_v2_protocol::native_fork::{Spec, Worker};
use std::{
    fs::{File, OpenOptions},
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::FILE_SHARE_READ,
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
        },
        Threading::*,
    },
};

const CAPACITY: usize = 2;
const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Pool {
    enabled: bool,
    root: PathBuf,
    dist: PathBuf,
    state: Mutex<State>,
    creating: Mutex<()>,
}

#[derive(Default)]
struct State {
    template: Option<Arc<Template>>,
    slots: Vec<Standby>,
    leases: Vec<Lease>,
    failures: usize,
}
struct Template {
    spec: Spec,
    _pins: Vec<File>,
}
struct Lease {
    transaction: u64,
    parent: ProcessHandle,
    child: Standby,
}
impl State {
    fn publish(&mut self, template: &Arc<Template>, stopping: bool, result: io::Result<Standby>) {
        // An obsolete or shutdown-racing candidate is still exclusively owned
        // here. Dropping it kills the process; no guest ever receives it.
        if stopping
            || self.slots.len() >= CAPACITY
            || !self
                .template
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, template))
        {
            return;
        }
        match result {
            Ok(slot) => {
                if trace() {
                    eprintln!("init native fork: preparing pid={}", slot.peer.host_pid);
                }
                self.slots.push(slot);
            }
            Err(error) => {
                if trace() {
                    eprintln!("init native fork: preparation failed: {:?}", error.kind());
                }
                self.failures += 1;
            }
        }
    }

    fn reap_leases(&mut self, manager: &kinakaze_v2_manager::StateManager) {
        self.leases.retain_mut(|lease| {
            match manager.native_fork_state(lease.transaction, lease.child.peer) {
                Some(true) => {
                    lease.child.kill = false;
                    false
                }
                Some(false) => !lease.child.exited() && !lease.parent.has_exited().unwrap_or(true),
                None => false,
            }
        });
    }
}
struct Standby {
    process: OwnedHandle,
    thread: OwnedHandle,
    ready: OwnedHandle,
    activate: OwnedHandle,
    parked: OwnedHandle,
    peer: PeerIdentity,
    tid: u32,
    since: Instant,
    kill: bool,
}
impl Standby {
    fn exited(&self) -> bool {
        unsafe {
            WaitForSingleObject(self.process.as_raw_handle(), 0)
                != windows_sys::Win32::Foundation::WAIT_TIMEOUT
        }
    }
    fn prepared(&self) -> bool {
        unsafe { WaitForSingleObject(self.parked.as_raw_handle(), 0) == WAIT_OBJECT_0 }
    }
}
impl Drop for Standby {
    fn drop(&mut self) {
        if self.kill {
            unsafe {
                TerminateProcess(self.process.as_raw_handle(), 125);
            }
        }
    }
}

impl Pool {
    pub fn cancel_delivery(&self, peer: PeerIdentity, transaction: u64) {
        self.state.lock().unwrap().leases.retain(|lease| {
            lease.transaction != transaction
                || lease.parent.pid() != peer.host_pid
                || lease.parent.birth() != peer.birth
        });
    }

    pub fn new(root: PathBuf, dist: PathBuf) -> Self {
        Self {
            enabled: std::env::var_os("KINAKAZE_FORK_POOL").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            root,
            dist,
            state: Mutex::new(State::default()),
            creating: Mutex::new(()),
        }
    }

    pub fn lock_creation(&self) -> MutexGuard<'_, ()> {
        self.creating.lock().unwrap()
    }

    fn prepare(
        &self,
        stopping: &AtomicBool,
        create: impl FnOnce() -> io::Result<Standby>,
    ) -> Option<io::Result<Standby>> {
        // stop holds this gate before setting stopping. Creation and Job
        // assignment finish before main may return, without blocking RPCs on
        // the manager mutex. Never acquire manager/state while holding the gate.
        let _creation = self.lock_creation();
        if stopping.load(Ordering::Acquire) {
            return None;
        }
        Some(create())
    }

    fn template(&self, spec: &Spec) -> io::Result<Option<Template>> {
        if !spec.valid() {
            return Ok(None);
        }
        let directory = kinakaze_v2_bridge::native::directory(&self.dist).canonicalize()?;
        let mut pins = Vec::new();
        for module in &spec.modules {
            let path = Path::new(&module.path).canonicalize()?;
            if path.parent() != Some(directory.as_path()) {
                if trace() {
                    eprintln!(
                        "init native fork: module outside native distribution: {}",
                        path.display()
                    );
                }
                return Ok(None);
            }
            // Hold each module against replacement while unused workers exist.
            pins.push(
                OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ)
                    .open(path)?,
            );
        }
        Ok(Some(Template {
            spec: spec.clone(),
            _pins: pins,
        }))
    }

    pub fn take(
        &self,
        peer: PeerIdentity,
        transaction: u64,
        spec: &Spec,
    ) -> io::Result<Option<(Worker, RemoteTransfer)>> {
        if !self.enabled {
            return Ok(None);
        }
        let mut state = self.state.lock().unwrap();
        if state.failures >= 3 {
            return Ok(None);
        }
        if !state.template.as_ref().is_some_and(|t| t.spec == *spec) {
            let Some(template) = self.template(spec)? else {
                return Ok(None);
            };
            state.slots.clear();
            state.template = Some(Arc::new(template));
            if trace() {
                eprintln!("init native fork: template modules={}", spec.modules.len());
            }
        }
        // The same transaction may retry only after its old native candidate died.
        if state
            .leases
            .iter()
            .any(|l| l.transaction == transaction && !l.child.exited())
        {
            return Ok(None);
        }
        if state.leases.len() >= 128 {
            return Ok(None);
        }
        let Some(index) = state.slots.iter().position(|s| !s.exited() && s.prepared()) else {
            return Ok(None);
        };
        let parent = ProcessHandle::open(peer.host_pid)?;
        if parent.birth() != peer.birth {
            return Err(io::Error::other("fork parent identity changed"));
        }
        let target = ProcessHandle::open(peer.host_pid)?;
        if target.birth() != peer.birth {
            return Err(io::Error::other("fork recipient identity changed"));
        }
        let slot = state.slots.swap_remove(index);
        let mut transfer = RemoteTransfer::new(target);
        for handle in [&slot.process, &slot.thread, &slot.ready, &slot.activate] {
            transfer.add(handle.as_raw_handle(), 0, true)?;
        }
        let worker = Worker {
            handles: transfer.handles().try_into().unwrap(),
            pid: slot.peer.host_pid,
            tid: slot.tid,
        };
        state.leases.push(Lease {
            transaction,
            parent,
            child: slot,
        });
        Ok(Some((worker, transfer)))
    }
}

pub(super) fn start(service: Arc<Service>) -> io::Result<()> {
    if !service
        .pool
        .as_ref()
        .is_some_and(|p| p.native_forks.enabled)
    {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("native-fork-pool".into())
        .spawn(move || {
            let pool = &service.pool.as_ref().unwrap().native_forks;
            loop {
                // Lock order is manager -> state. Process creation has its own
                // shutdown gate and never holds either of these mutexes.
                let manager = service.manager.lock().unwrap();
                let mut state = pool.state.lock().unwrap();
                if service.stopping.load(std::sync::atomic::Ordering::Acquire) {
                    state.slots.clear();
                    state.leases.clear();
                    return;
                }
                state.reap_leases(&manager);
                let mut failed = 0;
                state.slots.retain(|slot| {
                    let keep =
                        !slot.exited() && (slot.prepared() || slot.since.elapsed() < TIMEOUT);
                    if !keep {
                        failed += 1;
                    }
                    keep
                });
                state.failures += failed;
                if state.failures < 3 && state.slots.len() < CAPACITY && state.template.is_some() {
                    // Keep the old module pins alive even if take replaces the
                    // template while CreateProcess is in flight.
                    let template = Arc::clone(state.template.as_ref().unwrap());
                    drop(state);
                    drop(manager);
                    let result =
                        pool.prepare(&service.stopping, || spawn(&service, pool, &template.spec));
                    let manager = service.manager.lock().unwrap();
                    let mut state = pool.state.lock().unwrap();
                    if let Some(result) = result {
                        state.publish(&template, service.stopping.load(Ordering::Acquire), result);
                    }
                    drop(state);
                    drop(manager);
                    continue;
                }
                let busy = !state.leases.is_empty()
                    || (state.failures < 3
                        && state.template.is_some()
                        && (state.slots.len() < CAPACITY
                            || state.slots.iter().any(|s| !s.prepared())));
                drop(state);
                if busy {
                    drop(
                        service
                            .changed
                            .wait_timeout(manager, Duration::from_millis(10))
                            .unwrap(),
                    );
                } else {
                    drop(service.changed.wait(manager).unwrap());
                }
            }
        })?;
    Ok(())
}

fn trace() -> bool {
    std::env::var_os("KINAKAZE_FORK_TRACE").as_deref() == Some(std::ffi::OsStr::new("1"))
}

fn own(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn new(handles: &mut [windows_sys::Win32::Foundation::HANDLE]) -> io::Result<Self> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        let mut attrs = Self {
            storage: vec![0; size.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(attrs.pointer(), 1, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        attrs.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                attrs.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(attrs)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.pointer());
            }
        }
    }
}

fn spawn(service: &Service, pool: &Pool, spec: &Spec) -> io::Result<Standby> {
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let event = || own(unsafe { CreateEventW(&security, 1, 0, ptr::null()) });
    let ready = event()?;
    let activate = event()?;
    let parked = event()?;
    let bytes = spec
        .manifest()
        .ok_or_else(|| io::Error::other("invalid native manifest"))?;
    let mapping = own(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            &security,
            PAGE_READWRITE,
            0,
            bytes.len() as u32,
            ptr::null(),
        )
    })?;
    let view = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_WRITE, 0, 0, bytes.len()) };
    if view.Value.is_null() {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast(), bytes.len());
        UnmapViewOfFile(view);
    }
    let mut handles = vec![
        ready.as_raw_handle(),
        mapping.as_raw_handle(),
        activate.as_raw_handle(),
        parked.as_raw_handle(),
    ];
    let mut stdio = Vec::new();
    let mut std_handles = [ptr::null_mut(); 3];
    for (index, which) in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
        .into_iter()
        .enumerate()
    {
        let source = unsafe { GetStdHandle(which) };
        if source.is_null() || source == INVALID_HANDLE_VALUE {
            continue;
        }
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &mut raw,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        stdio.push(own(raw)?);
        handles.push(raw);
        std_handles[index] = raw;
    }
    let mut attrs = Attributes::new(&mut handles)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = std_handles[0];
    startup.StartupInfo.hStdOutput = std_handles[1];
    startup.StartupInfo.hStdError = std_handles[2];
    startup.lpAttributeList = attrs.pointer();
    let executable = std::env::current_exe()?.with_file_name("worker.exe");
    let executable: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let command = format!(
        "kinakaze-child --kinakaze-fork --crysoacu-fork-child={}:{}:{}:{}",
        ready.as_raw_handle() as usize,
        mapping.as_raw_handle() as usize,
        activate.as_raw_handle() as usize,
        parked.as_raw_handle() as usize
    );
    let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let mut environment: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> =
        std::env::vars_os().collect();
    for (key, value) in [
        (
            "KINAKAZE_V2_ENDPOINT",
            std::ffi::OsString::from(&service.endpoint),
        ),
        (
            "KINAKAZE_V2_TOKEN",
            std::ffi::OsString::from(&service.token),
        ),
        ("KINAKAZE_V2_ROOT", pool.root.as_os_str().to_owned()),
        ("KINAKAZE_V2_DIST", pool.dist.as_os_str().to_owned()),
    ] {
        environment.insert(key.into(), value);
    }
    environment.remove(std::ffi::OsStr::new("KINAKAZE_V2_ADOPTION"));
    environment.remove(std::ffi::OsStr::new("KINAKAZE_V2_ADOPTION_TICKET"));
    let mut block = Vec::new();
    for (key, value) in environment {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            executable.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT
                | kinakaze_v2_host_win::background_creation_flags(),
            block.as_ptr().cast(),
            ptr::null(),
            &startup.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut slot = Standby {
        process: own(process.hProcess)?,
        thread: own(process.hThread)?,
        ready,
        activate,
        parked,
        peer: PeerIdentity {
            host_pid: process.dwProcessId,
            birth: 0,
        },
        tid: process.dwThreadId,
        since: Instant::now(),
        kill: true,
    };
    let native = ProcessHandle::open(process.dwProcessId)?;
    service.job.assign(&native)?;
    slot.peer.birth = native.birth();
    if unsafe { ResumeThread(slot.thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(slot)
}

#[cfg(test)]
mod tests;
