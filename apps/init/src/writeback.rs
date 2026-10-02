//! Session-owned asynchronous data writeback. A hint outlives its submitting
//! worker. Native handles retain inode identity; failures survive until a waiter
//! consumes them. Full fsync remains a separate synchronous durability barrier.
use kinakaze_v2_host_win::ProcessHandle;
use kinakaze_v2_manager::PeerIdentity;
use std::{
    collections::HashMap,
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{Arc, Condvar, Mutex, OnceLock, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HANDLE},
    Storage::FileSystem::FlushFileBuffers,
};

const CAPACITY: usize = 128;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct Key(u32, u64);
struct Entry {
    pending: usize,
    error: u32,
    urgent: bool,
    // Failed I/O retains the inode until the error is reported, so its file ID
    // cannot be reused for a different inode with an unrelated pending error.
    failed_file: Option<OwnedHandle>,
}
#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    pending: usize,
    closing: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
struct Command {
    key: Key,
    file: OwnedHandle,
    ready: Instant,
}
struct Workers {
    sender: mpsc::SyncSender<Command>,
    threads: Vec<JoinHandle<()>>,
}
pub struct Queue {
    shared: Arc<Shared>,
    workers: OnceLock<Option<Workers>>,
    flush: fn(HANDLE) -> u32,
    grace: Duration,
}
impl Default for Queue {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            workers: OnceLock::new(),
            flush: flush_data,
            grace: Duration::from_millis(
                std::env::var("KINAKAZE_WRITEBACK_DELAY_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(2)
                    .min(1000),
            ),
        }
    }
}
impl Queue {
    /// Return [accepted, Win32 error]. Unknown/unsupported inputs fall back to
    /// the caller's established synchronous implementation.
    pub fn request(&self, peer: PeerIdentity, source: u64, wait: bool) -> Vec<u64> {
        let result = (|| {
            if source == 0 || source > isize::MAX as u64 {
                return Err(io::Error::other("invalid writeback handle"));
            }
            let process = ProcessHandle::open(peer.host_pid)
                .map_err(|error| io::Error::other(format!("open submitter: {error}")))?;
            if process.birth() != peer.birth {
                return Err(io::Error::other("writeback peer identity changed"));
            }
            let file = process
                .duplicate_object(source)
                .map_err(|error| io::Error::other(format!("duplicate file: {error}")))?;
            let key = key(file.as_raw_handle())
                .map_err(|error| io::Error::other(format!("query file identity: {error}")))?;
            if wait {
                Ok(vec![1, self.wait(key) as u64])
            } else {
                Ok(vec![u64::from(self.submit(key, file)), 0])
            }
        })();
        result.unwrap_or_else(|error| {
            static TRACE: OnceLock<bool> = OnceLock::new();
            static REPORTED: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if *TRACE.get_or_init(|| std::env::var_os("KINAKAZE_WRITEBACK_TRACE").is_some())
                && REPORTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16
            {
                eprintln!(
                    "writeback rejected pid={} source={source} wait={wait}: {error}",
                    peer.host_pid
                );
            }
            vec![0, 0]
        })
    }

