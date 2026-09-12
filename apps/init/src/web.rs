//! Small, read-only, loopback dashboard. Fixed workers bound both memory and sockets.
use crate::Service;
use kinakaze_v2_host_win::process_metrics;
use serde::Serialize;
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
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const WORKERS: usize = 2;

pub struct Server {
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
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
                            let _ = serve(&mut stream, &authority, &service, started);
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

fn request_path<'a>(request: &'a str, authority: &str) -> Result<&'a str, &'static str> {
    let mut lines = request.split("\r\n");
    let mut first = lines.next().ok_or("400 Bad Request")?.split(' ');
    let method = first.next().ok_or("400 Bad Request")?;
    let path = first.next().ok_or("400 Bad Request")?;
    if first.next() != Some("HTTP/1.1") || first.next().is_some() {
        return Err("400 Bad Request");
    }
    if method != "GET" {
        return Err("405 Method Not Allowed");
    }
    let mut host = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("400 Bad Request")?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("host") {
            if host.replace(value).is_some() {
                return Err("400 Bad Request");
            }
        } else if name.eq_ignore_ascii_case("origin") && value != format!("http://{authority}") {
            return Err("403 Forbidden");
        } else if name.eq_ignore_ascii_case("sec-fetch-site")
            && !matches!(value, "same-origin" | "none")
        {
            return Err("403 Forbidden");
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value != "0")
        {
            return Err("400 Bad Request");
        }
    }
    // Pin the browser origin, including port, against DNS rebinding.
    if host != Some(authority) {
        return Err("403 Forbidden");
    }
    Ok(path)
}

fn respond(stream: &mut TcpStream, status: &str, mime: &str, body: &[u8]) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn serve(
    stream: &mut TcpStream,
    authority: &str,
    service: &Service,
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
    let path = std::str::from_utf8(&buffer[..end])
        .map_err(|_| "400 Bad Request")
        .and_then(|request| request_path(request, authority));
    match path {
        Ok("/") => respond(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            include_bytes!("web/index.html"),
        ),
        Ok("/app.js") => respond(
            stream,
            "200 OK",
            "text/javascript; charset=utf-8",
            include_bytes!("web/app.js"),
        ),
        Ok("/style.css") => respond(
            stream,
            "200 OK",
            "text/css; charset=utf-8",
            include_bytes!("web/style.css"),
        ),
        Ok("/api/state") => {
            let body = snapshot(service, started)?;
            respond(stream, "200 OK", "application/json", &body)
        }
        Ok(_) => respond(stream, "404 Not Found", "text/plain", b"Not found\n"),
        Err(status) => respond(stream, status, "text/plain", status.as_bytes()),
    }
}

#[derive(Serialize)]
struct NativeMetrics {
    working_set_bytes: usize,
    private_bytes: usize,
    cpu_time_ms: u64,
}

fn snapshot(service: &Service, started: Instant) -> io::Result<Vec<u8>> {
    let state = service
        .manager
        .lock()
        .map_err(|_| io::Error::other("manager unavailable"))?
        .snapshot();
    #[derive(Serialize)]
    struct Process {
        #[serde(flatten)]
        process: kinakaze_v2_manager::ProcessSnapshot,
        native: Option<NativeMetrics>,
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
    }
    // Native calls and JSON encoding never run under the control-plane mutex.
    let processes = state
        .processes
        .into_iter()
        .map(|process| {
            let native = process_metrics(process.host_pid, process.host_birth)
                .ok()
                .map(|metrics| NativeMetrics {
                    working_set_bytes: metrics.working_set_bytes,
                    private_bytes: metrics.private_bytes,
                    cpu_time_ms: metrics.cpu_time_ms,
                });
            Process { process, native }
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
            request_path(
                "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n",
                authority
            ),
            Ok("/api/state")
        );
        for headers in [
            "Host: attacker.invalid",
            "Host: 127.0.0.1:8080\r\nOrigin: https://attacker.invalid",
            "Host: 127.0.0.1:8080\r\nSec-Fetch-Site: cross-site",
        ] {
            assert_eq!(
                request_path(&format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n"), authority),
                Err("403 Forbidden")
            );
        }
        for headers in [
            "Host: 127.0.0.1:8080\r\nHost: 127.0.0.1:8080",
            "Host: 127.0.0.1:8080\r\nContent-Length: 1",
            "Host: 127.0.0.1:8080\r\nTransfer-Encoding: chunked",
        ] {
            assert_eq!(
                request_path(&format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n"), authority),
                Err("400 Bad Request")
            );
        }
        assert_eq!(
            request_path("POST / HTTP/1.1\r\n\r\n", authority),
            Err("405 Method Not Allowed")
        );
    }

    #[test]
    fn real_http_snapshot_and_shutdown() {
        let service = Arc::new(Service {
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
            controller: kinakaze_v2_manager::PeerIdentity {
                host_pid: 1,
                birth: 1,
            },
            job: kinakaze_v2_host_win::Job::new_kill_on_close().unwrap(),
        });
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
        assert!(!response.contains("secret"));
        let address = server.address;
        drop(server);
        assert!(TcpStream::connect(address).is_err());
    }
}
