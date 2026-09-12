use super::classic::Instruction;
use super::*;
use crate::socket::*;
fn ret(value: u32) -> Instruction {
    Instruction {
        code: 6,
        jt: 0,
        jf: 0,
        k: value,
    }
}
#[repr(C)]
struct Fprog {
    count: u16,
    code: *const Instruction,
}
fn attach(fd: i32, code: &[Instruction]) -> Result<(), i32> {
    let fprog = Fprog {
        count: code.len() as u16,
        code: code.as_ptr(),
    };
    unsafe {
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_ATTACH_FILTER,
            (&fprog as *const Fprog).cast(),
            16,
        )
    }
}
fn scalar(fd: i32, name: i32, value: i32) -> Result<(), i32> {
    unsafe { setsockopt(fd, SOL_SOCKET, name, value.to_le_bytes().as_ptr(), 4) }
}
struct Fds(Vec<i32>);
impl Drop for Fds {
    fn drop(&mut self) {
        for fd in &self.0 {
            let _ = crate::close(*fd);
        }
    }
}
fn pair() -> (i32, i32, Fds) {
    let (a, b) = crate::unix::socketpair(SOCK_DGRAM | SOCK_NONBLOCK).unwrap();
    (a, b, Fds(vec![a, b]))
}
fn read(fd: i32, flags: i32) -> Result<Vec<u8>, i32> {
    let mut bytes = vec![0; 64];
    let n = unsafe { recv(fd, bytes.as_mut_ptr(), bytes.len(), flags) }?;
    bytes.truncate(n);
    Ok(bytes)
}
fn write(fd: i32, bytes: &[u8]) -> Result<usize, i32> {
    unsafe { send(fd, bytes.as_ptr(), bytes.len(), 0) }
}
fn original(fd: i32) -> Vec<Instruction> {
    let mut count = 0;
    unsafe {
        getsockopt(
            fd,
            SOL_SOCKET,
            SO_GET_FILTER,
            std::ptr::null_mut(),
            &mut count,
        )
    }
    .unwrap();
    let mut code = vec![ret(0); count as usize];
    if count != 0 {
        unsafe {
            getsockopt(
                fd,
                SOL_SOCKET,
                SO_GET_FILTER,
                code.as_mut_ptr().cast(),
                &mut count,
            )
        }
        .unwrap();
    }
    code
}

#[test]
fn unix_datagrams_filter_before_queue_and_preserve_sender_length() {
    let (a, b, _fds) = pair();
    let code = [
        Instruction {
            code: 0x30,
            jt: 0,
            jf: 0,
            k: 0,
        },
        Instruction {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: b'B' as u32,
        },
        ret(3),
        ret(0),
    ];
    attach(b, &code).unwrap();
    assert_eq!(write(a, b"AAAAAA"), Ok(6));
    assert_eq!(read(b, 0), Err(crate::EAGAIN));
    assert!(
        !crate::unix::poll_readiness(b)
            .unwrap()
            .contains(Readiness::READABLE)
    );
    assert_eq!(write(a, b"BBBBBB"), Ok(6));
    assert_eq!(read(b, MSG_PEEK), Ok(b"BBB".to_vec()));
    assert_eq!(read(b, MSG_PEEK), Ok(b"BBB".to_vec()));
    assert_eq!(read(b, 0), Ok(b"BBB".to_vec()));
    assert_eq!(read(b, 0), Err(crate::EAGAIN));
    assert_eq!(original(b), code);
    scalar(b, SO_DETACH_FILTER, 0).unwrap();
    assert!(original(b).is_empty());
    assert_eq!(write(a, b"AAAAAA"), Ok(6));
    assert_eq!(read(b, 0), Ok(b"AAAAAA".to_vec()));
}

#[test]
fn queued_packets_survive_replacement_and_invalid_program_is_atomic() {
    let (a, b, _fds) = pair();
    assert_eq!(write(a, b"before"), Ok(6));
    attach(b, &[ret(0)]).unwrap();
    assert_eq!(read(b, 0), Ok(b"before".to_vec()));
    assert_eq!(
        attach(
            b,
            &[
                Instruction {
                    code: 5,
                    jt: 0,
                    jf: 0,
                    k: u32::MAX
                },
                ret(1)
            ]
        ),
        Err(EINVAL)
    );
    assert_eq!(original(b), vec![ret(0)]);
    assert_eq!(write(a, b"discard"), Ok(7));
    attach(b, &[ret(u32::MAX)]).unwrap();
    assert_eq!(read(b, 0), Err(crate::EAGAIN));
    assert_eq!(write(a, b"after"), Ok(5));
    assert_eq!(read(b, 0), Ok(b"after".to_vec()));
}