    fn start(&self) -> Option<&Workers> {
        self.workers
            .get_or_init(|| {
                let (sender, receiver) = mpsc::sync_channel::<Command>(CAPACITY);
                let receiver = Arc::new(Mutex::new(receiver));
                let mut threads = Vec::new();
                for index in 0..2 {
                    let receiver = Arc::clone(&receiver);
                    let shared = Arc::clone(&self.shared);
                    let flush = self.flush;
                    let result = std::thread::Builder::new()
                        .name(format!("file-writeback-{index}"))
                        .spawn(move || {
                            loop {
                                let command = receiver.lock().unwrap().recv();
                                let Ok(Command { key, file, ready }) = command else {
                                    break;
                                };
                                let mut state = shared.state.lock().unwrap();
                                while !state.closing
                                    && !state.entries.get(&key).is_some_and(|entry| entry.urgent)
                                {
                                    let Some(remaining) =
                                        ready.checked_duration_since(Instant::now())
                                    else {
                                        break;
                                    };
                                    state =
                                        shared.changed.wait_timeout(state, remaining).unwrap().0;
                                }
                                drop(state);
                                let error = flush(file.as_raw_handle());
                                // Successful capabilities close before completion is
                                // published, including before verity's deny-write open.
                                let failed_file = if error == 0 {
                                    drop(file);
                                    None
                                } else {
                                    Some(file)
                                };
                                let mut state = shared.state.lock().unwrap();
                                let entry = state.entries.get_mut(&key).unwrap();
                                entry.pending -= 1;
                                if error != 0 {
                                    if entry.error == 0 {
                                        entry.error = error;
                                    }
                                    entry.failed_file = failed_file;
                                }
                                if entry.pending == 0 && entry.error == 0 {
                                    state.entries.remove(&key);
                                }
                                state.pending -= 1;
                                shared.changed.notify_all();
                            }
                        });
                    match result {
                        Ok(thread) => threads.push(thread),
                        Err(_) => {
                            drop(sender);
                            for thread in threads {
                                let _ = thread.join();
                            }
                            return None;
                        }
                    }
                }
                Some(Workers { sender, threads })
            })
            .as_ref()
    }

    fn submit(&self, key: Key, file: OwnedHandle) -> bool {
        if !writable(file.as_raw_handle()) {
            return false;
        }
        let Some(workers) = self.start() else {
            return false;
        };
        let mut state = self.shared.state.lock().unwrap();
        if state.closing
            || state.pending >= CAPACITY
            || state.entries.len() >= CAPACITY && !state.entries.contains_key(&key)
        {
            return false;
        }
        let entry = state.entries.entry(key).or_insert(Entry {
            pending: 0,
            error: 0,
            urgent: false,
            failed_file: None,
        });
        entry.pending += 1;
        state.pending += 1;
        if workers
            .sender
            .try_send(Command {
                key,
                file,
                ready: Instant::now() + self.grace,
            })
            .is_err()
        {
            let entry = state.entries.get_mut(&key).unwrap();
            entry.pending -= 1;
            if entry.pending == 0 && entry.error == 0 {
                state.entries.remove(&key);
            }
            state.pending -= 1;
            return false;
        }
        true
    }

    fn wait(&self, key: Key) -> u32 {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(entry) = state.entries.get_mut(&key) {
            entry.urgent = true;
            self.shared.changed.notify_all();
        }
        while state
            .entries
            .get(&key)
            .is_some_and(|entry| entry.pending != 0)
        {
            state = self.shared.changed.wait(state).unwrap();
        }
        state.entries.remove(&key).map_or(0, |entry| entry.error)
    }

    pub fn shutdown(&self) -> u32 {
        let mut state = self.shared.state.lock().unwrap();
        state.closing = true;
        self.shared.changed.notify_all();
        while state.pending != 0 {
            state = self.shared.changed.wait(state).unwrap();
        }
        state
            .entries
            .values()
            .find_map(|entry| (entry.error != 0).then_some(entry.error))
            .unwrap_or(0)
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        if let Some(Some(Workers { sender, threads })) = self.workers.take() {
            drop(sender);
            for thread in threads {
                let _ = thread.join();
            }
        }
    }
}
fn key(file: HANDLE) -> io::Result<Key> {
    let mut io = IoStatus {
        status: 0x103,
        information: 0,
    };
    let mut standard = [0u64; 3];
    let status =
        unsafe { NtQueryInformationFile(file, &mut io, standard.as_mut_ptr().cast(), 24, 5) };
    checked_status(status, &io)?;
    if standard[2] & (0xff << 40) != 0 {
        return Err(io::Error::other("writeback requires an ordinary file"));
    }
    let mut identity = 0u64;
    io.status = 0x103;
    let status =
        unsafe { NtQueryInformationFile(file, &mut io, (&mut identity as *mut u64).cast(), 8, 6) };
    checked_status(status, &io)?;
    let mut volume = [0u64; 64];
    io.status = 0x103;
    let status = unsafe {
        NtQueryVolumeInformationFile(
            file,
            &mut io,
            volume.as_mut_ptr().cast(),
            size_of_val(&volume) as u32,
            1,
        )
    };
    let status = completed_status(status, &io);
    if status < 0 && status as u32 != 0x8000_0005 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(Key(volume[1] as u32, identity))
}

