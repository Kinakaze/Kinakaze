//! Loopback application console. Fixed workers and bounded requests; no idle polling.
use crate::Service;
use kinakaze_v2_host_win::{ProcessHandle, process_metrics, random_token};
use kinakaze_v2_manager::{PeerIdentity, ProcessSnapshot, ProcessStatus};
use kinakaze_v2_protocol::{ClientRole, Hello, PROTOCOL_VERSION, PoolLaunch, Reply, Request};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const REQUEST_LIMIT: usize = 8192;
const BODY_LIMIT: usize = 16384;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const WORKERS: usize = 2;

struct Context {
    csrf: String,
    peer: PeerIdentity,
}

pub struct Server {
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://{}/", self.address)
    }
    pub fn start(address: &str, service: Arc<Service>) -> io::Result<Self> {
        let address: SocketAddr = address.parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "WebUI requires 127.0.0.1:PORT")
        })?;
        if address.ip() != std::net::Ipv4Addr::LOCALHOST {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WebUI requires 127.0.0.1:PORT",
            ));
        }
        let listener = TcpListener::bind(address)?;
        let process = ProcessHandle::open(std::process::id())?;
        let context = Arc::new(Context {
            csrf: random_token()?,
            peer: PeerIdentity {
                host_pid: process.pid(),
                birth: process.birth(),
            },
        });
        let mut server = Self {
            address: listener.local_addr()?,
            stopped: Arc::new(AtomicBool::new(false)),
            threads: Vec::with_capacity(WORKERS),
        };
        let started = Instant::now();
        for _ in 0..WORKERS {
            let listener = listener.try_clone()?;
            let service = Arc::clone(&service);
            let stopped = Arc::clone(&server.stopped);
            let authority = server.address.to_string();
            let context = Arc::clone(&context);
            server.threads.push(
                std::thread::Builder::new()
                    .name("init-web".into())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        // Blocking accept: no timers, background snapshots or idle polling.
                        while !stopped.load(Ordering::Acquire) {
                            let Ok((mut stream, _)) = listener.accept() else {
                                break;
                            };
                            if stopped.load(Ordering::Acquire) {
                                break;
                            }
                            let _ = serve(&mut stream, &authority, &service, &context, started);
                        }
                    })?,
            );
        }
        eprintln!("Kinakaze WebUI: http://{}/", server.address);
        Ok(server)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        for _ in &self.threads {
            let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, PartialEq)]
struct HttpRequest<'a> {
    path: &'a str,
    post: bool,
    length: usize,
}

fn parse_request<'a>(
    request: &'a str,
    authority: &str,
    csrf: &str,
) -> Result<HttpRequest<'a>, &'static str> {
    let mut lines = request.split("\r\n");
    let mut first = lines.next().ok_or("400 Bad Request")?.split(' ');
    let method = first.next().ok_or("400 Bad Request")?;
    let path = first.next().ok_or("400 Bad Request")?;
    if first.next() != Some("HTTP/1.1") || first.next().is_some() {
        return Err("400 Bad Request");
    }
    if !matches!(method, "GET" | "POST") {
        return Err("405 Method Not Allowed");
    }
    let mut host = None;
    let mut origin = None;
    let mut token = None;
    let mut length = None;
    let mut content_type = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("400 Bad Request")?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            if host.replace(value).is_some() {
                return Err("400 Bad Request");
            }
        } else if name.eq_ignore_ascii_case("origin") {
            if origin.replace(value).is_some() || value != format!("http://{authority}") {
                return Err("403 Forbidden");
            }
        } else if name.eq_ignore_ascii_case("x-kinakaze-token") {
            if token.replace(value).is_some() {
                return Err("400 Bad Request");
            }
        } else if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() || value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
                return Err("400 Bad Request");
            }
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "413 Content Too Large")?,
            );
        } else if name.eq_ignore_ascii_case("content-type") {
            if content_type.replace(value).is_some() {
                return Err("400 Bad Request");
            }
        } else if name.eq_ignore_ascii_case("sec-fetch-site")
            && !matches!(value, "same-origin" | "none")
        {
            return Err("403 Forbidden");
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("400 Bad Request");
        }
    }
    // Pin the browser origin, including port, against DNS rebinding.
    if host != Some(authority) {
        return Err("403 Forbidden");
    }
    let post = method == "POST";
    if post {
        if origin.is_none() || token != Some(csrf) {
            return Err("403 Forbidden");
        }
        if !content_type.is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap()
                .trim()
                .eq_ignore_ascii_case("application/json")
        }) {
            return Err("415 Unsupported Media Type");
        }
        if length.is_none() {
            return Err("411 Length Required");
        }
    } else if length.unwrap_or(0) != 0 {
        return Err("400 Bad Request");
    }
    let length = length.unwrap_or(0);
    if length > BODY_LIMIT {
        return Err("413 Content Too Large");
    }
    Ok(HttpRequest { path, post, length })
}

