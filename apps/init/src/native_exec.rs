//! One-use exec stock is parked after native DLL loading, before any Linux PID
//! or runtime session exists. Init retains rollback custody until exec commit.
use super::{
    Service,
    native_fork::{own, spawn_bootstrap},
};
use kinakaze_v2_host_win::{ProcessHandle, RemoteTransfer};
use kinakaze_v2_manager::{NativeExecState, PeerIdentity};
use kinakaze_v2_protocol::native_exec::{CAPACITY, Worker};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, Mutex, OnceLock, atomic::Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::FILE_SHARE_READ,
    System::{
        Memory::{CreateFileMappingW, PAGE_READWRITE},
        Threading::{CreateEventW, TerminateProcess, WaitForSingleObject},
    },
};

const STOCK: usize = 2;
const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Pool {
    enabled: bool,
    root: PathBuf,
    dist: PathBuf,
    state: Mutex<State>,
    creating: Mutex<()>,
    pins: OnceLock<io::Result<Vec<File>>>,
}

#[derive(Default)]
struct State {
    slots: Vec<Standby>,
    leases: Vec<Lease>,
    failures: usize,
}
struct Standby {
    process: OwnedHandle,
    ready: OwnedHandle,
    activate: OwnedHandle,
    control: OwnedHandle,
    peer: PeerIdentity,
    since: Instant,
    kill: bool,
}
impl Standby {
    fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) != WAIT_TIMEOUT }
    }
    fn prepared(&self) -> bool {
        unsafe { WaitForSingleObject(self.ready.as_raw_handle(), 0) == WAIT_OBJECT_0 }
    }
}
impl Drop for Standby {
    fn drop(&mut self) {
        if self.kill {
            unsafe { TerminateProcess(self.process.as_raw_handle(), 125) };
        }
    }
}
struct Lease {
    parent: ProcessHandle,
    child: Standby,
    delivered: Instant,
}
impl State {
    fn reap(&mut self, manager: &kinakaze_v2_manager::StateManager) {
        self.leases.retain_mut(|lease| {
            let parent = PeerIdentity {
                host_pid: lease.parent.pid(),
                birth: lease.parent.birth(),
            };
            match manager.native_exec_state(parent, lease.child.peer) {
                NativeExecState::Committed => {
                    // Exec commit precedes old native-process death. Custody
                    // must be released before testing that parent's liveness.
                    lease.child.kill = false;
                    false
                }
                NativeExecState::Aborted => false,
                state => {
                    !lease.child.exited()
                        && !lease.parent.has_exited().unwrap_or(true)
                        && (state == NativeExecState::Pending
                            || lease.delivered.elapsed() < TIMEOUT)
                }
            }
        });
    }
}

impl Pool {
    pub fn new(root: PathBuf, dist: PathBuf) -> Self {
        Self {
            enabled: std::env::var_os("KINAKAZE_EXEC_POOL").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            root,
            dist,
            state: Mutex::new(State::default()),
            creating: Mutex::new(()),
            pins: OnceLock::new(),
        }
    }
    pub fn lock_creation(&self) -> std::sync::MutexGuard<'_, ()> {
        self.creating.lock().unwrap()
    }

    pub fn cancel_delivery(&self, peer: PeerIdentity, candidate: u32) {
        self.state.lock().unwrap().leases.retain(|lease| {
            lease.parent.pid() != peer.host_pid
                || lease.parent.birth() != peer.birth
                || lease.child.peer.host_pid != candidate
        });
    }

    pub fn take(&self, peer: PeerIdentity) -> io::Result<Option<(Worker, RemoteTransfer)>> {
        if !self.enabled {
            return Ok(None);
        }
        let mut state = self.state.lock().unwrap();
        if state.leases.len() >= 128
            || state.leases.iter().any(|lease| {
                lease.parent.pid() == peer.host_pid
                    && lease.parent.birth() == peer.birth
                    && !lease.child.exited()
            })
        {
            return Ok(None);
        }
        let Some(index) = state
            .slots
            .iter()
            .position(|slot| !slot.exited() && slot.prepared())
        else {
            return Ok(None);
        };
        let parent = ProcessHandle::open(peer.host_pid)?;
        let target = ProcessHandle::open(peer.host_pid)?;
        if parent.birth() != peer.birth || target.birth() != peer.birth || parent.has_exited()? {
            return Err(io::Error::other("exec recipient identity changed"));
        }
        let slot = state.slots.swap_remove(index);
        let mut transfer = RemoteTransfer::new(target);
        for handle in [&slot.process, &slot.activate, &slot.control] {
            transfer.add(handle.as_raw_handle(), 0, true)?;
        }
        let worker = Worker {
            handles: transfer.handles().try_into().unwrap(),
            pid: slot.peer.host_pid,
        };
        state.leases.push(Lease {
            parent,
            child: slot,
            delivered: Instant::now(),
        });
        Ok(Some((worker, transfer)))
    }
}

