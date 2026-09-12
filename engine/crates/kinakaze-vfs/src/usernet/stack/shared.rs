//! One protocol endpoint in memory shared by independently loaded processes.
//!
//! smoltcp's buffers and SocketSet use borrowed storage in this mapping. No
//! process heap pointer, native handle, callback or waker is stored in it.
//! Every process maps at the recorded address and checks the exact module image
//! hash before interpreting Rust layout. An occupied address is an error; no
//! foreign mapping is moved or overwritten. Native objects stay process-local.
use super::{Ingress, IpCidr, IpEndpoint, TcpState, device::PacketDevice, packet::IpPacket};
use crate::socket::filter::{State, classic::Program};
use crate::{EAGAIN, EINVAL, EIO, ENOENT, EOPNOTSUPP, EPERM};
use sha2::{Digest, Sha256};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::HardwareAddress;
use std::collections::HashMap;
use std::mem::{MaybeUninit, size_of};
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Threading::*;
mod notification;
pub use notification::Notification;
mod allocation;
mod listener;
pub use listener::{Accepted, Listener};

const MAGIC: u64 = u64::from_le_bytes(*b"CYPKT007");
const BUFFER: usize = 65536;
const FILTER_SIZE: usize = 8 + 4096 * 8;
const SLOT_STRIDE: usize = size_of::<Layout>().next_multiple_of(65536);
const ARENA_SIZE: usize = SLOT_STRIDE * (listener::CAPACITY + 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Tcp,
    Udp,
}

struct Core {
    interface: Interface,
    sockets: SocketSet<'static>,
    socket: SocketHandle,
    kind: Kind,
    udp_peer: Option<IpEndpoint>,
}

#[repr(C)]
struct Layout {
    magic: u64,
    arena_id: u64,
    base: usize,
    size: usize,
    image: [u8; 32],
    poison: u64,
    ifindex: u32,
    filter_len: u32,
    revision: u64,
    filter: [u8; FILTER_SIZE],
    description: [u8; 120],
    core: MaybeUninit<Core>,
    storage: MaybeUninit<[SocketStorage<'static>; 1]>,
    receive: [u8; BUFFER],
    send: [u8; BUFFER],
    receive_meta: [udp::PacketMetadata; 64],
    send_meta: [udp::PacketMetadata; 64],
    notifications: notification::Directory,
    listener: listener::Control,
}

struct Handle(HANDLE);
impl Handle {
    fn new(raw: HANDLE) -> Result<Self, i32> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(crate::errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct View(MEMORY_MAPPED_VIEW_ADDRESS, (u64, u64));
// Ownership only; access to protocol contents is serialized by the arena mutex.
unsafe impl Send for View {}
unsafe impl Sync for View {}
impl Drop for View {
    fn drop(&mut self) {
        // Weak::upgrade can fail before this destructor finishes unmapping.
        // Keep that retiring entry visible until the address really is free;
        // another opener waits for this publication rather than racing MapView.
        let (directory, changed) = views();
        let mut directory = directory.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            UnmapViewOfFile(self.0);
        }
        if directory
            .get(&self.1)
            .is_some_and(|view| std::ptr::eq(view.as_ptr(), self))
        {
            directory.remove(&self.1);
            changed.notify_all();
        }
    }
}
// A connection can outlive the listener while retaining the same mapped arena.
// The first field addresses its slot; the Arc owns the original complete view.
struct ViewRef(MEMORY_MAPPED_VIEW_ADDRESS, Arc<View>);

/// This object pins the section, but is not itself stored in shared memory.
pub struct SharedEndpoint {
    id: u64,
    _lease: Option<Arc<crate::fs::object::Object>>,
    view: ViewRef,
    _section: Arc<Handle>,
    mutex: Arc<Handle>,
    cache: Mutex<Option<(u64, Arc<State>)>>,
    name: String,
    notifications: Mutex<notification::Local>,
    root_notifications: Mutex<notification::Local>,
    identity: Option<(usize, u64)>,
    _allocation: Option<Arc<crate::mount::shared::Store>>,
}
// Mapped references are only formed while holding the named mutex. The local
// program cache has its own mutex and contains no references into the mapping.
unsafe impl Send for SharedEndpoint {}
unsafe impl Sync for SharedEndpoint {}

struct Guard<'a>(&'a SharedEndpoint);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // A Rust unwind releases the native mutex normally, so Windows
            // cannot report abandonment. Preserve the same corruption barrier.
            unsafe {
                ptr::addr_of_mut!((*self.0.view.1.0.Value.cast::<Layout>()).poison).write(1);
            }
        }
        unsafe {
            ReleaseMutex(self.0.mutex.0);
        }
    }
}