fn respond(stream: &mut TcpStream, status: &str, mime: &str, body: &[u8]) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn serve(
    stream: &mut TcpStream,
    authority: &str,
    service: &Service,
    context: &Context,
    started: Instant,
) -> io::Result<()> {
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut buffer = [0u8; REQUEST_LIMIT];
    let mut length = 0;
    let end = loop {
        if length == buffer.len() {
            return respond(
                stream,
                "431 Request Header Fields Too Large",
                "text/plain",
                b"Headers too large\n",
            );
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut buffer[length..])?;
        if count == 0 {
            return Ok(());
        }
        length += count;
        if let Some(end) = buffer[..length]
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
        {
            break end + 4;
        }
    };
    let request = std::str::from_utf8(&buffer[..end])
        .map_err(|_| "400 Bad Request")
        .and_then(|request| parse_request(request, authority, &context.csrf));
    let request = match request {
        Ok(request) => request,
        Err(status) => return respond(stream, status, "text/plain", status.as_bytes()),
    };
    if request.post {
        if !matches!(request.path, "/api/launch" | "/api/terminate") {
            return respond(stream, "404 Not Found", "text/plain", b"Not found\n");
        }
        let mut body = vec![0u8; request.length];
        let received = (length - end).min(body.len());
        body[..received].copy_from_slice(&buffer[end..end + received]);
        let mut read = received;
        while read < body.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            stream.set_read_timeout(Some(remaining))?;
            let count = stream.read(&mut body[read..])?;
            if count == 0 {
                return Ok(());
            }
            read += count;
        }
        let result = if request.path == "/api/launch" {
            launch(service, context, &body)
        } else {
            terminate(service, &body)
        };
        let (status, body) = match result {
            Ok(body) => ("202 Accepted", body),
            Err((status, message)) => (status, serde_json::json!({"error": message})),
        };
        return respond(
            stream,
            status,
            "application/json",
            &serde_json::to_vec(&body)?,
        );
    }
    match request.path {
        "/" => respond(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            include_bytes!("web/index.html"),
        ),
        "/app.js" => respond(
            stream,
            "200 OK",
            "text/javascript; charset=utf-8",
            include_bytes!("web/app.js"),
        ),
        "/style.css" => respond(
            stream,
            "200 OK",
            "text/css; charset=utf-8",
            include_bytes!("web/style.css"),
        ),
        "/favicon.svg" => respond(
            stream,
            "200 OK",
            "image/svg+xml",
            include_bytes!("web/favicon.svg"),
        ),
        "/character.png" => respond(
            stream,
            "200 OK",
            "image/png",
            include_bytes!("web/character.png"),
        ),
        "/api/state" => {
            let body = snapshot(service, context, started)?;
            respond(stream, "200 OK", "application/json", &body)
        }
        _ => respond(stream, "404 Not Found", "text/plain", b"Not found\n"),
    }
}

#[derive(Serialize)]
struct NativeMetrics {
    working_set_bytes: usize,
    private_bytes: usize,
    cpu_time_ms: u64,
}

