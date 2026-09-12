//! An ordinary child process owns egress relays, so guest fork/exec cannot
//! destroy established flows. Control travels over private inherited pipes.
use super::usernet::Address;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Networking::WinSock::*;

struct Broker {
    process: Child,
    input: ChildStdin,
    output: ChildStdout,
}
static BROKER: Mutex<Option<Broker>> = Mutex::new(None);
const PROTOCOL_SIZE: usize = std::mem::size_of::<WSAPROTOCOL_INFOW>();
const REQUEST_SIZE: usize = 36 + PROTOCOL_SIZE;
fn io_errno(e: std::io::Error) -> i32 {
    e.raw_os_error()
        .map(crate::socket::errno_from_wsa)
        .unwrap_or(crate::EIO)
}
fn address(value: Address) -> SocketAddr {
    if value.family == 2 {
        SocketAddr::from((
            std::net::Ipv4Addr::from(<[u8; 4]>::try_from(&value.ip[..4]).unwrap()),
            value.port,
        ))
    } else {
        SocketAddr::V6(std::net::SocketAddrV6::new(
            std::net::Ipv6Addr::from(value.ip),
            value.port,
            0,
            value.scope,
        ))
    }
}
fn guest(value: SocketAddr) -> Address {
    let mut result = Address {
        port: value.port(),
        ..Address::default()
    };
    match value {
        SocketAddr::V4(a) => {
            result.family = 2;
            result.ip[..4].copy_from_slice(&a.ip().octets());
        }
        SocketAddr::V6(a) => {
            result.family = 10;
            result.ip = a.ip().octets();
            result.scope = a.scope_id();
        }
    }
    result
}
fn loopback(family: u16) -> SocketAddr {
    if family == 2 {
        SocketAddr::from(([127, 0, 0, 1], 0))
    } else {
        SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0))
    }
}
fn native_socket(protocol: &WSAPROTOCOL_INFOW) -> Result<SOCKET, i32> {
    crate::socket::ensure_winsock()?;
    let raw = unsafe { WSASocketW(-1, -1, -1, protocol, 0, WSA_FLAG_OVERLAPPED) };
    if raw == INVALID_SOCKET {
        Err(crate::socket::last_wsa_errno())
    } else {
        Ok(raw)
    }
}
struct Socket(SOCKET);
impl Drop for Socket {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.0);
        }
    }
}

/// Dispatch TCP connect asynchronously in the broker. The duplicated socket is
/// the same Winsock description monitored by the guest's existing AFD poll.
pub(crate) fn request(kind: i32, lease: u64, target: Address, raw: usize) -> Result<Address, i32> {
    #[cfg(test)]
    if matches!(kind, 3 | 4 | 5 | 6) {
        return crate::usernet::packet::gateway::start(kind, lease, target)
            .map(|(address, _)| address);
    }
    let mut slot = BROKER.lock().map_err(|_| crate::EIO)?;
    if slot
        .as_mut()
        .is_some_and(|b| b.process.try_wait().ok().flatten().is_some())
    {
        *slot = None;
    }
    if slot.is_none() {
        let executable = std::env::current_exe().map_err(io_errno)?;
        let mut process = crate::with_exec_handle_filter(|| {
            Command::new(executable)
                .arg("--usernet-broker")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x08000000)
                .spawn()
        })
        .map_err(io_errno)?
        .map_err(io_errno)?;
        let input = process.stdin.take().ok_or(crate::EIO)?;
        let output = process.stdout.take().ok_or(crate::EIO)?;
        *slot = Some(Broker {
            process,
            input,
            output,
        });
    }
    let broker = slot.as_mut().unwrap();
    let mut request = [0u8; REQUEST_SIZE];
    request[..4].copy_from_slice(&kind.to_le_bytes());
    request[4..12].copy_from_slice(&lease.to_le_bytes());
    request[12..36].copy_from_slice(&encode_address(target));
    if kind == 1 {
        let mut protocol = WSAPROTOCOL_INFOW::default();
        if unsafe { WSADuplicateSocketW(raw, broker.process.id(), &mut protocol) } != 0 {
            return Err(crate::socket::last_wsa_errno());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&protocol as *const WSAPROTOCOL_INFOW).cast::<u8>(),
                request[36..].as_mut_ptr(),
                PROTOCOL_SIZE,
            );
        }
    }
    broker.input.write_all(&request).map_err(io_errno)?;
    broker.input.flush().map_err(io_errno)?;
    let mut answer = [0; 28];
    broker.output.read_exact(&mut answer).map_err(io_errno)?;
    let error = i32::from_le_bytes(answer[..4].try_into().unwrap());
    if error != 0 {
        return Err(error);
    }
    decode_address(&answer[4..])
}
fn encode_address(a: Address) -> [u8; 24] {
    let mut b = [0; 24];
    b[..2].copy_from_slice(&a.family.to_le_bytes());
    b[2..4].copy_from_slice(&a.port.to_le_bytes());
    b[4..20].copy_from_slice(&a.ip);
    b[20..].copy_from_slice(&a.scope.to_le_bytes());
    b
}
fn decode_address(b: &[u8]) -> Result<Address, i32> {
    if b.len() != 24 {
        return Err(crate::EIO);
    }
    Ok(Address {
        family: u16::from_le_bytes(b[..2].try_into().unwrap()),
        port: u16::from_le_bytes(b[2..4].try_into().unwrap()),
        ip: b[4..20].try_into().unwrap(),
        scope: u32::from_le_bytes(b[20..].try_into().unwrap()),
    })
}

