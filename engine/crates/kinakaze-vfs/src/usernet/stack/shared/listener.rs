//! A shared TCP listener and connection bank. Pending connections are in the
//! listener's section, so no creating process has to keep separate child
//! sections alive. Accepted views pin the arena independently of the listener.
//! Socket-description ownership is external: `abort` explicitly retires a slot.
use super::*;
use std::collections::BTreeMap;

pub(super) const CAPACITY: usize = 64;
const SYN: u32 = 1;
const READY: u32 = 2;
const ACCEPTED: u32 = 3;
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(super) struct Slot {
    pub state: u32,
    _reserved: u32,
    pub generation: u64,
    ready_order: u64,
    parent_revision: u64,
    description: u64,
}
#[repr(C)]
pub(super) struct Control {
    active: u32,
    backlog: u32,
    next_generation: u64,
    next_ready: u64,
    pub slots: [Slot; CAPACITY],
}

pub struct Accepted {
    pub slot: usize,
    pub generation: u64,
    pub endpoint: Arc<SharedEndpoint>,
}
pub struct Listener {
    root: Arc<SharedEndpoint>,
    children: Mutex<BTreeMap<(usize, u64), Arc<SharedEndpoint>>>,
}
impl SharedEndpoint {
    pub(super) fn collect_empty(self: &Arc<Self>) -> Result<bool, i32> {
        let _guard = self.acquire()?;
        // Native leases cover mappings in other processes; Arc references also
        // protect an unpublished creator and local operations/observers.
        if Arc::strong_count(self) != 1 || self._lease.as_ref().ok_or(EIO)?.handle_count()? != 1 {
            return Ok(false);
        }
        if !crate::usernet::packet::managed_owners(self.id)?.is_empty() {
            return Ok(false);
        }
        let orphan = |core: &mut Core| -> Result<(), i32> {
            if core.kind == Kind::Tcp {
                let socket = core.sockets.get_mut::<tcp::Socket>(core.socket);
                if socket.timeout().is_none() {
                    socket.set_timeout(Some(smoltcp::time::Duration::from_secs(60)));
                }
                socket.close();
            }
            Ok(())
        };
        self.with_locked(orphan)?;
        let slots = unsafe { (*self.view.0.Value.cast::<Layout>()).listener.slots };
        for (index, slot) in slots
            .into_iter()
            .enumerate()
            .filter(|(_, slot)| slot.state != 0)
        {
            let child = self.child_view_locked(index, slot.generation)?;
            child.with_locked(orphan)?;
            child.poll_locked(None, 0, false)?;
            if child.tcp_state()? != TcpState::Closed {
                return Ok(false);
            }
        }
        self.poll_locked(None, 0, false)?;
        if self.with_locked(|core| {
            Ok(core.kind == Kind::Tcp
                && core.sockets.get::<tcp::Socket>(core.socket).state() != TcpState::Closed)
        })? {
            return Ok(false);
        }
        if Arc::strong_count(self) != 1 || self._lease.as_ref().ok_or(EIO)?.handle_count()? != 1 {
            return Ok(false);
        }
        unsafe {
            (*self.view.0.Value.cast::<Layout>()).arena_id = 0;
        }
        Ok(true)
    }
    pub(super) fn descriptions(&self) -> Result<Vec<(u64, [u8; 120])>, i32> {
        let _guard = self.acquire()?;
        let layout = unsafe { &*self.view.0.Value.cast::<Layout>() };
        let mut result = Vec::new();
        if layout.description != [0; 120] {
            result.push((self.id, layout.description));
        }
        for (index, slot) in layout.listener.slots.iter().enumerate() {
            if slot.state == 0 || slot.description == 0 {
                continue;
            }
            let child = self.child_view_locked(index, slot.generation)?;
            let bytes = unsafe { (*child.view.0.Value.cast::<Layout>()).description };
            if bytes != [0; 120] {
                result.push((slot.description, bytes));
            }
        }
        Ok(result)
    }
    fn child_view_locked(&self, index: usize, generation: u64) -> Result<Arc<Self>, i32> {
        if self.identity.is_some() || index >= CAPACITY {
            return Err(EINVAL);
        }
        let control = unsafe { &*ptr::addr_of!((*self.view.0.Value.cast::<Layout>()).listener) };
        if control.slots[index].state == 0 || control.slots[index].generation != generation {
            return Err(crate::EBADF);
        }
        let address = unsafe {
            self.view
                .0
                .Value
                .cast::<u8>()
                .add((index + 1) * SLOT_STRIDE)
        };
        Ok(Arc::new(Self {
            id: self.id,
            _lease: self._lease.clone(),
            view: ViewRef(
                MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: address.cast(),
                },
                self.view.1.clone(),
            ),
            _section: self._section.clone(),
            mutex: self.mutex.clone(),
            cache: Mutex::new(None),
            name: format!("{}.C{index}.{generation}", self.name),
            notifications: Mutex::new(Default::default()),
            root_notifications: Mutex::new(Default::default()),
            identity: Some((index, generation)),
            _allocation: self._allocation.clone(),
        }))
    }
    /// Reopen a previously accepted connection using its arena and generation.
    /// The caller must hold a section reference during the handoff.
    pub fn open_connection(
        domain: u64,
        id: u64,
        index: usize,
        generation: u64,
    ) -> Result<Arc<Self>, i32> {
        let root = Self::open(domain, id)?;
        let _guard = root.acquire()?;
        root.child_view_locked(index, generation)
    }
}
impl Listener {
    pub fn new(
        root: Arc<SharedEndpoint>,
        local: IpEndpoint,
        backlog: usize,
    ) -> Result<Arc<Self>, i32> {
        if root.identity.is_some() {
            return Err(EINVAL);
        }
        {
            let _guard = root.acquire()?;
            let control =
                unsafe { &mut *ptr::addr_of_mut!((*root.view.0.Value.cast::<Layout>()).listener) };
            if control.active != 0 {
                return Err(EINVAL);
            }
            root.listen(local)?;
            control.active = 1;
            control.backlog = backlog.clamp(1, CAPACITY) as u32;
        }
        Ok(Arc::new(Self {
            root,
            children: Mutex::new(BTreeMap::new()),
        }))
    }
    pub fn open(domain: u64, id: u64) -> Result<Arc<Self>, i32> {
        let root = SharedEndpoint::open(domain, id)?;
        {
            let _guard = root.acquire()?;
            if unsafe { (*Self::control(&root)).active } == 0 {
                return Err(EINVAL);
            }
        }
        Ok(Arc::new(Self {
            root,
            children: Mutex::new(BTreeMap::new()),
        }))
    }
    fn control(root: &SharedEndpoint) -> *mut Control {
        unsafe { ptr::addr_of_mut!((*root.view.0.Value.cast::<Layout>()).listener) }
    }
    pub fn endpoint(&self) -> &Arc<SharedEndpoint> {
        &self.root
    }
    /// Reconstruct the transport for accepted connections even after the
    /// listening description closed. A bank keeps its backlog field then.
    pub(crate) fn reopen(root: Arc<SharedEndpoint>) -> Result<Option<Arc<Self>>, i32> {
        {
            let _guard = root.acquire()?;
            if root.identity.is_some() {
                return Err(EINVAL);
            }
            if unsafe { (*Self::control(&root)).backlog } == 0 {
                return Ok(None);
            }
        }
        Ok(Some(Arc::new(Self {
            root,
            children: Mutex::new(BTreeMap::new()),
        })))
    }
    fn child_locked(&self, index: usize, generation: u64) -> Result<Arc<SharedEndpoint>, i32> {
        let mut cache = self.children.lock().map_err(|_| EIO)?;
        if let Some(child) = cache.get(&(index, generation)) {
            return Ok(child.clone());
        }
        let records = unsafe { (*Self::control(&self.root)).slots };
        cache.retain(|&(index, generation), _| {
            records[index].state != 0 && records[index].generation == generation
        });
        let child = self.root.child_view_locked(index, generation)?;
        cache.insert((index, generation), child.clone());
        Ok(child)
    }
    fn allocate_locked(
        &self,
        local: IpEndpoint,
        state: &State,
        parent_revision: u64,
    ) -> Result<(usize, Arc<SharedEndpoint>), i32> {
        let control = unsafe { &mut *Self::control(&self.root) };
        if control
            .slots
            .iter()
            .filter(|s| matches!(s.state, SYN | READY))
            .count()
            >= control.backlog as usize
        {
            return Err(EAGAIN);
        }
        let index = control
            .slots
            .iter()
            .position(|s| s.state == 0)
            .ok_or(EAGAIN)?;
        let generation = control
            .next_generation
            .checked_add(1)
            .ok_or(crate::EOVERFLOW)?;
        let layout = unsafe {
            self.root
                .view
                .0
                .Value
                .cast::<u8>()
                .add((index + 1) * SLOT_STRIDE)
                .cast::<Layout>()
        };
        if control.slots[index].generation == 0
            && unsafe {
                VirtualAlloc(
                    layout.cast(),
                    size_of::<Layout>(),
                    MEM_COMMIT,
                    PAGE_READWRITE,
                )
            }
            .is_null()
        {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        let (addresses, ifindex, image) = self.root.with_locked(|c| {
            Ok((
                c.interface.ip_addrs().to_vec(),
                unsafe { (*self.root.view.0.Value.cast::<Layout>()).ifindex },
                unsafe { (*self.root.view.0.Value.cast::<Layout>()).image },
            ))
        })?;
        unsafe {
            SharedEndpoint::initialize(
                layout,
                &addresses,
                ifindex,
                Kind::Tcp,
                image,
                self.root.id,
            )?;
        }
        control.slots[index] = Slot {
            state: SYN,
            generation,
            parent_revision,
            ..Slot::default()
        };
        control.next_generation = generation;
        let child = self.child_locked(index, generation)?;
        child.write_filter_locked(state);
        child.with_locked(|c| {
            let socket = c.sockets.get_mut::<tcp::Socket>(c.socket);
            socket.set_timeout(Some(smoltcp::time::Duration::from_secs(60)));
            socket.listen(local).map_err(|_| EINVAL)
        })?;
        Ok((index, child))
    }
    pub fn accept(&self) -> Result<Accepted, i32> {
        let _guard = self.root.acquire()?;
        let control = unsafe { &*Self::control(&self.root) };
        if control.active == 0 {
            return Err(EINVAL);
        }
        let (index, generation) = control
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.state == READY)
            .min_by_key(|(_, s)| s.ready_order)
            .map(|(i, s)| (i, s.generation))
            .ok_or(EAGAIN)?;
        let endpoint = self.child_locked(index, generation)?;
        unsafe {
            (*Self::control(&self.root)).slots[index].state = ACCEPTED;
        }
        self.root.notify_locked();
        Ok(Accepted {
            slot: index,
            generation,
            endpoint,
        })
    }
    pub(crate) fn publish_owner(&self, accepted: &Accepted, description: u64) -> Result<(), i32> {
        let _guard = self.root.acquire()?;
        let slots = unsafe { &mut (*Self::control(&self.root)).slots };
        let slot = slots.get_mut(accepted.slot).ok_or(EINVAL)?;
        if slot.state != ACCEPTED
            || slot.generation != accepted.generation
            || slot.description != 0
            || description == 0
        {
            return Err(EINVAL);
        }
        slot.description = description;
        Ok(())
    }
    pub(crate) fn reconcile_owners(&self, root: u64, present: &[u64]) -> Result<Vec<Vec<u8>>, i32> {
        let _guard = self.root.acquire()?;
        let mut output = Vec::new();
        if !present.contains(&root) && unsafe { (*Self::control(&self.root)).active } != 0 {
            output.extend(self.close()?);
        }
        let slots = unsafe { (*Self::control(&self.root)).slots };
        for (index, slot) in slots.into_iter().enumerate() {
            if slot.state != ACCEPTED
                || slot.description == 0
                || present.contains(&slot.description)
            {
                continue;
            }
            let endpoint = self.child_locked(index, slot.generation)?;
            endpoint.close_tcp()?;
            if endpoint.tcp_state()? == TcpState::Closed {
                output.extend(self.abort(&Accepted {
                    slot: index,
                    generation: slot.generation,
                    endpoint,
                })?);
            }
        }
        Ok(output)
    }
    pub fn readiness(&self) -> Result<(bool, bool, bool), i32> {
        let _guard = self.root.acquire()?;
        let control = unsafe { &*Self::control(&self.root) };
        Ok((
            control.slots.iter().any(|s| s.state == READY),
            false,
            control.active == 0,
        ))
    }
    pub fn set_backlog(&self, backlog: usize) -> Result<(), i32> {
        let _guard = self.root.acquire()?;
        let control = unsafe { &mut *Self::control(&self.root) };
        if control.active == 0 {
            return Err(EINVAL);
        }
        control.backlog = backlog.clamp(1, CAPACITY) as u32;
        Ok(())
    }
    /// Stop accepting and reset connections which have not been accepted.
    /// Established accepted connections retain their own protocol state.
    pub fn close(&self) -> Result<Vec<Vec<u8>>, i32> {
        let _guard = self.root.acquire()?;
        unsafe {
            (*Self::control(&self.root)).active = 0;
        }
        self.root.with_locked(|c| {
            c.sockets.get_mut::<tcp::Socket>(c.socket).close();
            Ok(())
        })?;
        let records = unsafe { (*Self::control(&self.root)).slots };
        let mut output = Vec::new();
        for (index, slot) in records
            .into_iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.state, SYN | READY))
        {
            let child = self.child_locked(index, slot.generation)?;
            child.with_locked(|c| {
                c.sockets.get_mut::<tcp::Socket>(c.socket).abort();
                Ok(())
            })?;
            output.extend(child.poll_locked(None, 0, false)?.1);
            unsafe {
                (*Self::control(&self.root)).slots[index].state = 0;
            }
            self.children
                .lock()
                .map_err(|_| EIO)?
                .remove(&(index, slot.generation));
        }
        self.root.notify_locked();
        Ok(output)
    }
    /// Abort and retire an accepted protocol slot. A stale view subsequently
    /// returns EBADF even when the same slot is used by another connection.
    pub fn abort(&self, accepted: &Accepted) -> Result<Vec<Vec<u8>>, i32> {
        let _guard = self.root.acquire()?;
        let control = unsafe { &*Self::control(&self.root) };
        let slot = *control.slots.get(accepted.slot).ok_or(crate::EBADF)?;
        if slot.state != ACCEPTED
            || slot.generation != accepted.generation
            || !Arc::ptr_eq(&accepted.endpoint.view.1, &self.root.view.1)
            || accepted.endpoint.identity != Some((accepted.slot, accepted.generation))
        {
            return Err(crate::EBADF);
        }
        accepted.endpoint.change(|c| {
            c.sockets.get_mut::<tcp::Socket>(c.socket).abort();
            Ok(())
        })?;
        let output = accepted.endpoint.poll(None)?.1;
        unsafe {
            (*Self::control(&self.root)).slots[accepted.slot].state = 0;
        }
        self.children
            .lock()
            .map_err(|_| EIO)?
            .remove(&(accepted.slot, accepted.generation));
        self.root.notify_locked();
        Ok(output)
    }
    fn finish_locked(&self, index: usize, child: &SharedEndpoint) -> Result<(), i32> {
        let slot = unsafe { (*Self::control(&self.root)).slots[index] };
        match child.with_locked(|c| Ok(c.sockets.get::<tcp::Socket>(c.socket).state()))? {
            TcpState::Established | TcpState::CloseWait if slot.state == SYN => {
                // Linux's accepted socket inherits the listener's rule at the
                // transition out of the request/handshake state, not at accept().
                child.write_filter_locked(&*self.root.state_locked()?);
                child.with_locked(|c| {
                    c.sockets.get_mut::<tcp::Socket>(c.socket).set_timeout(None);
                    Ok(())
                })?;
                let control = unsafe { &mut *Self::control(&self.root) };
                control.next_ready = control.next_ready.checked_add(1).ok_or(crate::EOVERFLOW)?;
                control.slots[index].ready_order = control.next_ready;
                control.slots[index].state = READY;
                self.root.notify_locked();
            }
            TcpState::Closed | TcpState::Listen if matches!(slot.state, SYN | READY) => {
                unsafe {
                    (*Self::control(&self.root)).slots[index].state = 0;
                }
                self.children
                    .lock()
                    .map_err(|_| EIO)?
                    .remove(&(index, slot.generation));
                self.root.notify_locked();
            }
            _ => {}
        }
        Ok(())
    }
    pub fn poll(
        &self,
        incoming: Option<Vec<u8>>,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        let _guard = self.root.acquire()?;
        self.poll_locked(incoming)
    }
    pub fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        if !Arc::ptr_eq(&self.root, &waiter.endpoint) {
            return Err(EINVAL);
        }
        let _guard = self.root.acquire()?;
        let _muted = waiter.mute_locked()?;
        self.poll_locked(incoming)
    }
    fn poll_locked(
        &self,
        incoming: Option<Vec<u8>>,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        let mut verdict = None;
        let mut output = Vec::new();
        let mut delay = None;
        let mut processed = None;
        let root_layout = self.root.view.0.Value.cast::<Layout>();
        let ifindex = unsafe { (*root_layout).ifindex };
        if let Some(bytes) = incoming {
            (|| -> Result<(), i32> {
                let packet = match IpPacket::parse(&bytes, ifindex) {
                    Ok(packet) => packet,
                    Err(error) => {
                        verdict = Some(Ingress::Invalid(error));
                        return Ok(());
                    }
                };
                let mut target = None;
                if let Some(packet) = packet.as_ref().filter(|p| p.protocol == 6) {
                    let records = unsafe { (*Self::control(&self.root)).slots };
                    for (index, slot) in records
                        .into_iter()
                        .enumerate()
                        .filter(|(_, s)| s.state != 0)
                    {
                        let child = self.child_locked(index, slot.generation)?;
                        if child.with_locked(|c| {
                            let s = c.sockets.get::<tcp::Socket>(c.socket);
                            Ok((s.local_endpoint(), s.remote_endpoint()))
                        })? == (Some(packet.destination), Some(packet.source))
                        {
                            if slot.state == SYN
                                && slot.parent_revision != unsafe { (*root_layout).revision }
                            {
                                child.write_filter_locked(&*self.root.state_locked()?);
                                unsafe {
                                    (*Self::control(&self.root)).slots[index].parent_revision =
                                        (*root_layout).revision;
                                }
                            }
                            target = Some((index, child));
                            break;
                        }
                    }
                    if target.is_none() {
                        let listener = self.root.with_locked(|c| {
                            Ok(c.sockets.get::<tcp::Socket>(c.socket).listen_endpoint())
                        })?;
                        let syn = packet.bytes[packet.start + 13] & 0x17 == 0x02;
                        if syn
                            && unsafe { (*Self::control(&self.root)).active != 0 }
                            && listener.port == packet.destination.port
                            && listener.addr.is_none_or(|a| a == packet.destination.addr)
                        {
                            let state = self.root.state_locked()?;
                            let Some(retain) = state.run(packet, packet.cap) else {
                                verdict = Some(Ingress::Filtered);
                                return Ok(());
                            };
                            match self.allocate_locked(packet.destination, &state, unsafe {
                                (*root_layout).revision
                            }) {
                                Ok((index, child)) => {
                                    let input = if retain < packet.bytes.len() - packet.start {
                                        packet.retained(retain)
                                    } else {
                                        bytes.clone()
                                    };
                                    let (v, frames, next) =
                                        child.poll_locked(Some(input), 0, true)?;
                                    verdict = v;
                                    output.extend(frames);
                                    delay = next;
                                    processed = Some(index);
                                    self.finish_locked(index, &child)?;
                                }
                                Err(EAGAIN) => verdict = Some(Ingress::Backpressure),
                                Err(error) => return Err(error),
                            }
                        }
                    }
                }
                if verdict.is_none() {
                    if let Some((index, child)) = target {
                        let (v, frames, next) = child.poll_locked(Some(bytes), 0, false)?;
                        verdict = v;
                        output.extend(frames);
                        delay = next;
                        processed = Some(index);
                        self.finish_locked(index, &child)?;
                    } else {
                        let (v, frames, next) = self.root.poll_locked(Some(bytes), 0, false)?;
                        verdict = v;
                        output.extend(frames);
                        delay = next;
                    }
                }
                Ok(())
            })()?;
        }
        let records = unsafe { (*Self::control(&self.root)).slots };
        for (index, slot) in records
            .into_iter()
            .enumerate()
            .filter(|(i, s)| s.state != 0 && Some(*i) != processed)
        {
            let child = self.child_locked(index, slot.generation)?;
            let (_, frames, next) = child.poll_locked(None, 0, false)?;
            output.extend(frames);
            delay = match (delay, next) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            self.finish_locked(index, &child)?;
        }
        Ok((verdict, output, delay))
    }
}

#[cfg(test)]
mod tests;