#[test]
fn option_validation_and_permanent_lock_follow_linux_contract() {
    let (a, b, _fds) = pair();
    assert_eq!(scalar(b, SO_DETACH_FILTER, 0), Err(ENOENT));
    assert_eq!(
        unsafe { setsockopt(b, SOL_SOCKET, SO_ATTACH_FILTER, std::ptr::null(), 0) },
        Err(EINVAL)
    );
    assert_eq!(
        unsafe { setsockopt(b, SOL_SOCKET, SO_ATTACH_FILTER, 1usize as *const u8, 16) },
        Err(EFAULT)
    );
    let fprog = Fprog {
        count: 1,
        code: 1usize as *const Instruction,
    };
    assert_eq!(
        unsafe {
            setsockopt(
                b,
                SOL_SOCKET,
                SO_ATTACH_FILTER,
                (&fprog as *const Fprog).cast(),
                16,
            )
        },
        Err(EFAULT)
    );
    attach(b, &[ret(7), ret(8)]).unwrap();
    let mut one = 1;
    let mut code = ret(0);
    assert_eq!(
        unsafe {
            getsockopt(
                b,
                SOL_SOCKET,
                SO_GET_FILTER,
                (&mut code as *mut Instruction).cast(),
                &mut one,
            )
        },
        Err(EINVAL)
    );
    assert_eq!(one, 1);
    scalar(b, SO_LOCK_FILTER, 1).unwrap();
    scalar(b, SO_LOCK_FILTER, 2).unwrap();
    assert_eq!(scalar(b, SO_LOCK_FILTER, 0), Err(EPERM));
    assert_eq!(attach(b, &[ret(0)]), Err(EPERM));
    assert_eq!(scalar(b, SO_DETACH_FILTER, 0), Err(EPERM));
    let mut lock = 0i32;
    let mut length = 4;
    unsafe {
        getsockopt(
            b,
            SOL_SOCKET,
            SO_LOCK_FILTER,
            (&mut lock as *mut i32).cast(),
            &mut length,
        )
    }
    .unwrap();
    assert_eq!((lock, length), (1, 4));
    scalar(a, SO_LOCK_FILTER, 1).unwrap();
    assert_eq!(attach(a, &[ret(0)]), Err(EPERM));
    assert_eq!(scalar(a, SO_DETACH_FILTER, 0), Err(EPERM));
}

#[test]
fn ofd_aliases_and_status_updates_preserve_filter_state() {
    let (a, b, mut fds) = pair();
    let entry = crate::get(b).unwrap();
    let mut handle = std::ptr::null_mut();
    let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
    assert_ne!(
        unsafe {
            windows_sys::Win32::Foundation::DuplicateHandle(
                process,
                entry.raw as _,
                process,
                &mut handle,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        },
        0
    );
    let alias = crate::install_duplicate(handle as usize, entry.kind, entry.flags, entry).unwrap();
    crate::unix::duplicate(b, alias, handle as usize).unwrap();
    fds.0.push(alias);
    attach(alias, &[ret(2)]).unwrap();
    assert_eq!(original(b), vec![ret(2)]);
    crate::ofd::with(alias, || Ok(())).unwrap();
    assert_eq!(original(b), vec![ret(2)]);
    assert_eq!(write(a, b"abcdef"), Ok(6));
    assert_eq!(read(b, 0), Ok(b"ab".to_vec()));
    scalar(alias, SO_DETACH_FILTER, 0).unwrap();
    assert!(original(b).is_empty());
}

#[test]
fn unix_stream_keeps_linux_no_datagram_filter_behavior() {
    let (a, b) = crate::unix::socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
    let _fds = Fds(vec![a, b]);
    attach(b, &[ret(0)]).unwrap();
    assert_eq!(write(a, b"stream"), Ok(6));
    assert_eq!(read(b, 0), Ok(b"stream".to_vec()));
}

#[test]
fn netlink_replies_are_filtered_before_epoll_readiness() {
    let fd =
        crate::netlink::socket(SOCK_RAW | SOCK_NONBLOCK, crate::netlink::NETLINK_ROUTE).unwrap();
    let _fds = Fds(vec![fd]);
    let mut address = [0u8; 12];
    address[..2].copy_from_slice(&16u16.to_ne_bytes());
    unsafe { crate::netlink::connect(fd, address.as_ptr(), 12) }.unwrap();
    attach(fd, &[ret(0)]).unwrap();
    let mut request = [0u8; 32];
    request[..4].copy_from_slice(&32u32.to_ne_bytes());
    request[4..6].copy_from_slice(&18u16.to_ne_bytes());
    request[6..8].copy_from_slice(&0x301u16.to_ne_bytes());
    assert_eq!(write(fd, &request), Ok(32));
    assert_eq!(read(fd, 0), Err(crate::EAGAIN));
    assert!(!crate::netlink::poll(fd).unwrap().0);
    scalar(fd, SO_DETACH_FILTER, 0).unwrap();
    assert_eq!(write(fd, &request), Ok(32));
    assert!(crate::netlink::poll(fd).unwrap().0);
}

#[test]
fn native_backend_does_not_report_unenforced_filter_success() {
    let fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0).unwrap();
    let _fds = Fds(vec![fd]);
    assert_eq!(attach(fd, &[ret(0)]), Err(EOPNOTSUPP));
    assert!(original(fd).is_empty());
}

