//! Distribution-independent persistent environment and terminal transport.
use crate::{
    Service,
    desktop_client::{self as wire, Options},
};
use kinakaze_v2_host_win::{
    Job, PipeListener, ProcessHandle, StartupGate, private_file, random_token,
};
use kinakaze_v2_manager::{PeerIdentity, StateManager};
use kinakaze_v2_rootfs::startup::{Startup, Terminal};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Write},
    net::{Shutdown, TcpListener, TcpStream},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 256 * 1024;
struct TerminalState {
    pid: u32,
    parent: u32,
    output: VecDeque<u8>,
    offset: u64,
    status: Option<i32>,
    closed: bool,
    lease: Option<(u64, TcpStream)>,
}
impl TerminalState {
    fn new(pid: u32, parent: u32) -> Self {
        Self {
            pid,
            parent,
            output: VecDeque::new(),
            offset: 0,
            status: None,
            closed: false,
            lease: None,
        }
    }
}
struct State {
    ready: bool,
    failed: bool,
    terminals: BTreeMap<String, TerminalState>,
    pending: BTreeMap<u64, mpsc::SyncSender<Value>>,
}
pub(super) struct Desktop {
    pub options: Options,
    pub startup: Startup,
    token: String,
    broker_token: String,
    broker: Mutex<Option<TcpStream>>,
    state: Mutex<State>,
    changed: Condvar,
    next: AtomicU64,
    pub closing: AtomicBool,
    service: OnceLock<Weak<Service>>,
    pub web_url: OnceLock<String>,
}
impl Desktop {
    fn new(options: Options, startup: Startup) -> io::Result<Self> {
        Ok(Self {
            options,
            startup,
            token: random_token()?,
            broker_token: random_token()?,
            broker: Mutex::new(None),
            state: Mutex::new(State {
                ready: false,
                failed: false,
                terminals: BTreeMap::new(),
                pending: BTreeMap::new(),
            }),
            changed: Condvar::new(),
            next: AtomicU64::new(1),
            closing: AtomicBool::new(false),
            service: OnceLock::new(),
            web_url: OnceLock::new(),
        })
    }
    pub fn call(&self, mut request: Value) -> io::Result<Value> {
        if self.closing.load(Ordering::Acquire) && request["op"] != "shutdown" {
            return Err(io::Error::other("environment stopping"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = mpsc::sync_channel(1);
        {
            let mut state = self.state.lock().unwrap();
            if !state.ready || state.failed {
                return Err(io::Error::other("session service is not ready"));
            }
            if state.pending.len() >= 128 {
                return Err(io::Error::other("too many session requests"));
            }
            state.pending.insert(id, send);
        }
        request["id"] = id.into();
        let timeout = if request["op"] == "shutdown" {
            self.startup.shutdown_timeout_seconds
        } else {
            30
        };
        let result = (|| {
            let mut broker = self.broker.lock().unwrap();
            wire::write(
                broker
                    .as_mut()
                    .ok_or_else(|| io::Error::other("session service disconnected"))?,
                &request,
            )
        })()
        .and_then(|_| {
            receive
                .recv_timeout(Duration::from_secs(timeout))
                .map_err(io::Error::other)
        });
        self.state.lock().unwrap().pending.remove(&id);
        let result = result?;
        if let Some(error) = result["error"].as_str() {
            return Err(io::Error::other(error.to_owned()));
        }
        Ok(result["ok"].clone())
    }
    pub fn snapshot(&self) -> Value {
        let state = self.state.lock().unwrap();
        let terminals: Vec<_> = state.terminals.iter().map(|(name,t)| json!({"name":name,"pid":t.pid,"ppid":t.parent,"status":t.status,"closed":t.closed,"attached":t.lease.is_some()})).collect();
        json!({"pid":std::process::id(),"ready":state.ready,"failed":state.failed,"closing":self.closing.load(Ordering::Acquire),"default_terminal":self.startup.default_terminal,"terminals":terminals,"web":self.web_url.get()})
    }
    pub fn shutdown(self: &Arc<Self>) {
        {
            let _state = self.state.lock().unwrap();
            if self.closing.swap(true, Ordering::AcqRel) {
                return;
            }
            // Wake attached terminals now, without waiting for guest services.
            self.changed.notify_all();
        }
        let deadline = Instant::now() + Duration::from_secs(self.startup.shutdown_timeout_seconds);
        eprintln!(
            "Shutdown requested; allowing up to {}s for the configured init.",
            self.startup.shutdown_timeout_seconds
        );
        let request = self.clone();
        std::thread::spawn(move || {
            // A blocked control write/reply must not extend the shutdown deadline.
            match request.call(json!({"op":"shutdown"})) {
                Ok(_) => eprintln!("Guest shutdown request accepted."),
                Err(error) => eprintln!("Guest shutdown request: {error}"),
            }
        });
        let this = self.clone();
        std::thread::spawn(move || {
            // The configured init receives its normal shutdown request first.
            while Instant::now() < deadline {
                let Some(service) = this.service.get().and_then(Weak::upgrade) else {
                    return;
                };
                if service.stopping.load(Ordering::Acquire) {
                    return;
                }
                let manager = service.manager.lock().unwrap();
                if manager.process_peer(1).is_none() {
                    drop(manager);
                    service.stop();
                    return;
                }
                let _ = service
                    .changed
                    .wait_timeout(manager, Duration::from_millis(200))
                    .unwrap();
            }
            if let Some(service) = this.service.get().and_then(Weak::upgrade) {
                eprintln!("Shutdown deadline reached; reclaiming the owned process tree.");
                service.stop();
            }
        });
    }
    pub fn open(&self, name: &str) -> io::Result<()> {
        wire::open_terminal(&self.options, name)
    }
    fn broker_loop(self: &Arc<Self>, mut reader: BufReader<TcpStream>) -> io::Result<()> {
        {
            let mut broker = self.broker.lock().unwrap();
            if broker.is_some() {
                return Err(io::Error::other(
                    "a session service already owns this environment",
                ));
            }
            *broker = Some(reader.get_ref().try_clone()?);
        }
        let result = (|| {
            loop {
                let event = wire::read(&mut reader)?;
                let mut state = self.state.lock().unwrap();
                if let Some(id) = event["id"].as_u64() {
                    if let Some(send) = state.pending.remove(&id) {
                        let _ = send.send(event);
                    }
                } else {
                    let name = event["name"].as_str().unwrap_or("");
                    match event["event"].as_str() {
                        Some("ready") => {
                            state.ready = true;
                        }
                        Some("terminal") => {
                            state.terminals.entry(name.into()).or_insert_with(|| {
                                TerminalState::new(
                                    event["pid"].as_u64().unwrap_or(0) as u32,
                                    event["ppid"].as_u64().unwrap_or(0) as u32,
                                )
                            });
                        }
                        Some("output") => {
                            if let Some(terminal) = state.terminals.get_mut(name) {
                                let bytes = wire::unhex(event["data"].as_str().unwrap_or(""))?;
                                terminal.output.extend(bytes);
                                let trim = terminal.output.len().saturating_sub(OUTPUT_LIMIT);
                                terminal.output.drain(..trim);
                                terminal.offset += trim as u64;
                            }
                        }
                        Some("exit") => {
                            if let Some(t) = state.terminals.get_mut(name) {
                                t.status = event["status"].as_i64().map(|s| s as i32);
                            }
                        }
                        Some("closed") => {
                            if let Some(t) = state.terminals.get_mut(name) {
                                t.closed = true;
                            }
                        }
                        _ => return Err(io::Error::other("unknown session service event")),
                    }
                }
                self.changed.notify_all();
            }
        })();
        self.broker.lock().unwrap().take();
        let mut state = self.state.lock().unwrap();
        state.failed = true;
        state.pending.clear();
        self.changed.notify_all();
        drop(state);
        // Losing the owner of the PTYs is an environment failure, not a reason
        // to silently replace it and strand its original applications.
        if !self.closing.load(Ordering::Acquire) {
            self.shutdown();
        }
        result
    }
    fn attach(self: &Arc<Self>, name: String, mut reader: BufReader<TcpStream>) -> io::Result<()> {
        let lease = self.next.fetch_add(1, Ordering::Relaxed);
        let mut output = reader.get_ref().try_clone()?;
        output.set_write_timeout(Some(Duration::from_secs(3)))?;
        let mut cursor = {
            let mut state = self.state.lock().unwrap();
            let terminal = state
                .terminals
                .get_mut(&name)
                .ok_or_else(|| io::Error::other("unknown terminal"))?;
            if let Some((_, old)) = terminal.lease.take() {
                let _ = old.shutdown(Shutdown::Both);
            }
            terminal.lease = Some((lease, output.try_clone()?));
            terminal.offset
        };
        wire::write(&mut output, &json!({"ok":true}))?;
        let this = self.clone();
        let terminal_name = name.clone();
        std::thread::spawn(move || {
            loop {
                let (data, status, finished, closing) = {
                    let mut state = this.state.lock().unwrap();
                    loop {
                        let Some(t) = state.terminals.get(&terminal_name) else {
                            return;
                        };
                        if t.lease.as_ref().map(|x| x.0) != Some(lease) {
                            return;
                        }
                        if this.closing.load(Ordering::Acquire) {
                            break (Vec::new(), None, false, true);
                        }
                        cursor = cursor.max(t.offset);
                        let start = (cursor - t.offset) as usize;
                        let data: Vec<u8> =
                            t.output.iter().skip(start).take(8192).copied().collect();
                        let finished = (t.closed && t.status.is_some()) || state.failed;
                        if !data.is_empty() || finished {
                            break (data, t.status, finished, false);
                        }
                        state = this.changed.wait(state).unwrap();
                    }
                };
                if closing {
                    let _ = wire::write(&mut output, &json!({"event":"shutdown"}));
                    break;
                }
                cursor += data.len() as u64;
                if !data.is_empty() {
                    if wire::write(
                        &mut output,
                        &json!({"event":"output","data":wire::hex(&data)}),
                    )
                    .is_err()
                    {
                        break;
                    }
                } else if finished {
                    let _ = wire::write(
                        &mut output,
                        &json!({"event":"exit","status":status.unwrap_or(125)}),
                    );
                    break;
                }
            }
            let _ = output.shutdown(Shutdown::Both);
        });
        reader.get_ref().set_read_timeout(None)?;
        while let Ok(mut request) = wire::read(&mut reader) {
            if !matches!(request["op"].as_str(), Some("input" | "resize")) {
                break;
            }
            if self
                .state
                .lock()
                .unwrap()
                .terminals
                .get(&name)
                .and_then(|t| t.lease.as_ref())
                .map(|x| x.0)
                != Some(lease)
            {
                break;
            }
            request["name"] = name.clone().into();
            // A resize can race the slave's close. Preserve the output thread's
            // final bytes and Linux exit-status event instead of cutting it off.
            if let Err(error) = self.call(request) {
                let state = self.state.lock().unwrap();
                if state.failed || self.closing.load(Ordering::Acquire) {
                    break;
                }
                if state
                    .terminals
                    .get(&name)
                    .is_some_and(|t| t.closed || t.status.is_some())
                {
                    continue;
                }
                eprintln!("terminal input: {error}");
            }
        }
        let mut state = self.state.lock().unwrap();
        if let Some(t) = state.terminals.get_mut(&name) {
            if t.lease.as_ref().map(|x| x.0) == Some(lease) {
                t.lease.take();
            }
        }
        self.changed.notify_all();
        let _ = reader.get_ref().shutdown(Shutdown::Both);
        Ok(())
    }
    fn connection(self: Arc<Self>, stream: TcpStream) -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream);
        let hello = wire::read(&mut reader)?;
        if hello["role"] == "broker" && hello["token"].as_str() == Some(&self.broker_token) {
            reader.get_ref().set_read_timeout(None)?;
            return self.broker_loop(reader);
        }
        if hello["role"] != "client" || hello["token"].as_str() != Some(&self.token) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid session credential",
            ));
        }
        wire::write(reader.get_mut(), &json!({"ok":true}))?;
        while let Ok(request) = wire::read(&mut reader) {
            let response = match request["op"].as_str() {
                Some("status") => self.snapshot(),
                Some("shutdown") => {
                    self.shutdown();
                    json!({"ok":true})
                }
                Some("attach") => {
                    let name = request["name"].as_str().unwrap_or("").to_owned();
                    if !self.state.lock().unwrap().terminals.contains_key(&name) {
                        wire::write(
                            reader.get_mut(),
                            &json!({"error":"terminal has not been started"}),
                        )?;
                        continue;
                    }
                    return self.attach(name, reader);
                }
                Some("start") => {
                    let validation = if let Some(terminal) = request.get("terminal") {
                        serde_json::from_value::<Terminal>(terminal.clone())
                            .map_err(|e| e.to_string())
                            .and_then(|t| t.validate())
                    } else {
                        Ok(())
                    };
                    match validation
                        .map_err(io::Error::other)
                        .and_then(|_| self.call(request))
                    {
                        Ok(value) => value,
                        Err(error) => json!({"error":error.to_string()}),
                    }
                }
                _ => json!({"error":"unknown session operation"}),
            };
            wire::write(reader.get_mut(), &response)?;
        }
        Ok(())
    }
}
struct Published {
    path: PathBuf,
    file: Option<File>,
}
impl Published {
    fn protect_guest(&mut self) -> io::Result<()> {
        // The original private handle denies secondary writers, including EA
        // writers. Close it only before boot, set Linux root:root 0600, reopen.
        drop(self.file.take());
        kinakaze_v2_rootfs::protect_session_config(&self.path)?;
        self.file = Some(File::open(&self.path)?);
        Ok(())
    }
    fn new(path: PathBuf, value: Value) -> wire::Result<Self> {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        let mut file = private_file(&path)?;
        file.write_all(&serde_json::to_vec(&value)?)?;
        file.sync_all()?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
}
impl Drop for Published {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn run(arguments: Vec<String>) -> wire::Result<i32> {
    let options = Options::parse(arguments)?;
    // This handle is held until every worker has stopped. It serializes startup
    // even before downloads finish and canonicalizes alternate root spellings.
    fs::create_dir_all(options.control_dir())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(options.control_dir().join("owner.lock"));
    let _lock = match lock {
        Ok(lock) => lock,
        Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => return Ok(2),
        Err(e) => return Err(e.into()),
    };
    let mut startup =
        kinakaze_v2_rootfs::startup::load(&options.dist, options.manifest.as_deref())?;
    if startup.command.is_empty() {
        return Err("manifest needs startup.command, terminals and a session service".into());
    }
    if let Some(web) = &options.web {
        startup.web = web.clone();
    }
    startup.validate()?;
    eprintln!("Checking rootfs: {}", options.root.display());
    kinakaze_v2_rootfs::prepare(&options.root, &options.dist, options.manifest.as_deref())?;
    eprintln!("Rootfs ready; starting the configured PID 1...");
    let desktop = Arc::new(Desktop::new(options, startup)?);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let endpoint = format!(r"\\.\pipe\kinakaze-v2-{}", random_token()?);
    let token = random_token()?;
    let mut pipes = PipeListener::bind(&endpoint)?;
    let native = ProcessHandle::open(std::process::id())?;
    let epoch = u64::from_str_radix(&random_token()?[..16], 16)?.max(1);
    let service = Arc::new(Service {
        manager: Mutex::new(StateManager::for_process_tree(epoch, token.clone())),
        images: Mutex::new(crate::image_cache::Cache::default()),
        kernel: Mutex::new(crate::kernel::Kernel::new(epoch)),
        changed: Condvar::new(),
        stopping: AtomicBool::new(false),
        connections: AtomicUsize::new(0),
        endpoint,
        token,
        launch_token: None,
        controller: PeerIdentity {
            host_pid: native.pid(),
            birth: native.birth(),
        },
        job: Job::new_kill_on_close()?,
        prewarm_process: None,
        pool: None,
        desktop: Some(desktop.clone()),
    });
    crate::kernel::start_collection(&service)?;
    desktop.service.set(Arc::downgrade(&service)).ok();
    let web = crate::web::Server::start(&desktop.startup.web, service.clone())?;
    desktop.web_url.set(web.url()).ok();
    let config_path = desktop
        .options
        .root
        .join(desktop.startup.session_config.trim_start_matches('/'));
    let mut part = desktop.options.root.clone();
    for component in
        PathBuf::from(desktop.startup.session_config.trim_start_matches('/')).components()
    {
        part.push(component);
        if let Ok(meta) = fs::symlink_metadata(&part) {
            if meta.file_attributes() & 0x400 != 0 {
                return Err("session config cannot traverse a reparse point".into());
            }
        }
    }
    fs::create_dir_all(config_path.parent().unwrap())?;
    let mut config = Published::new(
        config_path,
        json!({"port":port,"token":desktop.broker_token,"startup":desktop.startup}),
    )?;
    config.protect_guest()?;
    let shared = desktop.clone();
    std::thread::spawn(move || {
        let clients = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming().flatten() {
            if clients.fetch_add(1, Ordering::AcqRel) >= 64 {
                clients.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let desktop = shared.clone();
            let clients = clients.clone();
            std::thread::spawn(move || {
                let _ = desktop.connection(stream);
                clients.fetch_sub(1, Ordering::AcqRel);
            });
        }
    });
    // Assign PID 1 before releasing its startup gate. Every descendant inherits
    // the same kill-on-close Job, including fork/exec bootstrap failures.
    let gate = StartupGate::new()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(desktop.options.root.join(".kinakaze-boot.log"))?;
    let mut child = Command::new(desktop.options.dist.join("worker.exe"))
        .arg("guest")
        .arg("--root")
        .arg(&desktop.options.root)
        .arg("--dist")
        .arg(&desktop.options.dist)
        .arg("--")
        .args(&desktop.startup.command)
        .env("KINAKAZE_V2_ENDPOINT", &service.endpoint)
        .env("KINAKAZE_V2_TOKEN", &service.token)
        .env("KINAKAZE_V2_ROOT", &desktop.options.root)
        .env("KINAKAZE_V2_DIST", &desktop.options.dist)
        .env("KINAKAZE_V2_STARTUP_GATE", gate.name())
        .env_remove("KINAKAZE_V2_ADOPTION")
        .env_remove("KINAKAZE_V2_ADOPTION_TICKET")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(0x08000000)
        .spawn()?;
    if let Err(error) = ProcessHandle::open(child.id()).and_then(|p| service.job.assign(&p)) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let _child = child;
    let rpc = service.clone();
    std::thread::spawn(move || {
        while !rpc.stopping.load(Ordering::Acquire) {
            let Ok(pipe) = pipes.accept() else {
                rpc.stop();
                break;
            };
            if rpc.stopping.load(Ordering::Acquire) {
                break;
            }
            if rpc.connections.fetch_add(1, Ordering::AcqRel) >= super::MAX_CONNECTIONS {
                rpc.connections.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let shared = rpc.clone();
            let slot = super::ConnectionSlot(rpc.clone());
            std::thread::spawn(move || {
                let _ = super::serve(pipe, shared, slot);
            });
        }
    });
    gate.release()?;
    eprintln!(
        "Waiting for the guest session service (boot log: {})...",
        desktop.options.root.join(".kinakaze-boot.log").display()
    );
    let ready_deadline = Instant::now() + Duration::from_secs(90);
    {
        let mut state = desktop.state.lock().unwrap();
        while !state.ready && !state.failed && !service.stopping.load(Ordering::Acquire) {
            let remaining = ready_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                service.stop();
                return Err(
                    "configured PID 1 did not start the session service; see .kinakaze-boot.log"
                        .into(),
                );
            }
            state = desktop
                .changed
                .wait_timeout(state, remaining.min(Duration::from_secs(1)))
                .unwrap()
                .0;
        }
        if !state.ready {
            service.stop();
            return Err("boot/session service failed".into());
        }
    }
    let descriptor = Published::new(
        desktop.options.descriptor(),
        json!({"pid":native.pid(),"birth":native.birth(),"port":port,"token":desktop.token}),
    )?;
    let tray = if desktop.startup.tray && !desktop.options.no_tray {
        Some(crate::tray::start(desktop.clone())?)
    } else {
        None
    };
    eprintln!("Environment ready: {}", desktop.options.root.display());
    let mut manager = service.manager.lock().unwrap();
    while !service.stopping.load(Ordering::Acquire) {
        manager = service.changed.wait(manager).unwrap();
    }
    drop(manager);
    // Explicit termination completes before releasing the root lock; no second
    // boot can overlap live workers from this generation.
    service.job.terminate_and_wait()?;
    eprintln!("Process tree stopped; all owned workers have exited.");
    if let Some(tray) = tray {
        tray.close();
    }
    let descriptor_path = descriptor.path.clone();
    drop(descriptor);
    let _ = fs::remove_file(descriptor_path);
    let config_path = config.path.clone();
    drop(config);
    let _ = fs::remove_file(config_path);
    drop(web);
    Ok(0)
}
