//! Session-scoped common process manager. No guest code runs in this process.
mod pool;
mod web;
use kinakaze_v2_host_win::{Job, PipeConnection, PipeListener, ProcessHandle, random_token};
use kinakaze_v2_manager::{PeerIdentity, StateManager};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, PROTOCOL_VERSION, Request, RpcError, WireRequest, WireResponse,
    read_frame, write_frame,
};
use std::io::{self, Write};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

const MAX_CONNECTIONS: usize = 128;

struct Service {
    manager: Mutex<StateManager>,
    changed: Condvar,
    stopping: AtomicBool,
    connections: AtomicUsize,
    endpoint: String,
    token: String,
    controller: PeerIdentity,
    job: Job,
    prewarm_process: Option<ProcessHandle>,
    pool: Option<pool::Pool>,
}

impl Service {
    fn stop(&self) {
        let first = {
            // Share the waiter's mutex so stop cannot race its predicate check.
            let _manager = self.manager.lock().unwrap();
            let first = !self.stopping.swap(true, Ordering::AcqRel);
            self.changed.notify_all();
            first
        };
        if first {
            // This session owns the endpoint. Wake the synchronous accept once.
            let _ = PipeConnection::connect(&self.endpoint);
        }
    }
}

struct ConnectionSlot(Arc<Service>);
impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reject(pipe: &mut PipeConnection, id: u64, code: ErrorCode, message: &str) -> io::Result<()> {
    write_frame(
        pipe,
        &WireResponse {
            id,
            result: Err(RpcError::new(code, message)),
        },
    )
}