fn completed_status(mut status: i32, io: &IoStatus) -> i32 {
    if status == 0x103 {
        loop {
            status = unsafe { std::ptr::read_volatile(&io.status) } as i32;
            if status != 0x103 {
                break;
            }
            unsafe { windows_sys::Win32::System::Threading::Sleep(1) };
        }
    }
    status
}

fn checked_status(status: i32, io: &IoStatus) -> io::Result<()> {
    let status = completed_status(status, io);
    if status < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))
    } else {
        Ok(())
    }
}

#[repr(C)]
struct IoStatus {
    status: usize,
    information: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        file: HANDLE,
        io: *mut IoStatus,
        buffer: *mut std::ffi::c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtQueryVolumeInformationFile(
        file: HANDLE,
        io: *mut IoStatus,
        buffer: *mut std::ffi::c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtFlushBuffersFileEx(
        file: HANDLE,
        flags: u32,
        parameters: *const u8,
        length: u32,
        io: *mut IoStatus,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
    fn NtQueryObject(
        file: HANDLE,
        class: u32,
        buffer: *mut std::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}
fn writable(file: HANDLE) -> bool {
    let mut basic = [0u32; 14];
    (unsafe {
        NtQueryObject(
            file,
            0,
            basic.as_mut_ptr().cast(),
            size_of_val(&basic) as u32,
            std::ptr::null_mut(),
        )
    }) >= 0
        && basic[1] & 6 != 0
}
fn flush_data(file: HANDLE) -> u32 {
    let _span = kinakaze_v2_host_win::StartupSpan::begin("file-writeback-data");
    let mut io = IoStatus {
        status: 0x103,
        information: 0,
    };
    let status = unsafe { NtFlushBuffersFileEx(file, 1, std::ptr::null(), 0, &mut io) };
    let status = completed_status(status, &io);
    match status as u32 {
        0 => 0,
        0xc000_0002 | 0xc000_000d | 0xc000_0010 | 0xc000_00bb => {
            if unsafe { FlushFileBuffers(file) } != 0 {
                0
            } else {
                unsafe { GetLastError() }
            }
        }
        _ => unsafe { RtlNtStatusToDosError(status) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_GEN_FAILURE;
    fn fixture() -> (std::path::PathBuf, std::fs::File) {
        let path = std::env::temp_dir().join(format!(
            "writeback-{}-{}",
            std::process::id(),
            kinakaze_v2_host_win::random_token().unwrap()
        ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(7)
            .open(&path)
            .unwrap();
        (path, file)
    }

    #[test]
    fn a_wait_expedites_deferred_io_and_still_waits_for_completion() {
        static GATE: (Mutex<(bool, bool)>, Condvar) = (Mutex::new((false, false)), Condvar::new());
        fn blocked(_: HANDLE) -> u32 {
            let mut gate = GATE.0.lock().unwrap();
            gate.0 = true;
            GATE.1.notify_all();
            while !gate.1 {
                gate = GATE.1.wait(gate).unwrap();
            }
            0
        }
        let (path, file) = fixture();
        let key = key(file.as_raw_handle()).unwrap();
        let mut queue = Queue::default();
        queue.grace = Duration::from_secs(60);
        queue.flush = blocked;
        let queue = Arc::new(queue);
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        let gate = GATE.0.lock().unwrap();
        let (gate, _) = GATE
            .1
            .wait_timeout_while(gate, Duration::from_millis(30), |gate| !gate.0)
            .unwrap();
        assert!(!gate.0);
        drop(gate);
        let (sent, received) = mpsc::channel();
        let waiter = Arc::clone(&queue);
        let thread = std::thread::spawn(move || sent.send(waiter.wait(key)).unwrap());
        let gate = GATE.0.lock().unwrap();
        let (mut gate, _) = GATE
            .1
            .wait_timeout_while(gate, Duration::from_secs(5), |gate| !gate.0)
            .unwrap();
        let started = gate.0;
        let premature = received.try_recv().is_ok();
        gate.1 = true;
        GATE.1.notify_all();
        drop(gate);
        assert_eq!(queue.shutdown(), 0);
        assert!(started);
        assert!(!premature);
        assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        thread.join().unwrap();
        drop(queue);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn shutdown_expedites_deferred_io_and_preserves_its_error() {
        let (path, file) = fixture();
        let key = key(file.as_raw_handle()).unwrap();
        let mut queue = Queue::default();
        queue.grace = Duration::from_secs(60);
        queue.flush = |_| ERROR_GEN_FAILURE;
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        let started = Instant::now();
        assert_eq!(queue.shutdown(), ERROR_GEN_FAILURE);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(queue.wait(key), ERROR_GEN_FAILURE);
        assert_eq!(queue.shutdown(), 0);
        drop(queue);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn queue_is_bounded_and_waiters_retire_only_after_native_completion() {
        static GATE: (Mutex<(usize, bool)>, Condvar) = (Mutex::new((0, false)), Condvar::new());
        fn blocked(_: HANDLE) -> u32 {
            let mut gate = GATE.0.lock().unwrap();
            gate.0 += 1;
            GATE.1.notify_all();
            while !gate.1 {
                gate = GATE.1.wait(gate).unwrap();
            }
            0
        }
        let (path, file) = fixture();
        let key = key(file.as_raw_handle()).unwrap();
        let mut queue = Queue::default();
        queue.flush = blocked;
        let queue = Arc::new(queue);
        for _ in 0..CAPACITY {
            assert!(queue.submit(key, file.try_clone().unwrap().into()));
        }
        assert!(!queue.submit(key, file.try_clone().unwrap().into()));
        let (sent, received) = mpsc::channel();
        let waiter = Arc::clone(&queue);
        let thread = std::thread::spawn(move || {
            sent.send(waiter.wait(key)).unwrap();
        });
        let mut gate = GATE.0.lock().unwrap();
        while gate.0 < 2 {
            gate = GATE.1.wait(gate).unwrap();
        }
        assert!(received.try_recv().is_err());
        gate.1 = true;
        GATE.1.notify_all();
        drop(gate);
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            0
        );
        thread.join().unwrap();
        drop(queue);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn untrusted_source_and_reused_peer_pid_are_rejected() {
        let process = ProcessHandle::open(std::process::id()).unwrap();
        let queue = Queue::default();
        let peer = PeerIdentity {
            host_pid: process.pid(),
            birth: process.birth(),
        };
        assert_eq!(queue.request(peer, 0, false), [0, 0]);
        let (path, file) = fixture();
        let stale = PeerIdentity {
            birth: peer.birth ^ 1,
            ..peer
        };
        assert_eq!(
            queue.request(stale, file.as_raw_handle() as u64, false),
            [0, 0]
        );
        assert_eq!(
            queue.request(peer, file.as_raw_handle() as u64, false),
            [1, 0]
        );
        assert_eq!(
            queue.request(peer, file.as_raw_handle() as u64, true),
            [1, 0]
        );
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn async_data_writeback_retains_renamed_unlinked_inode_and_releases_writers() {
        use std::io::Write;
        let (path, mut file) = fixture();
        file.write_all(&vec![0x5a; 65536]).unwrap();
        let key = key(file.as_raw_handle()).unwrap();
        let queue = Queue::default();
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        let moved = path.with_extension("moved");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(queue.wait(key), 0);
        assert_eq!(std::fs::read(&moved).unwrap(), vec![0x5a; 65536]);
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        std::fs::remove_file(&moved).unwrap();
        assert_eq!(queue.wait(key), 0);
        drop(file);
        assert!(queue.shared.state.lock().unwrap().entries.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn writeback_errors_are_retained_for_the_same_inode_and_reported() {
        let (path, file) = fixture();
        let key = key(file.as_raw_handle()).unwrap();
        let mut queue = Queue::default();
        queue.flush = |_| ERROR_GEN_FAILURE;
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        assert_eq!(queue.wait(Key(key.0, key.1 ^ 1)), 0);
        assert_eq!(queue.wait(key), ERROR_GEN_FAILURE);
        assert_eq!(queue.wait(key), 0);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn readonly_native_capability_cannot_gain_write_access() {
        let (path, file) = fixture();
        drop(file);
        let file = std::fs::File::open(&path).unwrap();
        let key = key(file.as_raw_handle()).unwrap();
        let queue = Queue::default();
        assert!(!queue.submit(key, file.into()));
        assert_eq!(queue.wait(key), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn write_only_native_capability_can_submit_and_wait() {
        use std::os::windows::io::FromRawHandle;
        let (path, file) = fixture();
        let mut raw = std::ptr::null_mut();
        let current = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
        assert_ne!(
            unsafe {
                windows_sys::Win32::Foundation::DuplicateHandle(
                    current,
                    file.as_raw_handle(),
                    current,
                    &mut raw,
                    0x120116,
                    0,
                    0,
                )
            },
            0
        );
        let writer = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut info = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(
                    writer.as_raw_handle(),
                    &mut info,
                )
            },
            0
        );
        assert_eq!(
            key(writer.as_raw_handle()).unwrap(),
            key(file.as_raw_handle()).unwrap()
        );
        let process = ProcessHandle::open(std::process::id()).unwrap();
        let peer = PeerIdentity {
            host_pid: process.pid(),
            birth: process.birth(),
        };
        let queue = Queue::default();
        assert_eq!(
            queue.request(peer, writer.as_raw_handle() as u64, false),
            [1, 0]
        );
        assert_eq!(
            queue.request(peer, writer.as_raw_handle() as u64, true),
            [1, 0]
        );
        drop(writer);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn shutdown_rejects_new_work_and_drains_retained_handles() {
        static GATE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
        fn blocked(_: HANDLE) -> u32 {
            let mut released = GATE.0.lock().unwrap();
            while !*released {
                released = GATE.1.wait(released).unwrap();
            }
            ERROR_GEN_FAILURE
        }
        let (path, file) = fixture();
        let key = key(file.as_raw_handle()).unwrap();
        let mut queue = Queue::default();
        queue.flush = blocked;
        let queue = Arc::new(queue);
        assert!(queue.submit(key, file.try_clone().unwrap().into()));
        let draining = Arc::clone(&queue);
        let (sent, received) = mpsc::channel();
        let thread = std::thread::spawn(move || sent.send(draining.shutdown()).unwrap());
        let mut state = queue.shared.state.lock().unwrap();
        while !state.closing {
            state = queue.shared.changed.wait(state).unwrap();
        }
        drop(state);
        assert!(!queue.submit(key, file.try_clone().unwrap().into()));
        assert!(received.try_recv().is_err());
        drop(file);
        *GATE.0.lock().unwrap() = true;
        GATE.1.notify_all();
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            ERROR_GEN_FAILURE
        );
        thread.join().unwrap();
        assert_eq!(queue.wait(key), ERROR_GEN_FAILURE);
        assert_eq!(queue.shutdown(), 0);
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        drop(queue);
        std::fs::remove_file(path).unwrap();
    }
}