type ActionResult = Result<serde_json::Value, (&'static str, &'static str)>;

fn launch(service: &Service, context: &Context, body: &[u8]) -> ActionResult {
    let launch: PoolLaunch = serde_json::from_slice(body)
        .map_err(|_| ("400 Bad Request", "启动参数格式不正确，请检查输入。"))?;
    if !launch.valid()
        || launch.arguments.len() > 128
        || launch.environment.as_ref().is_some_and(|env| {
            env.len() > 128
                || env.iter().any(|entry| {
                    entry.split_once('=').is_none_or(|(name, _)| {
                        name.is_empty()
                            || !name.bytes().enumerate().all(|(i, c)| {
                                c == b'_'
                                    || c.is_ascii_alphabetic()
                                    || (i > 0 && c.is_ascii_digit())
                            })
                    })
                })
        })
    {
        return Err((
            "400 Bad Request",
            "程序与工作目录须为 Linux 绝对路径；环境变量须为 NAME=VALUE，参数和变量各不超过 128 项。",
        ));
    }
    if let Some(desktop) = &service.desktop {
        let environment: std::collections::BTreeMap<_, _> = launch
            .environment
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                entry
                    .split_once('=')
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
            })
            .collect();
        let name = format!(
            "app-{}",
            &random_token().map_err(|_| ("503 Service Unavailable", "无法分配会话"))?[..12]
        );
        let result = desktop.call(serde_json::json!({"op":"start", "terminal":{"name":name,"command":launch.arguments,"cwd":launch.cwd,"environment":environment,"autostart":true}}))
            .map_err(|_| ("503 Service Unavailable", "客体会话服务未就绪或启动失败"))?;
        let _ = desktop.open(&name);
        return Ok(result);
    }
    let pool = service.pool.as_ref().ok_or((
        "409 Conflict",
        "此会话不支持启动程序。请使用独立会话模式启动控制台。",
    ))?;
    let mut manager = service
        .manager
        .lock()
        .map_err(|_| ("503 Service Unavailable", "会话暂时不可用。"))?;
    if service.stopping.load(Ordering::Acquire) || pool.failed.load(Ordering::Acquire) {
        return Err((
            "503 Service Unavailable",
            "会话正在停止或准备失败，请检查启动终端。",
        ));
    }
    if manager.pool_ready_count() == 0 {
        pool.refill_for_waiter();
        service.changed.notify_all();
        return Err((
            "503 Service Unavailable",
            "运行环境正在准备，请稍后再次启动。",
        ));
    }
    let (client, _) = manager
        .connect(
            Hello {
                version: PROTOCOL_VERSION,
                token: service.token.clone(),
                role: ClientRole::Launcher,
                adoption_ticket: None,
            },
            context.peer,
        )
        .map_err(|_| {
            (
                "503 Service Unavailable",
                "暂时无法创建启动请求，请稍后重试。",
            )
        })?;
    let result = (|| {
        let Reply::PoolWorker { pid, .. } = manager
            .handle(client, Request::ReservePoolWorker)
            .map_err(|_| ("409 Conflict", "运行环境已被占用，请稍后重试。"))?
        else {
            return Err(("500 Internal Server Error", "无法分配运行环境。"));
        };
        manager
            .handle(client, Request::ActivatePoolWorker { pid, launch })
            .map_err(|_| ("409 Conflict", "启动请求未被接受，请检查会话状态。"))?;
        if let Some(peer) = manager.process_peer(pid) {
            pool.consumed(peer);
        }
        Ok(serde_json::json!({"pid": pid}))
    })();
    // Releases a reservation on failure; accepted programs survive disconnect.
    manager.disconnect(client);
    service.changed.notify_all();
    result
}

fn instance(process: &ProcessSnapshot) -> String {
    format!(
        "{}:{}:{}:{}",
        process.identity.pid, process.identity.generation, process.host_pid, process.host_birth
    )
}

