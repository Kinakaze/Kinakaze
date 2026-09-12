use super::*;
use std::io::Write;
use std::os::windows::io::IntoRawSocket;
use std::time::{Duration, Instant};

thread_local! {
    // One-shot fault injection exercises the real mixed-descriptor delivery
    // path without depending on a scheduler hitting the native cancel race.
    pub(super) static AFD_RESULT: std::cell::Cell<Option<Result<AfdPollResult, i32>>> = const { std::cell::Cell::new(None) };
    static SIGNAL_DELIVERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "sysv64" fn record_signal(_: i32) {
    SIGNAL_DELIVERED.set(true);
}

#[test]
fn a_later_afd_retry_or_interrupt_does_not_lose_a_ready_unix_oneshot() {
    let _serialized = signal::test_lock();
    let old_mask = signal::sigprocmask(signal::SIG_SETMASK, 0).unwrap();
    let old_action = signal::sigaction(
        signal::SIGUSR1,
        Some(signal::Action {
            disposition: signal::Disposition::Handle(record_signal, 0),
            ..signal::Action::default()
        }),
    )
    .unwrap();
    for outcome in [Ok(AfdPollResult::InterestChanged), Err(EINTR), Err(EIO)] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let tcp = install(
            stream.into_raw_socket() as usize,
            FdKind::Socket,
            FdFlags::NONBLOCK,
        )
        .unwrap();
        let (reader, writer) =
            crate::unix::socketpair(crate::socket::SOCK_STREAM | crate::socket::SOCK_NONBLOCK)
                .unwrap();
        let poller = epoll_create1(0).unwrap();
        for (fd, mask, data) in [
            (reader, EPOLLIN | EPOLLET | EPOLLONESHOT, 17),
            (tcp, EPOLLIN, 18),
        ] {
            epoll_ctl(
                poller,
                EPOLL_CTL_ADD,
                fd,
                Some(EpollEvent { events: mask, data }),
            )
            .unwrap();
        }
        crate::write(writer, b"reply").unwrap();
        SIGNAL_DELIVERED.set(false);
        let interrupted = matches!(outcome, Err(EINTR));
        if interrupted {
            signal::raise_signal(signal::SIGUSR1).unwrap();
        }
        AFD_RESULT.set(Some(outcome));
        let mut events = [EpollEvent::default(); 2];
        let result = epoll_wait(poller, &mut events, 0);
        AFD_RESULT.set(None);
        // Committed edge/one-shot state must always accompany a returned event.
        assert_eq!(result, Ok(1));
        assert_eq!(
            SIGNAL_DELIVERED.get(),
            interrupted,
            "preserving ready events must not swallow the consumed signal"
        );
        let cookie = events[0].data;
        assert_eq!(cookie, 17);
        assert_eq!(epoll_wait(poller, &mut events, 0), Ok(0));
        let mut bytes = [0; 5];
        assert_eq!(crate::read(reader, &mut bytes), Ok(5));
        assert_eq!(&bytes, b"reply");
        for fd in [poller, tcp, reader, writer] {
            crate::close(fd).unwrap();
        }
    }
    signal::sigaction(signal::SIGUSR1, Some(old_action)).unwrap();
    signal::sigprocmask(signal::SIG_SETMASK, old_mask).unwrap();
}

fn thread_cpu() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let mut times: [FILETIME; 4] = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &mut times[0],
                &mut times[1],
                &mut times[2],
                &mut times[3],
            )
        },
        0
    );
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    ticks(times[2]) + ticks(times[3])
}

#[test]
fn delivered_socket_edges_sleep_and_rearm_after_io() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    let socket = install(
        stream.into_raw_socket() as usize,
        FdKind::Socket,
        FdFlags::NONBLOCK,
    )
    .unwrap();
    let poller = epoll_create1(0).unwrap();
    let eventfd = crate::eventfd::create_eventfd(0, crate::eventfd::EFD_NONBLOCK).unwrap();
    for (fd, interest, data) in [
        (socket, EPOLLIN | EPOLLOUT | EPOLLRDHUP | EPOLLET, 1),
        (eventfd, EPOLLIN | EPOLLET, 2),
    ] {
        epoll_ctl(
            poller,
            EPOLL_CTL_ADD,
            fd,
            Some(EpollEvent {
                events: interest,
                data,
            }),
        )
        .unwrap();
    }
    let mut events = [EpollEvent::default(); 4];
    assert_eq!(epoll_wait(poller, &mut events, 1000).unwrap(), 1);
    assert_ne!(events[0].events & EPOLLOUT, 0);
    let cpu = thread_cpu();
    let start = Instant::now();
    assert_eq!(epoll_wait(poller, &mut events, 400).unwrap(), 0);
    let spent = Duration::from_nanos((thread_cpu() - cpu) * 100);
    eprintln!(
        "idle edge wait: wall={:?}, thread CPU={spent:?}",
        start.elapsed()
    );
    assert!(start.elapsed() >= Duration::from_millis(350));
    assert!(
        spent < Duration::from_millis(100),
        "idle edge wait burned {spent:?} CPU"
    );
    for value in [b'a', b'b'] {
        peer.write_all(&[value]).unwrap();
        assert_eq!(epoll_wait(poller, &mut events, 1000).unwrap(), 1);
        assert_ne!(events[0].events & EPOLLIN, 0);
        let mut byte = [0];
        assert_eq!(crate::read(socket, &mut byte), Ok(1));
        assert_eq!(byte[0], value);
        assert_eq!(crate::read(socket, &mut byte), Err(crate::EAGAIN));
    }
    drop(peer);
    assert_eq!(epoll_wait(poller, &mut events, 1000).unwrap(), 1);
    assert_ne!(events[0].events & EPOLLRDHUP, 0);
    assert_eq!(epoll_wait(poller, &mut events, 40).unwrap(), 0);
    for fd in [poller, eventfd, socket] {
        crate::close(fd).unwrap();
    }
}

#[test]
fn read_through_dup_rearms_a_blocked_waiter() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    let socket = install(
        stream.into_raw_socket() as usize,
        FdKind::Socket,
        FdFlags::NONBLOCK,
    )
    .unwrap();
    let source = crate::get(socket).unwrap();
    let mut copy = std::ptr::null_mut();
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source.raw as HANDLE,
                GetCurrentProcess(),
                &mut copy,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        },
        0
    );
    let duplicate =
        crate::install_duplicate(copy as usize, source.kind, source.flags, source).unwrap();
    let poller = epoll_create1(0).unwrap();
    epoll_ctl(
        poller,
        EPOLL_CTL_ADD,
        socket,
        Some(EpollEvent {
            events: EPOLLIN | EPOLLET,
            data: 3,
        }),
    )
    .unwrap();
    let mut events = [EpollEvent::default(); 1];
    peer.write_all(b"a").unwrap();
    assert_eq!(epoll_wait(poller, &mut events, 1000).unwrap(), 1);
    let reader = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(crate::read(duplicate, &mut [0]), Ok(1));
        // No intervening EAGAIN: successful consumption through the duplicate
        // must wake the existing wait and arm its new read edge.
        peer.write_all(b"b").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        peer
    });
    assert_eq!(epoll_wait(poller, &mut events, 1000).unwrap(), 1);
    let mut byte = [0];
    assert_eq!(crate::read(socket, &mut byte), Ok(1));
    assert_eq!(byte, [b'b']);
    let _peer = reader.join().unwrap();
    for fd in [poller, duplicate, socket] {
        crate::close(fd).unwrap();
    }
}
