//! POSIX message queues and the IPC-namespace-owned mqueue superblock.
use super::*;
use std::cell::Cell;
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects,
};
const MAX_MESSAGE: u64 = 16_777_216;
const MAX_MESSAGES: u64 = 65_536;
const MAX_PRIORITY: u32 = 32_768;
#[derive(Clone, Copy, Debug)]
pub struct Attr {
    pub flags: i64,
    pub maxmsg: i64,
    pub msgsize: i64,
    pub curmsgs: i64,
}
#[derive(Clone, Copy)]
pub struct Notification {
    pub pid: u32,
    pub host: u32,
    pub method: u32,
    pub signal: u32,
    pub value: u64,
    pub sender: u32,
    pub uid: u32,
}
#[derive(Clone)]
struct Message {
    priority: u32,
    data: Vec<u8>,
}
#[derive(Clone)]
pub(super) struct Queue {
    maxmsg: u64,
    msgsize: u64,
    uid: u32,
    messages: Vec<Message>,
    notify: Option<Notification>,
    receivers: Vec<(u32, u32)>,
}
pub(super) struct Instance {
    pub ipc: u64,
    owner: u64,
    limits: [u64; 5],
    pub queues: BTreeMap<u64, Queue>,
    delivered: Vec<Notification>,
}
thread_local! { static CREATE_ATTR: Cell<Option<Attr>> = const { Cell::new(None) }; }
impl Instance {
    pub(super) fn encode(value: &Option<Self>, b: &mut Vec<u8>) {
        word(b, u64::from(value.is_some()));
        let Some(m) = value else {
            return;
        };
        word(b, m.ipc);
        word(b, m.owner);
        for limit in m.limits {
            word(b, limit);
        }
        word(b, m.queues.len() as u64);
        for (id, q) in &m.queues {
            for v in [
                *id,
                q.maxmsg,
                q.msgsize,
                q.uid as u64,
                q.messages.len() as u64,
            ] {
                word(b, v);
            }
            for msg in &q.messages {
                word(b, msg.priority as u64);
                bytes(b, &msg.data);
            }
            encode_notify(q.notify, b);
            word(b, q.receivers.len() as u64);
            for (pid, tid) in &q.receivers {
                word(b, *pid as u64);
                word(b, *tid as u64);
            }
        }
        word(b, m.delivered.len() as u64);
        for n in &m.delivered {
            encode_notify(Some(*n), b);
        }
    }
    pub(super) fn decode(r: &mut Reader<'_>) -> Result<Option<Self>, i32> {
        match r.word()? {
            0 => return Ok(None),
            1 => (),
            _ => return Err(EIO),
        }
        let mut m = Self {
            ipc: r.word()?,
            owner: r.word()?,
            limits: [0; 5],
            queues: BTreeMap::new(),
            delivered: Vec::new(),
        };
        for v in &mut m.limits {
            *v = r.word()?;
        }
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            let id = r.word()?;
            let mut q = Queue {
                maxmsg: r.word()?,
                msgsize: r.word()?,
                uid: r.word()? as u32,
                messages: Vec::new(),
                notify: None,
                receivers: Vec::new(),
            };
            let count = r.word()?;
            if count > q.maxmsg {
                return Err(EIO);
            }
            for _ in 0..count {
                q.messages.push(Message {
                    priority: r.word()? as u32,
                    data: r.bytes()?.to_vec(),
                });
            }
            q.notify = decode_notify(r)?;
            let count = r.word()?;
            if count > 1_000_000 {
                return Err(EIO);
            }
            for _ in 0..count {
                q.receivers.push((r.word()? as u32, r.word()? as u32));
            }
            m.queues.insert(id, q);
        }
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            m.delivered.push(decode_notify(r)?.ok_or(EIO)?);
        }
        Ok(Some(m))
    }
}
fn encode_notify(n: Option<Notification>, b: &mut Vec<u8>) {
    word(b, u64::from(n.is_some()));
    if let Some(n) = n {
        for v in [
            n.pid as u64,
            n.host as u64,
            n.method as u64,
            n.signal as u64,
            n.value,
            n.sender as u64,
            n.uid as u64,
        ] {
            word(b, v);
        }
    }
}
fn decode_notify(r: &mut Reader<'_>) -> Result<Option<Notification>, i32> {
    match r.word()? {
        0 => Ok(None),
        1 => Ok(Some(Notification {
            pid: r.word()? as u32,
            host: r.word()? as u32,
            method: r.word()? as u32,
            signal: r.word()? as u32,
            value: r.word()?,
            sender: r.word()? as u32,
            uid: r.word()? as u32,
        })),
        _ => Err(EIO),
    }
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    if !options.is_empty() {
        return Err(EINVAL);
    }
    let ipc = namespaces::current_id(namespaces::IPC)?;
    let owner = namespaces::owner(namespaces::IPC, ipc)?;
    namespaces::ipc_update(ipc, |data| {
        if !data.is_empty() {
            let source = String::from_utf8(data.to_vec()).map_err(|_| EIO)?;
            let (id, _) = parse(&source)?;
            let _ = volume(id)?;
            return Ok((data.to_vec(), source));
        }
        let source = super::prepare_unkept("size=4k,nr_inodes=1000000,mode=1777")?;
        let (id, _) = parse(&source)?;
        volume(id)?
            .message_queues
            .store(true, std::sync::atomic::Ordering::Release);
        volume(id)?.change(|s| {
            s.nodes.get_mut(&1).ok_or(EIO)?.size = 40;
            s.mq = Some(Instance {
                ipc,
                owner,
                limits: [10, 10, 8192, 8192, 256],
                queues: BTreeMap::new(),
                delivered: Vec::new(),
            });
            Ok(())
        })?;
        catalog()?.update(|bytes| {
            let mut ids = decode_catalog(bytes)?;
            if !ids.contains(&id) {
                ids.push(id);
            }
            Ok((encode_catalog(&ids), ()))
        })?;
        start_keeper(id)?;
        Ok((source.as_bytes().to_vec(), source))
    })
}
pub(crate) fn reconfigure(_source: &str, options: &str) -> Result<(), i32> {
    if options.is_empty() {
        Ok(())
    } else {
        Err(EINVAL)
    }
}
pub(crate) fn namespace_alive(ipc: u64) -> bool {
    let key = if ipc == 1 {
        u64::MAX - namespaces::IPC as u64
    } else {
        ipc
    };
    // Do not register the keeper as a guest or retain the namespace ourselves.
    Store::user_object(key, false).is_ok()
        || kinakaze_runtime::job::everyone().into_iter().any(|p| {
            namespaces::process_id(p.namespace_pid, namespaces::IPC).is_ok_and(|id| id == ipc)
        })
}
fn process_live(pid: u32, host: u32) -> bool {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    if !kinakaze_runtime::job::process_info(pid).is_some_and(|p| p.entry.pid == host) {
        return false;
    }
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, host) };
    if h.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(h, 0) == 258 };
    unsafe {
        CloseHandle(h);
    }
    alive
}
fn thread_live(pid: u32, tid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{
        GetProcessIdOfThread, OpenThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SYNCHRONIZE,
        WaitForSingleObject,
    };
    let Some(p) = kinakaze_runtime::job::process_info(pid) else {
        return false;
    };
    let h = unsafe {
        OpenThread(
            THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
            0,
            tid,
        )
    };
    if h.is_null() {
        return false;
    }
    let alive =
        unsafe { GetProcessIdOfThread(h) == p.entry.pid && WaitForSingleObject(h, 0) == 258 };
    unsafe {
        CloseHandle(h);
    }
    alive
}
pub(super) fn collect(s: &mut State) {
    let Some(m) = &mut s.mq else {
        return;
    };
    for q in m.queues.values_mut() {
        if q.notify.is_some_and(|n| !process_live(n.pid, n.host)) {
            q.notify = None;
        }
        q.receivers.retain(|(pid, tid)| thread_live(*pid, *tid));
    }
    m.delivered.retain(|n| process_live(n.pid, n.host));
}
pub(super) fn insert(s: &mut State, start: u64, path: &str, mut n: Node) -> Result<u64, i32> {
    if n.mode & S_IFMT != S_IFREG {
        return Err(EPERM);
    }
    let m = s.mq.as_ref().ok_or(EIO)?;
    let a = CREATE_ATTR.get().unwrap_or(Attr {
        flags: 0,
        maxmsg: m.limits[0].min(m.limits[1]) as i64,
        msgsize: m.limits[2].min(m.limits[3]) as i64,
        curmsgs: 0,
    });
    if a.maxmsg <= 0
        || a.msgsize <= 0
        || a.maxmsg as u64 > MAX_MESSAGES
        || a.msgsize as u64 > MAX_MESSAGE
    {
        return Err(EINVAL);
    }
    let privileged = user_namespace::capable(m.owner, 24);
    if !privileged && (a.maxmsg as u64 > m.limits[1] || a.msgsize as u64 > m.limits[3]) {
        return Err(EINVAL);
    }
    if !privileged && m.queues.len() as u64 >= m.limits[4] {
        return Err(ENOSPC);
    }
    let uid = credentials::real_uid();
    let charge = charge(a.maxmsg as u64, a.msgsize as u64);
    let mut used = m
        .queues
        .values()
        .filter(|q| q.uid == uid)
        .map(|q| charge_queue(q))
        .sum::<u64>();
    for id in decode_catalog(&catalog()?.read()?.1)? {
        let Ok(store) = Store::user_object(id, false) else {
            continue;
        };
        let mut state = State::decode(&store.read()?.1)?;
        if state.mq.as_ref().is_some_and(|other| other.ipc == m.ipc) {
            continue;
        }
        state.collect();
        if let Some(other) = state.mq {
            used += other
                .queues
                .values()
                .filter(|q| q.uid == uid)
                .map(charge_queue)
                .sum::<u64>();
        }
    }
    if used.saturating_add(charge) > limits(0, None)?.0 {
        return Err(EMFILE);
    }
    // Leave metadata headroom in the atomic 32 MiB publication bank.
    if m.queues
        .values()
        .map(charge_queue)
        .sum::<u64>()
        .saturating_add(charge)
        > 24 * 1024 * 1024
    {
        return Err(ENOSPC);
    }
    n.size = 80;
    let m = s.mq.take().unwrap();
    let result = s.insert(start, path, n);
    s.mq = Some(m);
    let id = result?;
    s.mq.as_mut().unwrap().queues.insert(
        id,
        Queue {
            maxmsg: a.maxmsg as u64,
            msgsize: a.msgsize as u64,
            uid,
            messages: Vec::new(),
            notify: None,
            receivers: Vec::new(),
        },
    );
    Ok(id)
}
fn hidden() -> Result<Location, i32> {
    let source = prepare("")?;
    let (volume, node) = parse(&source)?;
    Ok(Location {
        volume,
        node,
        tail: String::new(),
        namespace: mount::namespace_id()?,
        mount: 0,
        flags: mount::MS_TMPFS | mount::MS_MQUEUE | 2 | 4 | 8,
        path: "/dev/mqueue".into(),
    })
}
fn name(name: &str) -> Result<(), i32> {
    if name.is_empty() {
        return Err(EINVAL);
    }
    if name.len() > 255 {
        return Err(ENAMETOOLONG);
    }
    if name.contains('/') || matches!(name, "." | "..") {
        return Err(EACCES);
    }
    Ok(())
}
pub fn open(name_: &str, flags: i32, mode: u32, attr: Option<Attr>) -> Result<i32, i32> {
    name(name_)?;
    if flags & !(O_ACCMODE | O_CREAT | O_EXCL | O_NONBLOCK | O_CLOEXEC) != 0
        || flags & O_ACCMODE == 3
    {
        return Err(EINVAL);
    }
    let mut l = hidden()?;
    let _transaction = transaction(l.volume)?;
    l.tail = name_.into();
    let old = CREATE_ATTR.replace(attr);
    let result = open_pinned(l, flags | O_CLOEXEC, mode);
    CREATE_ATTR.set(old);
    result
}
pub fn unlink(name_: &str) -> Result<(), i32> {
    name(name_)?;
    let l = hidden()?;
    volume(l.volume)?.change(|s| {
        let id = *s.nodes[&1].children.get(name_).ok_or(ENOENT)?;
        s.nodes[&1].access(3)?;
        s.sticky(1, id)?;
        s.nodes.get_mut(&1).unwrap().children.remove(name_);
        let n = s.nodes.get_mut(&id).unwrap();
        n.links -= 1;
        n.ctime = now();
        Ok(())
    })
}
fn queue_fd(fd: i32) -> Result<(Store, Location, u64, i32), i32> {
    let result = descriptor(fd)?;
    if get(fd)?.kind != FdKind::MessageQueue || result.3 & O_PATH != 0 {
        return Err(EBADF);
    }
    Ok(result)
}
pub fn getattr(fd: i32, flags: Option<i64>) -> Result<Attr, i32> {
    let (d, l, _, _) = queue_fd(fd)?;
    if flags.is_some_and(|f| f & !(O_NONBLOCK as i64) != 0) {
        return Err(EINVAL);
    }
    let v = volume(l.volume)?;
    d.update(|old| {
        let current = i64::from_le_bytes(old[old.len() - 8..].try_into().unwrap()) as i32;
        let a = v.change(|s| {
            let q =
                s.mq.as_ref()
                    .and_then(|m| m.queues.get(&l.node))
                    .ok_or(EBADF)?;
            Ok(Attr {
                flags: (current & O_NONBLOCK) as i64,
                maxmsg: q.maxmsg as i64,
                msgsize: q.msgsize as i64,
                curmsgs: q.messages.len() as i64,
            })
        })?;
        let mut bytes = old.to_vec();
        if let Some(flags) = flags {
            let end = bytes.len();
            bytes[end - 8..].copy_from_slice(
                &(((current & !O_NONBLOCK) | (flags as i32)) as u64).to_le_bytes(),
            );
        }
        Ok((bytes, a))
    })
}
struct Event(HANDLE);
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn event(l: &Location, receive: bool) -> Result<Event, i32> {
    let name: Vec<u16> = format!(
        "Local\\kinakaze.mqueue.{}.{}.{}.{}",
        kinakaze_runtime::authority::domain_id(),
        l.volume,
        l.node,
        if receive { "r" } else { "w" }
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let h = unsafe { CreateEventW(ptr::null(), 1, 0, name.as_ptr()) };
    if h.is_null() { Err(EIO) } else { Ok(Event(h)) }
}
fn publish(l: &Location, q: &Queue) {
    for (receive, ready) in [
        (true, !q.messages.is_empty()),
        (false, (q.messages.len() as u64) < q.maxmsg),
    ] {
        if let Ok(e) = event(l, receive) {
            unsafe {
                if ready {
                    SetEvent(e.0);
                } else {
                    ResetEvent(e.0);
                }
            }
        }
    }
}
fn wait(event: &Event, deadline: Option<[i64; 2]>) -> Result<(), i32> {
    let millis = if let Some([secs, nanos]) = deadline {
        if secs < 0 || !(0..1_000_000_000).contains(&nanos) {
            return Err(EINVAL);
        }
        let end = secs as u128 * 1_000_000_000 + nanos as u128;
        let current = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        if current >= end {
            return Err(110);
        }
        ((end - current).div_ceil(1_000_000)).min(1000) as u32
    } else {
        1000
    };
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    match unsafe { WaitForMultipleObjects(2, [event.0, interrupt].as_ptr(), 0, millis) } {
        0 | 258 => Ok(()),
        1 => {
            if signal::deliver_pending() == signal::Delivery::Interrupted {
                Err(EINTR)
            } else {
                Ok(())
            }
        }
        _ => Err(EIO),
    }
}
fn timeout_valid(timeout: Option<[i64; 2]>) -> Result<(), i32> {
    if timeout.is_some_and(|t| t[0] < 0 || !(0..1_000_000_000).contains(&t[1])) {
        Err(EINVAL)
    } else {
        Ok(())
    }
}
pub fn send(fd: i32, data: &[u8], priority: u32, deadline: Option<[i64; 2]>) -> Result<(), i32> {
    if priority >= MAX_PRIORITY {
        return Err(EINVAL);
    }
    timeout_valid(deadline)?;
    let (description, l, _, flags) = queue_fd(fd)?;
    if flags & O_ACCMODE == O_RDONLY {
        return Err(EBADF);
    }
    let v = volume(l.volume)?;
    let wake = event(&l, false)?;
    loop {
        let descriptor_bytes = description.read()?.1;
        let flags = u64::from_le_bytes(
            descriptor_bytes[descriptor_bytes.len() - 8..]
                .try_into()
                .unwrap(),
        ) as i32;
        let outcome = v.change(|s| {
            let m = s.mq.as_mut().ok_or(EBADF)?;
            let q = m.queues.get_mut(&l.node).ok_or(EBADF)?;
            if data.len() as u64 > q.msgsize {
                return Err(EMSGSIZE);
            }
            if q.messages.len() as u64 >= q.maxmsg {
                publish(&l, q);
                return Ok(None);
            }
            let was_empty = q.messages.is_empty();
            let at = q
                .messages
                .iter()
                .position(|m| m.priority < priority)
                .unwrap_or(q.messages.len());
            q.messages.insert(
                at,
                Message {
                    priority,
                    data: data.to_vec(),
                },
            );
            let notification = if was_empty && q.receivers.is_empty() {
                q.notify.take()
            } else {
                None
            };
            publish(&l, q);
            if let Some(mut n) = notification.filter(|n| n.method == 0) {
                n.sender = job::process_id();
                n.uid = credentials::real_uid();
                m.delivered.push(n);
            }
            let n = s.nodes.get_mut(&l.node).ok_or(EIO)?;
            n.mtime = now();
            n.ctime = n.mtime;
            Ok(Some(notification))
        })?;
        if let Some(notification) = outcome {
            if let Some(n) = notification.filter(|n| n.method == 0) {
                let _ = job::deliver_to(n.pid, n.signal as i32);
            }
            return Ok(());
        }
        if flags & O_NONBLOCK != 0 {
            return Err(EAGAIN);
        }
        wait(&wake, deadline)?;
    }
}
pub fn receive(fd: i32, length: usize, deadline: Option<[i64; 2]>) -> Result<(Vec<u8>, u32), i32> {
    timeout_valid(deadline)?;
    let (description, l, _, flags) = queue_fd(fd)?;
    if flags & O_ACCMODE == O_WRONLY {
        return Err(EBADF);
    }
    let v = volume(l.volume)?;
    let wake = event(&l, true)?;
    let waiter = (job::process_id(), crate::interrupt::current_thread_id());
    let run = || loop {
        let descriptor_bytes = description.read()?.1;
        let flags = u64::from_le_bytes(
            descriptor_bytes[descriptor_bytes.len() - 8..]
                .try_into()
                .unwrap(),
        ) as i32;
        let result = v.change(|s| {
            let q =
                s.mq.as_mut()
                    .and_then(|m| m.queues.get_mut(&l.node))
                    .ok_or(EBADF)?;
            if (length as u64) < q.msgsize {
                return Err(EMSGSIZE);
            }
            if !q.messages.is_empty() {
                let msg = q.messages.remove(0);
                q.receivers.retain(|w| *w != waiter);
                publish(&l, q);
                return Ok(Some((msg.data, msg.priority)));
            }
            if flags & O_NONBLOCK == 0 && !q.receivers.contains(&waiter) {
                q.receivers.push(waiter);
            }
            publish(&l, q);
            Ok(None)
        })?;
        if let Some(result) = result {
            return Ok(result);
        }
        if flags & O_NONBLOCK != 0 {
            return Err(EAGAIN);
        }
        wait(&wake, deadline)?;
    };
    let result = run();
    let _ = v.change(|s| {
        if let Some(q) = s.mq.as_mut().and_then(|m| m.queues.get_mut(&l.node)) {
            q.receivers.retain(|w| *w != waiter);
        }
        Ok(())
    });
    result
}
pub fn poll(fd: i32) -> Result<(bool, bool), i32> {
    let (_, l, _, _) = queue_fd(fd)?;
    volume(l.volume)?.change(|s| {
        let q =
            s.mq.as_ref()
                .and_then(|m| m.queues.get(&l.node))
                .ok_or(EBADF)?;
        Ok((!q.messages.is_empty(), (q.messages.len() as u64) < q.maxmsg))
    })
}
pub fn read_status(fd: i32, buffer: &mut [u8], position: Option<u64>) -> Result<usize, i32> {
    let (d, l, _, flags) = queue_fd(fd)?;
    if flags & O_ACCMODE == O_WRONLY {
        return Err(EBADF);
    }
    let v = volume(l.volume)?;
    d.update(|old| {
        let offset = u64::from_le_bytes(old[old.len() - 16..old.len() - 8].try_into().unwrap());
        let at = position.unwrap_or(offset);
        let text = v.change(|s| {
            let q =
                s.mq.as_ref()
                    .and_then(|m| m.queues.get(&l.node))
                    .ok_or(EBADF)?;
            let n = q.notify;
            Ok(format!(
                "QSIZE:{:<10} NOTIFY:{:<5} SIGNO:{:<5} NOTIFY_PID:{:<6}\n",
                q.messages.iter().map(|m| m.data.len()).sum::<usize>(),
                n.map_or(0, |n| n.method),
                n.filter(|n| n.method == 0).map_or(0, |n| n.signal),
                n.and_then(|n| kinakaze_runtime::job::namespaces::visible(n.pid))
                    .unwrap_or(0)
            ))
        })?;
        let at = (at as usize).min(text.len());
        let count = buffer.len().min(text.len() - at);
        buffer[..count].copy_from_slice(&text.as_bytes()[at..at + count]);
        Ok((
            encode_descriptor(
                &l,
                if position.is_none() {
                    offset + count as u64
                } else {
                    offset
                },
                flags,
            ),
            count,
        ))
    })
}
pub fn notify(fd: i32, value: Option<Notification>) -> Result<(), i32> {
    let (_, l, _, _) = queue_fd(fd)?;
    if let Some(n) = value {
        if n.method > 1 {
            return Err(EOPNOTSUPP);
        }
        if n.method == 0 && (n.signal == 0 || n.signal >= 65) {
            return Err(EINVAL);
        }
    }
    volume(l.volume)?.change(|s| {
        let q =
            s.mq.as_mut()
                .and_then(|m| m.queues.get_mut(&l.node))
                .ok_or(EBADF)?;
        if let Some(n) = value {
            if q.notify.is_some() {
                return Err(EBUSY);
            }
            q.notify = Some(n);
        } else if q.notify.is_some_and(|n| n.pid == job::process_id()) {
            q.notify = None;
        }
        Ok(())
    })
}
pub(crate) fn closed(entry: FdEntry) {
    let run = || {
        let d = shared::object_entry(entry)?;
        let data = d.read()?.1;
        let mut r = Reader(&data);
        let id = r.word()?;
        let node = r.word()?;
        volume(id)?.change(|s| {
            if let Some(q) = s.mq.as_mut().and_then(|m| m.queues.get_mut(&node)) {
                if q.notify.is_some_and(|n| n.pid == job::process_id()) {
                    q.notify = None;
                }
            }
            Ok(())
        })
    };
    let _ = run();
}
pub fn take_notification(signal: i32) -> Option<Notification> {
    let volumes: Vec<_> = VOLUMES.lock().ok()?.values().cloned().collect();
    for v in volumes
        .into_iter()
        .filter(|v| v.message_queues.load(std::sync::atomic::Ordering::Acquire))
    {
        if let Ok(Some(n)) = v.change(|s| {
            let Some(m) = &mut s.mq else { return Ok(None) };
            let at = m
                .delivered
                .iter()
                .position(|n| n.pid == job::process_id() && n.signal == signal as u32);
            Ok(at.map(|at| m.delivered.remove(at)))
        }) {
            return Some(n);
        }
    }
    None
}
pub fn sysctl(name: &str, value: Option<u64>) -> Result<u64, i32> {
    let slot = match name {
        "msg_default" => 0,
        "msg_max" => 1,
        "msgsize_default" => 2,
        "msgsize_max" => 3,
        "queues_max" => 4,
        _ => return Err(ENOENT),
    };
    let l = hidden()?;
    volume(l.volume)?.change(|s| {
        let m = s.mq.as_mut().ok_or(EIO)?;
        if let Some(value) = value {
            if !user_namespace::capable(m.owner, 21) {
                return Err(EPERM);
            }
            let (min, max) = match slot {
                0 | 1 => (1, MAX_MESSAGES),
                2 | 3 => (128, MAX_MESSAGE),
                _ => (1, i32::MAX as u64),
            };
            if !(min..=max).contains(&value) {
                return Err(EINVAL);
            }
            m.limits[slot] = value;
        }
        Ok(m.limits[slot])
    })
}

fn charge(maxmsg: u64, msgsize: u64) -> u64 {
    maxmsg * (msgsize + 48) + maxmsg.min(MAX_PRIORITY as u64) * 48
}
fn charge_queue(q: &Queue) -> u64 {
    charge(q.maxmsg, q.msgsize)
}
fn catalog() -> Result<Arc<Store>, i32> {
    static CATALOG: std::sync::OnceLock<Arc<Store>> = std::sync::OnceLock::new();
    if let Some(store) = CATALOG.get() {
        return Ok(store.clone());
    }
    let store = Arc::new(Store::user_object(u64::MAX - 24, true)?);
    let _ = CATALOG.set(store.clone());
    Ok(store)
}
fn decode_catalog(bytes: &[u8]) -> Result<Vec<u64>, i32> {
    if bytes.len() % 8 != 0 {
        return Err(EIO);
    }
    Ok(bytes
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .collect())
}
fn encode_catalog(ids: &[u64]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_le_bytes()).collect()
}
pub(super) struct Transaction(HANDLE);
impl Drop for Transaction {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
pub(super) fn transaction(id: u64) -> Result<Transaction, i32> {
    use windows_sys::Win32::System::Threading::{CreateMutexW, INFINITE, WaitForSingleObject};
    let name: Vec<u16> = format!(
        "Local\\kinakaze.mqueue.quota.{}",
        kinakaze_runtime::authority::domain_id()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let raw = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
    if raw.is_null() {
        return Err(EIO);
    }
    match unsafe { WaitForSingleObject(raw, INFINITE) } {
        0 | 128 => {
            let guard = Transaction(raw);
            catalog()?.update(|bytes| {
                let mut ids = decode_catalog(bytes)?;
                ids.retain(|id| Store::user_object(*id, false).is_ok());
                if !ids.contains(&id) {
                    ids.push(id);
                }
                Ok((encode_catalog(&ids), ()))
            })?;
            Ok(guard)
        }
        _ => {
            unsafe {
                CloseHandle(raw);
            }
            Err(EIO)
        }
    }
}
pub fn limits(pid: u32, value: Option<(u64, u64)>) -> Result<(u64, u64), i32> {
    let own = job::process_id();
    let target = if pid == 0 {
        own
    } else {
        kinakaze_runtime::job::namespaces::resolve(pid).ok_or(ESRCH)?
    };
    let old = kinakaze_runtime::job::mqueue_limits(target, None).map_err(|_| ESRCH)?;
    if let Some((soft, hard)) = value {
        if soft > hard {
            return Err(EINVAL);
        }
        if (target != own || hard > old.1) && !user_namespace::capable(1, 24) {
            return Err(EPERM);
        }
        kinakaze_runtime::job::mqueue_limits(target, Some((soft, hard))).map_err(|_| ESRCH)?;
    }
    Ok(old)
}

/// Fill the SI_MESGQ union used by handlers and sigwaitinfo/sigtimedwait.
pub fn signal_info(signal: i32, payload: &mut [u8; 116]) -> bool {
    let Some(n) = take_notification(signal) else {
        return false;
    };
    payload[4..8].copy_from_slice(
        &kinakaze_runtime::job::namespaces::visible(n.sender)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    payload[8..12].copy_from_slice(&user_namespace::visible(n.uid, false).to_le_bytes());
    payload[12..20].copy_from_slice(&n.value.to_le_bytes());
    true
}