fn is_standby(service: &Service, process: &ProcessSnapshot) -> bool {
    process.standby
        // In pool sessions, an unused root belongs to pool maintenance even
        // before it publishes MarkPoolReady. Do not let the UI kill preparation.
        || (service.pool.is_some()
            && process.program.is_none()
            && process.identity.parent_pid == 0
            && process.identity.generation == 1)
}

fn can_terminate(service: &Service, process: &ProcessSnapshot) -> bool {
    !is_standby(service, process)
        && process.status == ProcessStatus::Active
        && process.host_pid != std::process::id()
}

fn terminate(service: &Service, body: &[u8]) -> ActionResult {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Target {
        pid: u32,
        instance: String,
    }
    let target: Target =
        serde_json::from_slice(body).map_err(|_| ("400 Bad Request", "进程身份格式不正确。"))?;
    // Keep ownership stable through native termination, including concurrent exec.
    let manager = service
        .manager
        .lock()
        .map_err(|_| ("503 Service Unavailable", "会话暂时不可用。"))?;
    if service.stopping.load(Ordering::Acquire) {
        return Err(("409 Conflict", "会话正在停止。"));
    }
    let snapshot = manager.snapshot();
    let process = snapshot
        .processes
        .iter()
        .find(|p| p.identity.pid == target.pid)
        .ok_or(("409 Conflict", "进程已经退出，请刷新列表。"))?;
    if instance(process) != target.instance || !can_terminate(service, process) {
        return Err(("409 Conflict", "进程状态已变化，请刷新后重新选择。"));
    }
    let native = ProcessHandle::open(process.host_pid)
        .map_err(|_| ("409 Conflict", "进程已退出或无法访问，请刷新列表。"))?;
    if native.birth() != process.host_birth || native.has_exited().unwrap_or(true) {
        return Err(("409 Conflict", "进程已退出，请刷新列表。"));
    }
    if let Some(desktop) = &service.desktop {
        drop(manager);
        if target.pid == 1 {
            desktop.shutdown();
        } else {
            desktop
                .call(serde_json::json!({"op":"signal","pid":target.pid}))
                .map_err(|_| ("409 Conflict", "未能向客体进程发送终止信号。"))?;
        }
        return Ok(serde_json::json!({"pid":target.pid}));
    }
    native
        .terminate(137)
        .map_err(|_| ("409 Conflict", "未能结束进程，请刷新状态后重试。"))?;
    Ok(serde_json::json!({"pid": target.pid}))
}