fn image_identity() -> Result<[u8; 32], i32> {
    static IMAGE: OnceLock<Result<[u8; 32], i32>> = OnceLock::new();
    *IMAGE.get_or_init(|| {
        // Get the image containing this implementation, not an unrelated
        // executable which happens to load it (normally this is our libc DLL).
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetModuleHandleExW(
                flags: u32,
                address: *const u16,
                module: *mut *mut core::ffi::c_void,
            ) -> i32;
            fn GetModuleFileNameW(
                module: *mut core::ffi::c_void,
                name: *mut u16,
                length: u32,
            ) -> u32;
        }
        let mut module = ptr::null_mut();
        let mut name = vec![0u16; 32768];
        if unsafe { GetModuleHandleExW(6, image_identity as *const () as _, &mut module) } == 0 {
            return Err(EIO);
        }
        let length =
            unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        if length == 0 || length >= name.len() {
            return Err(EIO);
        }
        use std::os::windows::ffi::OsStringExt;
        let path = std::ffi::OsString::from_wide(&name[..length]);
        let bytes = std::fs::read(path).map_err(|_| EIO)?;
        Ok(Sha256::digest(bytes).into())
    })
}

fn cache() -> &'static Mutex<HashMap<(u64, u64), Weak<SharedEndpoint>>> {
    static CACHE: OnceLock<Mutex<HashMap<(u64, u64), Weak<SharedEndpoint>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn views() -> &'static (Mutex<HashMap<(u64, u64), Weak<View>>>, Condvar) {
    static VIEWS: OnceLock<(Mutex<HashMap<(u64, u64), Weak<View>>>, Condvar)> = OnceLock::new();
    VIEWS.get_or_init(Default::default)
}
fn existing_view(domain: u64, id: u64) -> Result<Option<Arc<View>>, i32> {
    let (directory, changed) = views();
    let mut directory = directory.lock().map_err(|_| EIO)?;
    while let Some(view) = directory.get(&(domain, id)) {
        if let Some(view) = view.upgrade() {
            return Ok(Some(view));
        }
        directory = changed.wait(directory).map_err(|_| EIO)?;
    }
    Ok(None)
}

