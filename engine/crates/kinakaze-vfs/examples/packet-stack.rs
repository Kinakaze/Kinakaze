//! Two independent Windows processes exchange real IP frames over a private
//! loopback UDP link. TCP itself runs in each process's user-mode stack.
use kinakaze_vfs::usernet::stack::{Ingress, IpAddress, IpCidr, IpEndpoint, Stack, TcpState};
use std::io::{BufRead, BufReader, Write};
use std::net::UdpSocket;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn ret(value: u32) -> Vec<u8> {
    [&[6, 0, 0, 0][..], &value.to_le_bytes()].concat()
}
fn endpoint(server: bool, v6: bool, udp: bool) -> IpEndpoint {
    let host = if server { 2 } else { 1 };
    let addr = if v6 {
        format!("fd80::{host}").parse().unwrap()
    } else {
        IpAddress::v4(10, 80, 0, host)
    };
    IpEndpoint::new(
        addr,
        if udp {
            8081
        } else if server {
            8080
        } else {
            41000
        },
    )
}
fn frame_wait(link: &UdpSocket, stack: &mut Stack, now: i64, maximum: u64) -> Option<Vec<u8>> {
    let delay = stack.poll_delay(now).unwrap_or(maximum).min(maximum).max(1);
    link.set_read_timeout(Some(Duration::from_millis(delay)))
        .unwrap();
    let mut packet = vec![0; 65536];
    match link.recv(&mut packet) {
        Ok(n) => {
            packet.truncate(n);
            Some(packet)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            None
        }
        Err(e) => panic!("packet link: {e}"),
    }
}
fn flush(stack: &mut Stack, link: &UdpSocket, now: i64) {
    stack.poll(now);
    while let Some(frame) = stack.take_packet() {
        assert_eq!(link.send(&frame).unwrap(), frame.len());
    }
}
fn peer(link: UdpSocket, server: bool, v6: bool) {
    let local = endpoint(server, v6, false);
    let udp_local = endpoint(server, v6, true);
    let mut stack = Stack::new(
        &[IpCidr::new(local.addr, if v6 { 64 } else { 24 })],
        if server { 8 } else { 7 },
        std::process::id() as u64,
        0,
    )
    .unwrap();
    let tcp = if server {
        stack.listen(local).unwrap()
    } else {
        stack.connect(local, endpoint(true, v6, false)).unwrap()
    };
    let udp = stack.bind_udp(udp_local).unwrap();
    if server {
        stack.attach_filter(tcp, &ret(0)).unwrap();
        stack.attach_filter(udp, &ret(0)).unwrap();
    } else {
        stack
            .send_udp(udp, b"drop", endpoint(true, v6, true))
            .unwrap();
    }
    let started = Instant::now();
    let mut detached = false;
    let mut tcp_sent = false;
    let mut tcp_received = false;
    let mut udp_received = false;
    let mut drops = 0;
    let mut waits = 0;
    loop {
        let now = started.elapsed().as_millis() as i64;
        assert!(
            now < 15000,
            "packet-stack deadline: server={server} tcp={:?}",
            stack.tcp_state(tcp)
        );
        if server && now >= 400 && !detached {
            assert!(
                drops >= 2,
                "both SYN and UDP must reach the rejecting filter"
            );
            assert_eq!(stack.tcp_state(tcp), Ok(TcpState::Listen));
            assert_eq!(
                stack.recv_udp(udp, &mut [0; 32], false),
                Err(kinakaze_vfs::EAGAIN)
            );
            stack.detach_filter(tcp).unwrap();
            stack.detach_filter(udp).unwrap();
            assert!(
                stack.take_packet().is_none(),
                "drop must not generate a SYN-ACK"
            );
            detached = true;
            link.send(b"D").unwrap();
        }
        if !server && stack.tcp_state(tcp) == Ok(TcpState::Established) && !tcp_sent {
            assert!(detached, "handshake completed while SYN was being dropped");
            assert_eq!(stack.send_tcp(tcp, b"tcp wire"), Ok(8));
            tcp_sent = true;
        }
        if stack.tcp_state(tcp) == Ok(TcpState::Established) {
            let mut buffer = [0; 32];
            if let Ok(n) = stack.recv_tcp(tcp, &mut buffer, false) {
                if n > 0 {
                    assert_eq!(&buffer[..n], b"tcp wire");
                    tcp_received = true;
                    if server {
                        assert_eq!(stack.send_tcp(tcp, &buffer[..n]), Ok(n));
                    }
                }
            }
        }
        let mut buffer = [0; 32];
        if let Ok((n, from, _)) = stack.recv_udp(udp, &mut buffer, false) {
            assert_eq!(&buffer[..n], b"pass");
            udp_received = true;
            if server {
                stack.send_udp(udp, &buffer[..n], from).unwrap();
            }
        }
        flush(&mut stack, &link, now);
        if !server && tcp_received && udp_received {
            link.send(b"Q").unwrap();
            println!(
                "{{\"role\":\"client\",\"ipv6\":{v6},\"passed\":true,\"elapsed_ms\":{now},\"waits\":{waits}}}"
            );
            return;
        }
        let maximum = if server && !detached {
            400u64.saturating_sub(now as u64).max(1)
        } else {
            (15000 - now) as u64
        };
        waits += 1;
        if let Some(bytes) = frame_wait(&link, &mut stack, now, maximum) {
            if bytes == b"D" && !server {
                assert_eq!(stack.tcp_state(tcp), Ok(TcpState::SynSent));
                detached = true;
                stack
                    .send_udp(udp, b"pass", endpoint(true, v6, true))
                    .unwrap();
            } else if bytes == b"Q" && server {
                assert!(tcp_received && udp_received);
                println!(
                    "{{\"role\":\"server\",\"ipv6\":{v6},\"passed\":true,\"drops\":{drops},\"elapsed_ms\":{now},\"waits\":{waits}}}"
                );
                return;
            } else {
                match stack.receive_packet(bytes, started.elapsed().as_millis() as i64) {
                    Ingress::Filtered => drops += 1,
                    Ingress::Delivered => {}
                    failure => panic!("packet ingress: {failure:?}"),
                }
            }
        }
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let v6 = args.iter().any(|arg| arg == "--ipv6");
    let link = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    if args.get(1).map(String::as_str) == Some("--peer") {
        link.connect(("127.0.0.1", args[2].parse::<u16>().unwrap()))
            .unwrap();
        println!("{}", link.local_addr().unwrap().port());
        std::io::stdout().flush().unwrap();
        peer(link, false, v6);
        return;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--peer")
        .arg(link.local_addr().unwrap().port().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .creation_flags(0x08000000);
    if v6 {
        command.arg("--ipv6");
    }
    let mut child = ChildGuard(command.spawn().unwrap());
    let mut output = BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    link.connect(("127.0.0.1", line.trim().parse::<u16>().unwrap()))
        .unwrap();
    peer(link, true, v6);
    line.clear();
    output.read_line(&mut line).unwrap();
    print!("{line}");
    assert!(child.0.wait().unwrap().success());
}