fn snapshot(service: &Service, context: &Context, started: Instant) -> io::Result<Vec<u8>> {
    let (state, ready) = {
        let manager = service
            .manager
            .lock()
            .map_err(|_| io::Error::other("manager unavailable"))?;
        (manager.snapshot(), manager.pool_ready_count())
    };
    #[derive(Serialize)]
    struct Process {
        #[serde(flatten)]
        process: kinakaze_v2_manager::ProcessSnapshot,
        native: Option<NativeMetrics>,
        instance: String,
        can_terminate: bool,
    }
    #[derive(Serialize)]
    struct State {
        schema_version: u32,
        version: &'static str,
        platform: &'static str,
        init_pid: u32,
        uptime_ms: u64,
        stopping: bool,
        stats: kinakaze_v2_protocol::Stats,
        processes: Vec<Process>,
        csrf_token: String,
        session_id: String,
        pool: PoolState,
        desktop: Option<serde_json::Value>,
    }
    #[derive(Serialize)]
    struct PoolState {
        enabled: bool,
        ready: usize,
        capacity: usize,
        failed: bool,
    }
    // The guest registry owns live PPID (including subreapers), not the startup
    // manager's original reservation. Never present that stale relation as a tree.
    let guest_processes = service
        .desktop
        .as_ref()
        .and_then(|d| d.call(serde_json::json!({"op":"processes"})).ok());
    // Native calls and JSON encoding never run under the control-plane mutex.
    let processes = state
        .processes
        .into_iter()
        .map(|mut process| {
            if let Some(guest) = guest_processes
                .as_ref()
                .and_then(|v| v.as_array())
                .and_then(|items| {
                    items
                        .iter()
                        .find(|p| p["pid"].as_u64() == Some(process.identity.pid as u64))
                })
            {
                if let Some(ppid) = guest["ppid"].as_u64() {
                    process.identity.parent_pid = ppid as u32;
                }
                if let Some(program) = guest["program"].as_str() {
                    process.program = Some(program.into());
                }
            }
            process.standby = is_standby(service, &process);
            let native = process_metrics(process.host_pid, process.host_birth)
                .ok()
                .map(|metrics| NativeMetrics {
                    working_set_bytes: metrics.working_set_bytes,
                    private_bytes: metrics.private_bytes,
                    cpu_time_ms: metrics.cpu_time_ms,
                });
            Process {
                instance: instance(&process),
                can_terminate: can_terminate(service, &process),
                process,
                native,
            }
        })
        .collect();
    serde_json::to_vec(&State {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION"),
        platform: "windows-x86_64",
        init_pid: std::process::id(),
        uptime_ms: started.elapsed().as_millis() as u64,
        stopping: service.stopping.load(Ordering::Acquire),
        stats: state.stats,
        processes,
        csrf_token: context.csrf.clone(),
        session_id: format!("{}:{}", context.peer.host_pid, context.peer.birth),
        desktop: service.desktop.as_ref().map(|desktop| desktop.snapshot()),
        pool: PoolState {
            enabled: service.pool.is_some(),
            ready,
            capacity: service.pool.as_ref().map_or(0, |pool| pool.size),
            failed: service
                .pool
                .as_ref()
                .is_some_and(|pool| pool.failed.load(Ordering::Acquire)),
        },
    })
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_and_body_restrictions() {
        let authority = "127.0.0.1:8080";
        assert_eq!(
            parse_request(
                "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n",
                authority,
                "csrf"
            ),
            Ok(HttpRequest {
                path: "/api/state",
                post: false,
                length: 0
            })
        );
        for headers in [
            "Host: attacker.invalid",
            "Host: 127.0.0.1:8080\r\nOrigin: https://attacker.invalid",
            "Host: 127.0.0.1:8080\r\nSec-Fetch-Site: cross-site",
        ] {
            assert_eq!(
                parse_request(
                    &format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n"),
                    authority,
                    "csrf"
                ),
                Err("403 Forbidden")
            );
        }
        for headers in [
            "Host: 127.0.0.1:8080\r\nHost: 127.0.0.1:8080",
            "Host: 127.0.0.1:8080\r\nContent-Length: 1",
            "Host: 127.0.0.1:8080\r\nTransfer-Encoding: chunked",
        ] {
            assert_eq!(
                parse_request(
                    &format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n"),
                    authority,
                    "csrf"
                ),
                Err("400 Bad Request")
            );
        }
        assert_eq!(
            parse_request("DELETE / HTTP/1.1\r\n\r\n", authority, "csrf"),
            Err("405 Method Not Allowed")
        );
    }

    #[test]
    fn mutations_require_same_origin_token_and_bounded_json() {
        let base = "POST /api/launch HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nOrigin: http://127.0.0.1:8080\r\nX-Kinakaze-Token: csrf\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n";
        let parse = |request: &str| {
            parse_request(request, "127.0.0.1:8080", "csrf").map(|r| (r.post, r.length))
        };
        assert_eq!(parse(base), Ok((true, 2)));
        for request in [
            base.replace("Origin: http://127.0.0.1:8080\r\n", ""),
            base.replace("http://127.0.0.1:8080", "https://foreign.invalid"),
            base.replace("X-Kinakaze-Token: csrf\r\n", ""),
            base.replace("Token: csrf", "Token: wrong"),
            base.replace(
                "Content-Length:",
                "Sec-Fetch-Site: cross-site\r\nContent-Length:",
            ),
        ] {
            assert_eq!(parse(&request), Err("403 Forbidden"));
        }
        for request in [
            base.replace(
                "Content-Length: 2",
                "Content-Length: 2\r\nContent-Length: 2",
            ),
            base.replace("Content-Length: 2", "Transfer-Encoding: chunked"),
            base.replace("Content-Length: 2", "Content-Length: +2"),
            base.replace("Token: csrf", "Token: csrf\r\nX-Kinakaze-Token: csrf"),
        ] {
            assert_eq!(parse(&request), Err("400 Bad Request"));
        }
        assert_eq!(
            parse(&base.replace("application/json", "text/plain")),
            Err("415 Unsupported Media Type")
        );
        assert_eq!(
            parse(&base.replace("Content-Length: 2\r\n", "")),
            Err("411 Length Required")
        );
        assert_eq!(
            parse(&base.replace("Content-Length: 2", "Content-Length: 16385")),
            Err("413 Content Too Large")
        );
    }

    #[test]
    fn launch_validation_and_stale_termination_do_not_mutate_state() {
        let service = test_service();
        let context = Context {
            csrf: "csrf".into(),
            peer: service.controller,
        };
        for body in [
            br#"{"arguments":["relative"],"cwd":"/","environment":null}"#.as_slice(),
            br#"{"arguments":["/bin/sh"],"cwd":"/","environment":["BAD-NAME=x"]}"#,
            br#"{"arguments":["/bin/sh"],"cwd":"/","environment":["MISSING_VALUE"]}"#,
            br#"{"arguments":["/bin/sh"],"cwd":"/","environment":null,"extra":true}"#,
        ] {
            assert_eq!(
                launch(&service, &context, body).unwrap_err().0,
                "400 Bad Request"
            );
        }
        assert_eq!(
            launch(
                &service,
                &context,
                br#"{"arguments":["/bin/sh"],"cwd":"/","environment":null}"#
            )
            .unwrap_err()
            .0,
            "409 Conflict"
        );
        assert_eq!(
            terminate(&service, br#"{"pid":77,"instance":"stale"}"#)
                .unwrap_err()
                .0,
            "409 Conflict"
        );
        assert_eq!(service.manager.lock().unwrap().stats().clients, 0);
        assert_eq!(service.manager.lock().unwrap().stats().processes, 0);
    }

    #[test]
    fn real_http_snapshot_and_shutdown() {
        let service = test_service();
        assert!(Server::start("0.0.0.0:0", Arc::clone(&service)).is_err());
        let server = Server::start("127.0.0.1:0", service).unwrap();
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "GET /api/state HTTP/1.1\r\nHost: {}\r\n\r\n",
            server.address
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        let state: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(state["stats"]["processes"], 0);
        assert_eq!(state["schema_version"], 1);
        assert_eq!(state["pool"]["enabled"], false);
        assert_eq!(state["csrf_token"].as_str().unwrap().len(), 64);
        assert!(!response.contains("secret"));
        // Exercise the socket body reader with a split JSON body.
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(stream, "POST /api/launch HTTP/1.1\r\nHost: {0}\r\nOrigin: http://{0}\r\nX-Kinakaze-Token: {1}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{", server.address, state["csrf_token"].as_str().unwrap()).unwrap();
        stream.write_all(b"}").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response.contains("application/json"));
        let address = server.address;
        drop(server);
        assert!(TcpStream::connect(address).is_err());
    }

    fn test_service() -> Arc<Service> {
        Arc::new(Service {
            images: std::sync::Mutex::new(crate::image_cache::Cache::default()),
            prewarm_process: None,
            pool: None,
            manager: std::sync::Mutex::new(kinakaze_v2_manager::StateManager::new(
                1,
                "secret".into(),
            )),
            changed: std::sync::Condvar::new(),
            stopping: AtomicBool::new(false),
            connections: std::sync::atomic::AtomicUsize::new(0),
            endpoint: String::new(),
            token: "secret".into(),
            launch_token: None,
            desktop: None,
            controller: kinakaze_v2_manager::PeerIdentity {
                host_pid: 1,
                birth: 1,
            },
            job: kinakaze_v2_host_win::Job::new_kill_on_close().unwrap(),
        })
    }
}
