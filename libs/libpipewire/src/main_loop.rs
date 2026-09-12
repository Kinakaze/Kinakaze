//! SPA loop ABI backed by the guest descriptor table. epoll/eventfd/timerfd
//! supply blocking wakeups; callbacks run on the caller's loop thread.
use super::*;
use kinakaze_vfs::{epoll as poll, eventfd, timerfd};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Condvar;
use std::thread::ThreadId;

mod system;
use system::SYSTEM_METHODS;

#[repr(C)]
pub struct PwLoop {
    system: *mut SpaInterface,
    loop_: *mut SpaInterface,
    control: *mut SpaInterface,
    utils: *mut SpaInterface,
    name: *const c_char,
}
#[repr(C)]
pub struct Source {
    loop_: *mut SpaInterface,
    callback: Option<unsafe extern "sysv64" fn(*mut Source)>,
    data: *mut c_void,
    fd: i32,
    mask: u32,
    rmask: u32,
    private: *mut c_void,
}
type Io = unsafe extern "sysv64" fn(*mut c_void, i32, u32);
type Count = unsafe extern "sysv64" fn(*mut c_void, u64);
type Idle = unsafe extern "sysv64" fn(*mut c_void);
type Signal = unsafe extern "sysv64" fn(*mut c_void, i32);
type Invoke = unsafe extern "sysv64" fn(
    *mut SpaInterface,
    bool,
    u32,
    *const c_void,
    usize,
    *mut c_void,
) -> i32;
#[derive(Clone, Copy)]
enum Callback {
    Raw,
    Io(Io),
    Event(Count),
    Timer(Count),
    Idle(Idle),
    Signal(Signal),
}
struct Registration {
    pointer: usize,
    storage: Option<Box<Source>>,
    fd: i32,
    close: bool,
    callback: Callback,
}
unsafe impl Send for Registration {}
impl Drop for Registration {
    fn drop(&mut self) {
        if self.close {
            let _ = kinakaze_vfs::close(self.fd);
        }
    }
}
type Completion = Arc<(Mutex<Option<i32>>, Condvar)>;
struct Work {
    callback: Invoke,
    sequence: u32,
    bytes: Vec<u8>,
    user: usize,
    completion: Option<Completion>,
}
#[derive(Default)]
struct State {
    sources: BTreeMap<u64, Registration>,
    ids: HashMap<usize, u64>,
    next: u64,
    work: VecDeque<Work>,
    bytes: usize,
    entered: Option<ThreadId>,
    depth: usize,
}
#[repr(C)]
pub(super) struct HostLoop {
    public: PwLoop,
    audio: *mut pw_thread_loop,
    interfaces: [SpaInterface; 4],
    poll: i32,
    wake: i32,
    state: Mutex<State>,
    running: AtomicBool,
    quit: AtomicBool,
    pending_core: AtomicBool,
    hooks: std::cell::UnsafeCell<[usize; 2]>,
}
unsafe impl Send for HostLoop {}
unsafe impl Sync for HostLoop {}
fn owners() -> &'static Mutex<HashMap<usize, usize>> {
    static OWNERS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    OWNERS.get_or_init(Default::default)
}
pub(super) fn audio_for(pointer: *mut c_void) -> Option<*mut pw_thread_loop> {
    lock(owners())
        .get(&(pointer as usize))
        .copied()
        .map(|p| p as _)
}
impl HostLoop {
    pub(super) fn create(audio: *mut pw_thread_loop) -> Result<*mut Self, i32> {
        let fd = poll::epoll_create1(0x80000)?;
        let wake = match eventfd::create_eventfd(0, 0x80800) {
            Ok(v) => v,
            Err(e) => {
                let _ = kinakaze_vfs::close(fd);
                return Err(e);
            }
        };
        if let Err(e) = poll::epoll_ctl(fd, 1, wake, Some(poll::EpollEvent { events: 1, data: 0 }))
        {
            let _ = kinakaze_vfs::close(wake);
            let _ = kinakaze_vfs::close(fd);
            return Err(e);
        }
        let null = ptr::null_mut();
        let mut object = Box::new(Self {
            public: PwLoop {
                system: null,
                loop_: null,
                control: null,
                utils: null,
                name: c"kinakaze-loop".as_ptr(),
            },
            audio,
            interfaces: [make_interface(ptr::null(), 0, ptr::null(), ptr::null_mut()); 4],
            poll: fd,
            wake,
            state: Mutex::new(State::default()),
            running: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            pending_core: AtomicBool::new(false),
            hooks: std::cell::UnsafeCell::new([0; 2]),
        });
        let address = (&raw mut *object).cast();
        object.interfaces = [
            make_interface(
                c"Spa:Pointer:Interface:System".as_ptr(),
                0,
                (&raw const SYSTEM_METHODS).cast(),
                address,
            ),
            make_interface(
                c"Spa:Pointer:Interface:Loop".as_ptr(),
                0,
                (&raw const LOOP_METHODS).cast(),
                address,
            ),
            make_interface(
                c"Spa:Pointer:Interface:LoopControl".as_ptr(),
                1,
                (&raw const CONTROL_METHODS).cast(),
                address,
            ),
            make_interface(
                c"Spa:Pointer:Interface:LoopUtils".as_ptr(),
                0,
                (&raw const UTILS_METHODS).cast(),
                address,
            ),
        ];
        object.public.system = &raw mut object.interfaces[0];
        object.public.loop_ = &raw mut object.interfaces[1];
        object.public.control = &raw mut object.interfaces[2];
        object.public.utils = &raw mut object.interfaces[3];
        let head = object.hooks.get().cast::<usize>();
        unsafe {
            head.write(head as usize);
            head.add(1).write(head as usize);
        }
        let raw = Box::into_raw(object);
        lock(owners()).insert(raw as usize, audio as usize);
        Ok(raw)
    }
    pub(super) fn wake(&self) {
        let _ = eventfd::write_eventfd(self.wake, &1u64.to_ne_bytes(), true);
    }
    pub(super) fn schedule_core(&self) {
        self.pending_core.store(true, Ordering::Release);
        self.wake();
    }
    fn complete(work: Work, value: i32) {
        if let Some(wait) = work.completion {
            *lock(&wait.0) = Some(value);
            wait.1.notify_all();
        }
    }
    unsafe fn dispatch_work(&self) -> usize {
        let mut count = 0;
        let queued = lock(&self.state).work.len();
        for _ in 0..queued {
            let work = {
                let mut state = lock(&self.state);
                let Some(work) = state.work.pop_front() else {
                    break;
                };
                state.bytes -= work.bytes.len();
                work
            };
            let result = unsafe {
                (work.callback)(
                    self.public.loop_,
                    true,
                    work.sequence,
                    work.bytes.as_ptr().cast(),
                    work.bytes.len(),
                    work.user as _,
                )
            };
            Self::complete(work, result);
            count += 1;
        }
        if !lock(&self.state).work.is_empty() {
            self.wake();
        }
        count
    }
    unsafe fn hooks(&self, before: bool) {
        let head = self.hooks.get().cast::<usize>();
        let direction = usize::from(before);
        let mut hook = unsafe { *head.add(direction) } as *mut usize;
        while hook != head {
            let next = unsafe { *hook.add(direction) } as *mut usize;
            let functions = unsafe { *hook.add(2) } as *const HookEvents;
            if !functions.is_null() {
                let callback = unsafe {
                    if before {
                        (*functions).before
                    } else {
                        (*functions).after
                    }
                };
                if let Some(call) = callback {
                    unsafe { call(*hook.add(3) as _) };
                }
            }
            hook = next;
        }
    }
}
impl Drop for HostLoop {
    fn drop(&mut self) {
        lock(owners()).remove(&(self as *mut Self as usize));
        let state = self.state.get_mut().unwrap_or_else(|p| p.into_inner());
        for work in state.work.drain(..) {
            Self::complete(work, -125);
        }
        for entry in state.sources.values() {
            if entry.storage.is_none() {
                let source = entry.pointer as *mut Source;
                unsafe {
                    (*source).loop_ = ptr::null_mut();
                    (*source).private = ptr::null_mut();
                }
            }
        }
        state.sources.clear();
        let head = self.hooks.get().cast::<usize>();
        // Hook owners retain their allocation; unlink it before retiring the head.
        unsafe {
            while *head != head as usize {
                let hook = *head as *mut usize;
                let next = *hook as *mut usize;
                *head = next as usize;
                *next.add(1) = head as usize;
                *hook = hook as usize;
                *hook.add(1) = hook as usize;
            }
        }
        let _ = kinakaze_vfs::close(self.wake);
        let _ = kinakaze_vfs::close(self.poll);
    }
}
unsafe fn object<'a>(data: *mut c_void) -> &'a HostLoop {
    unsafe { &*data.cast::<HostLoop>() }
}