fn pin_images(dist: &Path) -> io::Result<Vec<File>> {
    let directory = kinakaze_v2_bridge::native::directory(dist).canonicalize()?;
    let mut pins = Vec::new();
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path().canonicalize()?;
        if path.parent() != Some(directory.as_path()) {
            return Err(io::Error::other("native image outside distribution"));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)?;
        let mut magic = [0; 2];
        if file.read(&mut magic)? == 2 && &magic == b"MZ" {
            pins.push(file);
        }
    }
    if pins.is_empty() {
        return Err(io::Error::other("exec stock has no native images"));
    }
    pins.push(
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(std::env::current_exe()?.with_file_name("worker.exe"))?,
    );
    Ok(pins)
}

fn spawn(service: &Service, pool: &Pool) -> io::Result<Standby> {
    pool.pins
        .get_or_init(|| pin_images(&pool.dist))
        .as_ref()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let event = || own(unsafe { CreateEventW(&security, 1, 0, ptr::null()) });
    let ready = event()?;
    let activate = event()?;
    let control = own(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            &security,
            PAGE_READWRITE,
            0,
            CAPACITY as u32,
            ptr::null(),
        )
    })?;
    let command = format!(
        "kinakaze-child --kinakaze-exec-pool {}:{}:{}",
        ready.as_raw_handle() as usize,
        activate.as_raw_handle() as usize,
        control.as_raw_handle() as usize
    );
    let native = spawn_bootstrap(
        service,
        &pool.root,
        &pool.dist,
        command,
        vec![
            ready.as_raw_handle(),
            activate.as_raw_handle(),
            control.as_raw_handle(),
        ],
    )?;
    Ok(Standby {
        process: native.process,
        ready,
        activate,
        control,
        peer: native.peer,
        since: Instant::now(),
        kill: true,
    })
}

pub(super) fn start(service: Arc<Service>) -> io::Result<()> {
    if !service
        .pool
        .as_ref()
        .is_some_and(|pool| pool.native_execs.enabled)
    {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("native-exec-pool".into())
        .spawn(move || {
            let pool = &service.pool.as_ref().unwrap().native_execs;
            loop {
                let manager = service.manager.lock().unwrap();
                let mut state = pool.state.lock().unwrap();
                if service.stopping.load(Ordering::Acquire) {
                    state.slots.clear();
                    state.leases.clear();
                    return;
                }
                state.reap(&manager);
                let before = state.slots.len();
                state.slots.retain(|slot| {
                    !slot.exited() && (slot.prepared() || slot.since.elapsed() < TIMEOUT)
                });
                state.failures += before - state.slots.len();
                if state.failures < 3 && state.slots.len() < STOCK {
                    drop(state);
                    drop(manager);
                    let result = {
                        let _creation = pool.lock_creation();
                        if service.stopping.load(Ordering::Acquire) {
                            None
                        } else {
                            Some(spawn(&service, pool))
                        }
                    };
                    let manager = service.manager.lock().unwrap();
                    let mut state = pool.state.lock().unwrap();
                    if !service.stopping.load(Ordering::Acquire) {
                        match result {
                            Some(Ok(slot)) => state.slots.push(slot),
                            Some(Err(_)) => state.failures += 1,
                            None => (),
                        }
                    }
                    drop(state);
                    drop(manager);
                    continue;
                }
                let busy =
                    !state.leases.is_empty() || state.slots.iter().any(|slot| !slot.prepared());
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

#[cfg(test)]
mod tests;
