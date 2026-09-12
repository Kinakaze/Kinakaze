//! `libpipewire-0.3.so.0` compatibility provider for hosted Linux programs.
//!
//! This implements the small PipeWire client surface used by OpenAL Soft.  It
//! presents an in-process graph with one stereo sink and one silent source.
//! Playback uses event-driven shared-mode WASAPI with waveOut as a compatibility
//! fallback. No external daemon, socket, or Linux audio service is required.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_uint, c_void};
#[cfg(test)]
use core::slice;
use core::{mem, ptr};
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

mod client;
mod hooks;
mod loop_lock;
mod main_loop;
use loop_lock::LoopLock;
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE as WasapiHandle;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
    IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX as WasapiWaveFormat,
    eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_EVENT, HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveOutClose, waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader,
    waveOutWrite,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};

const PW_STREAM_STATE_UNCONNECTED: c_int = 0;
const PW_STREAM_STATE_PAUSED: c_int = 2;
const PW_STREAM_STATE_STREAMING: c_int = 3;
const PW_DIRECTION_OUTPUT: c_int = 1;
const SPA_IO_RATE_MATCH: c_uint = 8;
const PROXY_REGISTRY: c_uint = 1;
const PROXY_NODE: c_uint = 2;
const PROXY_CLIENT: c_uint = 3;
const NATIVE_NODE_ID: c_uint = 2;
const FRAMES_PER_BUFFER: usize = 512;
const SAMPLE_RATE: u32 = 48_000;
// Eight 512-frame packets provide ~85 ms of scheduling headroom.  This absorbs
// compilation and chunk-generation spikes without making the producer wake at a
// fixed high frequency or adding the very large latency of second-scale buffers.
const PLAYBACK_QUEUE_BUFFERS: usize = 8;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

struct MmcssRegistration(HANDLE);

impl MmcssRegistration {
    fn audio() -> Option<Self> {
        const AUDIO_TASK: [u16; 6] = [
            'A' as u16, 'u' as u16, 'd' as u16, 'i' as u16, 'o' as u16, 0,
        ];
        let mut task_index = 0;
        let handle =
            unsafe { AvSetMmThreadCharacteristicsW(AUDIO_TASK.as_ptr(), &raw mut task_index) };
        (!handle.is_null()).then_some(Self(handle))
    }
}

impl Drop for MmcssRegistration {
    fn drop(&mut self) {
        unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpaCallbacks {
    funcs: *const c_void,
    data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpaInterface {
    type_name: *const c_char,
    version: c_uint,
    callbacks: SpaCallbacks,
}

#[repr(C)]
pub struct SpaHook {
    _opaque: [usize; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpaDictItem {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpaDict {
    flags: c_uint,
    n_items: c_uint,
    items: *const SpaDictItem,
}

#[repr(C)]
pub struct pw_properties {
    dict: SpaDict,
    flags: c_uint,
}

#[repr(C)]
struct Properties {
    public: pw_properties,
    values: Vec<(CString, CString)>,
    items: Vec<SpaDictItem>,
}

impl Properties {
    fn new() -> Box<Self> {
        Box::new(Self {
            public: pw_properties {
                dict: SpaDict {
                    flags: 0,
                    n_items: 0,
                    items: ptr::null(),
                },
                flags: 0,
            },
            values: Vec::new(),
            items: Vec::new(),
        })
    }

    fn refresh(&mut self) {
        self.items.clear();
        self.items
            .extend(self.values.iter().map(|(key, value)| SpaDictItem {
                key: key.as_ptr(),
                value: value.as_ptr(),
            }));
        self.public.dict.n_items = self.items.len() as c_uint;
        self.public.dict.items = if self.items.is_empty() {
            ptr::null()
        } else {
            self.items.as_ptr()
        };
    }

    fn set(&mut self, key: &CStr, value: &CStr) {
        if let Some(entry) = self
            .values
            .iter_mut()
            .find(|(candidate, _)| candidate.as_c_str() == key)
        {
            entry.1 = value.to_owned();
        } else {
            self.values.push((key.to_owned(), value.to_owned()));
        }
        self.refresh();
    }
}

unsafe fn properties_mut<'a>(props: *mut pw_properties) -> &'a mut Properties {
    unsafe { &mut *props.cast::<Properties>() }
}

unsafe fn copy_properties(props: *const pw_properties) -> *mut pw_properties {
    let original = unsafe { &*props.cast::<Properties>() };
    let mut copy = Properties::new();
    copy.values = original.values.clone();
    copy.refresh();
    Box::into_raw(copy).cast()
}

unsafe fn free_properties(props: *mut pw_properties) {
    if !props.is_null() {
        drop(unsafe { Box::from_raw(props.cast::<Properties>()) });
    }
}

#[repr(C)]
struct ProxyHeader {
    interface: SpaInterface,
    kind: c_uint,
    user_data: Vec<u8>,
}

fn new_user_data(size: usize) -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    data.try_reserve_exact(size).map_err(|_| 12)?;
    data.resize(size, 0);
    Ok(data)
}

type CoreDone = unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int);

#[repr(C)]
struct PwCoreEvents {
    version: c_uint,
    info: Option<unsafe extern "sysv64" fn(*mut c_void, *const c_void)>,
    done: Option<CoreDone>,
    ping: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int)>,
    error: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int, c_int, *const c_char)>,
    remove_id: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint)>,
    bound_id: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_uint)>,
    add_mem: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_uint, c_int, c_uint)>,
    remove_mem: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint)>,
    bound_props: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_uint, *const SpaDict)>,
}

type RegistryGlobal =
    unsafe extern "sysv64" fn(*mut c_void, c_uint, c_uint, *const c_char, c_uint, *const SpaDict);

#[repr(C)]
struct PwRegistryEvents {
    version: c_uint,
    global: Option<RegistryGlobal>,
    global_remove: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint)>,
}

#[repr(C)]
struct PwNodeInfo {
    id: c_uint,
    max_input_ports: c_uint,
    max_output_ports: c_uint,
    change_mask: u64,
    n_input_ports: c_uint,
    n_output_ports: c_uint,
    state: c_int,
    error: *const c_char,
    props: *mut SpaDict,
    params: *mut c_void,
    n_params: c_uint,
}

#[repr(C)]
struct PwNodeEvents {
    version: c_uint,
    info: Option<unsafe extern "sysv64" fn(*mut c_void, *const PwNodeInfo)>,
    param: Option<
        unsafe extern "sysv64" fn(*mut c_void, c_int, c_uint, c_uint, c_uint, *const c_void),
    >,
}

#[repr(C)]
pub struct pw_thread_loop {
    sync: Arc<LoopLock>,
    started: AtomicBool,
    core: Mutex<Vec<(usize, usize)>>,
    native_loop: AtomicUsize,
    loop_worker: Mutex<Option<JoinHandle<()>>>,
    stopped: AtomicBool,
}

unsafe impl Send for pw_thread_loop {}
unsafe impl Sync for pw_thread_loop {}

#[repr(C)]
pub struct pw_context {
    loop_: *mut pw_thread_loop,
    props: *mut pw_properties,
}

#[repr(C)]
pub struct pw_core {
    interface: SpaInterface,
    loop_: *mut pw_thread_loop,
    props: *mut pw_properties,
    listeners: hooks::Hooks,
    registry: *mut pw_registry,
    sequence: c_int,
    serial: usize,
    pending_sync: std::collections::VecDeque<(u32, i32)>,
    client: *mut client::Client,
    permissions: Vec<client::Permission>,
}

#[repr(C)]
pub struct pw_registry {
    header: ProxyHeader,
    core: *mut pw_core,
    listeners: hooks::Hooks,
    visible_permissions: u32,
    props: *mut pw_properties,
}

#[repr(C)]
pub struct pw_node {
    header: ProxyHeader,
    id: c_uint,
    props: *mut pw_properties,
    listeners: hooks::Hooks,
}

unsafe impl Send for pw_core {}
unsafe impl Sync for pw_core {}

#[repr(C)]
struct CoreMethods {
    version: c_uint,
    add_listener: Option<
        unsafe extern "sysv64" fn(
            *mut c_void,
            *mut SpaHook,
            *const PwCoreEvents,
            *mut c_void,
        ) -> c_int,
    >,
    hello: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint) -> c_int>,
    sync: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int) -> c_int>,
    pong: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int) -> c_int>,
    error: Option<
        unsafe extern "sysv64" fn(*mut c_void, c_uint, c_int, c_int, *const c_char) -> c_int,
    >,
    get_registry: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, usize) -> *mut pw_registry>,
    create_object: Option<
        unsafe extern "sysv64" fn(
            *mut c_void,
            *const c_char,
            *const c_char,
            c_uint,
            *const SpaDict,
            usize,
        ) -> *mut c_void,
    >,
    destroy: Option<unsafe extern "sysv64" fn(*mut c_void, *mut c_void) -> c_int>,
}

