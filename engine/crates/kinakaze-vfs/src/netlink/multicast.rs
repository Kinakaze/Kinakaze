//! Namespace link notifications committed with routing state. Independent
//! loaders consume a bounded journal and wait on named publication events;
//! there is no broker process, background polling thread or shared Rust pointer.
use super::*;
use crate::epoll::native_wait::Source;
use crate::route_state::{Link, Network};

const CAPACITY: usize = 256;
const MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Journal {
    pub sequence: u64,
    messages: VecDeque<(u64, u32, Vec<u8>)>,
}

impl Journal {
    pub fn encode(&self, bytes: &mut Vec<u8>) -> Result<(), i32> {
        bytes.extend_from_slice(b"CRYMC002");
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&(self.messages.len() as u32).to_le_bytes());
        for (sequence, source, message) in &self.messages {
            bytes.extend_from_slice(&sequence.to_le_bytes());
            bytes.extend_from_slice(&source.to_le_bytes());
            bytes.extend_from_slice(
                &u32::try_from(message.len())
                    .map_err(|_| EMSGSIZE)?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(message);
        }
        Ok(())
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self, i32> {
        // Older routing stores have no journal extension.
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let has_source = bytes.starts_with(b"CRYMC002");
        if has_source {
            bytes = &bytes[8..];
        }
        fn take<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], i32> {
            let result = bytes
                .get(..N)
                .ok_or(EINVAL)?
                .try_into()
                .map_err(|_| EINVAL)?;
            *bytes = &bytes[N..];
            Ok(result)
        }
        let sequence = u64::from_le_bytes(take(&mut bytes)?);
        let count = u32::from_le_bytes(take(&mut bytes)?) as usize;
        if count > CAPACITY {
            return Err(EINVAL);
        }
        let mut journal = Self {
            sequence,
            messages: VecDeque::new(),
        };
        let first = sequence.checked_sub(count as u64).ok_or(EINVAL)?;
        let mut total = 0;
        for offset in 1..=count {
            let serial = u64::from_le_bytes(take(&mut bytes)?);
            let source = if has_source {
                u32::from_le_bytes(take(&mut bytes)?)
            } else {
                0
            };
            let length = u32::from_le_bytes(take(&mut bytes)?) as usize;
            let message = bytes.get(..length).ok_or(EINVAL)?;
            if serial != first + offset as u64
                || (source == 0 && parse_header(message)?.length as usize != length)
            {
                return Err(EINVAL);
            }
            total += length;
            if total > MAX_BYTES {
                return Err(EINVAL);
            }
            journal
                .messages
                .push_back((serial, source, message.to_vec()));
            bytes = &bytes[length..];
        }
        if !bytes.is_empty() {
            return Err(EINVAL);
        }
        Ok(journal)
    }

    fn push(&mut self, message: Vec<u8>, source: u32) -> Result<(), i32> {
        if message.len() > MAX_BYTES {
            return Err(EMSGSIZE);
        }
        self.sequence = self.sequence.checked_add(1).ok_or(crate::EOVERFLOW)?;
        self.messages.push_back((self.sequence, source, message));
        let mut size: usize = self.messages.iter().map(|(_, _, m)| m.len()).sum();
        while self.messages.len() > CAPACITY || size > MAX_BYTES {
            size -= self.messages.pop_front().ok_or(EIO)?.2.len();
        }
        Ok(())
    }
}