fn serve(mut pipe: PipeConnection, service: Arc<Service>, _slot: ConnectionSlot) -> io::Result<()> {
    let first: WireRequest = read_frame(&mut pipe)?;
    let Request::Hello(hello) = first.request else {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::InvalidRequest,
            "first request must be Hello",
        );
    };
    if first.id == 0 {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::InvalidRequest,
            "request id must be positive",
        );
    }
    if hello.version != PROTOCOL_VERSION {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::VersionMismatch,
            "unsupported protocol version",
        );
    }
    if hello.token != service.token {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::Unauthorized,
            "invalid session credential",
        );
    }
    let process = ProcessHandle::open(pipe.peer_pid()?)?;
    let peer = PeerIdentity {
        host_pid: process.pid(),
        birth: process.birth(),
    };
    let is_worker = hello.role == ClientRole::Worker;
    let is_managed = is_worker || hello.role == ClientRole::Helper;
    if is_managed
        && (peer.host_pid == service.controller.host_pid || peer.host_pid == std::process::id())
    {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::Unauthorized,
            "infrastructure process cannot become a worker",
        );
    }
    if !is_managed
        && (peer.host_pid != service.controller.host_pid || peer.birth != service.controller.birth)
    {
        return reject(
            &mut pipe,
            first.id,
            ErrorCode::Unauthorized,
            "controller native identity mismatch",
        );
    }
    // The job covers authenticated worker candidates even if adoption is rejected.
    // It never includes the controller or this manager itself.
    if is_managed {
        service.job.assign(&process)?;
    }
    let connected = service.manager.lock().unwrap().connect(hello, peer);
    let (client, reply) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            return write_frame(
                &mut pipe,
                &WireResponse {
                    id: first.id,
                    result: Err(error),
                },
            );
        }
    };
    if is_worker {
        let watcher = Arc::clone(&service);
        if let Err(error) = std::thread::Builder::new()
            .name(format!("worker-exit-{}", peer.host_pid))
            .spawn(move || match process.wait() {
                Ok(()) => {
                    let status = process.exit_code().unwrap_or(1) as i32;
                    watcher
                        .manager
                        .lock()
                        .unwrap()
                        .process_exited_with_status(peer, status);
                    watcher.changed.notify_all();
                }
                Err(error) => {
                    eprintln!("native process wait failed: {error}");
                    watcher.stop();
                }
            })
        {
            service.stop();
            return Err(error);
        }
    }
    let result = (|| {
        write_frame(
            &mut pipe,
            &WireResponse {
                id: first.id,
                result: Ok(reply),
            },
        )?;
        let mut previous_id = first.id;
        while !service.stopping.load(Ordering::Acquire) {
            let wire: WireRequest = read_frame(&mut pipe)?;
            if wire.id <= previous_id {
                reject(
                    &mut pipe,
                    wire.id,
                    ErrorCode::InvalidRequest,
                    "request ids must strictly increase",
                )?;
                break;
            }
            previous_id = wire.id;
            let shutdown = matches!(wire.request, Request::Shutdown);
            let awaiting = matches!(
                wire.request,
                Request::AwaitActivation
                    | Request::AwaitForkReady { .. }
                    | Request::AwaitExecReady { .. }
                    | Request::AwaitExit { .. }
                    | Request::AwaitPrewarmReady { .. }
                    | Request::AwaitPrewarmActivation
                    | Request::AwaitPoolReady { .. }
                    | Request::ReservePoolWorker
                    | Request::AwaitPoolActivation
            );
            let result = {
                let mut manager = service.manager.lock().unwrap();
                loop {
                    if service.stopping.load(Ordering::Acquire) {
                        break Err(RpcError::new(ErrorCode::Aborted, "session stopping"));
                    }
                    if matches!(
                        wire.request,
                        Request::AwaitPoolReady { .. }
                            | Request::ReservePoolWorker
                            | Request::ReleasePoolWorker { .. }
                            | Request::ActivatePoolWorker { .. }
                            | Request::MarkPoolReady
                            | Request::AwaitPoolActivation
                    ) {
                        let Some(pool) = &service.pool else {
                            break Err(RpcError::new(
                                ErrorCode::InvalidRequest,
                                "init has no prewarm pool",
                            ));
                        };
                        if matches!(wire.request, Request::AwaitPoolReady { minimum } if minimum as usize > pool.size)
                        {
                            break Err(RpcError::new(
                                ErrorCode::InvalidRequest,
                                "requested readiness exceeds pool capacity",
                            ));
                        }
                    }
                    let result = manager.handle(client, wire.request.clone());
                    if result.is_ok()
                        && let Request::ActivatePoolWorker { pid, .. } = &wire.request
                        && let Some(pool) = &service.pool
                        && let Some(peer) = manager.process_peer(*pid)
                    {
                        pool.consumed(peer);
                    }
                    if awaiting
                        && matches!(&result, Err(error) if error.code == ErrorCode::NotReady)
                    {
                        if matches!(wire.request, Request::ReservePoolWorker)
                            && let Some(pool) = &service.pool
                        {
                            pool.refill_for_waiter();
                            service.changed.notify_all();
                        }
                        if matches!(
                            wire.request,
                            Request::AwaitPoolReady { .. }
                                | Request::ReservePoolWorker
                                | Request::AwaitPoolActivation
                        ) && service
                            .pool
                            .as_ref()
                            .is_some_and(|pool| pool.failed.load(Ordering::Acquire))
                        {
                            break Err(RpcError::new(
                                ErrorCode::Aborted,
                                "prewarm pool preparation failed",
                            ));
                        }
                        if matches!(wire.request, Request::AwaitPrewarmReady { .. })
                            && service
                                .prewarm_process
                                .as_ref()
                                .is_some_and(|process| process.has_exited().unwrap_or(true))
                        {
                            break Err(RpcError::new(
                                ErrorCode::Aborted,
                                "prewarming worker exited",
                            ));
                        }
                        manager = service.changed.wait(manager).unwrap();
                    } else {
                        break result;
                    }
                }
            };
            service.changed.notify_all();
            let stop = shutdown && result.is_ok();
            let sent = write_frame(
                &mut pipe,
                &WireResponse {
                    id: wire.id,
                    result,
                },
            );
            // A failed reply must not cancel an already accepted shutdown.
            if stop {
                service.stop();
            }
            sent?;
            if stop {
                break;
            }
        }
        Ok(())
    })();
    service.manager.lock().unwrap().disconnect(client);
    service.changed.notify_all();
    result
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut endpoint = None;
    let mut controller_pid = None;
    let mut web_address = None;
    let (mut prewarm_root, mut prewarm_dist) = (None, None);
    let mut prewarm_command = Vec::new();
    let mut pool_size = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--pipe" => endpoint = args.next(),
            "--web" => web_address = Some(args.next().ok_or("missing --web address")?),
            "--prewarm-root" => prewarm_root = Some(args.next().ok_or("missing prewarm root")?),
            "--prewarm-dist" => prewarm_dist = Some(args.next().ok_or("missing prewarm distribution")?),
            "--prewarm-pool" => pool_size = Some(args.next().ok_or("missing pool size")?.parse::<usize>()?),
            "--" => { prewarm_command.extend(args); break; }
            "--controller-pid" => controller_pid = Some(args.next().ok_or("missing controller PID")?.parse::<u32>()?),
            _ => return Err("usage: init --pipe ENDPOINT --controller-pid PID [--web 127.0.0.1:PORT] (KINAKAZE_V2_TOKEN required)".into()),
        }
    }
    let endpoint = endpoint.ok_or("missing --pipe")?;
    let token = std::env::var("KINAKAZE_V2_TOKEN")?;
    if !(32..=512).contains(&token.len()) {
        return Err("session credential must have 32..=512 bytes".into());
    }
    let controller = ProcessHandle::open(controller_pid.ok_or("missing --controller-pid")?)?;
    if controller.pid() == std::process::id() {
        return Err("controller must be a separate native process".into());
    }
    let mut listener = PipeListener::bind(&endpoint)?;
    let job = Job::new_kill_on_close()?;
    let pool = if let Some(size) = pool_size {
        if !prewarm_command.is_empty() {
            return Err(
                "a generic pool takes the application at activation, not on init's command line"
                    .into(),
            );
        }
        Some(pool::Pool::new(
            size,
            prewarm_root
                .as_ref()
                .ok_or("pool requires --prewarm-root")?
                .into(),
            prewarm_dist
                .as_ref()
                .ok_or("pool requires --prewarm-dist")?
                .into(),
        )?)
    } else {
        None
    };
    let _prewarm_child = if pool.is_some() {
        None
    } else {
        match (prewarm_root, prewarm_dist, prewarm_command.is_empty()) {
            (None, None, true) => None,
            (Some(root), Some(dist), false) => {
                use std::os::windows::process::CommandExt;
                use std::path::PathBuf;
                use std::process::Command;
                if !prewarm_command[0].starts_with('/') {
                    return Err("prewarm executable must be an absolute Linux path".into());
                }
                let root = PathBuf::from(root).canonicalize()?;
                let dist = PathBuf::from(dist).canonicalize()?;
                let mut child = Command::new(std::env::current_exe()?.with_file_name("worker.exe"))
                    .arg("guest-prewarm")
                    .arg("--root")
                    .arg(&root)
                    .arg("--dist")
                    .arg(&dist)
                    .arg("--")
                    .args(&prewarm_command)
                    .env("KINAKAZE_V2_ENDPOINT", &endpoint)
                    .env("KINAKAZE_V2_TOKEN", &token)
                    .env("KINAKAZE_V2_ROOT", &root)
                    .env("KINAKAZE_V2_DIST", &dist)
                    .env_remove("KINAKAZE_V2_ADOPTION")
                    .env_remove("KINAKAZE_V2_ADOPTION_TICKET")
                    .creation_flags(kinakaze_v2_host_win::background_creation_flags())
                    .spawn()?;
                let assigned =
                    ProcessHandle::open(child.id()).and_then(|process| job.assign(&process));
                if let Err(error) = assigned {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
                Some(child)
            }
            _ => {
                return Err(
                "prewarm requires --prewarm-root ROOT --prewarm-dist DIST -- /program [args...]"
                    .into(),
            );
            }
        }
    };
    let prewarm_process = _prewarm_child
        .as_ref()
        .map(|child| ProcessHandle::open(child.id()))
        .transpose()?;
    let epoch = u64::from_str_radix(&random_token()?[..16], 16)?.max(1);
    let service = Arc::new(Service {
        manager: Mutex::new(StateManager::new(epoch, token.clone())),
        changed: Condvar::new(),
        stopping: AtomicBool::new(false),
        connections: AtomicUsize::new(0),
        endpoint,
        token,
        controller: PeerIdentity {
            host_pid: controller.pid(),
            birth: controller.birth(),
        },
        job,
        prewarm_process,
        pool,
    });
    let _web = web_address
        .map(|address| web::Server::start(&address, Arc::clone(&service)))
        .transpose()?;
    let watcher = Arc::clone(&service);
    std::thread::Builder::new()
        .name("controller-exit".into())
        .spawn(move || {
            let _ = controller.wait();
            watcher.stop();
        })?;
    if let Some(child) = &_prewarm_child {
        let process = ProcessHandle::open(child.id())?;
        let watcher = Arc::clone(&service);
        std::thread::Builder::new()
            .name("prewarm-exit".into())
            .spawn(move || {
                let _ = process.wait();
                // Share the wait mutex so early bootstrap death cannot lose a wakeup.
                let _manager = watcher.manager.lock().unwrap();
                watcher.changed.notify_all();
            })?;
    }
    if service.pool.is_some() {
        pool::start(Arc::clone(&service))?;
    }
    println!("READY");
    io::stdout().flush()?;
    while !service.stopping.load(Ordering::Acquire) {
        let pipe = listener.accept()?;
        if service.stopping.load(Ordering::Acquire) {
            break;
        }
        if service.connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            service.connections.fetch_sub(1, Ordering::AcqRel);
            drop(pipe);
            continue;
        }
        let slot = ConnectionSlot(Arc::clone(&service));
        let shared = Arc::clone(&service);
        std::thread::Builder::new()
            .name("control-rpc".into())
            .spawn(move || {
                if let Err(error) = serve(pipe, shared, slot)
                    && !matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe
                    )
                {
                    // Do not include request payloads: Hello contains credentials.
                    eprintln!("control connection closed: {:?}", error.kind());
                }
            })?;
    }
    // Returning main terminates handler threads and closes this process's Job
    // handle, including Arc copies, so no owned worker outlives the session.
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("init: {error}");
        std::process::exit(1);
    }
}