fn connect_transport(raw: SOCKET, target: Address) -> Result<(), i32> {
    let (bytes, len) = target.bytes(true);
    let _ = crate::socket::connect_policy::configure(raw, &bytes[..len as usize]);
    let value = unsafe { connect(raw, bytes.as_ptr().cast(), len) };
    if value == SOCKET_ERROR {
        let error = crate::socket::last_wsa_errno();
        if error != crate::EAGAIN && error != crate::EINPROGRESS {
            return Err(error);
        }
    }
    Ok(())
}
fn failed_connect(raw: SOCKET, family: u16, lease: u64, error: i32) {
    let _ = crate::usernet::connection_result(lease, error);
    // A refused local TCP handshake supplies a real AFD connect-failure event;
    // SO_ERROR exposes the original remote error recorded above.
    if let Ok(listener) = TcpListener::bind(loopback(family)) {
        if let Ok(local) = listener.local_addr() {
            drop(listener);
            let _ = connect_transport(raw, guest(local));
        }
    }
}
fn udp(
    lease: u64,
    target: Address,
    active: Arc<std::sync::atomic::AtomicUsize>,
) -> Result<Address, i32> {
    let frontend = UdpSocket::bind(loopback(target.family)).map_err(io_errno)?;
    let backend = UdpSocket::bind(if target.family == 2 {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    })
    .map_err(io_errno)?;
    backend.connect(address(target)).map_err(io_errno)?;
    frontend.set_nonblocking(true).map_err(io_errno)?;
    backend.set_nonblocking(true).map_err(io_errno)?;
    let local = guest(frontend.local_addr().map_err(io_errno)?);
    active.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    std::thread::spawn(move || {
        let mut client = None;
        let mut buffer = vec![0; 65536];
        while crate::usernet::lease_exists(lease) {
            let mut progress = false;
            match frontend.recv_from(&mut buffer) {
                Ok((n, source)) => {
                    if crate::usernet::transport_matches(lease, source.port()) {
                        client = Some(source);
                        let _ = backend.send(&buffer[..n]);
                        progress = true;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(_) => break,
            }
            match backend.recv(&mut buffer) {
                Ok(n) => {
                    if let Some(client) = client {
                        let _ = frontend.send_to(&buffer[..n], client);
                        progress = true;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(_) => break,
            }
            if !progress {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        active.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    });
    Ok(local)
}

/// Called by elf-loader before normal guest initialization.
pub fn run() {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut packet_workers = Vec::new();
    loop {
        let mut request = [0; REQUEST_SIZE];
        if input.read_exact(&mut request).is_err() {
            break;
        }
        let kind = i32::from_le_bytes(request[..4].try_into().unwrap());
        let lease = u64::from_le_bytes(request[4..12].try_into().unwrap());
        let result = decode_address(&request[12..36]).and_then(|target| {
            if !crate::usernet::lease_exists(lease) {
                return Err(crate::EBADF);
            }
            if matches!(kind, 3 | 4 | 5 | 6) {
                packet_workers.retain(|worker: &std::thread::JoinHandle<()>| !worker.is_finished());
                let (address, worker) =
                    crate::usernet::packet::gateway::start(kind, lease, target)?;
                packet_workers.push(worker);
                return Ok(address);
            }
            if kind == 2 {
                return udp(lease, target, active.clone());
            }
            if kind != 1 {
                return Err(crate::EINVAL);
            }
            let protocol = unsafe {
                request[36..]
                    .as_ptr()
                    .cast::<WSAPROTOCOL_INFOW>()
                    .read_unaligned()
            };
            let socket = Socket(native_socket(&protocol)?);
            let count = active.clone();
            count.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            std::thread::spawn(move || {
                // Resolve errors before handing ownership to the forwarding loop.
                let destination =
                    TcpStream::connect_timeout(&address(target), Duration::from_secs(30));
                match destination {
                    Ok(remote) => tcp_connected(lease, target, socket, remote),
                    Err(error) => failed_connect(socket.0, target.family, lease, io_errno(error)),
                }
                count.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            });
            Ok(Address::default())
        });
        let mut answer = [0; 28];
        match result {
            Ok(address) => answer[4..].copy_from_slice(&encode_address(address)),
            Err(e) => answer[..4].copy_from_slice(&e.to_le_bytes()),
        }
        if output
            .write_all(&answer)
            .and_then(|_| output.flush())
            .is_err()
        {
            break;
        }
    }
    for worker in packet_workers {
        let _ = worker.join();
    }
    // TCP descriptions can survive the controller through guest fork/exec.
    while active.load(std::sync::atomic::Ordering::Acquire) != 0 {
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn tcp_connected(lease: u64, target: Address, socket: Socket, remote: TcpStream) {
    let mut socket = Some(socket);
    let result = (|| -> Result<(), i32> {
        let listener = TcpListener::bind(loopback(target.family)).map_err(io_errno)?;
        listener.set_nonblocking(true).map_err(io_errno)?;
        connect_transport(
            socket.as_ref().unwrap().0,
            guest(listener.local_addr().map_err(io_errno)?),
        )?;
        crate::usernet::connection_result(lease, 0)?;
        let started = std::time::Instant::now();
        let local = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if !crate::usernet::lease_exists(lease)
                        || started.elapsed() > Duration::from_secs(30)
                    {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(io_errno(e)),
            }
        };
        drop(listener);
        // Winsock accepted sockets inherit the listener's nonblocking mode.
        // The relay workers own blocking streams; WouldBlock must not become EOF.
        local.set_nonblocking(false).map_err(io_errno)?;
        drop(socket.take());
        let mut incoming = local.try_clone().map_err(io_errno)?;
        let mut outgoing = remote.try_clone().map_err(io_errno)?;
        let thread = std::thread::spawn(move || {
            let _ = std::io::copy(&mut incoming, &mut outgoing);
            let _ = outgoing.shutdown(std::net::Shutdown::Write);
        });
        let mut local = local;
        let mut remote = remote;
        let _ = std::io::copy(&mut remote, &mut local);
        let _ = local.shutdown(std::net::Shutdown::Write);
        let _ = thread.join();
        Ok(())
    })();
    if let (Err(e), Some(socket)) = (result, socket) {
        failed_connect(socket.0, target.family, lease, e);
    }
}