pub(crate) struct Publication(HANDLE);
impl Publication {
    fn open(namespace: u64, next: u64) -> Result<Self, i32> {
        // The kernel domain is independent of the calling thread's chroot.
        let key = kinakaze_runtime::authority::domain_id();
        let name: Vec<u16> =
            format!(r"Local\kinakaze.route.{key:016x}.ns-{namespace}.link-{next}.v1")
                .encode_utf16()
                .chain(Some(0))
                .collect();
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(EIO);
        }
        Ok(Self(handle))
    }
    pub fn signal(&self) -> Result<(), i32> {
        if unsafe { SetEvent(self.0) } == 0 {
            Err(EIO)
        } else {
            Ok(())
        }
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn record(
    namespace: u64,
    before: (u32, Vec<Link>),
    state: &mut Network,
    broadcast: Option<(u32, Vec<u8>)>,
) -> Result<Option<Publication>, i32> {
    let header = MessageHeader {
        length: 0,
        kind: RTM_NEWLINK,
        flags: 0,
        sequence: 0,
        port: 0,
    };
    let start = state.notifications.sequence;
    if let Some((source, bytes)) = broadcast {
        state.notifications.push(bytes, source)?;
    }
    for old in &before.1 {
        if !state.links.iter().any(|l| l.index == old.index) {
            let mut message = stored_link_reply(header, 0, old, false)?;
            message[4..6].copy_from_slice(&RTM_DELLINK.to_ne_bytes());
            state.notifications.push(message, 0)?;
        }
    }
    for link in &state.links {
        if before.1.iter().find(|l| l.index == link.index) != Some(link) {
            state
                .notifications
                .push(stored_link_reply(header, 0, link, false)?, 0)?;
        }
    }
    if namespace != 1 && before.0 != state.loopback_flags {
        let loopback = Loopback {
            index: 1,
            mtu: 65536,
            flags: state.loopback_flags,
            operational: state.loopback_flags & 1 != 0,
            address: vec![0; 6],
        };
        state
            .notifications
            .push(loopback_link_reply(header, 0, &loopback, false)?, 0)?;
    }
    if state.notifications.sequence == start {
        return Ok(None);
    }
    Ok(Some(Publication::open(
        namespace,
        start.checked_add(1).ok_or(crate::EOVERFLOW)?,
    )?))
}

fn subscribed(state: &EndpointState) -> bool {
    state.bound.is_some_and(|b| b.groups & 1 != 0)
}

pub(super) fn refresh(item: &Endpoint, state: &mut EndpointState) -> Result<(), i32> {
    if item.protocol != NETLINK_ROUTE || !subscribed(state) {
        return Ok(());
    }
    let journal = crate::route_state::snapshot_for(item.namespace)?.notifications;
    let filter = crate::socket::filter::for_description(
        item.description.load(std::sync::atomic::Ordering::Acquire),
    )?;
    refresh_journal_filtered(state, journal, filter.as_deref())?;
    if !state.queue.is_empty() || state.multicast_overflow {
        if unsafe { SetEvent(item.event) } == 0 {
            return Err(EIO);
        }
    }
    Ok(())
}

#[cfg(test)]
fn refresh_journal(state: &mut EndpointState, journal: Journal) -> Result<(), i32> {
    refresh_journal_filtered(state, journal, None)
}
fn refresh_journal_filtered(
    state: &mut EndpointState,
    journal: Journal,
    filter: Option<&crate::socket::filter::State>,
) -> Result<(), i32> {
    if journal.sequence == state.multicast_cursor {
        return Ok(());
    }
    let first = journal.sequence - journal.messages.len() as u64;
    if state.multicast_cursor < first || state.multicast_cursor > journal.sequence {
        state.multicast_overflow = true;
    } else {
        for (serial, source, mut message) in journal.messages {
            if serial > state.multicast_cursor
                && (source == 0 || state.bound.is_none_or(|b| b.port != source))
            {
                if let Some(filter) = filter {
                    let Some(length) = filter.run(&crate::socket::filter::LocalPacket(&message), 0)
                    else {
                        continue;
                    };
                    message.truncate(length);
                }
                state.queue.push_back(Datagram {
                    bytes: message,
                    source: NetlinkAddress {
                        port: source,
                        groups: 1,
                    },
                });
            }
        }
    }
    state.multicast_cursor = journal.sequence;
    if state.queue.iter().map(|d| d.bytes.len()).sum::<usize>() > MAX_BYTES {
        state.queue.clear();
        state.multicast_overflow = true;
    }
    Ok(())
}

pub(crate) fn prepare_wait(fd: i32) -> Result<(bool, Vec<Source>), i32> {
    let item = endpoint(fd)?;
    prepare(&item)
}

pub(super) fn prepare(item: &Endpoint) -> Result<(bool, Vec<Source>), i32> {
    let mut state = item.state.lock().map_err(|_| EIO)?;
    refresh(item, &mut state)?;
    let mut sources = vec![unsafe { Source::duplicate(item.event)? }];
    if item.protocol == NETLINK_ROUTE && subscribed(&state) {
        let event = Publication::open(
            item.namespace,
            state
                .multicast_cursor
                .checked_add(1)
                .ok_or(crate::EOVERFLOW)?,
        )?;
        sources.push(unsafe { Source::duplicate(event.0)? });
        // Enrollment precedes the second snapshot: a commit in the gap is
        // observed here or signals the pinned event, so wakeups cannot be lost.
        refresh(item, &mut state)?;
    }
    Ok((!state.queue.is_empty() || state.multicast_overflow, sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoll::*;
    use std::os::windows::process::CommandExt;

    fn subscribe() -> i32 {
        let fd = socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_ROUTE).unwrap();
        let mut address = [0u8; SOCKADDR_NL_LEN];
        address[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
        address[8..12].copy_from_slice(&1u32.to_ne_bytes());
        unsafe { bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        fd
    }

    fn test_link(index: u32) -> Link {
        Link {
            index,
            name: format!("nt{index:x}"),
            kind: "dummy".into(),
            flags: 8,
            mtu: 1500,
            address: vec![2, 0, 0, 0, 0, 1],
            broadcast: vec![255; 6],
            master: 0,
            peer: 0,
            attributes: Vec::new(),
        }
    }

    #[test]
    fn multicast_writer_helper() {
        let Ok(value) = std::env::var("KINAKAZE_TEST_LINK_INDEX") else {
            return;
        };
        let index: u32 = value.parse().unwrap();
        for flags in [8, 9, 8] {
            crate::route_state::transaction_for(1, |state| {
                if let Some(link) = state.links.iter_mut().find(|l| l.index == index) {
                    link.flags = flags;
                } else {
                    state.links.push(test_link(index));
                }
                Ok::<_, i32>(())
            })
            .unwrap()
            .unwrap();
        }
        crate::route_state::transaction_for(1, |state| {
            state.links.retain(|l| l.index != index);
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
    }

    #[test]
    fn independent_process_notifies_all_subscribers_and_preserves_transitions() {
        let _scope = crate::usernet::scope(1);
        let first = subscribe();
        let second = subscribe();
        let epfd = epoll_create1(0).unwrap();
        epoll_ctl(
            epfd,
            EPOLL_CTL_ADD,
            first,
            Some(EpollEvent {
                events: EPOLLIN,
                data: 71,
            }),
        )
        .unwrap();
        let (ready, enrolled) = prepare_wait(second).unwrap();
        assert!(!ready);
        let index = 0x1000_0000 + std::process::id();
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "netlink::multicast::tests::multicast_writer_helper",
                    "--nocapture",
                ])
                .env("KINAKAZE_TEST_LINK_INDEX", index.to_string())
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll_wait(epfd, &mut events, 5000).unwrap(), 1);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(std::time::Instant::now() < until, "writer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(enrolled.iter().any(|s| unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(s.raw(), 0)
        } == 0));
        for fd in [first, second] {
            let mut seen = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                match unsafe { recv(fd, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) } {
                    Ok(count) => {
                        let header = parse_header(&buffer[..count]).unwrap();
                        assert_eq!((header.sequence, header.port), (0, 0));
                        let event_index = u32::from_ne_bytes(buffer[20..24].try_into().unwrap());
                        if event_index == index {
                            seen.push((
                                header.kind,
                                u32::from_ne_bytes(buffer[24..28].try_into().unwrap()),
                            ));
                        }
                    }
                    Err(EAGAIN) => break,
                    Err(error) => panic!("recv failed: {error}"),
                }
            }
            assert_eq!(
                seen,
                [
                    (RTM_NEWLINK, 8),
                    (RTM_NEWLINK, 9),
                    (RTM_NEWLINK, 8),
                    (RTM_DELLINK, 8)
                ]
            );
            crate::close(fd).unwrap();
        }
        crate::close(epfd).unwrap();
    }

    #[test]
    fn journal_overrun_reports_enobufs_and_old_encoding_is_accepted() {
        let item = Endpoint::new(NETLINK_ROUTE, SOCK_RAW).unwrap();
        let mut journal = Journal::default();
        let header = MessageHeader {
            length: 0,
            kind: RTM_NEWLINK,
            flags: 0,
            sequence: 0,
            port: 0,
        };
        for _ in 0..=CAPACITY {
            journal
                .push(
                    stored_link_reply(header, 0, &test_link(99), false).unwrap(),
                    0,
                )
                .unwrap();
        }
        let mut encoded = Vec::new();
        journal.encode(&mut encoded).unwrap();
        assert_eq!(Journal::decode(&encoded).unwrap(), journal);
        assert_eq!(Journal::decode(&[]).unwrap(), Journal::default());
        let mut state = item.state.lock().unwrap();
        refresh_journal(&mut state, journal.clone()).unwrap();
        assert!(state.multicast_overflow);
        assert_eq!(state.multicast_cursor, journal.sequence);
        encoded.push(0);
        assert!(Journal::decode(&encoded).is_err());
    }

    #[test]
    fn failed_route_mutation_does_not_publish_a_link_event() {
        let _scope = crate::usernet::scope(1);
        let fd = subscribe();
        let before = crate::route_state::snapshot().unwrap();
        assert_eq!(
            crate::route_state::transaction(|s| {
                s.links.push(test_link(55));
                Err::<(), _>(EINVAL)
            })
            .unwrap(),
            Err(EINVAL)
        );
        assert_eq!(crate::route_state::snapshot().unwrap(), before);
        assert_eq!(poll(fd).unwrap(), (false, true));
        crate::close(fd).unwrap();
    }

    #[test]
    fn subscribed_address_can_query_kernel_and_broadcast_without_echo() {
        let _scope = crate::usernet::scope(1);
        let fd = subscribe();
        let observer = subscribe();
        let mut address = [0u8; SOCKADDR_NL_LEN];
        address[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
        address[8..12].copy_from_slice(&1u32.to_ne_bytes());
        let mut query = Vec::new();
        append_header(
            &mut query,
            RTM_GETLINK,
            NLM_F_REQUEST | NLM_F_DUMP,
            83,
            0,
            IFINFO_MSG_LEN,
        )
        .unwrap();
        query.resize(NLMSG_HEADER_LEN + IFINFO_MSG_LEN, 0);
        unsafe {
            sendto(
                fd,
                query.as_ptr(),
                query.len(),
                0,
                address.as_ptr(),
                address.len() as i32,
            )
        }
        .unwrap();
        let mut buffer = [0; 4096];
        let mut from = [0u8; SOCKADDR_NL_LEN];
        let mut from_len = from.len() as i32;
        let n = unsafe {
            recvfrom(
                observer,
                buffer.as_mut_ptr(),
                buffer.len(),
                MSG_DONTWAIT,
                from.as_mut_ptr(),
                &mut from_len,
            )
        }
        .unwrap();
        assert_eq!(&buffer[..n], query.as_slice());
        let sender = endpoint(fd)
            .unwrap()
            .state
            .lock()
            .unwrap()
            .bound
            .unwrap()
            .port;
        assert_eq!(u32::from_ne_bytes(from[4..8].try_into().unwrap()), sender);
        let mut done = false;
        loop {
            match unsafe { recv(fd, buffer.as_mut_ptr(), buffer.len(), MSG_DONTWAIT) } {
                Ok(n) => {
                    let header = parse_header(&buffer[..n]).unwrap();
                    assert_ne!(
                        header.kind, RTM_GETLINK,
                        "sender must not receive its own multicast"
                    );
                    assert_eq!(header.sequence, 83);
                    done |= header.kind == NLMSG_DONE;
                }
                Err(EAGAIN) => break,
                Err(error) => panic!("query failed: {error}"),
            }
        }
        assert!(done);
        for fd in [fd, observer] {
            crate::close(fd).unwrap();
        }
    }
}