#[repr(C)]
struct LoopMethods {
    version: u32,
    add: unsafe extern "sysv64" fn(*mut c_void, *mut Source) -> i32,
    update: unsafe extern "sysv64" fn(*mut c_void, *mut Source) -> i32,
    remove: unsafe extern "sysv64" fn(*mut c_void, *mut Source) -> i32,
    invoke: unsafe extern "sysv64" fn(
        *mut c_void,
        Option<Invoke>,
        u32,
        *const c_void,
        usize,
        bool,
        *mut c_void,
    ) -> i32,
}
static LOOP_METHODS: LoopMethods = LoopMethods {
    version: 0,
    add,
    update,
    remove,
    invoke,
};
unsafe fn register(
    loop_: &HostLoop,
    source: *mut Source,
    storage: Option<Box<Source>>,
    callback: Callback,
    close: bool,
) -> Result<(), i32> {
    if source.is_null() {
        return Err(22);
    }
    let mut state = lock(&loop_.state);
    if state.ids.contains_key(&(source as usize)) {
        return Err(17);
    }
    state.next = state.next.checked_add(1).ok_or(75)?;
    let id = state.next;
    let fd = unsafe { (*source).fd };
    poll::epoll_ctl(
        loop_.poll,
        1,
        fd,
        Some(poll::EpollEvent {
            events: unsafe { (*source).mask },
            data: id,
        }),
    )?;
    unsafe {
        (*source).loop_ = loop_.public.loop_;
        (*source).private = id as usize as _;
    }
    state.ids.insert(source as usize, id);
    state.sources.insert(
        id,
        Registration {
            pointer: source as usize,
            storage,
            fd,
            close,
            callback,
        },
    );
    loop_.wake();
    Ok(())
}
unsafe extern "sysv64" fn add(data: *mut c_void, source: *mut Source) -> i32 {
    unsafe { register(object(data), source, None, Callback::Raw, false) }.map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn update(data: *mut c_void, source: *mut Source) -> i32 {
    let loop_ = unsafe { object(data) };
    let state = lock(&loop_.state);
    let Some(&id) = state.ids.get(&(source as usize)) else {
        return -2;
    };
    let entry = &state.sources[&id];
    if unsafe { (*source).fd } != entry.fd {
        return -22;
    }
    poll::epoll_ctl(
        loop_.poll,
        3,
        entry.fd,
        Some(poll::EpollEvent {
            events: unsafe { (*source).mask },
            data: id,
        }),
    )
    .map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn remove(data: *mut c_void, source: *mut Source) -> i32 {
    let loop_ = unsafe { object(data) };
    let mut state = lock(&loop_.state);
    let Some(&id) = state.ids.get(&(source as usize)) else {
        return -2;
    };
    if state.sources[&id].storage.is_some() {
        return -22;
    }
    let entry = state.sources.remove(&id).unwrap();
    state.ids.remove(&(source as usize));
    let _ = poll::epoll_ctl(loop_.poll, 2, entry.fd, None);
    unsafe {
        (*source).loop_ = ptr::null_mut();
        (*source).private = ptr::null_mut();
    }
    0
}
unsafe extern "sysv64" fn invoke(
    data: *mut c_void,
    callback: Option<Invoke>,
    sequence: u32,
    bytes: *const c_void,
    size: usize,
    block: bool,
    user: *mut c_void,
) -> i32 {
    let Some(callback) = callback else { return -22 };
    if size != 0 && bytes.is_null() {
        return -22;
    }
    let loop_ = unsafe { object(data) };
    if lock(&loop_.state).entered == Some(std::thread::current().id()) {
        unsafe {
            loop_.dispatch_work();
            return callback(loop_.public.loop_, false, sequence, bytes, size, user);
        }
    }
    let completion = block.then(|| Arc::new((Mutex::new(None), Condvar::new())));
    {
        let mut state = lock(&loop_.state);
        if state.work.len() >= 1024 || size > 1024 * 1024usize - state.bytes {
            return -32;
        }
        let mut copy = Vec::new();
        if copy.try_reserve_exact(size).is_err() {
            return -12;
        }
        if size != 0 {
            copy.extend_from_slice(unsafe {
                core::slice::from_raw_parts(bytes.cast::<u8>(), size)
            });
        }
        state.bytes += size;
        state.work.push_back(Work {
            callback,
            sequence,
            bytes: copy,
            user: user as usize,
            completion: completion.clone(),
        });
        loop_.wake();
    }
    if let Some(wait) = completion {
        let mut result = lock(&wait.0);
        while result.is_none() {
            result = wait.1.wait(result).unwrap_or_else(|p| p.into_inner());
        }
        result.unwrap()
    } else if sequence == u32::MAX {
        0
    } else {
        (sequence | 0x40000000) as i32
    }
}
#[repr(C)]
struct HookEvents {
    version: u32,
    before: Option<Idle>,
    after: Option<Idle>,
}
#[repr(C)]
struct ControlMethods {
    version: u32,
    get_fd: unsafe extern "sysv64" fn(*mut c_void) -> i32,
    add_hook: unsafe extern "sysv64" fn(*mut c_void, *mut SpaHook, *const HookEvents, *mut c_void),
    enter: unsafe extern "sysv64" fn(*mut c_void),
    leave: unsafe extern "sysv64" fn(*mut c_void),
    iterate: unsafe extern "sysv64" fn(*mut c_void, i32) -> i32,
    check: unsafe extern "sysv64" fn(*mut c_void) -> i32,
}
static CONTROL_METHODS: ControlMethods = ControlMethods {
    version: 1,
    get_fd,
    add_hook,
    enter,
    leave,
    iterate,
    check,
};
unsafe extern "sysv64" fn get_fd(data: *mut c_void) -> i32 {
    unsafe { object(data) }.poll
}
unsafe extern "sysv64" fn add_hook(
    data: *mut c_void,
    hook: *mut SpaHook,
    events: *const HookEvents,
    user: *mut c_void,
) {
    if hook.is_null() || events.is_null() {
        return;
    }
    let head = unsafe { object(data) }.hooks.get().cast::<usize>();
    let hook = hook.cast::<usize>();
    unsafe {
        let prev = *head.add(1) as *mut usize;
        hook.write(head as usize);
        hook.add(1).write(prev as usize);
        hook.add(2).write(events as usize);
        hook.add(3).write(user as usize);
        hook.add(4).write(0);
        hook.add(5).write(0);
        *prev = hook as usize;
        *head.add(1) = hook as usize;
    }
}
unsafe extern "sysv64" fn enter(data: *mut c_void) {
    let mut state = lock(&unsafe { object(data) }.state);
    let thread = std::thread::current().id();
    if state.entered.is_none() || state.entered == Some(thread) {
        state.entered = Some(thread);
        state.depth += 1;
    }
}
unsafe extern "sysv64" fn leave(data: *mut c_void) {
    let mut state = lock(&unsafe { object(data) }.state);
    if state.entered == Some(std::thread::current().id()) {
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.entered = None;
        }
    }
}
unsafe extern "sysv64" fn check(data: *mut c_void) -> i32 {
    i32::from(lock(&unsafe { object(data) }.state).entered == Some(std::thread::current().id()))
}
unsafe extern "sysv64" fn iterate(data: *mut c_void, timeout: i32) -> i32 {
    let loop_ = unsafe { object(data) };
    if unsafe { check(data) } != 1 || timeout < -1 {
        return -22;
    }
    let _ = eventfd::read_eventfd(loop_.wake, &mut [0; 8], true);
    let mut count = unsafe { loop_.dispatch_work() };
    if loop_.pending_core.swap(false, Ordering::AcqRel) {
        unsafe { dispatch_initial_events(loop_.audio) };
        count += 1;
    }
    let timeout = if count != 0 || loop_.quit.load(Ordering::Acquire) {
        0
    } else {
        timeout
    };
    unsafe { loop_.hooks(true) };
    let mut events = [poll::EpollEvent::default(); 64];
    let ready = poll::epoll_wait(loop_.poll, &mut events, timeout);
    unsafe { loop_.hooks(false) };
    let ready = match ready {
        Ok(n) => n,
        Err(e) => return -e,
    };
    for event in &events[..ready] {
        let id = event.data;
        if id == 0 {
            continue;
        }
        let (pointer, callback, fd, user) = {
            let state = lock(&loop_.state);
            let Some(entry) = state.sources.get(&id) else {
                continue;
            };
            let source = entry.pointer as *mut Source;
            unsafe {
                (*source).rmask = event.events;
            }
            (source, entry.callback, entry.fd, unsafe { (*source).data })
        };
        match callback {
            Callback::Raw => {
                if let Some(call) = unsafe { (*pointer).callback } {
                    unsafe { call(pointer) }
                }
            }
            Callback::Io(call) => unsafe { call(user, fd, event.events) },
            Callback::Idle(call) => unsafe { call(user) },
            Callback::Event(call) | Callback::Timer(call) => {
                let mut bytes = [0; 8];
                let result = if matches!(callback, Callback::Timer(_)) {
                    timerfd::read(fd, &mut bytes)
                } else {
                    eventfd::read_eventfd(fd, &mut bytes, true)
                };
                if result.is_ok() {
                    unsafe { call(user, u64::from_ne_bytes(bytes)) };
                }
            }
            Callback::Signal(call) => {
                let mut bytes = [0u8; 128];
                if unsafe { libc::kinakaze_read(fd, bytes.as_mut_ptr().cast(), 128) } == 128 {
                    unsafe { call(user, i32::from_ne_bytes(bytes[..4].try_into().unwrap())) };
                }
            }
        }
        count += 1;
    }
    count as i32
}

#[repr(C)]
struct UtilsMethods {
    version: u32,
    add_io: unsafe extern "sysv64" fn(
        *mut c_void,
        i32,
        u32,
        bool,
        Option<Io>,
        *mut c_void,
    ) -> *mut Source,
    update_io: unsafe extern "sysv64" fn(*mut c_void, *mut Source, u32) -> i32,
    add_idle:
        unsafe extern "sysv64" fn(*mut c_void, bool, Option<Idle>, *mut c_void) -> *mut Source,
    enable_idle: unsafe extern "sysv64" fn(*mut c_void, *mut Source, bool) -> i32,
    add_event: unsafe extern "sysv64" fn(*mut c_void, Option<Count>, *mut c_void) -> *mut Source,
    signal_event: unsafe extern "sysv64" fn(*mut c_void, *mut Source) -> i32,
    add_timer: unsafe extern "sysv64" fn(*mut c_void, Option<Count>, *mut c_void) -> *mut Source,
    update_timer: unsafe extern "sysv64" fn(
        *mut c_void,
        *mut Source,
        *const timerfd::Timespec,
        *const timerfd::Timespec,
        bool,
    ) -> i32,
    add_signal:
        unsafe extern "sysv64" fn(*mut c_void, i32, Option<Signal>, *mut c_void) -> *mut Source,
    destroy: unsafe extern "sysv64" fn(*mut c_void, *mut Source),
}
static UTILS_METHODS: UtilsMethods = UtilsMethods {
    version: 0,
    add_io,
    update_io,
    add_idle,
    enable_idle,
    add_event,
    signal_event,
    add_timer,
    update_timer,
    add_signal,
    destroy: destroy_source,
};
unsafe fn utility(
    data: *mut c_void,
    fd: i32,
    mask: u32,
    callback: Callback,
    user: *mut c_void,
    close: bool,
) -> *mut Source {
    let mut storage = Box::new(Source {
        loop_: ptr::null_mut(),
        callback: None,
        data: user,
        fd,
        mask,
        rmask: 0,
        private: ptr::null_mut(),
    });
    let pointer = &raw mut *storage;
    if let Err(e) = unsafe { register(object(data), pointer, Some(storage), callback, close) } {
        if close {
            let _ = kinakaze_vfs::close(fd);
        }
        kinakaze_tls::set_errno(e);
        return ptr::null_mut();
    }
    pointer
}
unsafe extern "sysv64" fn add_io(
    data: *mut c_void,
    fd: i32,
    mask: u32,
    close: bool,
    call: Option<Io>,
    user: *mut c_void,
) -> *mut Source {
    let Some(call) = call else {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    unsafe { utility(data, fd, mask, Callback::Io(call), user, close) }
}
unsafe extern "sysv64" fn update_io(data: *mut c_void, source: *mut Source, mask: u32) -> i32 {
    if !lock(&unsafe { object(data) }.state)
        .ids
        .contains_key(&(source as usize))
    {
        return -2;
    }
    unsafe {
        (*source).mask = mask;
        update(data, source)
    }
}
unsafe extern "sysv64" fn add_event(
    data: *mut c_void,
    call: Option<Count>,
    user: *mut c_void,
) -> *mut Source {
    let Some(call) = call else {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    match eventfd::create_eventfd(0, 0x80800) {
        Ok(fd) => unsafe { utility(data, fd, 1, Callback::Event(call), user, true) },
        Err(e) => {
            kinakaze_tls::set_errno(e);
            ptr::null_mut()
        }
    }
}
unsafe extern "sysv64" fn signal_event(data: *mut c_void, source: *mut Source) -> i32 {
    let loop_ = unsafe { object(data) };
    let state = lock(&loop_.state);
    let Some(id) = state.ids.get(&(source as usize)) else {
        return -2;
    };
    let entry = &state.sources[id];
    if !matches!(entry.callback, Callback::Event(_)) {
        return -22;
    }
    eventfd::write_eventfd(entry.fd, &1u64.to_ne_bytes(), true).map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn add_idle(
    data: *mut c_void,
    enabled: bool,
    call: Option<Idle>,
    user: *mut c_void,
) -> *mut Source {
    let Some(call) = call else {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    match eventfd::create_eventfd(u32::from(enabled), 0x80800) {
        Ok(fd) => unsafe { utility(data, fd, 1, Callback::Idle(call), user, true) },
        Err(e) => {
            kinakaze_tls::set_errno(e);
            ptr::null_mut()
        }
    }
}
unsafe extern "sysv64" fn enable_idle(
    data: *mut c_void,
    source: *mut Source,
    enabled: bool,
) -> i32 {
    let state = lock(&unsafe { object(data) }.state);
    let Some(id) = state.ids.get(&(source as usize)) else {
        return -2;
    };
    let entry = &state.sources[id];
    if !matches!(entry.callback, Callback::Idle(_)) {
        return -22;
    }
    let _ = eventfd::read_eventfd(entry.fd, &mut [0; 8], true);
    if enabled {
        eventfd::write_eventfd(entry.fd, &1u64.to_ne_bytes(), true).map_or_else(|e| -e, |_| 0)
    } else {
        0
    }
}
unsafe extern "sysv64" fn add_timer(
    data: *mut c_void,
    call: Option<Count>,
    user: *mut c_void,
) -> *mut Source {
    let Some(call) = call else {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    match timerfd::create(1, 0x80800) {
        Ok(fd) => unsafe { utility(data, fd, 1, Callback::Timer(call), user, true) },
        Err(e) => {
            kinakaze_tls::set_errno(e);
            ptr::null_mut()
        }
    }
}
unsafe extern "sysv64" fn update_timer(
    data: *mut c_void,
    source: *mut Source,
    value: *const timerfd::Timespec,
    interval: *const timerfd::Timespec,
    absolute: bool,
) -> i32 {
    let state = lock(&unsafe { object(data) }.state);
    let Some(id) = state.ids.get(&(source as usize)) else {
        return -2;
    };
    let entry = &state.sources[id];
    if !matches!(entry.callback, Callback::Timer(_)) {
        return -22;
    }
    let value = timerfd::Itimerspec {
        value: unsafe { value.as_ref().copied() }.unwrap_or_default(),
        interval: unsafe { interval.as_ref().copied() }.unwrap_or_default(),
    };
    timerfd::settime(entry.fd, i32::from(absolute), value).map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn add_signal(
    data: *mut c_void,
    number: i32,
    call: Option<Signal>,
    user: *mut c_void,
) -> *mut Source {
    let Some(call) = call else {
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    let fd = unsafe { system::signal_create(ptr::null_mut(), number, 3) };
    if fd < 0 {
        kinakaze_tls::set_errno(-fd);
        return ptr::null_mut();
    }
    unsafe { utility(data, fd, 1, Callback::Signal(call), user, true) }
}
unsafe extern "sysv64" fn destroy_source(data: *mut c_void, source: *mut Source) {
    let loop_ = unsafe { object(data) };
    let mut state = lock(&loop_.state);
    let Some(&id) = state.ids.get(&(source as usize)) else {
        return;
    };
    if state.sources[&id].storage.is_none() {
        return;
    }
    let entry = state.sources.remove(&id).unwrap();
    state.ids.remove(&(source as usize));
    let _ = poll::epoll_ctl(loop_.poll, 2, entry.fd, None);
    drop(state);
    drop(entry);
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_main_loop_new")]
pub unsafe extern "sysv64" fn pw_main_loop_new(props: *const SpaDict) -> *mut c_void {
    unsafe { pw_loop_new(props) }
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_main_loop_get_loop")]
pub unsafe extern "sysv64" fn pw_main_loop_get_loop(loop_: *mut c_void) -> *mut c_void {
    if audio_for(loop_).is_some() {
        loop_
    } else {
        ptr::null_mut()
    }
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_main_loop_destroy")]
pub unsafe extern "sysv64" fn pw_main_loop_destroy(loop_: *mut c_void) {
    unsafe { pw_loop_destroy(loop_) }
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_main_loop_quit")]
pub unsafe extern "sysv64" fn pw_main_loop_quit(data: *mut c_void) -> i32 {
    if audio_for(data).is_none() {
        return -22;
    }
    let loop_ = unsafe { object(data) };
    loop_.quit.store(true, Ordering::Release);
    loop_.wake();
    0
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_main_loop_run")]
pub unsafe extern "sysv64" fn pw_main_loop_run(data: *mut c_void) -> i32 {
    if audio_for(data).is_none() {
        return -22;
    }
    let loop_ = unsafe { object(data) };
    if loop_.running.swap(true, Ordering::AcqRel) {
        return -16;
    }
    unsafe { enter(data) };
    let mut result = 0;
    while !loop_.quit.load(Ordering::Acquire) {
        let current = unsafe { iterate(data, -1) };
        if current < 0 && current != -4 {
            result = current;
            break;
        }
    }
    unsafe { leave(data) };
    loop_.running.store(false, Ordering::Release);
    loop_.quit.store(false, Ordering::Release);
    result
}

// The guest thread lock protects callbacks; it is never held while parked in
// epoll. Level-triggered readiness is consumed by iterate after reacquiring it.
pub(super) unsafe fn run_audio(data: *mut c_void) {
    let loop_ = unsafe { object(data) };
    let audio = unsafe { &*loop_.audio };
    unsafe { enter(data) };
    while !audio.stopped.load(Ordering::Acquire) {
        let mut ready = [poll::EpollEvent::default(); 64];
        if let Err(error) = poll::epoll_wait(loop_.poll, &mut ready, -1) {
            if error == 4 {
                continue;
            }
            break;
        }
        let Some(_guard) = audio.sync.callback(&audio.stopped) else {
            break;
        };
        unsafe { iterate(data, 0) };
    }
    unsafe { leave(data) };
}
