use super::*;

struct Fds(Vec<i32>);
impl Drop for Fds {
    fn drop(&mut self) {
        for fd in self.0.drain(..) {
            let _ = close(fd);
        }
    }
}

#[test]
fn routed_io_keeps_device_errors_and_unix_message_boundaries() {
    let null = fs::open("/dev/null", fs::O_RDWR, 0).unwrap();
    let zero = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
    let full = fs::open("/dev/full", fs::O_WRONLY, 0).unwrap();
    let _devices = Fds(vec![null, zero, full]);
    assert_eq!(read(-1, &mut [0; 1]), Err(EBADF));
    assert_eq!(write(-1, &[0]), Err(EBADF));
    assert_eq!(write(null, b"discard"), Ok(7));
    assert_eq!(read(null, &mut [0; 3]), Ok(0));
    let mut buffer = [0xa5; 37];
    assert_eq!(read(zero, &mut buffer), Ok(buffer.len()));
    assert_eq!(buffer, [0; 37]);
    assert_eq!(write(full, b"x"), Err(ENOSPC));
    for kind in [socket::SOCK_STREAM, socket::SOCK_SEQPACKET] {
        let (left, right) = unix::socketpair(kind).unwrap();
        let _pair = Fds(vec![left, right]);
        assert_eq!(write(left, b"first"), Ok(5));
        assert_eq!(write(left, b"second"), Ok(6));
        let mut payload = [0; 11];
        let count = read(right, &mut payload).unwrap();
        if kind == socket::SOCK_STREAM {
            assert_eq!(count, 11);
            assert_eq!(&payload, b"firstsecond");
        } else {
            assert_eq!(count, 5);
            assert_eq!(&payload[..count], b"first");
            assert_eq!(read(right, &mut payload), Ok(6));
            assert_eq!(&payload[..6], b"second");
        }
    }
}

#[test]
#[ignore = "paired release VFS/Unix I/O routing benchmark"]
fn benchmark_io_routes() {
    let null = fs::open("/dev/null", fs::O_RDWR, 0).unwrap();
    let _device = Fds(vec![null]);
    let mut payload = [0x75; 64];
    let began = std::time::Instant::now();
    for _ in 0..200_000 {
        assert_eq!(write(null, &payload), Ok(64));
        assert_eq!(read(null, &mut payload), Ok(0));
    }
    let device_ns = began.elapsed().as_nanos();
    let (left, right) = unix::socketpair(socket::SOCK_STREAM).unwrap();
    let _pair = Fds(vec![left, right]);
    let mut output = [0; 64];
    let began = std::time::Instant::now();
    for _ in 0..10_000 {
        assert_eq!(write(left, &payload), Ok(64));
        assert_eq!(read(right, &mut output), Ok(64));
        assert_eq!(output, payload);
    }
    println!(
        "IO_ROUTE_BENCH {{\"optimized\":{},\"device_400000_ns\":{device_ns},\"unix_roundtrip_10000_ns\":{}}}",
        io_route_optimized(),
        began.elapsed().as_nanos()
    );
}