impl SharedEndpoint {
    pub fn create(
        domain: u64,
        id: u64,
        addresses: &[IpCidr],
        ifindex: u32,
        kind: Kind,
    ) -> Result<Arc<Self>, i32> {
        if addresses.is_empty() || addresses.len() > 2 || ifindex == 0 || id == 0 {
            return Err(EINVAL);
        }
        Self::open_inner(domain, id, Some((addresses, ifindex, kind)), None)
    }
    pub fn open(domain: u64, id: u64) -> Result<Arc<Self>, i32> {
        Self::open_inner(domain, id, None, None)
    }
    fn open_inner(
        domain: u64,
        id: u64,
        create: Option<(&[IpCidr], u32, Kind)>,
        address: Option<usize>,
    ) -> Result<Arc<Self>, i32> {
        let mut cache = cache().lock().map_err(|_| EIO)?;
        if let Some(endpoint) = cache.get(&(domain, id)).and_then(Weak::upgrade) {
            return Ok(endpoint);
        }
        cache.retain(|_, entry| entry.strong_count() != 0);
        let image = image_identity()?;
        let managed = address.or(if create.is_none() {
            allocation::address(id)?
        } else {
            None
        });
        let offset = managed.map_or(0, allocation::offset) as u64;
        let name = format!("Local\\Kinakaze.Packet.{domain:016x}.{id:016x}");
        let section_name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mutex_name: Vec<u16> = if managed.is_some() {
            format!("Local\\Kinakaze.Packet.{domain:016x}.Storage.{offset:016x}.Lock")
        } else {
            format!("{name}.Lock")
        }
        .encode_utf16()
        .chain([0])
        .collect();
        let mutex = Handle::new(unsafe { CreateMutexW(ptr::null(), 0, mutex_name.as_ptr()) })?;
        let result = unsafe { WaitForSingleObject(mutex.0, INFINITE) };
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            return Err(EIO);
        }
        // A small RAII guard covers setup failures before an endpoint exists.
        struct SetupGuard(HANDLE);
        impl Drop for SetupGuard {
            fn drop(&mut self) {
                unsafe {
                    ReleaseMutex(self.0);
                }
            }
        }
        let _guard = SetupGuard(mutex.0);
        let section = if managed.is_some() {
            Handle::new(allocation::section(domain, create.is_some())?.into_raw())?
        } else {
            Handle::new(unsafe {
                if create.is_some() {
                    CreateFileMappingW(
                        INVALID_HANDLE_VALUE,
                        ptr::null(),
                        PAGE_READWRITE | SEC_RESERVE,
                        0,
                        ARENA_SIZE as u32,
                        section_name.as_ptr(),
                    )
                } else {
                    OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, section_name.as_ptr())
                }
            })?
        };
        let existing = existing_view(domain, id)?;
        let mut view = existing.unwrap_or_else(|| {
            Arc::new(View(
                unsafe {
                    MapViewOfFileEx(
                        section.0,
                        FILE_MAP_ALL_ACCESS,
                        (offset >> 32) as u32,
                        offset as u32,
                        ARENA_SIZE,
                        managed.unwrap_or(0) as _,
                    )
                },
                (domain, id),
            ))
        });
        if view.0.Value.is_null() {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        if unsafe {
            VirtualAlloc(
                view.0.Value,
                size_of::<Layout>(),
                MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .is_null()
        {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        let header = view.0.Value.cast::<Layout>();
        // Only integer header fields are inspected before layout compatibility.
        let magic = unsafe { ptr::addr_of!((*header).magic).read() };
        if magic == 0
            || (managed.is_some() && create.is_some() && unsafe { (*header).arena_id } == 0)
        {
            let (addresses, ifindex, kind) = create.ok_or(EIO)?;
            unsafe {
                Self::initialize(header, addresses, ifindex, kind, image, id)?;
            }
        } else {
            if magic != MAGIC
                || unsafe { ptr::addr_of!((*header).size).read() } != ARENA_SIZE
                || unsafe { ptr::addr_of!((*header).image).read() } != image
            {
                return Err(EIO);
            }
            if unsafe { (*header).arena_id } != id {
                return Err(ENOENT);
            }
            let base = unsafe { ptr::addr_of!((*header).base).read() };
            if base != view.0.Value as usize {
                drop(view);
                view = Arc::new(View(
                    unsafe {
                        MapViewOfFileEx(
                            section.0,
                            FILE_MAP_ALL_ACCESS,
                            (offset >> 32) as u32,
                            offset as u32,
                            ARENA_SIZE,
                            base as _,
                        )
                    },
                    (domain, id),
                ));
                if view.0.Value as usize != base {
                    return Err(crate::errno_from_win32(unsafe { GetLastError() }));
                }
            }
            if result == WAIT_ABANDONED {
                unsafe {
                    ptr::addr_of_mut!((*view.0.Value.cast::<Layout>()).poison).write(1);
                }
            }
        }
        {
            let mut views = views().0.lock().map_err(|_| EIO)?;
            views.insert((domain, id), Arc::downgrade(&view));
        }
        let endpoint = Arc::new(Self {
            id,
            _lease: if managed.is_some() {
                let lease: Vec<u16> = format!("{name}.Views").encode_utf16().chain([0]).collect();
                Some(Arc::new(crate::fs::object::Object::owned(unsafe {
                    CreateEventW(ptr::null(), 1, 0, lease.as_ptr())
                })?))
            } else {
                None
            },
            _allocation: allocation::pin(view.0.Value as usize)?,
            view: ViewRef(
                MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: view.0.Value,
                },
                view,
            ),
            _section: Arc::new(section),
            mutex: Arc::new(mutex),
            cache: Mutex::new(None),
            name,
            notifications: Mutex::new(Default::default()),
            root_notifications: Mutex::new(Default::default()),
            identity: None,
        });
        cache.insert((domain, id), Arc::downgrade(&endpoint));
        Ok(endpoint)
    }

    /// The selected smoltcp features keep Interface inline, and buffers are
    /// explicitly borrowed. Enabling allocating protocol features requires a
    /// new shared representation; do not add them without revisiting this code.
    unsafe fn initialize(
        layout: *mut Layout,
        addresses: &[IpCidr],
        ifindex: u32,
        kind: Kind,
        image: [u8; 32],
        id: u64,
    ) -> Result<(), i32> {
        let mut device = PacketDevice::default();
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = (layout as usize as u64) ^ now_ms() as u64;
        let mut interface = Interface::new(config, &mut device, Instant::from_millis(now_ms()));
        interface.update_ip_addrs(|ips| {
            for addr in addresses {
                ips.push(*addr).unwrap();
            }
        });
        unsafe {
            ptr::addr_of_mut!((*layout).storage).write(MaybeUninit::new([SocketStorage::EMPTY]));
            ptr::addr_of_mut!((*layout).receive_meta).write([udp::PacketMetadata::EMPTY; 64]);
            ptr::addr_of_mut!((*layout).send_meta).write([udp::PacketMetadata::EMPTY; 64]);
            let storage =
                &mut *ptr::addr_of_mut!((*layout).storage).cast::<[SocketStorage<'static>; 1]>();
            let receive = &mut *ptr::addr_of_mut!((*layout).receive);
            let send = &mut *ptr::addr_of_mut!((*layout).send);
            let receive_meta = &mut *ptr::addr_of_mut!((*layout).receive_meta);
            let send_meta = &mut *ptr::addr_of_mut!((*layout).send_meta);
            let mut sockets = SocketSet::new(&mut storage[..]);
            let socket = match kind {
                Kind::Tcp => sockets.add(tcp::Socket::new(
                    tcp::SocketBuffer::new(&mut receive[..]),
                    tcp::SocketBuffer::new(&mut send[..]),
                )),
                Kind::Udp => sockets.add(udp::Socket::new(
                    udp::PacketBuffer::new(&mut receive_meta[..], &mut receive[..]),
                    udp::PacketBuffer::new(&mut send_meta[..], &mut send[..]),
                )),
            };
            ptr::addr_of_mut!((*layout).core).write(MaybeUninit::new(Core {
                interface,
                sockets,
                socket,
                kind,
                udp_peer: None,
            }));
            ptr::addr_of_mut!((*layout).size).write(ARENA_SIZE);
            ptr::addr_of_mut!((*layout).arena_id).write(id);
            ptr::addr_of_mut!((*layout).base).write(layout as usize);
            ptr::addr_of_mut!((*layout).image).write(image);
            ptr::addr_of_mut!((*layout).ifindex).write(ifindex);
            ptr::addr_of_mut!((*layout).filter_len).write(0);
            ptr::addr_of_mut!((*layout).description).write([0; 120]);
            ptr::addr_of_mut!((*layout).revision).write(0);
            ptr::addr_of_mut!((*layout).poison).write(0);
            ptr::addr_of_mut!((*layout).notifications).write(std::mem::zeroed());
            ptr::addr_of_mut!((*layout).listener).write(std::mem::zeroed());
            // Published last under the named mutex. No native handle is placed in the image.
            ptr::addr_of_mut!((*layout).magic).write(MAGIC);
        }
        Ok(())
    }

    fn acquire(&self) -> Result<Guard<'_>, i32> {
        let status = unsafe { WaitForSingleObject(self.mutex.0, INFINITE) };
        if status != WAIT_OBJECT_0 && status != WAIT_ABANDONED {
            return Err(EIO);
        }
        let guard = Guard(self);
        let layout = self.view.1.0.Value.cast::<Layout>();
        if unsafe { (*layout).arena_id } != self.id {
            return Err(crate::EBADF);
        }
        if status == WAIT_ABANDONED {
            unsafe {
                ptr::addr_of_mut!((*layout).poison).write(1);
            }
        }
        if unsafe { ptr::addr_of!((*layout).poison).read() } != 0 {
            return Err(EIO);
        }
        if !self.identity_valid_locked() {
            return Err(crate::EBADF);
        }
        Ok(guard)
    }
    fn identity_valid_locked(&self) -> bool {
        self.identity.is_none_or(|(index, generation)| {
            let control =
                unsafe { &*ptr::addr_of!((*self.view.1.0.Value.cast::<Layout>()).listener) };
            control
                .slots
                .get(index)
                .is_some_and(|slot| slot.state != 0 && slot.generation == generation)
        })
    }
    fn with<T>(&self, action: impl FnOnce(&mut Core) -> Result<T, i32>) -> Result<T, i32> {
        let _guard = self.acquire()?;
        self.with_locked(action)
    }
    // The caller already holds this arena's named mutex. Listener dispatch
    // processes several slots without a native lock/unlock pair per slot.
    fn with_locked<T>(&self, action: impl FnOnce(&mut Core) -> Result<T, i32>) -> Result<T, i32> {
        // Do not form an &mut Layout: its buffer fields are already borrowed
        // by the socket. Borrow only the disjoint Core field under the mutex.
        let core = unsafe {
            &mut *ptr::addr_of_mut!((*self.view.0.Value.cast::<Layout>()).core).cast::<Core>()
        };
        action(core)
    }
    fn change<T>(&self, action: impl FnOnce(&mut Core) -> Result<T, i32>) -> Result<T, i32> {
        let _guard = self.acquire()?;
        let core = unsafe {
            &mut *ptr::addr_of_mut!((*self.view.0.Value.cast::<Layout>()).core).cast::<Core>()
        };
        let result = action(core);
        if result.is_ok() {
            self.notify_locked();
        }
        result
    }

    /// A separate reference for a fork/exec handoff or an in-flight rights
    /// message. It pins only this section and does not copy its contents.
    pub(crate) fn pin(&self) -> Result<crate::fs::object::Object, i32> {
        crate::fs::object::Object::duplicate(self._section.0)
    }
    /// Escrow processes only retain the section. They must not map or interpret
    /// protocol structs compiled into a different module image.
    pub(crate) fn pin_reference(domain: u64, id: u64) -> Result<crate::fs::object::Object, i32> {
        if allocation::address(id)?.is_some() {
            return allocation::section(domain, false);
        }
        let name: Vec<u16> = format!("Local\\Kinakaze.Packet.{domain:016x}.{id:016x}")
            .encode_utf16()
            .chain([0])
            .collect();
        crate::fs::object::Object::owned(unsafe {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr())
        })
    }
    pub(crate) fn save_description(&self, bytes: &[u8]) -> Result<(), i32> {
        let bytes = bytes.get(..120).ok_or(EINVAL)?;
        let _guard = self.acquire()?;
        let target = unsafe { &mut (*self.view.0.Value.cast::<Layout>()).description };
        if target != bytes {
            target.copy_from_slice(bytes);
            self.notify_locked();
        }
        Ok(())
    }
    fn state_locked(&self) -> Result<Arc<State>, i32> {
        let layout = self.view.0.Value.cast::<Layout>();
        let revision = unsafe { ptr::addr_of!((*layout).revision).read() };
        let mut cache = self.cache.lock().map_err(|_| EIO)?;
        if let Some((rev, state)) = &*cache {
            if *rev == revision {
                return Ok(state.clone());
            }
        }
        let length = unsafe { ptr::addr_of!((*layout).filter_len).read() } as usize;
        if length > FILTER_SIZE {
            return Err(EIO);
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(ptr::addr_of!((*layout).filter).cast::<u8>(), length)
        };
        let state = Arc::new(State::decode(bytes)?);
        *cache = Some((revision, state.clone()));
        Ok(state)
    }
    pub fn attach_filter(&self, code: &[u8]) -> Result<(), i32> {
        {
            let _guard = self.acquire()?;
            if self.state_locked()?.locked {
                return Err(EPERM);
            }
        }
        let program = Program::decode(code)?;
        self.attach_verified_filter(program)
    }
    pub(crate) fn attach_verified_filter(&self, program: Program) -> Result<(), i32> {
        self.change_filter(|state| {
            if state.locked {
                return Err(EPERM);
            }
            state.program = Some(program);
            Ok(())
        })
    }
    pub fn detach_filter(&self) -> Result<(), i32> {
        self.change_filter(|state| {
            if state.locked {
                return Err(EPERM);
            }
            state.program.take().ok_or(ENOENT)?;
            Ok(())
        })
    }
    pub fn lock_filter(&self, locked: bool) -> Result<(), i32> {
        self.change_filter(|state| {
            if state.locked && !locked {
                return Err(EPERM);
            }
            state.locked = locked;
            Ok(())
        })
    }
    pub fn filter_program(&self) -> Result<Vec<u8>, i32> {
        let _guard = self.acquire()?;
        Ok(self
            .state_locked()?
            .program
            .as_ref()
            .map_or_else(Vec::new, Program::encode))
    }
    pub(crate) fn filter_state(&self) -> Result<Arc<State>, i32> {
        let _guard = self.acquire()?;
        self.state_locked()
    }
    pub(crate) fn set_addresses(&self, addresses: &[IpCidr]) -> Result<(), i32> {
        if addresses.is_empty() || addresses.len() > 2 {
            return Err(EINVAL);
        }
        self.change(|core| {
            let active = match core.kind {
                Kind::Tcp => {
                    core.sockets.get::<tcp::Socket>(core.socket).state() != TcpState::Closed
                }
                Kind::Udp => core.sockets.get::<udp::Socket>(core.socket).is_open(),
            };
            if active {
                return Err(EINVAL);
            }
            core.interface.update_ip_addrs(|current| {
                current.clear();
                for address in addresses {
                    current.push(*address).unwrap();
                }
            });
            core.interface.routes_mut().update(|routes| routes.clear());
            for address in addresses {
                match address.address() {
                    super::IpAddress::Ipv4(ip) => {
                        core.interface
                            .routes_mut()
                            .add_default_ipv4_route(ip)
                            .map_err(|_| EINVAL)?;
                    }
                    super::IpAddress::Ipv6(ip) => {
                        core.interface
                            .routes_mut()
                            .add_default_ipv6_route(ip)
                            .map_err(|_| EINVAL)?;
                    }
                }
            }
            // The namespace router checks ownership of the actual destination.
            core.interface.set_any_ip(true);
            Ok(())
        })
    }
    fn change_filter(&self, action: impl FnOnce(&mut State) -> Result<(), i32>) -> Result<(), i32> {
        let _guard = self.acquire()?;
        let mut state = (*self.state_locked()?).clone();
        action(&mut state)?;
        self.write_filter_locked(&state);
        self.notify_locked();
        Ok(())
    }
    fn write_filter_locked(&self, state: &State) {
        let bytes = state.encode();
        let layout = self.view.0.Value.cast::<Layout>();
        unsafe {
            ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                ptr::addr_of_mut!((*layout).filter).cast::<u8>(),
                bytes.len(),
            );
            ptr::addr_of_mut!((*layout).filter_len).write(bytes.len() as u32);
            let revision = ptr::addr_of!((*layout).revision).read();
            ptr::addr_of_mut!((*layout).revision).write(revision.wrapping_add(1));
        }
    }
    pub fn listen(&self, local: IpEndpoint) -> Result<(), i32> {
        self.change(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            c.sockets
                .get_mut::<tcp::Socket>(c.socket)
                .listen(smoltcp::wire::IpListenEndpoint {
                    addr: (!local.addr.is_unspecified()).then_some(local.addr),
                    port: local.port,
                })
                .map_err(|_| EINVAL)
        })
    }
    pub fn connect(&self, local: IpEndpoint, peer: IpEndpoint) -> Result<(), i32> {
        self.change(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            c.sockets
                .get_mut::<tcp::Socket>(c.socket)
                .connect(c.interface.context(), peer, local)
                .map_err(|_| EINVAL)
        })
    }
    pub fn bind_udp(&self, local: IpEndpoint) -> Result<(), i32> {
        self.change(|c| {
            if c.kind != Kind::Udp {
                return Err(EOPNOTSUPP);
            }
            c.sockets
                .get_mut::<udp::Socket>(c.socket)
                .bind(smoltcp::wire::IpListenEndpoint {
                    addr: (!local.addr.is_unspecified()).then_some(local.addr),
                    port: local.port,
                })
                .map_err(|_| EINVAL)
        })
    }
    pub(crate) fn connect_udp(&self, peer: Option<IpEndpoint>) -> Result<(), i32> {
        self.change(|core| {
            if core.kind != Kind::Udp {
                return Err(EOPNOTSUPP);
            }
            core.udp_peer = peer;
            Ok(())
        })
    }
    pub fn tcp_state(&self) -> Result<TcpState, i32> {
        self.with(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            Ok(c.sockets.get::<tcp::Socket>(c.socket).state())
        })
    }
    pub fn close_tcp(&self) -> Result<(), i32> {
        self.change(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            c.sockets.get_mut::<tcp::Socket>(c.socket).close();
            Ok(())
        })
    }
    pub(crate) fn set_nagle(&self, enabled: bool) -> Result<(), i32> {
        self.change(|core| {
            if core.kind != Kind::Tcp {
                return Err(crate::ENOPROTOOPT);
            }
            core.sockets
                .get_mut::<tcp::Socket>(core.socket)
                .set_nagle_enabled(enabled);
            Ok(())
        })
    }
    pub(crate) fn nagle(&self) -> Result<bool, i32> {
        self.with(|core| {
            if core.kind != Kind::Tcp {
                return Err(crate::ENOPROTOOPT);
            }
            Ok(core.sockets.get::<tcp::Socket>(core.socket).nagle_enabled())
        })
    }
    pub fn tcp_endpoints(&self) -> Result<(Option<IpEndpoint>, Option<IpEndpoint>), i32> {
        self.with(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            let socket = c.sockets.get::<tcp::Socket>(c.socket);
            Ok((socket.local_endpoint(), socket.remote_endpoint()))
        })
    }
    pub fn send_tcp(&self, data: &[u8]) -> Result<usize, i32> {
        self.change(|c| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            c.sockets
                .get_mut::<tcp::Socket>(c.socket)
                .send_slice(data)
                .map_err(|_| crate::ENOTCONN)
                .and_then(|n| {
                    if n == 0 && !data.is_empty() {
                        Err(EAGAIN)
                    } else {
                        Ok(n)
                    }
                })
        })
    }
    pub fn recv_tcp(&self, data: &mut [u8]) -> Result<usize, i32> {
        self.recv_tcp_flags(data, false)
    }
    pub fn recv_tcp_flags(&self, data: &mut [u8], peek: bool) -> Result<usize, i32> {
        let action = |c: &mut Core| {
            if c.kind != Kind::Tcp {
                return Err(EOPNOTSUPP);
            }
            let s = c.sockets.get_mut::<tcp::Socket>(c.socket);
            if data.is_empty() {
                return Ok(0);
            }
            if !s.can_recv() {
                return if s.may_recv() { Err(EAGAIN) } else { Ok(0) };
            }
            if peek {
                s.peek_slice(data)
            } else {
                s.recv_slice(data)
            }
            .map_err(|_| EIO)
        };
        if peek {
            self.with(action)
        } else {
            self.change(action)
        }
    }
    pub fn send_udp(&self, data: &[u8], peer: IpEndpoint) -> Result<(), i32> {
        self.send_udp_from(data, peer, None)
    }
    pub(crate) fn send_udp_from(
        &self,
        data: &[u8],
        peer: IpEndpoint,
        local_address: Option<super::IpAddress>,
    ) -> Result<(), i32> {
        self.change(|c| {
            if c.kind != Kind::Udp {
                return Err(EOPNOTSUPP);
            }
            let limit = if matches!(peer.addr, super::IpAddress::Ipv4(_)) {
                1472
            } else {
                1452
            };
            if data.len() > limit {
                return Err(crate::EMSGSIZE);
            }
            c.sockets
                .get_mut::<udp::Socket>(c.socket)
                .send_slice(
                    data,
                    udp::UdpMetadata {
                        endpoint: peer,
                        local_address,
                        meta: Default::default(),
                    },
                )
                .map_err(|_| EAGAIN)
        })
    }
    pub fn recv_udp(&self, data: &mut [u8], peek: bool) -> Result<(usize, IpEndpoint, usize), i32> {
        let action = |c: &mut Core| {
            if c.kind != Kind::Udp {
                return Err(EOPNOTSUPP);
            }
            let s = c.sockets.get_mut::<udp::Socket>(c.socket);
            let (bytes, meta) = if peek {
                let (b, m) = s.peek().map_err(|_| EAGAIN)?;
                (b, *m)
            } else {
                s.recv().map_err(|_| EAGAIN)?
            };
            let count = bytes.len().min(data.len());
            data[..count].copy_from_slice(&bytes[..count]);
            Ok((count, meta.endpoint, bytes.len()))
        };
        if peek {
            self.with(action)
        } else {
            self.change(action)
        }
    }
    pub fn readiness(&self) -> Result<(bool, bool, bool), i32> {
        self.with(|c| {
            Ok(match c.kind {
                Kind::Tcp => {
                    let s = c.sockets.get::<tcp::Socket>(c.socket);
                    (
                        s.can_recv()
                            || matches!(
                                s.state(),
                                TcpState::CloseWait | TcpState::Closed | TcpState::TimeWait
                            ),
                        s.can_send(),
                        !s.may_recv()
                            && !matches!(
                                s.state(),
                                TcpState::Listen | TcpState::SynSent | TcpState::SynReceived
                            ),
                    )
                }
                Kind::Udp => {
                    let s = c.sockets.get::<udp::Socket>(c.socket);
                    (s.can_recv(), s.can_send(), false)
                }
            })
        })
    }
    pub(crate) fn queued_bytes(&self) -> Result<usize, i32> {
        self.with(|core| {
            Ok(match core.kind {
                Kind::Tcp => core.sockets.get::<tcp::Socket>(core.socket).recv_queue(),
                Kind::Udp => core
                    .sockets
                    .get_mut::<udp::Socket>(core.socket)
                    .peek()
                    .map_or(0, |(bytes, _)| bytes.len()),
            })
        })
    }
    pub fn poll(
        &self,
        incoming: Option<Vec<u8>>,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        self.poll_inner(incoming, 0, false)
    }
    pub(super) fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        if !ptr::eq(self, Arc::as_ptr(&waiter.endpoint)) {
            return Err(EINVAL);
        }
        self.poll_inner(incoming, waiter.id, false)
    }
    fn poll_inner(
        &self,
        incoming: Option<Vec<u8>>,
        current_waiter: u64,
        prefiltered: bool,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        let _guard = self.acquire()?;
        self.poll_locked(incoming, current_waiter, prefiltered)
    }
    fn poll_locked(
        &self,
        incoming: Option<Vec<u8>>,
        current_waiter: u64,
        prefiltered: bool,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        let layout = self.view.0.Value.cast::<Layout>();
        let ifindex = unsafe { ptr::addr_of!((*layout).ifindex).read() };
        let state = self.state_locked()?;
        let core = unsafe { &mut *ptr::addr_of_mut!((*layout).core).cast::<Core>() };
        let mut device = PacketDevice::default();
        let now = Instant::from_millis(now_ms());
        let mut ingress = None;
        if let Some(bytes) = incoming {
            match IpPacket::parse(&bytes, ifindex) {
                Err(error) => ingress = Some(Ingress::Invalid(error)),
                Ok(packet) => {
                    let verdict = if prefiltered {
                        None
                    } else {
                        packet
                            .as_ref()
                            .filter(|p| match core.kind {
                                Kind::Tcp if p.protocol == 6 => {
                                    let s = core.sockets.get::<tcp::Socket>(core.socket);
                                    (s.local_endpoint() == Some(p.destination)
                                        && s.remote_endpoint() == Some(p.source))
                                        || (s.state() == TcpState::Listen
                                            && s.listen_endpoint().port == p.destination.port
                                            && s.listen_endpoint()
                                                .addr
                                                .is_none_or(|a| a == p.destination.addr))
                                }
                                Kind::Udp if p.protocol == 17 => {
                                    let s = core.sockets.get::<udp::Socket>(core.socket);
                                    s.endpoint().port == p.destination.port
                                        && s.endpoint().addr.is_none_or(|a| a == p.destination.addr)
                                }
                                _ => false,
                            })
                            .map(|p| state.run(p, p.cap))
                    };
                    if verdict == Some(None)
                        || packet.as_ref().is_some_and(|packet| {
                            core.kind == Kind::Udp
                                && packet.protocol == 17
                                && core.udp_peer.is_some_and(|peer| packet.source != peer)
                        })
                    {
                        ingress = Some(Ingress::Filtered);
                    } else {
                        let input = match (packet, verdict) {
                            (Some(p), Some(Some(len))) if len < p.bytes.len() - p.start => {
                                p.retained(len)
                            }
                            _ => bytes,
                        };
                        device.input = Some(input);
                        core.interface
                            .poll_ingress_single(now, &mut device, &mut core.sockets);
                        ingress = Some(Ingress::Delivered);
                    }
                }
            }
        }
        core.interface
            .poll_egress(now, &mut device, &mut core.sockets);
        let mut output = Vec::new();
        while let Some(frame) = device.take() {
            output.push(frame);
        }
        let delay = core
            .interface
            .poll_delay(now, &core.sockets)
            .map(|d| d.total_millis());
        // Timer-only polls which do no work must stay silent. Otherwise two
        // waiters can wake each other forever while both sockets are idle.
        if ingress == Some(Ingress::Delivered) || !output.is_empty() {
            // The current transport observes these changes in this call. Wake
            // other waiters; waking ourselves would force a redundant AFD pass.
            self.notify_except_locked(current_waiter);
        }
        Ok((ingress, output, delay))
    }
}

pub fn now_ms() -> i64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    }
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 0;
        assert_ne!(unsafe { QueryPerformanceFrequency(&mut value) }, 0);
        assert!(value > 0);
        value
    });
    let mut value = 0;
    assert_ne!(unsafe { QueryPerformanceCounter(&mut value) }, 0);
    ((value as i128 * 1000) / frequency as i128) as i64
}

#[cfg(test)]
mod tests;
