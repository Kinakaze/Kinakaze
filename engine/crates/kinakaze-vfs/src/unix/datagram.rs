//! Named Unix datagrams use a shared packet queue, rather than one pipe peer.
//! The queue is part of the socket's existing lifetime-pinned procnet record.
use super::*;
use crate::state_codec::{Reader, bytes, word};
use std::collections::VecDeque;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const LIMIT: usize = 212_992;
#[derive(Clone)]
struct Message {
    source: UnixAddress,
    credentials: credentials::Sender,
    payload: Vec<u8>,
    rights: Option<rights::Record>,
}
struct Queue {
    binding: String,
    peer: u64,
    address: UnixAddress,
    read_closed: bool,
    write_closed: bool,
    messages: VecDeque<Message>,
}
fn address(r: &mut Reader<'_>) -> Result<UnixAddress, i32> {
    let namespace = match r.word()? {
        1 => Namespace::Pathname,
        2 => Namespace::Abstract,
        3 => Namespace::Unnamed,
        _ => return Err(EIO),
    };
    let name = r.bytes()?.to_vec();
    if name.len() > SUN_PATH_MAX {
        return Err(EIO);
    }
    Ok(UnixAddress { namespace, name })
}
fn put_address(out: &mut Vec<u8>, value: &UnixAddress) {
    word(out, namespace_code(&value.namespace) as u64);
    bytes(out, &value.name);
}
impl Queue {
    fn decode(input: &[u8]) -> Result<Self, i32> {
        let mut queue = Self {
            binding: String::new(),
            peer: 0,
            address: UnixAddress::unnamed(),
            read_closed: false,
            write_closed: false,
            messages: VecDeque::new(),
        };
        if input.is_empty() {
            return Ok(queue);
        }
        let mut r = Reader(input);
        if r.word()? != u64::from_le_bytes(*b"KDGRAM02") {
            return Err(EIO);
        }
        queue.binding = r.text()?;
        queue.peer = r.word()?;
        queue.address = address(&mut r)?;
        queue.read_closed = r.word()? != 0;
        queue.write_closed = r.word()? != 0;
        let count = r.word()?;
        if count > 65536 {
            return Err(EIO);
        }
        for _ in 0..count {
            queue.messages.push_back(Message {
                source: address(&mut r)?,
                credentials: credentials::Sender::read(&mut r)?,
                payload: r.bytes()?.to_vec(),
                rights: if r.word()? == 0 {
                    None
                } else {
                    Some(rights::Record::read(&mut r)?)
                },
            });
        }
        r.end()?;
        Ok(queue)
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        word(&mut out, u64::from_le_bytes(*b"KDGRAM02"));
        bytes(&mut out, self.binding.as_bytes());
        word(&mut out, self.peer);
        put_address(&mut out, &self.address);
        word(&mut out, u64::from(self.read_closed));
        word(&mut out, u64::from(self.write_closed));
        word(&mut out, self.messages.len() as u64);
        for message in &self.messages {
            put_address(&mut out, &message.source);
            message.credentials.write(&mut out);
            bytes(&mut out, &message.payload);
            word(&mut out, u64::from(message.rights.is_some()));
            if let Some(rights) = &message.rights {
                rights.write(&mut out);
            }
        }
        out
    }
}
fn update<T>(
    record: &procnet::Record,
    action: impl FnOnce(&mut Queue) -> Result<T, i32>,
) -> Result<T, i32> {
    record.update_data(|input| {
        let mut queue = Queue::decode(input)?;
        let result = action(&mut queue)?;
        Ok((queue.encode(), result))
    })
}
pub(super) fn selected(fd: i32) -> Result<bool, i32> {
    let socket = snapshot(fd)?;
    Ok(socket.socket_type == SOCK_DGRAM && socket.ancillary.is_none())
}
pub(super) fn bind(record: &procnet::Record, key: &str) -> Result<(), i32> {
    update(record, |q| {
        q.binding = key.into();
        Ok(())
    })
}
fn lookup(parsed: &UnixAddress, network: u64) -> Result<Arc<procnet::Record>, i32> {
    let key = if let Some(path) = parsed.filesystem_path() {
        if let Some((stat, _pin)) = crate::tmpfs::socket::lookup(&path)? {
            stat_pipe_name(&stat)?
        } else {
            let native = crate::fs::resolve(&path)?;
            let pin = crate::fs::object::Object::open(
                &native,
                windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES,
            )?;
            inode_pipe_name(pin.raw())?
        }
    } else if parsed.namespace == Namespace::Abstract {
        parsed.pipe_name_in(network)
    } else {
        return Err(EINVAL);
    };
    for record in procnet::records()? {
        if Queue::decode(&record.data()?)?.binding == key {
            return Ok(record);
        }
    }
    Err(ECONNREFUSED)
}
pub(super) fn connect(fd: i32, target: &UnixAddress) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    let peer = lookup(target, socket.network)?;
    update(&socket.record, |queue| {
        queue.peer = peer.id();
        queue.address = target.clone();
        Ok(())
    })?;
    modify(fd, |socket| {
        socket.peer = target.clone();
        socket.state = State::Connected;
        Ok(())
    })
}
pub(super) fn peer(fd: i32) -> Result<UnixAddress, i32> {
    let queue = Queue::decode(&snapshot(fd)?.record.data()?)?;
    if queue.peer == 0 {
        return Err(ENOTCONN);
    }
    Ok(queue.address)
}
fn wait() -> Result<(), i32> {
    if matches!(signal::deliver_pending(), signal::Delivery::Interrupted) {
        return Err(EINTR);
    }
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    unsafe {
        WaitForSingleObject(interrupt, 10);
    }
    Ok(())
}
pub(super) unsafe fn send(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    rights: &[i32],
    supplied: Option<&credentials::Sender>,
    destination: Option<&UnixAddress>,
) -> Result<usize, i32> {
    if flags & 1 != 0 {
        return Err(EOPNOTSUPP);
    }
    if len > LIMIT {
        return Err(crate::EMSGSIZE);
    }
    let socket = snapshot(fd)?;
    let own = Queue::decode(&socket.record.data()?)?;
    if own.write_closed {
        return Err(EPIPE);
    }
    let target = if let Some(destination) = destination {
        lookup(destination, socket.network)?
    } else if own.peer == 0 {
        return Err(EDESTADDRREQ);
    } else {
        procnet::Record::restore(own.peer).map_err(|_| ECONNREFUSED)?
    };
    let pending = if rights.is_empty() {
        None
    } else {
        Some(rights::export(target.id(), rights)?)
    };
    let message = Message {
        source: socket.local.clone(),
        credentials: match supplied {
            Some(value) => value.clone(),
            None => credentials::Sender::current(false)?,
        },
        payload: if len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec()
        },
        rights: pending.as_ref().map(rights::Pending::record),
    };
    loop {
        let result = update(&target, |queue| {
            if queue.read_closed {
                return Err(ECONNREFUSED);
            }
            if queue.peer != 0 && queue.peer != socket.record.id() {
                return Err(crate::EPERM);
            }
            if queue.messages.len() >= 64
                || queue
                    .messages
                    .iter()
                    .map(|m| m.payload.len())
                    .sum::<usize>()
                    + len
                    > LIMIT
            {
                return Err(EAGAIN);
            }
            queue.messages.push_back(message.clone());
            Ok(len)
        });
        if result != Err(EAGAIN)
            || flags & MSG_DONTWAIT != 0
            || get(fd)?.flags.contains(FdFlags::NONBLOCK)
        {
            if result.is_ok()
                && let Some(pending) = pending
            {
                pending.commit();
            }
            return result;
        }
        wait()?;
    }
}
pub(super) unsafe fn recv(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    control: usize,
    recvmsg: bool,
) -> Result<
    (
        usize,
        Vec<i32>,
        i32,
        Option<credentials::Ucred>,
        UnixAddress,
    ),
    i32,
