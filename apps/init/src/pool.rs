//! A bounded supply of unused workers. Guest state is never recycled.
use super::Service;
use kinakaze_v2_host_win::{ProcessHandle, background_creation_flags};
use kinakaze_v2_manager::PeerIdentity;
use std::{
    io,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const REFILL_DELAY: Duration = Duration::from_millis(250);
const PREPARATION_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Pool {
    pub size: usize,
    pub failed: AtomicBool,
    demand: AtomicBool,
    root: PathBuf,
    dist: PathBuf,
    consumed: Mutex<Vec<PeerIdentity>>,
    run_priority: u32,
}

struct Standby {
    child: Child,
    native: ProcessHandle,
    since: Instant,
}

impl Standby {
    fn peer(&self) -> PeerIdentity {
        PeerIdentity {
            host_pid: self.native.pid(),
            birth: self.native.birth(),
        }
    }
}

impl Pool {
    pub fn new(size: usize, root: PathBuf, dist: PathBuf) -> io::Result<Self> {
        if !(1..=8).contains(&size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prewarm pool size must be 1..=8",
            ));
        }
        Ok(Self {
            size,
            root: root.canonicalize()?,
            dist: dist.canonicalize()?,
            failed: AtomicBool::new(false),
            demand: AtomicBool::new(false),
            consumed: Mutex::new(Vec::new()),
            run_priority: kinakaze_v2_host_win::inherited_process_priority()?,
        })
    }

    // Called under the manager mutex before waking the guest. This event cannot
    // be lost even if a very short guest exits before the maintenance thread runs.
    pub fn consumed(&self, peer: PeerIdentity) {
        let mut consumed = self.consumed.lock().unwrap();
        if !consumed.contains(&peer) {
            consumed.push(peer);
        }
    }

    // A queued launch should wait only for preparation, not the background
    // refill grace period. The manager mutex orders this before its notification.
    pub fn refill_for_waiter(&self) {
        self.demand.store(true, Ordering::Release);
    }

    fn spawn(&self, service: &Service) -> io::Result<Standby> {
        let mut child = Command::new(std::env::current_exe()?.with_file_name("worker.exe"))
            .arg("guest-pool")
            .arg("--root")
            .arg(&self.root)
            .arg("--dist")
            .arg(&self.dist)
            .env("KINAKAZE_V2_ENDPOINT", &service.endpoint)
            .env("KINAKAZE_V2_TOKEN", &service.token)
            .env("KINAKAZE_V2_ROOT", &self.root)
            .env("KINAKAZE_V2_DIST", &self.dist)
            .env("KINAKAZE_V2_POOL_PRIORITY", self.run_priority.to_string())
            .env_remove("KINAKAZE_V2_ADOPTION")
            .env_remove("KINAKAZE_V2_ADOPTION_TICKET")
            // Pooled applications receive EOF, never compete for init's input.
            .stdin(Stdio::null())
            // Preparation may overlap an earlier application's startup. Restore
            // ordinary child priority before publishing readiness in runtime.
            .creation_flags(
                background_creation_flags()
                    | if self.run_priority == 0x40 {
                        0x40
                    } else {
                        0x4000
                    },
            )
            .spawn()?;
        match ProcessHandle::open(child.id()).and_then(|process| {
            service.job.assign(&process)?;
            Ok(process)
        }) {
            Ok(native) => Ok(Standby {
                child,
                native,
                since: Instant::now(),
            }),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
}

pub(super) fn start(service: Arc<Service>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("prewarm-pool".into())
        .spawn(move || maintain(service))?;
    Ok(())
}

fn maintain(service: Arc<Service>) {
    let pool = service
        .pool
        .as_ref()
        .expect("pool configured before maintenance");
    let mut slots: Vec<Standby> = Vec::new();
    let mut next_spawn = Instant::now();
    let mut failures = 0;
    loop {
        let manager = service.manager.lock().unwrap();
        if service.stopping.load(Ordering::Acquire) {
            return;
        }
        let consumed = std::mem::take(&mut *pool.consumed.lock().unwrap());
        let mut all_prepared = true;
        let was_full = slots.len() == pool.size;
        let mut released = false;
        slots.retain_mut(|slot| {
            if consumed.contains(&slot.peer()) {
                // Let the latency-sensitive activation run before preparing its
                // replacement. Long-lived applications still get replenishment.
                if !released {
                    let deadline = Instant::now() + REFILL_DELAY;
                    next_spawn = if was_full {
                        deadline
                    } else {
                        next_spawn.min(deadline)
                    };
                    released = true;
                }
                return false;
            }
            let prepared = manager.pool_peer_is_prepared(slot.peer());
            if slot.native.has_exited().unwrap_or(true) {
                failures += 1;
                return false;
            }
            if !prepared && slot.since.elapsed() >= PREPARATION_TIMEOUT {
                let _ = slot.child.kill();
                failures += 1;
                return false;
            }
            all_prepared &= prepared;
            true
        });
        if pool.demand.swap(false, Ordering::AcqRel) {
            next_spawn = Instant::now();
        }
        if failures >= 3 {
            // Fail the pool, not already running applications in this session.
            pool.failed.store(true, Ordering::Release);
            service.changed.notify_all();
            eprintln!("init: prewarm pool disabled after repeated preparation failures");
            return;
        }
        if slots.len() == pool.size && all_prepared {
            failures = 0;
            // No polling or timers while the pool is full and idle.
            drop(service.changed.wait(manager).unwrap());
            continue;
        }
        if slots.len() < pool.size && Instant::now() >= next_spawn {
            // stop() takes this same mutex before allowing main to return.
            // Keep native creation + Job assignment atomic with respect to
            // shutdown, so no newly created worker can escape the closing Job.
            let spawned = pool.spawn(&service);
            drop(manager);
            match spawned {
                Ok(slot) => slots.push(slot),
                Err(error) => {
                    eprintln!("init: pool worker creation failed: {:?}", error.kind());
                    failures += 1;
                    next_spawn = Instant::now() + REFILL_DELAY;
                }
            }
            continue;
        }
        let delay = next_spawn.saturating_duration_since(Instant::now());
        let delay = if slots.len() == pool.size || delay.is_zero() {
            Duration::from_millis(100)
        } else {
            delay.min(Duration::from_millis(100))
        };
        drop(service.changed.wait_timeout(manager, delay).unwrap());
    }
}
