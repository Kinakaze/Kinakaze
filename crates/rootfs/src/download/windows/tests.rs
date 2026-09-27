use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

fn server(status: u32, body: &'static [u8]) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("test HTTP server: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        request
    });
    (address, server)
}

#[test]
fn windows_transport_routes_through_proxy_without_resolving_the_origin() {
    let (address, server) = server(200, b"proxy payload");
    let client = Client::new(&Proxy::Http(address)).unwrap();
    let url = "http://kinakaze-no-dns.invalid/package.deb?version=1";
    let mut progress = Vec::new();
    assert_eq!(
        client
            .read(url, 100, |received| progress.push(received))
            .unwrap(),
        b"proxy payload"
    );
    assert_eq!(progress.last(), Some(&13));
    assert_eq!(server.join().unwrap(), format!("GET {url} HTTP/1.1\r\n"));
}

#[test]
fn windows_transport_honors_proxy_bypass_rules() {
    let (address, server) = server(200, b"direct payload");
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = unused.local_addr().unwrap().to_string();
    let client = Client::open(
        WINHTTP_ACCESS_TYPE_NAMED_PROXY,
        Some(&proxy),
        Some("127.0.0.1"),
    )
    .unwrap();
    assert_eq!(
        client
            .read(&format!("http://{address}/package.deb"), 100, |_| {})
            .unwrap(),
        b"direct payload"
    );
    assert_eq!(server.join().unwrap(), "GET /package.deb HTTP/1.1\r\n");
}

#[test]
fn windows_transport_rejects_http_errors_and_oversized_bodies() {
    for (status, limit) in [(404, 100), (200, 2)] {
        let (address, server) = server(status, b"payload");
        let client = Client::new(&Proxy::Direct).unwrap();
        assert!(
            client
                .read(&format!("http://{address}/package.deb"), limit, |_| {})
                .is_err()
        );
        server.join().unwrap();
    }
}