#[repr(C)]
struct RegistryMethods {
    version: c_uint,
    add_listener: Option<
        unsafe extern "sysv64" fn(
            *mut c_void,
            *mut SpaHook,
            *const PwRegistryEvents,
            *mut c_void,
        ) -> c_int,
    >,
    bind: Option<
        unsafe extern "sysv64" fn(*mut c_void, c_uint, *const c_char, c_uint, usize) -> *mut c_void,
    >,
    destroy: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint) -> c_int>,
}

#[repr(C)]
struct NodeMethods {
    version: c_uint,
    add_listener: Option<
        unsafe extern "sysv64" fn(
            *mut c_void,
            *mut SpaHook,
            *const PwNodeEvents,
            *mut c_void,
        ) -> c_int,
    >,
    subscribe_params: Option<unsafe extern "sysv64" fn(*mut c_void, *mut c_uint, c_uint) -> c_int>,
    enum_params: Option<
        unsafe extern "sysv64" fn(
            *mut c_void,
            c_int,
            c_uint,
            c_uint,
            c_uint,
            *const c_void,
        ) -> c_int,
    >,
    set_param:
        Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, c_uint, *const c_void) -> c_int>,
    send_command: Option<unsafe extern "sysv64" fn(*mut c_void, *const c_void) -> c_int>,
}

unsafe extern "sysv64" fn core_add_listener(
    object: *mut c_void,
    listener: *mut SpaHook,
    events: *const PwCoreEvents,
    data: *mut c_void,
) -> c_int {
    let core = unsafe { &mut *object.cast::<pw_core>() };
    let result = unsafe { core.listeners.add(listener, events.cast(), data) };
    if result == 0 {
        schedule_core(core.loop_);
    }
    result
}

unsafe extern "sysv64" fn core_sync(object: *mut c_void, id: c_uint, _sequence: c_int) -> c_int {
    let core = unsafe { &mut *object.cast::<pw_core>() };
    if unsafe { client::permissions(core, 0) } & client::EXECUTE == 0 {
        return -13;
    }
    if core.pending_sync.len() >= 1024 {
        return -28;
    }
    core.sequence = core.sequence.wrapping_add(1) & 0x3fff_ffff;
    core.pending_sync.push_back((id, core.sequence));
    schedule_core(core.loop_);
    core.sequence
}

unsafe fn notify_core_error(core: *mut pw_core, id: u32, result: i32, message: *const c_char) {
    let (loop_, serial, sequence, hooks) = unsafe {
        (
            (*core).loop_,
            (*core).serial,
            (*core).sequence,
            (*core).listeners.snapshot(),
        )
    };
    for (hook, callbacks, _) in hooks {
        if !core_is_live(loop_, core, serial) {
            return;
        }
        if !unsafe { (*core).listeners.contains(hook) } {
            continue;
        }
        if let Some(call) = unsafe { (*callbacks.funcs.cast::<PwCoreEvents>()).error } {
            unsafe { call(callbacks.data, id, sequence, result, message) };
        }
    }
}

fn make_interface(
    type_name: *const c_char,
    version: c_uint,
    funcs: *const c_void,
    data: *mut c_void,
) -> SpaInterface {
    SpaInterface {
        type_name,
        version,
        callbacks: SpaCallbacks { funcs, data },
    }
}

unsafe extern "sysv64" fn core_get_registry(
    object: *mut c_void,
    version: c_uint,
    user_data: usize,
) -> *mut pw_registry {
    let core = unsafe { &mut *object.cast::<pw_core>() };
    if !core.registry.is_null() {
        return core.registry;
    }
    let data = match new_user_data(user_data) {
        Ok(data) => data,
        Err(e) => {
            kinakaze_tls::set_errno(e);
            return ptr::null_mut();
        }
    };
    let props = make_node_properties();
    let mut registry = Box::new(pw_registry {
        header: ProxyHeader {
            interface: make_interface(
                c"PipeWire:Interface:Registry".as_ptr(),
                version.min(3),
                (&raw const REGISTRY_METHODS).cast(),
                ptr::null_mut(),
            ),
            kind: PROXY_REGISTRY,
            user_data: data,
        },
        core,
        listeners: hooks::Hooks::new(),
        visible_permissions: 0,
        props,
    });
    let address = (&raw mut *registry).cast::<c_void>();
    registry.header.interface.callbacks.data = address;
    let registry = Box::into_raw(registry);
    core.registry = registry;
    registry
}

unsafe extern "sysv64" fn registry_add_listener(
    object: *mut c_void,
    listener: *mut SpaHook,
    events: *const PwRegistryEvents,
    data: *mut c_void,
) -> c_int {
    let registry = unsafe { &mut *object.cast::<pw_registry>() };
    let result = unsafe { registry.listeners.add(listener, events.cast(), data) };
    if result == 0 {
        schedule_core(unsafe { (*registry.core).loop_ });
    }
    result
}

unsafe extern "sysv64" fn registry_bind(
    object: *mut c_void,
    id: c_uint,
    type_name: *const c_char,
    version: c_uint,
    user_data: usize,
) -> *mut c_void {
    let registry = unsafe { &*object.cast::<pw_registry>() };
    let error = if id != NATIVE_NODE_ID {
        Some(2)
    } else if type_name.is_null()
        || unsafe { CStr::from_ptr(type_name) } != c"PipeWire:Interface:Node"
    {
        Some(22)
    } else if unsafe { client::permissions(registry.core, id) } & client::READ == 0 {
        Some(13)
    } else {
        None
    };
    if let Some(error) = error {
        kinakaze_tls::set_errno(error);
        return ptr::null_mut();
    }
    let user_data = match new_user_data(user_data) {
        Ok(data) => data,
        Err(e) => {
            kinakaze_tls::set_errno(e);
            return ptr::null_mut();
        }
    };
    let mut node = Box::new(pw_node {
        header: ProxyHeader {
            interface: make_interface(
                c"PipeWire:Interface:Node".as_ptr(),
                version.min(3),
                (&raw const NODE_METHODS).cast(),
                ptr::null_mut(),
            ),
            kind: PROXY_NODE,
            user_data,
        },
        id,
        props: make_node_properties(),
        listeners: hooks::Hooks::new(),
    });
    node.header.interface.callbacks.data = (&raw mut *node).cast();
    Box::into_raw(node).cast()
}

unsafe extern "sysv64" fn node_add_listener(
    object: *mut c_void,
    listener: *mut SpaHook,
    events: *const PwNodeEvents,
    data: *mut c_void,
) -> c_int {
    let (result, id, props) = unsafe {
        let node = &mut *object.cast::<pw_node>();
        let result = node.listeners.add(listener, events.cast(), data);
        (
            result,
            node.id,
            Box::from_raw(copy_properties(node.props).cast::<Properties>()),
        )
    };
    if result < 0 {
        return result;
    }
    if let Some(callback) = unsafe { (*events).info } {
        let info = PwNodeInfo {
            id,
            max_input_ports: 64,
            max_output_ports: 0,
            change_mask: 1 << 3,
            n_input_ports: 2,
            n_output_ports: 0,
            state: 2,
            error: ptr::null(),
            props: (&raw const props.public.dict).cast_mut(),
            params: ptr::null_mut(),
            n_params: 0,
        };
        unsafe { callback(data, &info) };
    }
    0
}

unsafe extern "sysv64" fn node_subscribe_params(
    _object: *mut c_void,
    _ids: *mut c_uint,
    _count: c_uint,
) -> c_int {
    0
}

static CORE_METHODS: CoreMethods = CoreMethods {
    version: 0,
    add_listener: Some(core_add_listener),
    hello: None,
    sync: Some(core_sync),
    pong: None,
    error: None,
    get_registry: Some(core_get_registry),
    create_object: None,
    destroy: None,
};

static REGISTRY_METHODS: RegistryMethods = RegistryMethods {
    version: 0,
    add_listener: Some(registry_add_listener),
    bind: Some(registry_bind),
    destroy: None,
};

static NODE_METHODS: NodeMethods = NodeMethods {
    version: 0,
    add_listener: Some(node_add_listener),
    subscribe_params: Some(node_subscribe_params),
    enum_params: None,
    set_param: None,
    send_command: None,
};

fn make_node_properties() -> *mut pw_properties {
    let mut props = Properties::new();
    props.set(c"media.class", c"Audio/Sink");
    props.set(c"node.name", c"kinakaze.output");
    props.set(c"node.description", c"Kinakaze PipeWire Output");
    props.set(c"object.serial", c"1");
    Box::into_raw(props).cast()
}

fn schedule_core(loop_: *mut pw_thread_loop) {
    let pointer = unsafe { (*loop_).native_loop.load(Ordering::Acquire) };
    if pointer != 0 {
        unsafe { &*(pointer as *const main_loop::HostLoop) }.schedule_core();
    }
}