#[test]
fn output_copy_respects_native_readonly_pages_and_copy_on_write() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::*;
    let (_a, b, _fds) = pair();
    attach(b, &[ret(123)]).unwrap();
    let mapping = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            65536,
            std::ptr::null(),
        )
    };
    assert!(!mapping.is_null());
    let shared = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 65536) };
    let private = unsafe { MapViewOfFile(mapping, FILE_MAP_COPY, 0, 0, 65536) };
    assert!(!shared.Value.is_null() && !private.Value.is_null());
    let mut count = 1;
    let readonly_result = unsafe {
        getsockopt(
            b,
            SOL_SOCKET,
            SO_GET_FILTER,
            shared.Value.cast(),
            &mut count,
        )
    };
    let mut count = 1;
    let cow_result = unsafe {
        getsockopt(
            b,
            SOL_SOCKET,
            SO_GET_FILTER,
            private.Value.cast(),
            &mut count,
        )
    };
    let shared_bytes = unsafe { std::slice::from_raw_parts(shared.Value.cast::<u8>(), 8) }.to_vec();
    let private_bytes =
        unsafe { std::slice::from_raw_parts(private.Value.cast::<u8>(), 8) }.to_vec();
    unsafe {
        UnmapViewOfFile(private);
        UnmapViewOfFile(shared);
        CloseHandle(mapping);
    }
    assert_eq!(readonly_result, Err(EFAULT));
    assert_eq!(cow_result, Ok(()));
    assert_eq!(shared_bytes, vec![0; 8]);
    assert_eq!(
        private_bytes,
        Program::new(vec![ret(123)]).unwrap().encode()
    );
}

#[test]
fn multicast_filter_changes_do_not_reclassify_already_published_messages() {
    let _scope = crate::usernet::scope(1);
    let fd =
        crate::netlink::socket(SOCK_RAW | SOCK_NONBLOCK, crate::netlink::NETLINK_ROUTE).unwrap();
    let _fds = Fds(vec![fd]);
    let mut address = [0u8; 12];
    address[..2].copy_from_slice(&16u16.to_ne_bytes());
    address[8..12].copy_from_slice(&1u32.to_ne_bytes());
    unsafe { crate::netlink::bind(fd, address.as_ptr(), 12) }.unwrap();
    let index = 0x2500_0000 + std::process::id();
    struct LinkCleanup(u32);
    impl Drop for LinkCleanup {
        fn drop(&mut self) {
            let _ = crate::route_state::transaction_for(1, |state| {
                state.links.retain(|link| link.index != self.0);
                Ok::<_, i32>(())
            });
        }
    }
    let _cleanup = LinkCleanup(index);
    crate::route_state::transaction_for(1, |state| {
        state.links.push(crate::route_state::Link {
            index,
            name: format!("bf{index:x}"),
            kind: "dummy".into(),
            flags: 8,
            mtu: 1500,
            address: vec![2, 0, 0, 0, 0, 1],
            broadcast: vec![255; 6],
            master: 0,
            peer: 0,
            attributes: Vec::new(),
        });
        Ok::<_, i32>(())
    })
    .unwrap()
    .unwrap();
    attach(fd, &[ret(0)]).unwrap();
    assert!(
        crate::netlink::poll(fd).unwrap().0,
        "pre-attach notification disappeared"
    );
    let mut buffer = [0u8; 4096];
    while unsafe { recv(fd, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) }.is_ok() {}
    let publish = |flags| {
        crate::route_state::transaction_for(1, |state| {
            state
                .links
                .iter_mut()
                .find(|link| link.index == index)
                .unwrap()
                .flags = flags;
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
    };
    publish(9);
    scalar(fd, SO_DETACH_FILTER, 0).unwrap();
    assert!(
        !crate::netlink::poll(fd).unwrap().0,
        "dropped notification survived detach"
    );
    publish(8);
    assert!(
        crate::netlink::poll(fd).unwrap().0,
        "post-detach notification missing"
    );
}

#[test]
fn peer_retains_last_filter_after_receiver_closes_before_first_send() {
    let (a, b, mut fds) = pair();
    attach(b, &[ret(0)]).unwrap();
    crate::close(b).unwrap();
    fds.0.retain(|fd| *fd != b);
    assert_eq!(write(a, b"discard after peer close"), Ok(24));
    let (a, b, mut other) = pair();
    attach(b, &[ret(0)]).unwrap();
    attach(b, &[ret(u32::MAX)]).unwrap();
    crate::close(b).unwrap();
    other.0.retain(|fd| *fd != b);
    assert_eq!(write(a, b"closed"), Err(crate::EPIPE));
}
