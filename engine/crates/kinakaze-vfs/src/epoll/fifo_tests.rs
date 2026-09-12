use super::*;
use crate::fs::{self, O_NONBLOCK, O_RDONLY, O_WRONLY};
use crate::socket::{self, AF_INET, SOCK_DGRAM};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kinakaze-epoll-fifo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn pair(&self, name: &str) -> (i32, i32) {
        let path = crate::to_guest_path(&self.0.join(name));
        fs::create_fifo(&path, 0o600).unwrap();
        let reader = fs::open(&path, O_RDONLY | O_NONBLOCK, 0).unwrap();
        let writer = fs::open(&path, O_WRONLY | O_NONBLOCK, 0).unwrap();
        assert_eq!(crate::get(reader).unwrap().kind, FdKind::Fifo);
        (reader, writer)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn quiet_socket() -> i32 {
    let fd = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
    let mut address = [0u8; 16];
    address[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    address[4..8].copy_from_slice(&[127, 0, 0, 1]);
    unsafe { socket::bind(fd, address.as_ptr(), 16) }.unwrap();
    fd
}

fn watch(epfd: i32, fd: i32, data: u64) {
    epoll_ctl(
        epfd,
        EPOLL_CTL_ADD,
        fd,
        Some(EpollEvent {
            events: EPOLLIN,
            data,
        }),
    )
    .unwrap();
}

#[test]
fn fifo_traffic_wakes_a_mixed_socket_wait_and_reports_final_hangup() {
    let fixture = Fixture::new();
    let (reader, writer) = fixture.pair("mixed");
    let socket = quiet_socket();
    let epfd = epoll_create1(0).unwrap();
    watch(epfd, socket, 1);
    watch(epfd, reader, 2);
    let mut events = [EpollEvent::default(); 2];
    assert_eq!(epoll_wait(epfd, &mut events, 0), Ok(0));
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(crate::write(writer, b"x"), Ok(1));
        crate::close(writer).unwrap();
    });
    let start = Instant::now();
    assert_eq!(epoll_wait(epfd, &mut events, 2000), Ok(1));
    assert!(start.elapsed() < Duration::from_secs(1));
    let cookie = events[0].data;
    assert_eq!(cookie, 2);
    producer.join().unwrap();
    assert_eq!(crate::read(reader, &mut [0u8]), Ok(1));
    assert_eq!(epoll_wait(epfd, &mut events, 2000), Ok(1));
    assert_ne!(events[0].events & EPOLLHUP, 0);
    assert_eq!(crate::read(reader, &mut [0u8]), Ok(0));
    for fd in [epfd, socket, reader] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn fifo_sources_above_the_native_limit_are_not_dropped() {
    let fixture = Fixture::new();
    let epfd = epoll_create1(0).unwrap();
    let mut pairs = Vec::new();
    for index in 0..40 {
        let pair = fixture.pair(&format!("large-{index}"));
        watch(epfd, pair.0, index);
        pairs.push(pair);
    }
    // Each FIFO contributes two distinct notification sources, plus set wake
    // and interrupt. The last descriptor must still participate in the wait.
    let writer = pairs[39].1;
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(crate::write(writer, b"x"), Ok(1));
    });
    let mut events = [EpollEvent::default(); 1];
    assert_eq!(epoll_wait(epfd, &mut events, 2000), Ok(1));
    let cookie = events[0].data;
    assert_eq!(cookie, 39);
    producer.join().unwrap();
    crate::close(epfd).unwrap();
    for (reader, writer) in pairs {
        crate::close(reader).unwrap();
        crate::close(writer).unwrap();
    }
}

static SIGNAL_READER: AtomicI32 = AtomicI32::new(-1);
static SIGNAL_WRITER: AtomicI32 = AtomicI32::new(-1);
static SIGNAL_OBSERVED_RETIRED: AtomicBool = AtomicBool::new(false);

unsafe extern "sysv64" fn close_fifo_from_handler(_: i32) {
    let reader = SIGNAL_READER.load(Ordering::SeqCst);
    let writer = SIGNAL_WRITER.load(Ordering::SeqCst);
    let retired = crate::close(reader).is_ok()
        && crate::get(writer).ok().and_then(|entry| {
            crate::fifo::prepare_wait(writer, entry)
                .ok()
                .map(|(ready, _)| ready.error)
        }) == Some(true);
    SIGNAL_OBSERVED_RETIRED.store(retired, Ordering::SeqCst);
}

#[test]
fn signal_handlers_run_after_fifo_pins_and_afd_callbacks_retire_without_restart() {
    let _signals = signal::test_lock();
    let previous = signal::sigaction(
        signal::SIGUSR1,
        Some(signal::Action {
            disposition: signal::Disposition::Handle(close_fifo_from_handler, 0),
            flags: signal::SA_RESTART,
            mask: 0,
            restorer: 0,
        }),
    )
    .unwrap();
    for mixed_socket in [false, true] {
        let fixture = Fixture::new();
        let (reader, writer) = fixture.pair("signal");
        let epfd = epoll_create1(0).unwrap();
        watch(epfd, reader, 1);
        let socket = mixed_socket.then(quiet_socket);
        if let Some(socket) = socket {
            watch(epfd, socket, 2);
        }
        SIGNAL_READER.store(reader, Ordering::SeqCst);
        SIGNAL_WRITER.store(writer, Ordering::SeqCst);
        SIGNAL_OBSERVED_RETIRED.store(false, Ordering::SeqCst);
        assert!(!interrupt::current().is_null());
        let thread_id = interrupt::current_thread_id();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal::raise_thread_signal(thread_id, signal::SIGUSR1).unwrap();
        });
        assert_eq!(
            epoll_wait(epfd, &mut [EpollEvent::default(); 1], 1000),
            Err(EINTR)
        );
        sender.join().unwrap();
        assert!(SIGNAL_OBSERVED_RETIRED.load(Ordering::SeqCst));
        assert_eq!(crate::get(reader).err(), Some(EBADF));
        crate::close(epfd).unwrap();
        crate::close(writer).unwrap();
        if let Some(socket) = socket {
            crate::close(socket).unwrap();
        }
    }
    signal::sigaction(signal::SIGUSR1, Some(previous)).unwrap();
}