fn core_is_live(loop_: *mut pw_thread_loop, core: *mut pw_core, serial: usize) -> bool {
    lock(unsafe { &(*loop_).core }).contains(&(core as usize, serial))
}
unsafe fn dispatch_initial_events(loop_: *mut pw_thread_loop) {
    let cores = lock(unsafe { &(*loop_).core }).clone();
    for (pointer, serial) in cores {
        let core = pointer as *mut pw_core;
        if core_is_live(loop_, core, serial) {
            unsafe { dispatch_core(loop_, core, serial) };
        }
    }
}
unsafe fn dispatch_core(loop_: *mut pw_thread_loop, core: *mut pw_core, serial: usize) {
    unsafe { client::dispatch(core, loop_, serial) };
    if !core_is_live(loop_, core, serial) {
        return;
    }
    let registry = unsafe { (*core).registry };
    if !registry.is_null() {
        let permissions = unsafe { client::permissions(core, NATIVE_NODE_ID) };
        let (previous, hooks, props) = unsafe {
            let object = &mut *registry;
            let previous = std::mem::replace(&mut object.visible_permissions, permissions);
            (
                previous,
                object.listeners.snapshot(),
                Box::from_raw(copy_properties(object.props).cast::<Properties>()),
            )
        };
        for (hook, callbacks, initial) in hooks {
            if !core_is_live(loop_, core, serial) || unsafe { (*core).registry } != registry {
                return;
            }
            if !unsafe { (*registry).listeners.contains(hook) } {
                continue;
            }
            unsafe {
                hooks::Hooks::initialized(hook);
            }
            let events = callbacks.funcs.cast::<PwRegistryEvents>();
            if permissions & client::READ != 0 && (initial || previous != permissions) {
                if let Some(call) = unsafe { (*events).global } {
                    unsafe {
                        call(
                            callbacks.data,
                            NATIVE_NODE_ID,
                            permissions,
                            c"PipeWire:Interface:Node".as_ptr(),
                            3,
                            &props.public.dict,
                        )
                    };
                }
            } else if !initial && previous & client::READ != 0 && permissions & client::READ == 0 {
                if let Some(call) = unsafe { (*events).global_remove } {
                    unsafe { call(callbacks.data, NATIVE_NODE_ID) };
                }
            }
        }
    }
    if !core_is_live(loop_, core, serial) {
        return;
    }
    let pending = unsafe { std::mem::take(&mut (*core).pending_sync) };
    for (id, sequence) in pending {
        let hooks = unsafe { (*core).listeners.snapshot() };
        for (hook, callbacks, _) in hooks {
            if !core_is_live(loop_, core, serial) {
                return;
            }
            if !unsafe { (*core).listeners.contains(hook) } {
                continue;
            }
            if let Some(done) = unsafe { (*callbacks.funcs.cast::<PwCoreEvents>()).done } {
                unsafe { done(callbacks.data, id, sequence) };
            }
        }
        if !core_is_live(loop_, core, serial) {
            return;
        }
    }
}

#[repr(C)]
pub struct SpaChunk {
    offset: c_uint,
    size: c_uint,
    stride: c_int,
    flags: c_int,
}

#[repr(C)]
pub struct SpaData {
    type_: c_uint,
    flags: c_uint,
    fd: i64,
    map_offset: c_uint,
    max_size: c_uint,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
pub struct SpaBuffer {
    n_metas: c_uint,
    n_datas: c_uint,
    metas: *mut c_void,
    datas: *mut SpaData,
}

#[repr(C)]
pub struct pw_buffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    requested: u64,
    time: u64,
}

struct AudioBuffer {
    public: pw_buffer,
    spa: SpaBuffer,
    datas: Vec<SpaData>,
    chunks: Vec<SpaChunk>,
    planes: Vec<Box<[f32]>>,
}

impl AudioBuffer {
    fn new(channels: usize) -> Box<Self> {
        let channels = channels.max(1);
        let mut value = Box::new(Self {
            public: pw_buffer {
                buffer: ptr::null_mut(),
                user_data: ptr::null_mut(),
                size: 0,
                requested: FRAMES_PER_BUFFER as u64,
                time: 0,
            },
            spa: SpaBuffer {
                n_metas: 0,
                n_datas: channels as c_uint,
                metas: ptr::null_mut(),
                datas: ptr::null_mut(),
            },
            datas: Vec::with_capacity(channels),
            chunks: Vec::with_capacity(channels),
            planes: (0..channels)
                .map(|_| vec![0.0f32; FRAMES_PER_BUFFER].into_boxed_slice())
                .collect(),
        });
        value.chunks.extend((0..channels).map(|_| SpaChunk {
            offset: 0,
            size: 0,
            stride: mem::size_of::<f32>() as c_int,
            flags: 0,
        }));
        for index in 0..channels {
            value.datas.push(SpaData {
                type_: 1,
                flags: 3,
                fd: -1,
                map_offset: 0,
                max_size: (FRAMES_PER_BUFFER * mem::size_of::<f32>()) as c_uint,
                data: value.planes[index].as_mut_ptr().cast(),
                chunk: &raw mut value.chunks[index],
            });
        }
        value.spa.datas = value.datas.as_mut_ptr();
        value.public.buffer = &raw mut value.spa;
        value
    }
}

#[repr(C)]
struct SpaIoRateMatch {
    delay: c_uint,
    size: c_uint,
    rate: f64,
    flags: c_uint,
    delay_fraction: c_int,
    padding: [c_uint; 6],
}

type StreamStateChanged = unsafe extern "sysv64" fn(*mut c_void, c_int, c_int, *const c_char);
type StreamIoChanged = unsafe extern "sysv64" fn(*mut c_void, c_uint, *mut c_void, c_uint);
type StreamProcess = unsafe extern "sysv64" fn(*mut c_void);

#[repr(C)]
pub struct PwStreamEvents {
    version: c_uint,
    destroy: Option<unsafe extern "sysv64" fn(*mut c_void)>,
    state_changed: Option<StreamStateChanged>,
    control_info: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, *const c_void)>,
    io_changed: Option<StreamIoChanged>,
    param_changed: Option<unsafe extern "sysv64" fn(*mut c_void, c_uint, *const c_void)>,
    add_buffer: Option<unsafe extern "sysv64" fn(*mut c_void, *mut pw_buffer)>,
    remove_buffer: Option<unsafe extern "sysv64" fn(*mut c_void, *mut pw_buffer)>,
    process: Option<StreamProcess>,
    drained: Option<unsafe extern "sysv64" fn(*mut c_void)>,
    command: Option<unsafe extern "sysv64" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "sysv64" fn(*mut c_void)>,
}

struct WaveBuffer {
    data: Box<[i16]>,
    header: Box<WAVEHDR>,
    queued: bool,
}

struct WaveOut {
    handle: HWAVEOUT,
    done_event: HANDLE,
    buffers: Vec<WaveBuffer>,
}

unsafe impl Send for WaveOut {}

impl WaveOut {
    fn open() -> Option<Self> {
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 2,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let done_event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if done_event.is_null() {
            return None;
        }
        let mut handle = ptr::null_mut();
        let result = unsafe {
            waveOutOpen(
                &raw mut handle,
                WAVE_MAPPER,
                &raw const format,
                done_event as usize,
                0,
                CALLBACK_EVENT,
            )
        };
        if result != 0 || handle.is_null() {
            unsafe { CloseHandle(done_event) };
            return None;
        }
        let mut output = Self {
            handle,
            done_event,
            buffers: Vec::with_capacity(PLAYBACK_QUEUE_BUFFERS),
        };
        // Prepare a fixed packet pool once. The render loop only rewrites a
        // completed packet and submits it again; it performs no allocation and
        // no prepare/unprepare kernel transition.
        for _ in 0..PLAYBACK_QUEUE_BUFFERS {
            let mut data = vec![0i16; FRAMES_PER_BUFFER * 2].into_boxed_slice();
            let mut header = Box::new(WAVEHDR::default());
            header.lpData = data.as_mut_ptr().cast();
            header.dwBufferLength = (data.len() * mem::size_of::<i16>()) as u32;
            if unsafe {
                waveOutPrepareHeader(
                    output.handle,
                    &raw mut *header,
                    mem::size_of::<WAVEHDR>() as u32,
                )
            } != 0
            {
                return None;
            }
            output.buffers.push(WaveBuffer {
                data,
                header,
                queued: false,
            });
        }
        Some(output)
    }

    fn reap(&mut self) {
        let mut index = 0;
        while index < self.buffers.len() {
            if self.buffers[index].queued {
                // winmm changes this field asynchronously; a normal load may be
                // hoisted or reused and leave completed packets permanently queued.
                let flags =
                    unsafe { ptr::addr_of!((*self.buffers[index].header).dwFlags).read_volatile() };
                if flags & WHDR_DONE != 0 {
                    self.buffers[index].queued = false;
                }
            }
            index += 1;
        }
    }

    fn queued_count(&self) -> usize {
        self.buffers.iter().filter(|buffer| buffer.queued).count()
    }

    fn write_planar(&mut self, left: &[f32], right: &[f32], frames: usize) -> bool {
        self.reap();
        let Some(buffer) = self.buffers.iter_mut().find(|buffer| !buffer.queued) else {
            return false;
        };
        let frames = frames
            .min(left.len())
            .min(right.len())
            .min(FRAMES_PER_BUFFER);
        interleave_planar_stereo(left, right, &mut buffer.data, frames);
        buffer.header.dwBufferLength = (frames * 2 * mem::size_of::<i16>()) as u32;
        if unsafe {
            waveOutWrite(
                self.handle,
                &raw mut *buffer.header,
                mem::size_of::<WAVEHDR>() as u32,
            )
        } == 0
        {
            buffer.queued = true;
            true
        } else {
            false
        }
    }