> {
    if flags & 1 != 0 {
        return Err(EOPNOTSUPP);
    }
    let socket = snapshot(fd)?;
    let description = get(fd)?.description_id;
    loop {
        let passcred = recvmsg && socket.record.passcred()?;
        // Keep imported descriptors private until the queue transaction commits.
        // Import while serialized so a simultaneous consume cannot release a
        // peek's native bundle before it has duplicated its references.
        let mut received = Vec::new();
        let mut out_flags = 0;
        let mut credentials = None;
        let result = update(&socket.record, |queue| {
            if queue.read_closed && queue.messages.is_empty() {
                return Ok(None);
            }
            let message = queue.messages.front().ok_or(EAGAIN)?;
            if passcred {
                if control >= 32 {
                    credentials = Some(message.credentials.visible()?);
                } else {
                    out_flags |= 8;
                }
            }
            let capacity = if recvmsg {
                control
                    .saturating_sub(if passcred { 32 } else { 0 })
                    .saturating_sub(16)
                    / 4
            } else {
                control
            };
            if let Some(rights) = &message.rights {
                let truncated;
                (received, truncated) = rights.receive(capacity.min(253), flags);
                if truncated {
                    out_flags |= 8;
                }
            }
            Ok(if flags & MSG_PEEK != 0 {
                Some(message.clone())
            } else {
                queue.messages.pop_front()
            })
        });
        if result.is_err() {
            for fd in received.drain(..) {
                let _ = crate::close(fd);
            }
        }
        match result {
            Ok(None) => return Ok((0, received, 0, None, UnixAddress::unnamed())),
            Ok(Some(message)) => {
                if flags & MSG_PEEK == 0 {
                    if let Some(rights) = &message.rights {
                        rights.release();
                    }
                    crate::epoll::readiness_consumed(
                        description,
                        crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
                    );
                }
                let copied = len.min(message.payload.len());
                if copied != 0 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(message.payload.as_ptr(), buffer, copied);
                    }
                }
                if copied < message.payload.len() {
                    out_flags |= 0x20;
                }
                return Ok((
                    if flags & 0x20 != 0 {
                        message.payload.len()
                    } else {
                        copied
                    },
                    received,
                    out_flags,
                    credentials,
                    message.source,
                ));
            }
            Err(EAGAIN)
                if flags & MSG_DONTWAIT == 0 && !get(fd)?.flags.contains(FdFlags::NONBLOCK) =>
            {
                wait()?
            }
            Err(e) => return Err(e),
        }
    }
}
pub(super) fn poll(fd: i32) -> Result<Readiness, i32> {
    let queue = Queue::decode(&snapshot(fd)?.record.data()?)?;
    let mut ready = Readiness(0);
    if queue.read_closed || !queue.messages.is_empty() {
        ready = ready.union(Readiness::READABLE);
    }
    if !queue.write_closed {
        let writable = if queue.peer == 0 {
            true
        } else {
            procnet::Record::restore(queue.peer)
                .and_then(|r| Queue::decode(&r.data()?))
                .map_or(true, |q| {
                    q.read_closed
                        || q.messages.len() < 64
                            && q.messages.iter().map(|m| m.payload.len()).sum::<usize>() < LIMIT
                })
        };
        if writable {
            ready = ready.union(Readiness::WRITABLE);
        }
    }
    Ok(ready)
}
pub(super) fn shutdown(fd: i32, how: i32) -> Result<(), i32> {
    if !matches!(how, SHUT_RD | SHUT_WR | SHUT_RDWR) {
        return Err(EINVAL);
    }
    let discarded = update(&snapshot(fd)?.record, |q| {
        let mut discarded = Vec::new();
        if how != SHUT_WR {
            q.read_closed = true;
            discarded.extend(q.messages.drain(..).filter_map(|m| m.rights));
        }
        if how != SHUT_RD {
            q.write_closed = true;
        }
        Ok(discarded)
    })?;
    for rights in discarded {
        rights.release();
    }
    Ok(())
}