    fn wait_for_completion(&self) {
        // The event is signalled by winmm for buffer completions.  A finite wait
        // also lets stream shutdown and activation changes take effect promptly.
        unsafe { WaitForSingleObject(self.done_event, 20) };
    }

    fn suspend(&mut self) {
        unsafe { waveOutReset(self.handle) };
        for buffer in &mut self.buffers {
            buffer.queued = false;
        }
    }
}

fn interleave_planar_stereo(left: &[f32], right: &[f32], output: &mut [i16], frames: usize) {
    debug_assert!(left.len() >= frames && right.len() >= frames && output.len() >= frames * 2);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if std::arch::is_x86_feature_detected!("avx2") {
            return interleave_planar_stereo_avx2(left, right, output, frames);
        }
        return interleave_planar_stereo_sse2(left, right, output, frames);
    }
    #[cfg(not(target_arch = "x86_64"))]
    interleave_planar_stereo_scalar(left, right, output, frames);
}

fn interleave_planar_stereo_scalar(left: &[f32], right: &[f32], output: &mut [i16], frames: usize) {
    for frame in 0..frames {
        output[frame * 2] = sample_to_i16(left[frame]);
        output[frame * 2 + 1] = sample_to_i16(right[frame]);
    }
}

#[inline]
fn sample_to_i16(value: f32) -> i16 {
    if value.is_nan() {
        0
    } else {
        (value.clamp(-1.0, 1.0) * 32767.0) as i16
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn interleave_planar_stereo_sse2(
    left: &[f32],
    right: &[f32],
    output: &mut [i16],
    frames: usize,
) {
    use core::arch::x86_64::*;

    let scale = _mm_set1_ps(32767.0);
    let minimum = _mm_set1_ps(-1.0);
    let maximum = _mm_set1_ps(1.0);
    let mut frame = 0usize;
    while frame + 4 <= frames {
        let mut l = unsafe { _mm_loadu_ps(left.as_ptr().add(frame)) };
        let mut r = unsafe { _mm_loadu_ps(right.as_ptr().add(frame)) };
        l = _mm_and_ps(l, _mm_cmpord_ps(l, l));
        r = _mm_and_ps(r, _mm_cmpord_ps(r, r));
        l = _mm_mul_ps(_mm_min_ps(_mm_max_ps(l, minimum), maximum), scale);
        r = _mm_mul_ps(_mm_min_ps(_mm_max_ps(r, minimum), maximum), scale);
        let packed = _mm_packs_epi32(_mm_cvttps_epi32(l), _mm_cvttps_epi32(r));
        let interleaved = _mm_unpacklo_epi16(packed, _mm_srli_si128::<8>(packed));
        unsafe {
            _mm_storeu_si128(
                output.as_mut_ptr().add(frame * 2).cast::<__m128i>(),
                interleaved,
            )
        };
        frame += 4;
    }
    interleave_planar_stereo_scalar(
        &left[frame..],
        &right[frame..],
        &mut output[frame * 2..],
        frames - frame,
    );
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn interleave_planar_stereo_avx2(
    left: &[f32],
    right: &[f32],
    output: &mut [i16],
    frames: usize,
) {
    use core::arch::x86_64::*;

    let scale = _mm256_set1_ps(32767.0);
    let minimum = _mm256_set1_ps(-1.0);
    let maximum = _mm256_set1_ps(1.0);
    let mut frame = 0usize;
    while frame + 8 <= frames {
        let mut l = unsafe { _mm256_loadu_ps(left.as_ptr().add(frame)) };
        let mut r = unsafe { _mm256_loadu_ps(right.as_ptr().add(frame)) };
        l = _mm256_and_ps(l, _mm256_cmp_ps::<_CMP_ORD_Q>(l, l));
        r = _mm256_and_ps(r, _mm256_cmp_ps::<_CMP_ORD_Q>(r, r));
        l = _mm256_mul_ps(_mm256_min_ps(_mm256_max_ps(l, minimum), maximum), scale);
        r = _mm256_mul_ps(_mm256_min_ps(_mm256_max_ps(r, minimum), maximum), scale);
        let li = _mm256_cvttps_epi32(l);
        let ri = _mm256_cvttps_epi32(r);
        let lp = _mm_packs_epi32(
            _mm256_castsi256_si128(li),
            _mm256_extracti128_si256::<1>(li),
        );
        let rp = _mm_packs_epi32(
            _mm256_castsi256_si128(ri),
            _mm256_extracti128_si256::<1>(ri),
        );
        unsafe {
            _mm_storeu_si128(
                output.as_mut_ptr().add(frame * 2).cast::<__m128i>(),
                _mm_unpacklo_epi16(lp, rp),
            );
            _mm_storeu_si128(
                output.as_mut_ptr().add(frame * 2 + 8).cast::<__m128i>(),
                _mm_unpackhi_epi16(lp, rp),
            );
        }
        frame += 8;
    }
    unsafe {
        interleave_planar_stereo_sse2(
            &left[frame..],
            &right[frame..],
            &mut output[frame * 2..],
            frames - frame,
        )
    };
}

impl Drop for WaveOut {
    fn drop(&mut self) {
        unsafe { waveOutReset(self.handle) };
        for buffer in &mut self.buffers {
            unsafe {
                waveOutUnprepareHeader(
                    self.handle,
                    &raw mut *buffer.header,
                    mem::size_of::<WAVEHDR>() as u32,
                )
            };
        }
        unsafe { waveOutClose(self.handle) };
        unsafe { CloseHandle(self.done_event) };
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct EventHandle(HANDLE);

impl EventHandle {
    fn create() -> windows::core::Result<Self> {
        let handle = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if handle.is_null() {
            Err(windows::core::Error::from_win32())
        } else {
            Ok(Self(handle))
        }
    }

    fn wasapi(&self) -> WasapiHandle {
        WasapiHandle(self.0)
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct StereoI16Ring {
    data: Box<[i16]>,
    read_frame: usize,
    frames: usize,
}

impl StereoI16Ring {
    fn new(capacity_frames: usize) -> Self {
        Self {
            data: vec![0; capacity_frames * 2].into_boxed_slice(),
            read_frame: 0,
            frames: 0,
        }
    }

    fn capacity_frames(&self) -> usize {
        self.data.len() / 2
    }

    fn free_frames(&self) -> usize {
        self.capacity_frames() - self.frames
    }

    fn clear(&mut self) {
        self.read_frame = 0;
        self.frames = 0;
    }

    fn push_planar(&mut self, left: &[f32], right: &[f32], frames: usize) -> bool {
        let frames = frames.min(left.len()).min(right.len());
        if frames > self.free_frames() {
            return false;
        }
        let capacity = self.capacity_frames();
        let write_frame = (self.read_frame + self.frames) % capacity;
        let first = frames.min(capacity - write_frame);
        interleave_planar_stereo(
            &left[..first],
            &right[..first],
            &mut self.data[write_frame * 2..(write_frame + first) * 2],
            first,
        );
        let second = frames - first;
        if second != 0 {
            interleave_planar_stereo(
                &left[first..frames],
                &right[first..frames],
                &mut self.data[..second * 2],
                second,
            );
        }
        self.frames += frames;
        true
    }

    unsafe fn copy_front_to(&self, output: *mut i16, frames: usize) {
        debug_assert!(frames <= self.frames);
        let capacity = self.capacity_frames();
        let first = frames.min(capacity - self.read_frame);
        unsafe {
            ptr::copy_nonoverlapping(
                self.data.as_ptr().add(self.read_frame * 2),
                output,
                first * 2,
            )
        };
        let second = frames - first;
        if second != 0 {
            unsafe {
                ptr::copy_nonoverlapping(self.data.as_ptr(), output.add(first * 2), second * 2)
            };
        }
    }

    fn consume(&mut self, frames: usize) {
        debug_assert!(frames <= self.frames);
        self.read_frame = (self.read_frame + frames) % self.capacity_frames();
        self.frames -= frames;
    }
}

struct WasapiOutput {
    render_client: IAudioRenderClient,
    audio_client: IAudioClient,
    event: EventHandle,
    _apartment: ComApartment,
    ring: StereoI16Ring,
    buffer_frames: u32,
    stream_latency_100ns: i64,
    writable_hint: usize,
    started: bool,
    failed: bool,
}

impl WasapiOutput {
    fn open() -> windows::core::Result<Self> {
        let apartment = ComApartment::initialize()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let endpoint = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole)? };
        let audio_client: IAudioClient = unsafe { endpoint.Activate(CLSCTX_ALL, None)? };
        let format = WasapiWaveFormat {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 2,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        unsafe { audio_client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, &format, None)? };
        let event = EventHandle::create()?;
        unsafe { audio_client.SetEventHandle(event.wasapi())? };
        let buffer_frames = unsafe { audio_client.GetBufferSize()? };
        if buffer_frames == 0 {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x8000_4005u32 as i32),
                "WASAPI returned an empty endpoint buffer",
            ));
        }
        let stream_latency_100ns = unsafe { audio_client.GetStreamLatency()? };
        let render_client: IAudioRenderClient = unsafe { audio_client.GetService()? };
        let ring_capacity = (buffer_frames as usize + FRAMES_PER_BUFFER * 2).next_power_of_two();
        let mut output = Self {
            render_client,
            audio_client,
            event,
            _apartment: apartment,
            ring: StereoI16Ring::new(ring_capacity),
            buffer_frames,
            stream_latency_100ns,
            writable_hint: 0,
            started: false,
            failed: false,
        };
        output.start()?;
        Ok(output)
    }

    fn start(&mut self) -> windows::core::Result<()> {
        if self.started {
            return Ok(());
        }
        let buffer = unsafe { self.render_client.GetBuffer(self.buffer_frames)? };
        unsafe {
            self.render_client
                .ReleaseBuffer(self.buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
            self.audio_client.Start()?;
        }
        debug_assert!(!buffer.is_null());
        self.started = true;
        Ok(())
    }

    fn suspend(&mut self) {
        if self.started {
            let _ = unsafe { self.audio_client.Stop() };
            let _ = unsafe { self.audio_client.Reset() };
        }
        self.started = false;
        self.writable_hint = 0;
        self.ring.clear();
    }

    fn resume(&mut self) {
        if !self.started && self.start().is_err() {
            self.failed = true;
        }
    }

    fn flush_frames(&mut self, frames: usize) -> windows::core::Result<usize> {
        let frames = frames.min(self.ring.frames);
        if frames == 0 {
            return Ok(0);
        }
        let buffer = unsafe { self.render_client.GetBuffer(frames as u32)? };
        unsafe { self.ring.copy_front_to(buffer.cast(), frames) };
        unsafe { self.render_client.ReleaseBuffer(frames as u32, 0)? };
        self.ring.consume(frames);
        Ok(frames)
    }

    fn requested_packets(&mut self) -> Option<usize> {
        if self.failed || !self.started {
            return None;
        }
        let padding = match unsafe { self.audio_client.GetCurrentPadding() } {
            Ok(value) => value.min(self.buffer_frames),
            Err(_) => {
                self.failed = true;
                return None;
            }
        };
        let available = (self.buffer_frames - padding) as usize;
        let flushed = match self.flush_frames(available) {
            Ok(value) => value,
            Err(_) => {
                self.failed = true;
                return None;
            }
        };
        self.writable_hint = available - flushed;
        let by_endpoint = self.writable_hint.div_ceil(FRAMES_PER_BUFFER);
        let by_ring = self.ring.free_frames() / FRAMES_PER_BUFFER;
        Some(by_endpoint.min(by_ring))
    }

    fn write_planar(&mut self, left: &[f32], right: &[f32], frames: usize) -> bool {
        if self.failed || !self.ring.push_planar(left, right, frames) {
            return false;
        }
        let writable = self.writable_hint.min(self.ring.frames);
        if writable != 0 {
            match self.flush_frames(writable) {
                Ok(written) => self.writable_hint -= written,
                Err(_) => {
                    self.failed = true;
                    return false;
                }
            }
        }
        true
    }

    fn wait_for_completion(&self) {
        unsafe { WaitForSingleObject(self.event.0, 20) };
    }

    fn latency_milliseconds(&self) -> f64 {
        self.stream_latency_100ns as f64 / 10_000.0
    }

    fn buffer_milliseconds(&self) -> f64 {
        self.buffer_frames as f64 * 1000.0 / SAMPLE_RATE as f64
    }
}

impl Drop for WasapiOutput {
    fn drop(&mut self) {
        if self.started {
            let _ = unsafe { self.audio_client.Stop() };
        }
    }
}

enum AudioOutput {
    Wasapi(WasapiOutput),
    WaveOut(WaveOut),
}

impl AudioOutput {
    fn open() -> Option<Self> {
        match WasapiOutput::open() {
            Ok(output) => {
                eprintln!(
                    "kinakaze: PipeWire output uses WASAPI shared event mode ({:.2} ms / {} frames endpoint buffer, {:.2} ms engine latency)",
                    output.buffer_milliseconds(),
                    output.buffer_frames,
                    output.latency_milliseconds(),
                );
                Some(Self::Wasapi(output))
            }
            Err(error) => {
                eprintln!("kinakaze: WASAPI unavailable ({error}); falling back to waveOut");
                WaveOut::open().map(Self::WaveOut)
            }
        }
    }

    fn requested_packets(&mut self) -> Option<usize> {
        match self {
            Self::Wasapi(output) => output.requested_packets(),
            Self::WaveOut(output) => {
                output.reap();
                Some(PLAYBACK_QUEUE_BUFFERS.saturating_sub(output.queued_count()))
            }
        }
    }

    fn write_planar(&mut self, left: &[f32], right: &[f32], frames: usize) -> bool {
        match self {
            Self::Wasapi(output) => output.write_planar(left, right, frames),
            Self::WaveOut(output) => output.write_planar(left, right, frames),
        }
    }

    fn wait_for_completion(&self) {
        match self {
            Self::Wasapi(output) => output.wait_for_completion(),
            Self::WaveOut(output) => output.wait_for_completion(),
        }
    }

    fn suspend(&mut self) {
        match self {
            Self::Wasapi(output) => output.suspend(),
            Self::WaveOut(output) => output.suspend(),
        }
    }

    fn resume(&mut self) {
        if let Self::Wasapi(output) = self {
            output.resume();
        }
    }
}

#[repr(C)]
pub struct pw_stream {
    sync: Arc<LoopLock>,
    core: *mut pw_core,
    props: *mut pw_properties,
    events: *const PwStreamEvents,
    event_data: *mut c_void,
    state: AtomicI32,
    direction: AtomicI32,
    active: AtomicBool,
    stopped: AtomicBool,
    buffer: Mutex<Box<AudioBuffer>>,
    rate_match: SpaIoRateMatch,
    audio: Mutex<Option<AudioOutput>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

unsafe impl Send for pw_stream {}
unsafe impl Sync for pw_stream {}

fn initialize_guest_thread_tls() {
    kinakaze_tls::initialize_thread_tls().expect("audio worker ELF TLS initialization");
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_loop_new")]
pub unsafe extern "sysv64" fn pw_loop_new(properties: *const SpaDict) -> *mut c_void {
    let audio = unsafe { pw_thread_loop_new(ptr::null(), properties) };
    let public = pw_thread_loop_get_loop(audio);
    if public.is_null() {
        unsafe { pw_thread_loop_destroy(audio) };
    }
    public
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_loop_destroy")]
pub unsafe extern "sysv64" fn pw_loop_destroy(loop_: *mut c_void) {
    if let Some(audio) = main_loop::audio_for(loop_) {
        unsafe { pw_thread_loop_destroy(audio) };
    }
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_get_node_id")]
pub unsafe extern "sysv64" fn pw_stream_get_node_id(_stream: *mut pw_stream) -> c_uint {
    // Audio is hosted directly by WASAPI; there is no PipeWire server node to
    // share with another process. PW_ID_ANY is the API's unregistered value.
    c_uint::MAX
}
#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_update_params")]
pub unsafe extern "sysv64" fn pw_stream_update_params(
    stream: *mut pw_stream,
    params: *const *const c_void,
    count: c_uint,
) -> c_int {
    if stream.is_null() || (count != 0 && params.is_null()) {
        return -22;
    }
    // No changes is valid; the fixed native audio format cannot negotiate SPA
    // buffer/format changes or DMA-BUF video frames.
    if count == 0 { 0 } else { -95 }
}

unsafe fn invoke_stream_state(stream: *mut pw_stream, old: c_int, state: c_int) {
    let sync = unsafe { Arc::clone(&(*stream).sync) };
    let _guard = sync.guard();
    if !unsafe { (*stream).events }.is_null()
        && let Some(callback) = unsafe { (*(*stream).events).state_changed }
    {
        unsafe { callback((*stream).event_data, old, state, ptr::null()) };
    }
}

fn ensure_stream_worker(stream: *mut pw_stream) {
    if stream.is_null() || lock(unsafe { &(*stream).worker }).is_some() {
        return;
    }
    let address = stream as usize;
    let Ok(handle) = std::thread::Builder::new()
        .name("kinakaze-pipewire".into())
        .spawn(move || {
            initialize_guest_thread_tls();
            let stream = address as *mut pw_stream;
            let mut was_active = false;
            let mut mmcss = None;
            loop {
                let instance = unsafe { &*stream };
                if instance.stopped.load(Ordering::Acquire) {
                    break;
                }
                if !instance.active.load(Ordering::Acquire) {
                    if was_active {
                        if let Some(output) = lock(&instance.audio).as_mut() {
                            output.suspend();
                        }
                        was_active = false;
                        mmcss = None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                let output = instance.direction.load(Ordering::Acquire) == PW_DIRECTION_OUTPUT;
                if mmcss.is_none() {
                    mmcss = MmcssRegistration::audio();
                }
                if output {
                    let mut audio = lock(&instance.audio);
                    if audio.is_none() {
                        *audio = AudioOutput::open();
                    }
                    if !was_active && let Some(device) = audio.as_mut() {
                        device.resume();
                    }
                }
                was_active = true;
                if !output {
                    let mut buffer = lock(&instance.buffer);
                    for plane in &mut buffer.planes {
                        plane.fill(0.0);
                    }
                    for chunk in &mut buffer.chunks {
                        chunk.offset = 0;
                        chunk.size = (FRAMES_PER_BUFFER * 4) as u32;
                    }
                }
                let requested = if output {
                    let mut audio = lock(&instance.audio);
                    match audio.as_mut().and_then(AudioOutput::requested_packets) {
                        Some(requested) => requested,
                        None => {
                            *audio = None;
                            0
                        }
                    }
                } else {
                    1
                };
                for _ in 0..requested {
                    let Some(_guard) = instance.sync.callback(&instance.stopped) else {
                        break;
                    };
                    if !instance.stopped.load(Ordering::Acquire)
                        && !instance.events.is_null()
                        && let Some(process) = unsafe { (*instance.events).process }
                    {
                        unsafe { process(instance.event_data) };
                    }
                }
                if output {
                    let audio = lock(&instance.audio);
                    if let Some(device) = audio.as_ref() {
                        device.wait_for_completion();
                    } else {
                        drop(audio);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                } else {
                    std::thread::sleep(Duration::from_micros(10_667));
                }
            }
            // WASAPI COM objects and the apartment that owns them must be
            // released on this worker, before its COM apartment is torn down.
            drop(lock(unsafe { &(*stream).audio }).take());
            drop(mmcss);
        })
    else {
        return;
    };
    *lock(unsafe { &(*stream).worker }) = Some(handle);
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_get_library_version")]
pub extern "sysv64" fn pw_get_library_version() -> *const c_char {
    c"99.0.0-kinakaze".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_init")]
pub extern "sysv64" fn pw_init(_argc: *mut c_int, _argv: *mut *mut *mut c_char) {}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_new")]
pub unsafe extern "sysv64" fn pw_thread_loop_new(
    _name: *const c_char,
    _props: *const SpaDict,
) -> *mut pw_thread_loop {
    Box::into_raw(Box::new(pw_thread_loop {
        sync: Arc::new(LoopLock::default()),
        started: AtomicBool::new(false),
        core: Mutex::new(Vec::new()),
        native_loop: AtomicUsize::new(0),
        loop_worker: Mutex::new(None),
        stopped: AtomicBool::new(true),
    }))
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_destroy")]
pub unsafe extern "sysv64" fn pw_thread_loop_destroy(loop_: *mut pw_thread_loop) {
    if !loop_.is_null() {
        pw_thread_loop_stop(loop_);
        let native = unsafe { (*loop_).native_loop.swap(0, Ordering::AcqRel) };
        if native != 0 {
            drop(unsafe { Box::from_raw(native as *mut main_loop::HostLoop) });
        }
        drop(unsafe { Box::from_raw(loop_) });
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_get_loop")]
pub extern "sysv64" fn pw_thread_loop_get_loop(loop_: *mut pw_thread_loop) -> *mut c_void {
    let Some(audio) = (unsafe { loop_.as_ref() }) else {
        return ptr::null_mut();
    };
    let current = audio.native_loop.load(Ordering::Acquire);
    if current != 0 {
        return current as _;
    }
    let native = match main_loop::HostLoop::create(loop_) {
        Ok(pointer) => pointer,
        Err(error) => {
            kinakaze_tls::set_errno(error);
            return ptr::null_mut();
        }
    };
    match audio.native_loop.compare_exchange(
        0,
        native as usize,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => native.cast(),
        Err(current) => {
            drop(unsafe { Box::from_raw(native) });
            current as _
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_start")]
pub unsafe extern "sysv64" fn pw_thread_loop_start(loop_: *mut pw_thread_loop) -> c_int {
    if loop_.is_null() {
        return -22;
    }
    let audio = unsafe { &*loop_ };
    let mut worker = lock(&audio.loop_worker);
    if worker.is_some() {
        return 0;
    }
    let native = pw_thread_loop_get_loop(loop_);
    if native.is_null() {
        return -kinakaze_tls::errno();
    }
    audio.sync.set_running(true);
    audio.stopped.store(false, Ordering::Release);
    audio.started.store(true, Ordering::Release);
    schedule_core(loop_);
    let pointer = native as usize;
    match std::thread::Builder::new()
        .name("pipewire-loop".into())
        .spawn(move || {
            initialize_guest_thread_tls();
            unsafe { main_loop::run_audio(pointer as _) };
        }) {
        Ok(handle) => {
            *worker = Some(handle);
            0
        }
        Err(_) => {
            audio.stopped.store(true, Ordering::Release);
            audio.started.store(false, Ordering::Release);
            audio.sync.set_running(false);
            -11
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_stop")]
pub extern "sysv64" fn pw_thread_loop_stop(loop_: *mut pw_thread_loop) {
    if let Some(audio) = unsafe { loop_.as_ref() } {
        audio.stopped.store(true, Ordering::Release);
        audio.sync.wake();
        let pointer = audio.native_loop.load(Ordering::Acquire);
        if pointer != 0 {
            unsafe { &*(pointer as *const main_loop::HostLoop) }.wake();
        }
        if let Some(worker) = lock(&audio.loop_worker).take() {
            let _ = worker.join();
        }
        audio.sync.set_running(false);
        audio.started.store(false, Ordering::Release);
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_lock")]
pub extern "sysv64" fn pw_thread_loop_lock(loop_: *mut pw_thread_loop) {
    unsafe { (*loop_).sync.lock() };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_unlock")]
pub extern "sysv64" fn pw_thread_loop_unlock(loop_: *mut pw_thread_loop) {
    unsafe { (*loop_).sync.unlock() };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_wait")]
pub extern "sysv64" fn pw_thread_loop_wait(loop_: *mut pw_thread_loop) {
    unsafe { (*loop_).sync.wait() };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_signal")]
pub extern "sysv64" fn pw_thread_loop_signal(loop_: *mut pw_thread_loop, wait_for_accept: bool) {
    unsafe { (*loop_).sync.signal(wait_for_accept) };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_thread_loop_accept")]
pub extern "sysv64" fn pw_thread_loop_accept(loop_: *mut pw_thread_loop) {
    unsafe { (*loop_).sync.accept(true) };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_context_new")]
pub unsafe extern "sysv64" fn pw_context_new(
    loop_: *mut c_void,
    props: *mut pw_properties,
    _user_data_size: usize,
) -> *mut pw_context {
    let Some(audio) = main_loop::audio_for(loop_) else {
        unsafe { free_properties(props) };
        kinakaze_tls::set_errno(22);
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(pw_context {
        loop_: audio,
        props,
    }))
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_context_destroy")]
pub unsafe extern "sysv64" fn pw_context_destroy(context: *mut pw_context) {
    if context.is_null() {
        return;
    }
    let context = unsafe { Box::from_raw(context) };
    unsafe { free_properties(context.props) };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_context_connect")]
pub unsafe extern "sysv64" fn pw_context_connect(
    context: *mut pw_context,
    props: *mut pw_properties,
    _user_data_size: usize,
) -> *mut pw_core {
    if context.is_null() {
        unsafe { free_properties(props) };
        return ptr::null_mut();
    }
    let loop_ = unsafe { (*context).loop_ };
    let props = if props.is_null() {
        Box::into_raw(Properties::new()).cast()
    } else {
        props
    };
    static NEXT_CORE: AtomicUsize = AtomicUsize::new(1);
    let serial = NEXT_CORE.fetch_add(1, Ordering::Relaxed);
    let mut core = Box::new(pw_core {
        interface: make_interface(
            c"PipeWire:Interface:Core".as_ptr(),
            4,
            (&raw const CORE_METHODS).cast(),
            ptr::null_mut(),
        ),
        loop_,
        props,
        listeners: hooks::Hooks::new(),
        registry: ptr::null_mut(),
        sequence: 0,
        serial,
        pending_sync: Default::default(),
        client: ptr::null_mut(),
        permissions: vec![client::Permission {
            id: u32::MAX,
            permissions: client::ALL,
        }],
    });
    let address = (&raw mut *core).cast::<c_void>();
    core.interface.callbacks.data = address;
    let core = Box::into_raw(core);
    lock(unsafe { &(*loop_).core }).push((core as usize, serial));
    core
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_core_disconnect")]
pub unsafe extern "sysv64" fn pw_core_disconnect(core: *mut pw_core) -> c_int {
    if core.is_null() {
        return 0;
    }
    let core = unsafe { Box::from_raw(core) };
    if !core.loop_.is_null() {
        let mut current = lock(unsafe { &(*core.loop_).core });
        current.retain(|&(pointer, serial)| {
            pointer != (&raw const *core) as usize || serial != core.serial
        });
    }
    if !core.registry.is_null() {
        unsafe { pw_proxy_destroy(core.registry.cast()) };
    }
    if !core.client.is_null() {
        unsafe { client::destroy(core.client) };
    }
    unsafe { free_properties(core.props) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_proxy_destroy")]
pub unsafe extern "sysv64" fn pw_proxy_destroy(proxy: *mut c_void) {
    if proxy.is_null() {
        return;
    }
    match unsafe { (*proxy.cast::<ProxyHeader>()).kind } {
        PROXY_REGISTRY => {
            let registry = unsafe { Box::from_raw(proxy.cast::<pw_registry>()) };
            if !registry.core.is_null()
                && unsafe { (*registry.core).registry } == &raw const *registry as *mut _
            {
                unsafe { (*registry.core).registry = ptr::null_mut() };
            }
            unsafe { free_properties(registry.props) };
        }
        PROXY_CLIENT => {
            unsafe { client::destroy(proxy.cast()) };
        }
        PROXY_NODE => {
            let node = unsafe { Box::from_raw(proxy.cast::<pw_node>()) };
            unsafe { free_properties(node.props) };
        }
        _ => {}
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_proxy_add_object_listener")]
pub unsafe extern "sysv64" fn pw_proxy_add_object_listener(
    proxy: *mut c_void,
    listener: *mut SpaHook,
    events: *const c_void,
    data: *mut c_void,
) {
    if proxy.is_null() {
        return;
    }
    match unsafe { (*proxy.cast::<ProxyHeader>()).kind } {
        PROXY_REGISTRY => {
            unsafe { registry_add_listener(proxy, listener, events.cast(), data) };
        }
        PROXY_NODE => {
            unsafe { node_add_listener(proxy, listener, events.cast(), data) };
        }
        PROXY_CLIENT => {
            unsafe { client::object_listener(proxy, listener, events, data) };
        }
        _ => {}
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_proxy_get_user_data")]
pub unsafe extern "sysv64" fn pw_proxy_get_user_data(proxy: *mut c_void) -> *mut c_void {
    if proxy.is_null() {
        return ptr::null_mut();
    }
    let data = unsafe { &mut (*proxy.cast::<ProxyHeader>()).user_data };
    if data.is_empty() {
        ptr::null_mut()
    } else {
        data.as_mut_ptr().cast()
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_properties_new")]
pub unsafe extern "sysv64" fn pw_properties_new(
    key1: *const c_char,
    value1: *const c_char,
    key2: *const c_char,
    value2: *const c_char,
    key3: *const c_char,
    value3: *const c_char,
    key4: *const c_char,
    value4: *const c_char,
    key5: *const c_char,
    value5: *const c_char,
    key6: *const c_char,
    value6: *const c_char,
    terminator: *const c_char,
) -> *mut pw_properties {
    let mut props = Properties::new();
    let entries = [
        (key1, value1),
        (key2, value2),
        (key3, value3),
        (key4, value4),
        (key5, value5),
        (key6, value6),
    ];
    for (key, value) in entries {
        if key.is_null() {
            break;
        }
        if value.is_null() {
            break;
        }
        props.set(unsafe { CStr::from_ptr(key) }, unsafe {
            CStr::from_ptr(value)
        });
    }
    let _ = terminator;
    Box::into_raw(props).cast()
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_properties_free")]
pub unsafe extern "sysv64" fn pw_properties_free(props: *mut pw_properties) {
    unsafe { free_properties(props) };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_properties_set")]
pub unsafe extern "sysv64" fn pw_properties_set(
    props: *mut pw_properties,
    key: *const c_char,
    value: *const c_char,
) -> c_int {
    if props.is_null() || key.is_null() {
        return -22;
    }
    if value.is_null() {
        return 0;
    }
    unsafe { properties_mut(props) }.set(unsafe { CStr::from_ptr(key) }, unsafe {
        CStr::from_ptr(value)
    });
    1
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_properties_setf")]
pub unsafe extern "sysv64" fn pw_properties_setf(
    props: *mut pw_properties,
    key: *const c_char,
    format: *const c_char,
    first: usize,
    second: usize,
) -> c_int {
    if props.is_null() || key.is_null() || format.is_null() {
        return -22;
    }
    let format = unsafe { CStr::from_ptr(format) }.to_bytes();
    let text = if format.windows(5).any(|part| part == b"%u/%u") {
        format!("{first}/{second}")
    } else {
        first.to_string()
    };
    let Ok(text) = CString::new(text) else {
        return -22;
    };
    unsafe { properties_mut(props) }.set(unsafe { CStr::from_ptr(key) }, &text);
    1
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_new")]
pub unsafe extern "sysv64" fn pw_stream_new(
    core: *mut pw_core,
    _name: *const c_char,
    props: *mut pw_properties,
) -> *mut pw_stream {
    if core.is_null() {
        unsafe { free_properties(props) };
        return ptr::null_mut();
    }
    Box::into_raw(Box::new(pw_stream {
        sync: unsafe { Arc::clone(&(*(*core).loop_).sync) },
        core,
        props,
        events: ptr::null(),
        event_data: ptr::null_mut(),
        state: AtomicI32::new(PW_STREAM_STATE_UNCONNECTED),
        direction: AtomicI32::new(PW_DIRECTION_OUTPUT),
        active: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        buffer: Mutex::new(AudioBuffer::new(2)),
        rate_match: SpaIoRateMatch {
            delay: 0,
            size: FRAMES_PER_BUFFER as u32,
            rate: 1.0,
            flags: 1,
            delay_fraction: 0,
            padding: [0; 6],
        },
        audio: Mutex::new(None),
        worker: Mutex::new(None),
    }))
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_add_listener")]
pub unsafe extern "sysv64" fn pw_stream_add_listener(
    stream: *mut pw_stream,
    _listener: *mut SpaHook,
    events: *const PwStreamEvents,
    data: *mut c_void,
) {
    if !stream.is_null() {
        unsafe {
            (*stream).events = events;
            (*stream).event_data = data;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_connect")]
pub unsafe extern "sysv64" fn pw_stream_connect(
    stream: *mut pw_stream,
    direction: c_int,
    _target_id: c_uint,
    _flags: c_uint,
    _params: *const *const c_void,
    _n_params: c_uint,
) -> c_int {
    if stream.is_null() {
        return -22;
    }
    let instance = unsafe { &*stream };
    instance.direction.store(direction, Ordering::Release);
    let old = instance
        .state
        .swap(PW_STREAM_STATE_PAUSED, Ordering::AcqRel);
    if !instance.events.is_null()
        && let Some(io_changed) = unsafe { (*instance.events).io_changed }
    {
        unsafe {
            io_changed(
                instance.event_data,
                SPA_IO_RATE_MATCH,
                (&raw const instance.rate_match).cast_mut().cast(),
                mem::size_of::<SpaIoRateMatch>() as c_uint,
            )
        };
    }
    unsafe { invoke_stream_state(stream, old, PW_STREAM_STATE_PAUSED) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_get_state")]
pub unsafe extern "sysv64" fn pw_stream_get_state(
    stream: *mut pw_stream,
    error: *mut *const c_char,
) -> c_int {
    if !error.is_null() {
        unsafe { error.write(ptr::null()) };
    }
    if stream.is_null() {
        -1
    } else {
        unsafe { (*stream).state.load(Ordering::Acquire) }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_set_active")]
pub unsafe extern "sysv64" fn pw_stream_set_active(stream: *mut pw_stream, active: bool) -> c_int {
    if stream.is_null() {
        return -22;
    }
    let instance = unsafe { &*stream };
    instance.active.store(active, Ordering::Release);
    let next = if active {
        PW_STREAM_STATE_STREAMING
    } else {
        PW_STREAM_STATE_PAUSED
    };
    let old = instance.state.swap(next, Ordering::AcqRel);
    unsafe { invoke_stream_state(stream, old, next) };
    if active {
        ensure_stream_worker(stream);
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_dequeue_buffer")]
pub unsafe extern "sysv64" fn pw_stream_dequeue_buffer(stream: *mut pw_stream) -> *mut pw_buffer {
    if stream.is_null() {
        return ptr::null_mut();
    }
    let mut buffer = lock(unsafe { &(*stream).buffer });
    buffer.public.requested = FRAMES_PER_BUFFER as u64;
    &raw mut buffer.public
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_queue_buffer")]
pub unsafe extern "sysv64" fn pw_stream_queue_buffer(
    stream: *mut pw_stream,
    _public: *mut pw_buffer,
) -> c_int {
    if stream.is_null() {
        return -22;
    }
    let instance = unsafe { &*stream };
    if instance.direction.load(Ordering::Acquire) != PW_DIRECTION_OUTPUT {
        return 0;
    }
    let buffer = lock(&instance.buffer);
    if buffer.datas.is_empty() {
        return 0;
    }
    let frames = buffer
        .chunks
        .iter()
        .map(|chunk| (chunk.size as usize) / mem::size_of::<f32>())
        .min()
        .unwrap_or(0)
        .min(FRAMES_PER_BUFFER);
    let left = &buffer.planes[0];
    let right = &buffer.planes[1.min(buffer.planes.len() - 1)];
    let mut audio = lock(&instance.audio);
    if let Some(audio) = audio.as_mut() {
        audio.write_planar(left, right, frames);
    }
    0
}

#[repr(C)]
struct SpaFraction {
    numerator: c_uint,
    denominator: c_uint,
}

#[repr(C)]
struct PwTime {
    now: i64,
    rate: SpaFraction,
    ticks: u64,
    delay: i64,
    queued: u64,
    buffered: u64,
    queued_buffers: c_uint,
    available_buffers: c_uint,
    size: u64,
}

fn monotonic_nanoseconds() -> i64 {
    let mut counter = 0i64;
    unsafe { QueryPerformanceCounter(&raw mut counter) };
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 0i64;
        unsafe { QueryPerformanceFrequency(&raw mut value) };
        value
    });
    if frequency <= 0 {
        0
    } else {
        ((counter as i128 * 1_000_000_000i128) / frequency as i128) as i64
    }
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_get_time_n")]
pub unsafe extern "sysv64" fn pw_stream_get_time_n(
    _stream: *mut pw_stream,
    output: *mut c_void,
    output_size: usize,
) -> c_int {
    if output.is_null() {
        return -22;
    }
    let value = PwTime {
        now: monotonic_nanoseconds(),
        rate: SpaFraction {
            numerator: 1,
            denominator: SAMPLE_RATE,
        },
        ticks: 0,
        delay: FRAMES_PER_BUFFER as i64,
        queued: 0,
        buffered: 0,
        queued_buffers: 0,
        available_buffers: 2,
        size: FRAMES_PER_BUFFER as u64,
    };
    unsafe {
        ptr::copy_nonoverlapping(
            (&raw const value).cast::<u8>(),
            output.cast::<u8>(),
            output_size.min(mem::size_of::<PwTime>()),
        )
    };
    0
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_pw_stream_destroy")]
pub unsafe extern "sysv64" fn pw_stream_destroy(stream: *mut pw_stream) {
    if stream.is_null() {
        return;
    }
    let stream = unsafe { Box::from_raw(stream) };
    stream.active.store(false, Ordering::Release);
    stream.stopped.store(true, Ordering::Release);
    stream.sync.wake();
    if let Some(worker) = lock(&stream.worker).take() {
        let _ = worker.join();
    }
    unsafe { free_properties(stream.props) };
}

#[unsafe(export_name = "kinakaze_engine_libpipewire_spa_pod_get_array")]
pub unsafe extern "sysv64" fn spa_pod_get_array(
    _pod: *const c_void,
    count: *mut c_uint,
) -> *const c_void {
    if !count.is_null() {
        unsafe { count.write(0) };
    }
    ptr::null()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn properties_have_pipewire_dictionary_layout() {
        let props = make_node_properties();
        let dict = unsafe { &(*props).dict };
        assert_eq!(dict.n_items, 4);
        let entries = unsafe { slice::from_raw_parts(dict.items, dict.n_items as usize) };
        assert!(entries.iter().any(|entry| unsafe {
            CStr::from_ptr(entry.key) == c"media.class"
                && CStr::from_ptr(entry.value) == c"Audio/Sink"
        }));
        unsafe { free_properties(props) };
    }

    #[test]
    fn audio_buffer_exposes_two_planar_channels() {
        let mut buffer = AudioBuffer::new(2);
        assert_eq!(buffer.spa.n_datas, 2);
        assert_eq!(buffer.public.buffer, &raw mut buffer.spa);
        assert_eq!(buffer.datas[0].max_size as usize, FRAMES_PER_BUFFER * 4);
    }

    #[test]
    fn simd_interleave_matches_scalar_clamping_and_nan_policy() {
        let left = [
            -2.0,
            -1.0,
            -0.75,
            -0.5,
            -0.1,
            0.0,
            0.1,
            0.5,
            0.75,
            1.0,
            2.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.25,
            -0.25,
            0.9,
        ];
        let mut right = left;
        right.reverse();
        let mut scalar = [0i16; 34];
        let mut simd = [0i16; 34];
        interleave_planar_stereo_scalar(&left, &right, &mut scalar, left.len());
        interleave_planar_stereo(&left, &right, &mut simd, left.len());
        assert_eq!(simd, scalar);
    }

    #[test]
    fn stereo_ring_preserves_frames_across_wraparound() {
        let first_left = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let first_right = [-0.1, -0.2, -0.3, -0.4, -0.5, -0.6];
        let second_left = [0.7, 0.8, 0.9, 1.0, -1.0];
        let second_right = [-0.7, -0.8, -0.9, -1.0, 1.0];
        let mut ring = StereoI16Ring::new(8);
        assert!(ring.push_planar(&first_left, &first_right, first_left.len()));
        ring.consume(5);
        assert!(ring.push_planar(&second_left, &second_right, second_left.len()));

        let expected_left = [0.6, 0.7, 0.8, 0.9, 1.0, -1.0];
        let expected_right = [-0.6, -0.7, -0.8, -0.9, -1.0, 1.0];
        let mut expected = [0i16; 12];
        interleave_planar_stereo_scalar(
            &expected_left,
            &expected_right,
            &mut expected,
            expected_left.len(),
        );
        let mut actual = [0i16; 12];
        unsafe { ring.copy_front_to(actual.as_mut_ptr(), expected_left.len()) };
        assert_eq!(actual, expected);
    }

    #[test]
    #[ignore = "diagnostic microbenchmark"]
    fn benchmark_planar_conversion() {
        let left: Vec<f32> = (0..FRAMES_PER_BUFFER)
            .map(|index| ((index as f32 * 0.03125).sin() * 1.1).clamp(-1.1, 1.1))
            .collect();
        let right: Vec<f32> = left.iter().rev().copied().collect();
        let mut scalar = vec![0i16; FRAMES_PER_BUFFER * 2];
        let mut simd = scalar.clone();
        const ITERATIONS: usize = 200_000;

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            interleave_planar_stereo_scalar(
                black_box(&left),
                black_box(&right),
                black_box(&mut scalar),
                FRAMES_PER_BUFFER,
            );
        }
        let scalar_time = start.elapsed();

        let start = Instant::now();
        for _ in 0..ITERATIONS {
            interleave_planar_stereo(
                black_box(&left),
                black_box(&right),
                black_box(&mut simd),
                FRAMES_PER_BUFFER,
            );
        }
        let simd_time = start.elapsed();
        assert_eq!(simd, scalar);
        eprintln!(
            "512-frame planar conversion: scalar {:.1} ns, SIMD {:.1} ns, {:.2}x",
            scalar_time.as_nanos() as f64 / ITERATIONS as f64,
            simd_time.as_nanos() as f64 / ITERATIONS as f64,
            scalar_time.as_secs_f64() / simd_time.as_secs_f64(),
        );
    }

    #[test]
    #[ignore = "requires a Windows playback endpoint"]
    fn prepared_packet_pool_reuses_completed_headers() {
        let mut output = WaveOut::open().expect("waveOut playback endpoint");
        let silence = [0.0f32; FRAMES_PER_BUFFER];
        for _ in 0..PLAYBACK_QUEUE_BUFFERS {
            assert!(output.write_planar(&silence, &silence, FRAMES_PER_BUFFER,));
        }
        assert!(!output.write_planar(&silence, &silence, FRAMES_PER_BUFFER,));
        let deadline = Instant::now() + Duration::from_secs(1);
        while output.queued_count() == PLAYBACK_QUEUE_BUFFERS && Instant::now() < deadline {
            output.wait_for_completion();
            output.reap();
        }
        assert!(output.queued_count() < PLAYBACK_QUEUE_BUFFERS);
        assert!(output.write_planar(&silence, &silence, FRAMES_PER_BUFFER,));
    }

    #[test]
    #[ignore = "requires a Windows WASAPI playback endpoint"]
    fn wasapi_shared_event_stream_refills() {
        let mut output = WasapiOutput::open().expect("WASAPI shared playback endpoint");
        let silence = [0.0f32; FRAMES_PER_BUFFER];
        let deadline = Instant::now() + Duration::from_secs(2);
        let start = Instant::now();
        let mut wakeups = 0usize;
        let mut packets = 0usize;
        while wakeups < 8 && Instant::now() < deadline {
            output.wait_for_completion();
            let requested = output
                .requested_packets()
                .expect("healthy WASAPI render stream");
            if requested == 0 {
                continue;
            }
            wakeups += 1;
            for _ in 0..requested {
                assert!(output.write_planar(&silence, &silence, FRAMES_PER_BUFFER));
                packets += 1;
            }
            if wakeups == 4 {
                output.suspend();
                assert!(!output.started);
                output.resume();
                assert!(output.started && !output.failed);
            }
        }
        assert_eq!(wakeups, 8, "WASAPI endpoint stopped signalling");
        eprintln!(
            "WASAPI: {:.2} ms / {} frames endpoint buffer, {:.2} ms engine latency, {} source packets over {:.1} ms",
            output.buffer_milliseconds(),
            output.buffer_frames,
            output.latency_milliseconds(),
            packets,
            start.elapsed().as_secs_f64() * 1000.0,
        );
    }
}

mod object_layout;
