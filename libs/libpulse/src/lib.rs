//! `libpulse.so.0` compatibility provider for hosted Linux applications.
//!
//! The PulseAudio client state machine is implemented in-process and its PCM
//! streams are sent to the native Windows waveOut device.  This intentionally
//! does not require a PulseAudio daemon: from the guest's point of view it is a
//! local playback endpoint. Recording and server introspection still have
//! incomplete legacy paths; native playback does not imply native capture.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

mod channelmap;
mod loop_lock;
mod mainloop;
mod threaded;
pub use mainloop::*;
pub use threaded::*;
mod mixer_math;
mod proplist;
mod volume;
pub use proplist::*;
mod introspection;
pub use channelmap::*;
use loop_lock::LoopLock;
use std::thread::JoinHandle;
use std::time::Duration;
pub use volume::*;
use windows_sys::Win32::Media::Audio::waveOutSetVolume;

const PA_OK: c_int = 0;
const PA_CONTEXT_UNCONNECTED: c_int = 0;
const PA_CONTEXT_READY: c_int = 4;
const PA_CONTEXT_TERMINATED: c_int = 6;
const PA_STREAM_UNCONNECTED: c_int = 0;
const PA_STREAM_READY: c_int = 2;
const PA_STREAM_TERMINATED: c_int = 4;
const PA_OPERATION_DONE: c_int = 1;
const PA_SAMPLE_U8: c_int = 0;
const PA_SAMPLE_S16LE: c_int = 3;
const PA_SAMPLE_FLOAT32LE: c_int = 5;
const PA_SAMPLE_S32LE: c_int = 7;
const PA_SAMPLE_S24LE: c_int = 9;
const PA_SAMPLE_S24_32LE: c_int = 11;
const PA_STREAM_START_CORKED: c_int = 0x0001;
const MMSYSERR_NOERROR: u32 = 0;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

static SINK_NAME: &[u8] = b"kinakaze.output\0";
static SINK_DESCRIPTION: &[u8] = b"Kinakaze PulseAudio Output\0";
static SOURCE_NAME: &[u8] = b"kinakaze.input\0";
static SOURCE_DESCRIPTION: &[u8] = b"Kinakaze PulseAudio Input\0";
static SERVER_NAME: &[u8] = b"kinakaze-pulse\0";
static SERVER_VERSION: &[u8] = b"17.0-kinakaze\0";
static USER_NAME: &[u8] = b"kinakaze\0";
static HOST_NAME: &[u8] = b"windows\0";
static ERROR_OK: &[u8] = b"OK\0";
static ERROR_GENERIC: &[u8] = b"PulseAudio compatibility error\0";

mod timing;
mod waveout;
use waveout::WaveOut;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct pa_sample_spec {
    pub format: c_int,
    pub rate: u32,
    pub channels: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct pa_channel_map {
    pub channels: u8,
    pub map: [c_int; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct pa_buffer_attr {
    pub maxlength: u32,
    pub tlength: u32,
    pub prebuf: u32,
    pub minreq: u32,
    pub fragsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pa_cvolume {
    pub channels: u8,
    pub values: [u32; 32],
}

#[repr(C)]
pub struct pa_sink_port_info {
    pub name: *const c_char,
    pub description: *const c_char,
    pub priority: u32,
    pub available: c_int,
    pub availability_group: *const c_char,
    pub kind: u32,
}

#[repr(C)]
pub struct pa_sink_info {
    pub name: *const c_char,
    pub index: u32,
    pub description: *const c_char,
    pub sample_spec: pa_sample_spec,
    pub channel_map: pa_channel_map,
    pub owner_module: u32,
    pub volume: pa_cvolume,
    pub mute: c_int,
    pub monitor_source: u32,
    pub monitor_source_name: *const c_char,
    pub latency: u64,
    pub driver: *const c_char,
    pub flags: c_int,
    pub proplist: *mut c_void,
    pub configured_latency: u64,
    pub base_volume: u32,
    pub state: c_int,
    pub n_volume_steps: u32,
    pub card: u32,
    pub n_ports: u32,
    pub ports: *mut *mut pa_sink_port_info,
    pub active_port: *mut pa_sink_port_info,
    pub n_formats: u8,
    pub formats: *mut *mut c_void,
}

#[repr(C)]
pub struct pa_source_info {
    pub name: *const c_char,
    pub index: u32,
    pub description: *const c_char,
    pub sample_spec: pa_sample_spec,
    pub channel_map: pa_channel_map,
}

#[repr(C)]
pub struct pa_server_info {
    pub user_name: *const c_char,
    pub host_name: *const c_char,
    pub server_version: *const c_char,
    pub server_name: *const c_char,
    pub sample_spec: pa_sample_spec,
    pub default_sink_name: *const c_char,
    pub default_source_name: *const c_char,
    pub cookie: u32,
    pub channel_map: pa_channel_map,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct pa_sink_input_info {
    pub index: u32,
    pub name: *const c_char,
    pub owner_module: u32,
    pub client: u32,
    pub sink: u32,
    pub sample_spec: pa_sample_spec,
    pub channel_map: pa_channel_map,
    pub volume: pa_cvolume,
    pub buffer_usec: u64,
    pub sink_usec: u64,
    pub resample_method: *const c_char,
    pub driver: *const c_char,
    pub mute: c_int,
    pub proplist: *mut c_void,
    pub corked: c_int,
    pub has_volume: c_int,
    pub volume_writable: c_int,
    pub format: *mut c_void,
}

pub type ContextNotify = unsafe extern "sysv64" fn(*mut pa_context, *mut c_void);
pub type ContextSuccess = unsafe extern "sysv64" fn(*mut pa_context, c_int, *mut c_void);
pub type SubscribeNotify = unsafe extern "sysv64" fn(*mut pa_context, c_int, u32, *mut c_void);
pub type SinkInfoNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const pa_sink_info, c_int, *mut c_void);
pub type SourceInfoNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const pa_source_info, c_int, *mut c_void);
pub type ServerInfoNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const pa_server_info, *mut c_void);
pub type SinkInputInfoNotify =
    unsafe extern "sysv64" fn(*mut pa_context, *const pa_sink_input_info, c_int, *mut c_void);
pub type StreamNotify = unsafe extern "sysv64" fn(*mut pa_stream, *mut c_void);
pub type StreamRequest = unsafe extern "sysv64" fn(*mut pa_stream, usize, *mut c_void);
pub type StreamSuccess = unsafe extern "sysv64" fn(*mut pa_stream, c_int, *mut c_void);
pub type FreeCallback = unsafe extern "sysv64" fn(*mut c_void);

fn mainloop_apis() -> &'static Mutex<HashMap<usize, Arc<LoopLock>>> {
    static APIS: OnceLock<Mutex<HashMap<usize, Arc<LoopLock>>>> = OnceLock::new();
    APIS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[repr(C)]
pub struct pa_context {
    refs: AtomicUsize,
    pending: AtomicUsize,
    api: *mut pa_mainloop_api,
    name: std::ffi::CString,
    properties: pa_proplist,
    sync: Option<Arc<LoopLock>>,
    state: AtomicI32,
    error: AtomicI32,
    state_cb: AtomicUsize,
    state_userdata: AtomicUsize,
    subscribe_cb: AtomicUsize,
    subscribe_userdata: AtomicUsize,
    subscribe_mask: AtomicI32,
    restore_cb: AtomicUsize,
    restore_userdata: AtomicUsize,
}

#[repr(C)]
pub struct pa_operation {
    refs: AtomicUsize,
    state: AtomicI32,
}

#[repr(C)]
pub struct pa_stream {
    refs: AtomicUsize,
    sync: Option<Arc<LoopLock>>,
    context: *mut pa_context,
    state: AtomicI32,
    spec: pa_sample_spec,
    channel_map: pa_channel_map,
    attr: pa_buffer_attr,
    playback: AtomicBool,
    corked: AtomicBool,
    stopped: AtomicBool,
    state_cb: AtomicUsize,
    state_userdata: AtomicUsize,
    write_cb: AtomicUsize,
    write_userdata: AtomicUsize,
    read_cb: AtomicUsize,
    read_userdata: AtomicUsize,
    moved_cb: AtomicUsize,
    moved_userdata: AtomicUsize,
    attr_cb: AtomicUsize,
    attr_userdata: AtomicUsize,
    underflow_cb: AtomicUsize,
    underflow_userdata: AtomicUsize,
    overflow_cb: AtomicUsize,
    overflow_userdata: AtomicUsize,
    latency_update_cb: AtomicUsize,
    latency_update_userdata: AtomicUsize,
    suspended_cb: AtomicUsize,
    suspended_userdata: AtomicUsize,
    audio: Mutex<Option<WaveOut>>,
    pending_write: Mutex<Option<usize>>,
    capture_packet: Mutex<Option<Box<[u8]>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

unsafe impl Send for pa_stream {}
unsafe impl Sync for pa_stream {}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn default_spec() -> pa_sample_spec {
    pa_sample_spec {
        format: PA_SAMPLE_S16LE,
        rate: 48_000,
        channels: 2,
    }
}

fn default_map(channels: u8) -> pa_channel_map {
    let mut map = pa_channel_map {
        channels,
        map: [0; 32],
    };
    let standard: &[c_int] = match channels {
        1 => &[0],
        2 => &[1, 2],
        3 => &[1, 2, 3],
        4 => &[1, 2, 5, 6],
        5 => &[1, 2, 3, 5, 6],
        6 => &[1, 2, 3, 7, 5, 6],
        7 => &[1, 2, 3, 7, 4, 10, 11],
        8 => &[1, 2, 3, 7, 5, 6, 10, 11],
        _ => &[],
    };
    if standard.is_empty() {
        for index in 0..usize::from(channels.min(32)) {
            map.map[index] = 12 + index as c_int;
        }
    } else {
        map.map[..standard.len()].copy_from_slice(standard);
    }
    map
}

fn default_attr(spec: pa_sample_spec, requested: Option<pa_buffer_attr>) -> pa_buffer_attr {
    let frame = frame_size_value(spec).max(1) as u32;
    let fallback = (spec.rate / 100).max(64).saturating_mul(frame);
    let mut attr = requested.unwrap_or(pa_buffer_attr {
        maxlength: u32::MAX,
        tlength: fallback.saturating_mul(4),
        prebuf: 0,
        minreq: fallback,
        fragsize: fallback,
    });
    if attr.minreq == u32::MAX || attr.minreq == 0 {
        attr.minreq = fallback;
    }
    if attr.fragsize == u32::MAX || attr.fragsize == 0 {
        attr.fragsize = fallback;
    }
    if attr.tlength == u32::MAX || attr.tlength < attr.minreq {
        attr.tlength = attr.minreq.saturating_mul(4);
    }
    attr.tlength = attr.tlength.clamp(attr.minreq, 1024 * 1024);
    attr
}

fn sample_size(format: c_int) -> usize {
    match format {
        PA_SAMPLE_U8 => 1,
        PA_SAMPLE_S16LE => 2,
        PA_SAMPLE_S24LE => 3,
        PA_SAMPLE_FLOAT32LE | PA_SAMPLE_S32LE | PA_SAMPLE_S24_32LE => 4,
        _ => 0,
    }
}

fn frame_size_value(spec: pa_sample_spec) -> usize {
    sample_size(spec.format).saturating_mul(usize::from(spec.channels))
}

fn operation_done() -> *mut pa_operation {
    let operation = Box::into_raw(Box::new(pa_operation {
        refs: AtomicUsize::new(1),
        state: AtomicI32::new(PA_OPERATION_DONE),
    }));
    operation
}

unsafe fn callback_from_usize<T: Copy>(value: usize) -> Option<T> {
    if value == 0 {
        None
    } else {
        Some(unsafe { ptr::read((&value as *const usize).cast::<T>()) })
    }
}

fn initialize_guest_thread_tls() {
    kinakaze_tls::initialize_thread_tls().expect("audio worker ELF TLS initialization");
}

fn ensure_worker(stream: *mut pa_stream) {
    if stream.is_null() || lock(unsafe { &(*stream).worker }).is_some() {
        return;
    }
    if unsafe {
        (*stream).write_cb.load(Ordering::Acquire) == 0
            && (*stream).read_cb.load(Ordering::Acquire) == 0
    } {
        return;
    }
    let address = stream as usize;
    let Ok(handle) = std::thread::Builder::new()
        .name("kinakaze-pulse".into())
        .spawn(move || {
            initialize_guest_thread_tls();
            let stream = address as *mut pa_stream;
            loop {
                let instance = unsafe { &*stream };
                if instance.stopped.load(Ordering::Acquire) {
                    break;
                }
                if instance.corked.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                let callback_guard = if let Some(sync) = instance.sync.as_ref() {
                    let Some(guard) = sync.callback(&instance.stopped) else {
                        break;
                    };
                    Some(guard)
                } else {
                    None
                };
                let (callback, userdata, bytes) = if instance.playback.load(Ordering::Acquire) {
                    (
                        instance.write_cb.load(Ordering::Acquire),
                        instance.write_userdata.load(Ordering::Acquire),
                        instance.attr.minreq.max(64) as usize,
                    )
                } else {
                    (
                        instance.read_cb.load(Ordering::Acquire),
                        instance.read_userdata.load(Ordering::Acquire),
                        instance.attr.fragsize.max(64) as usize,
                    )
                };
                if let Some(callback) = unsafe { callback_from_usize::<StreamRequest>(callback) } {
                    if !instance.stopped.load(Ordering::Acquire) {
                        unsafe { callback(stream, bytes, userdata as *mut c_void) };
                    }
                }
                let per_second = instance.spec.rate as usize * frame_size_value(instance.spec);
                let micros = if per_second == 0 {
                    10_000
                } else {
                    ((bytes.saturating_mul(1_000_000) / per_second).clamp(2_000, 50_000)) as u64
                };
                drop(callback_guard);
                std::thread::sleep(Duration::from_micros(micros));
            }
        })
    else {
        return;
    };
    *lock(unsafe { &(*stream).worker }) = Some(handle);
}

fn stop_worker(stream: &mut pa_stream) {
    stream.stopped.store(true, Ordering::Release);
    if let Some(sync) = stream.sync.as_ref() {
        sync.wake();
    }
    if let Some(handle) = lock(&stream.worker).take() {
        let _ = handle.join();
    }
}

unsafe fn invoke_context_state(context: *mut pa_context) {
    let sync = unsafe { (*context).sync.clone() };
    let _guard = sync.as_ref().map(|sync| sync.guard());
    let callback = unsafe { (*context).state_cb.load(Ordering::Acquire) };
    let userdata = unsafe { (*context).state_userdata.load(Ordering::Acquire) };
    if let Some(callback) = unsafe { callback_from_usize::<ContextNotify>(callback) } {
        unsafe { callback(context, userdata as *mut c_void) };
    }
}

unsafe fn invoke_stream_state(stream: *mut pa_stream) {
    let sync = unsafe { (*stream).sync.clone() };
    let _guard = sync.as_ref().map(|sync| sync.guard());
    let callback = unsafe { (*stream).state_cb.load(Ordering::Acquire) };
    let userdata = unsafe { (*stream).state_userdata.load(Ordering::Acquire) };
    if let Some(callback) = unsafe { callback_from_usize::<StreamNotify>(callback) } {
        unsafe { callback(stream, userdata as *mut c_void) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_new")]
pub unsafe extern "sysv64" fn pa_context_new(
    api: *mut c_void,
    name: *const c_char,
) -> *mut pa_context {
    let context = Box::into_raw(Box::new(pa_context {
        refs: AtomicUsize::new(1),
        pending: AtomicUsize::new(0),
        api: api.cast(),
        name: introspection::context_name(name),
        properties: pa_proplist::default(),
        sync: lock(mainloop_apis()).get(&(api as usize)).cloned(),
        state: AtomicI32::new(PA_CONTEXT_UNCONNECTED),
        error: AtomicI32::new(PA_OK),
        state_cb: AtomicUsize::new(0),
        state_userdata: AtomicUsize::new(0),
        subscribe_cb: AtomicUsize::new(0),
        subscribe_userdata: AtomicUsize::new(0),
        subscribe_mask: AtomicI32::new(0),
        restore_cb: AtomicUsize::new(0),
        restore_userdata: AtomicUsize::new(0),
    }));
    context
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_unref")]
pub unsafe extern "sysv64" fn pa_context_unref(context: *mut pa_context) {
    if !context.is_null() && unsafe { (*context).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        unsafe { drop(Box::from_raw(context)) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_connect")]
pub unsafe extern "sysv64" fn pa_context_connect(
    context: *mut pa_context,
    _server: *const c_char,
    _flags: c_int,
    _api: *const c_void,
) -> c_int {
    if context.is_null() {
        return -1;
    }
    unsafe { (*context).state.store(1, Ordering::Release) };
    let op = unsafe {
        introspection::enqueue(context, |context| {
            (*context).state.store(PA_CONTEXT_READY, Ordering::Release);
            invoke_context_state(context);
        })
    };
    if op.is_null() {
        return -1;
    }
    unsafe { pa_operation_unref(op) };
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_disconnect")]
pub unsafe extern "sysv64" fn pa_context_disconnect(context: *mut pa_context) {
    if !context.is_null() {
        unsafe {
            (*context)
                .state
                .store(PA_CONTEXT_TERMINATED, Ordering::Release)
        };
        unsafe { invoke_context_state(context) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_state")]
pub unsafe extern "sysv64" fn pa_context_get_state(context: *const pa_context) -> c_int {
    if context.is_null() {
        PA_CONTEXT_TERMINATED
    } else {
        unsafe { (*context).state.load(Ordering::Acquire) }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_errno")]
pub unsafe extern "sysv64" fn pa_context_errno(context: *const pa_context) -> c_int {
    if context.is_null() {
        1
    } else {
        unsafe { (*context).error.load(Ordering::Acquire) }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_state_callback")]
pub unsafe extern "sysv64" fn pa_context_set_state_callback(
    context: *mut pa_context,
    callback: Option<ContextNotify>,
    userdata: *mut c_void,
) {
    if !context.is_null() {
        unsafe {
            (*context)
                .state_userdata
                .store(userdata as usize, Ordering::Release);
            (*context).state_cb.store(
                callback.map_or(0, |value| value as usize),
                Ordering::Release,
            );
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_subscribe_callback")]
pub unsafe extern "sysv64" fn pa_context_set_subscribe_callback(
    context: *mut pa_context,
    callback: Option<SubscribeNotify>,
    userdata: *mut c_void,
) {
    if !context.is_null() {
        unsafe {
            (*context)
                .subscribe_userdata
                .store(userdata as usize, Ordering::Release);
            (*context).subscribe_cb.store(
                callback.map_or(0, |value| value as usize),
                Ordering::Release,
            );
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_subscribe")]
pub unsafe extern "sysv64" fn pa_context_subscribe(
    context: *mut pa_context,
    mask: c_int,
    callback: Option<ContextSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    if context.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        (*context).subscribe_mask.store(mask, Ordering::Release);
        introspection::success(context, callback, userdata, 0)
    }
}

fn sink_info_value() -> pa_sink_info {
    pa_sink_info {
        name: SINK_NAME.as_ptr().cast(),
        index: 0,
        description: SINK_DESCRIPTION.as_ptr().cast(),
        sample_spec: default_spec(),
        channel_map: default_map(2),
        owner_module: u32::MAX,
        volume: *lock(sink_channels()),
        mute: GLOBAL_SINK_MUTE.load(Ordering::Acquire),
        monitor_source: u32::MAX,
        monitor_source_name: ptr::null(),
        latency: 0,
        driver: SERVER_NAME.as_ptr().cast(),
        flags: 0x0001 | 0x0020 | 0x0040,
        proplist: ptr::null_mut(),
        configured_latency: 0,
        base_volume: 0x1_0000,
        state: 0,
        n_volume_steps: 65_537,
        card: u32::MAX,
        n_ports: 0,
        ports: ptr::null_mut(),
        active_port: ptr::null_mut(),
        n_formats: 0,
        formats: ptr::null_mut(),
    }
}

fn source_info_value() -> pa_source_info {
    pa_source_info {
        name: SOURCE_NAME.as_ptr().cast(),
        index: 0,
        description: SOURCE_DESCRIPTION.as_ptr().cast(),
        sample_spec: default_spec(),
        channel_map: default_map(2),
    }
}

unsafe fn notify_sink(
    context: *mut pa_context,
    callback: Option<SinkInfoNotify>,
    userdata: *mut c_void,
) {
    let sync = unsafe { context.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if let Some(callback) = callback {
        let mut info = sink_info_value();
        let mut properties = pa_proplist::default();
        info.proplist = (&raw mut properties).cast();
        unsafe {
            callback(context, &raw const info, 0, userdata);
            callback(context, ptr::null(), 1, userdata);
        }
    }
}

unsafe fn notify_source(
    context: *mut pa_context,
    callback: Option<SourceInfoNotify>,
    userdata: *mut c_void,
) {
    let sync = unsafe { context.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if let Some(callback) = callback {
        unsafe {
            callback(context, ptr::null(), 1, userdata);
        }
    }
}

macro_rules! sink_query {
    ($name:ident $(, $extra:ident : $type:ty)*) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libpulse_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            context: *mut pa_context,
            $($extra: $type,)*
            callback: Option<SinkInfoNotify>,
            userdata: *mut c_void,
        ) -> *mut pa_operation {
            $(let _ = $extra;)*
            unsafe { introspection::enqueue(context, move |context| notify_sink(context, callback, userdata)) }
        }
    };
}

sink_query!(pa_context_get_sink_info_list);
sink_query!(pa_context_get_sink_info_by_name, name: *const c_char);
sink_query!(pa_context_get_sink_info_by_index, index: u32);

macro_rules! source_query {
    ($name:ident $(, $extra:ident : $type:ty)*) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libpulse_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            context: *mut pa_context,
            $($extra: $type,)*
            callback: Option<SourceInfoNotify>,
            userdata: *mut c_void,
        ) -> *mut pa_operation {
            $(let _ = $extra;)*
            unsafe { introspection::enqueue(context, move |context| notify_source(context, callback, userdata)) }
        }
    };
}

source_query!(pa_context_get_source_info_list);
source_query!(pa_context_get_source_info_by_name, name: *const c_char);
source_query!(pa_context_get_source_info_by_index, index: u32);

static GLOBAL_SINK_VOLUME: AtomicU32 = AtomicU32::new(0x1_0000);
static GLOBAL_SINK_MUTE: AtomicI32 = AtomicI32::new(0);

fn sink_channels() -> &'static Mutex<pa_cvolume> {
    static VOLUME: OnceLock<Mutex<pa_cvolume>> = OnceLock::new();
    VOLUME.get_or_init(|| {
        Mutex::new(pa_cvolume {
            channels: 2,
            values: [0x1_0000; 32],
        })
    })
}

fn apply_hardware_volume() -> bool {
    let mute = GLOBAL_SINK_MUTE.load(Ordering::Acquire);
    let vol = if mute != 0 {
        0
    } else {
        let values = *lock(sink_channels());
        let scale = |v: u32| {
            ((v as f64 / 65536.).powi(3) * 65535.)
                .round()
                .clamp(0., 65535.) as u32
        };
        scale(values.values[0]) | (scale(values.values[1]) << 16)
    };
    unsafe { waveOutSetVolume(core::ptr::null_mut(), vol) == 0 }
}

fn sink_input_info_value() -> pa_sink_input_info {
    let mute = GLOBAL_SINK_MUTE.load(Ordering::Acquire);
    pa_sink_input_info {
        index: 1,
        name: SINK_DESCRIPTION.as_ptr().cast(),
        owner_module: u32::MAX,
        client: 0,
        sink: 0,
        sample_spec: default_spec(),
        channel_map: default_map(2),
        volume: *lock(sink_channels()),
        buffer_usec: 0,
        sink_usec: 0,
        resample_method: ptr::null(),
        driver: SERVER_NAME.as_ptr().cast(),
        mute,
        proplist: ptr::null_mut(),
        corked: 0,
        has_volume: 1,
        volume_writable: 1,
        format: ptr::null_mut(),
    }
}

unsafe fn notify_sink_input(
    context: *mut pa_context,
    callback: Option<SinkInputInfoNotify>,
    userdata: *mut c_void,
) {
    let sync = unsafe { context.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if let Some(callback) = callback {
        let info = sink_input_info_value();
        unsafe {
            callback(context, &raw const info, 0, userdata);
            callback(context, ptr::null(), 1, userdata);
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_sink_input_info")]
pub unsafe extern "sysv64" fn pa_context_get_sink_input_info(
    context: *mut pa_context,
    _idx: u32,
    callback: Option<SinkInputInfoNotify>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    unsafe { notify_sink_input(context, callback, userdata) };
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_sink_input_volume")]
pub unsafe extern "sysv64" fn pa_context_set_sink_input_volume(
    context: *mut pa_context,
    _idx: u32,
    volume: *const pa_cvolume,
    callback: Option<ContextSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    if !volume.is_null() {
        let avg = unsafe { pa_cvolume_avg(volume) };
        GLOBAL_SINK_VOLUME.store(avg, Ordering::Release);
        let mut channels = lock(sink_channels());
        if unsafe { (*volume).channels } == 2 {
            *channels = unsafe { *volume };
        } else {
            channels.values[0] = avg;
            channels.values[1] = avg;
        }
        drop(channels);
        apply_hardware_volume();
    }
    if let Some(callback) = callback {
        unsafe { callback(context, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_set_sink_input_mute")]
pub unsafe extern "sysv64" fn pa_context_set_sink_input_mute(
    context: *mut pa_context,
    _idx: u32,
    mute: c_int,
    callback: Option<ContextSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    GLOBAL_SINK_MUTE.store(mute, Ordering::Release);
    apply_hardware_volume();
    if let Some(callback) = callback {
        unsafe { callback(context, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_context_get_server_info")]
pub unsafe extern "sysv64" fn pa_context_get_server_info(
    context: *mut pa_context,
    callback: Option<ServerInfoNotify>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    unsafe {
        introspection::enqueue(context, move |context| {
            if let Some(callback) = callback {
                let info = pa_server_info {
                    user_name: USER_NAME.as_ptr().cast(),
                    host_name: HOST_NAME.as_ptr().cast(),
                    server_version: SERVER_VERSION.as_ptr().cast(),
                    server_name: SERVER_NAME.as_ptr().cast(),
                    sample_spec: default_spec(),
                    default_sink_name: SINK_NAME.as_ptr().cast(),
                    default_source_name: ptr::null(),
                    cookie: 0x4352_5953,
                    channel_map: default_map(2),
                };
                unsafe { callback(context, &raw const info, userdata) };
            }
        })
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_operation_get_state")]
pub unsafe extern "sysv64" fn pa_operation_get_state(operation: *const pa_operation) -> c_int {
    if operation.is_null() {
        PA_OPERATION_DONE
    } else {
        let state = unsafe { (*operation).state.load(Ordering::Acquire) };
        state
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_operation_unref")]
pub unsafe extern "sysv64" fn pa_operation_unref(operation: *mut pa_operation) {
    if !operation.is_null() && unsafe { (*operation).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        unsafe { drop(Box::from_raw(operation)) };
    }
}

unsafe fn create_stream(
    context: *mut pa_context,
    spec: *const pa_sample_spec,
    map: *const pa_channel_map,
) -> *mut pa_stream {
    if context.is_null() || spec.is_null() || unsafe { pa_sample_spec_valid(spec) } == 0 {
        return ptr::null_mut();
    }
    let spec = unsafe { *spec };
    let channel_map = if map.is_null() {
        default_map(spec.channels)
    } else {
        unsafe { *map }
    };
    let stream = Box::into_raw(Box::new(pa_stream {
        refs: AtomicUsize::new(1),
        sync: unsafe { (*context).sync.clone() },
        context,
        state: AtomicI32::new(PA_STREAM_UNCONNECTED),
        spec,
        channel_map,
        attr: default_attr(spec, None),
        playback: AtomicBool::new(true),
        corked: AtomicBool::new(true),
        stopped: AtomicBool::new(false),
        state_cb: AtomicUsize::new(0),
        state_userdata: AtomicUsize::new(0),
        write_cb: AtomicUsize::new(0),
        write_userdata: AtomicUsize::new(0),
        read_cb: AtomicUsize::new(0),
        read_userdata: AtomicUsize::new(0),
        moved_cb: AtomicUsize::new(0),
        moved_userdata: AtomicUsize::new(0),
        attr_cb: AtomicUsize::new(0),
        attr_userdata: AtomicUsize::new(0),
        underflow_cb: AtomicUsize::new(0),
        underflow_userdata: AtomicUsize::new(0),
        overflow_cb: AtomicUsize::new(0),
        overflow_userdata: AtomicUsize::new(0),
        latency_update_cb: AtomicUsize::new(0),
        latency_update_userdata: AtomicUsize::new(0),
        suspended_cb: AtomicUsize::new(0),
        suspended_userdata: AtomicUsize::new(0),
        audio: Mutex::new(None),
        pending_write: Mutex::new(None),
        capture_packet: Mutex::new(None),
        worker: Mutex::new(None),
    }));
    stream
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_new")]
pub unsafe extern "sysv64" fn pa_stream_new(
    context: *mut pa_context,
    _name: *const c_char,
    spec: *const pa_sample_spec,
    map: *const pa_channel_map,
) -> *mut pa_stream {
    unsafe { create_stream(context, spec, map) }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_new_with_proplist")]
pub unsafe extern "sysv64" fn pa_stream_new_with_proplist(
    context: *mut pa_context,
    _name: *const c_char,
    spec: *const pa_sample_spec,
    map: *const pa_channel_map,
    _proplist: *mut c_void,
) -> *mut pa_stream {
    unsafe { create_stream(context, spec, map) }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_unref")]
pub unsafe extern "sysv64" fn pa_stream_unref(stream: *mut pa_stream) {
    if !stream.is_null() && unsafe { (*stream).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        let mut stream = unsafe { Box::from_raw(stream) };
        stop_worker(&mut stream);
        if let Some(address) = lock(&stream.pending_write).take() {
            unsafe { kinakaze_alloc::guest::free(address as *mut u8) };
        }
        drop(stream);
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_connect_playback")]
pub unsafe extern "sysv64" fn pa_stream_connect_playback(
    stream: *mut pa_stream,
    _device: *const c_char,
    attr: *const pa_buffer_attr,
    flags: c_int,
    _volume: *const c_void,
    _sync_stream: *mut pa_stream,
) -> c_int {
    if stream.is_null() {
        return -1;
    }
    let requested = (!attr.is_null()).then(|| unsafe { *attr });
    unsafe {
        (*stream).attr = default_attr((*stream).spec, requested);
        (*stream).playback.store(true, Ordering::Release);
        (*stream)
            .corked
            .store(flags & PA_STREAM_START_CORKED != 0, Ordering::Release);
        *lock(&(*stream).audio) = WaveOut::open((*stream).spec);
        if lock(&(*stream).audio).is_none() {
            return -1;
        }
        if (*stream).corked.load(Ordering::Acquire)
            && !lock(&(*stream).audio).as_ref().unwrap().pause(true)
        {
            return timing::fail(stream, timing::IO);
        }
        (*stream).state.store(PA_STREAM_READY, Ordering::Release);
        invoke_stream_state(stream);
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_connect_record")]
pub unsafe extern "sysv64" fn pa_stream_connect_record(
    stream: *mut pa_stream,
    _device: *const c_char,
    attr: *const pa_buffer_attr,
    flags: c_int,
) -> c_int {
    if stream.is_null() {
        return -1;
    }
    let requested = (!attr.is_null()).then(|| unsafe { *attr });
    unsafe {
        (*stream).attr = default_attr((*stream).spec, requested);
        (*stream).playback.store(false, Ordering::Release);
        (*stream)
            .corked
            .store(flags & PA_STREAM_START_CORKED != 0, Ordering::Release);
        (*stream).state.store(PA_STREAM_READY, Ordering::Release);
        invoke_stream_state(stream);
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_disconnect")]
pub unsafe extern "sysv64" fn pa_stream_disconnect(stream: *mut pa_stream) -> c_int {
    if stream.is_null() {
        return -1;
    }
    unsafe {
        (*stream).corked.store(true, Ordering::Release);
        (*stream)
            .state
            .store(PA_STREAM_TERMINATED, Ordering::Release);
        invoke_stream_state(stream);
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_state")]
pub unsafe extern "sysv64" fn pa_stream_get_state(stream: *const pa_stream) -> c_int {
    if stream.is_null() {
        PA_STREAM_TERMINATED
    } else {
        let state = unsafe { (*stream).state.load(Ordering::Acquire) };
        state
    }
}

macro_rules! stream_callback_setter {
    ($name:ident, $callback_type:ty, $field:ident, $userdata_field:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libpulse_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            stream: *mut pa_stream,
            callback: Option<$callback_type>,
            userdata: *mut c_void,
        ) {
            if !stream.is_null() {
                unsafe {
                    (*stream)
                        .$userdata_field
                        .store(userdata as usize, Ordering::Release);
                    (*stream).$field.store(
                        callback.map_or(0, |value| value as usize),
                        Ordering::Release,
                    );
                }
            }
        }
    };
}

stream_callback_setter!(
    pa_stream_set_suspended_callback,
    StreamNotify,
    suspended_cb,
    suspended_userdata
);

stream_callback_setter!(
    pa_stream_set_state_callback,
    StreamNotify,
    state_cb,
    state_userdata
);
stream_callback_setter!(
    pa_stream_set_write_callback,
    StreamRequest,
    write_cb,
    write_userdata
);
stream_callback_setter!(
    pa_stream_set_read_callback,
    StreamRequest,
    read_cb,
    read_userdata
);
stream_callback_setter!(
    pa_stream_set_moved_callback,
    StreamNotify,
    moved_cb,
    moved_userdata
);
stream_callback_setter!(
    pa_stream_set_buffer_attr_callback,
    StreamNotify,
    attr_cb,
    attr_userdata
);
stream_callback_setter!(
    pa_stream_set_underflow_callback,
    StreamNotify,
    underflow_cb,
    underflow_userdata
);
stream_callback_setter!(
    pa_stream_set_overflow_callback,
    StreamNotify,
    overflow_cb,
    overflow_userdata
);
stream_callback_setter!(
    pa_stream_set_latency_update_callback,
    StreamNotify,
    latency_update_cb,
    latency_update_userdata
);

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_cork")]
pub unsafe extern "sysv64" fn pa_stream_cork(
    stream: *mut pa_stream,
    cork: c_int,
    callback: Option<StreamSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    let sync = unsafe { stream.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if stream.is_null() {
        return ptr::null_mut();
    }
    let corked = cork != 0;
    unsafe {
        if let Some(audio) = lock(&(*stream).audio).as_ref() {
            if !audio.pause(corked) {
                timing::fail(stream, timing::IO);
                return ptr::null_mut();
            }
        }
        (*stream).corked.store(corked, Ordering::Release);
    }
    if !corked {
        ensure_worker(stream);
    }
    if let Some(callback) = callback {
        unsafe { callback(stream, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_flush")]
pub unsafe extern "sysv64" fn pa_stream_flush(
    stream: *mut pa_stream,
    callback: Option<StreamSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    let sync = unsafe { stream.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if stream.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        if let Some(audio) = lock(&(*stream).audio).as_mut() {
            if !audio.reset() || !audio.pause((*stream).corked.load(Ordering::Acquire)) {
                timing::fail(stream, timing::IO);
                return ptr::null_mut();
            }
        }
    }
    if let Some(callback) = callback {
        unsafe { callback(stream, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_index")]
pub unsafe extern "sysv64" fn pa_stream_get_index(stream: *const pa_stream) -> u32 {
    if stream.is_null() { u32::MAX } else { 1 }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_device_index")]
pub unsafe extern "sysv64" fn pa_stream_get_device_index(stream: *const pa_stream) -> u32 {
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return u32::MAX;
    };
    // Both default device records in introspection use index zero. The device
    // is meaningful only after the connection reaches READY.
    if stream.state.load(Ordering::Acquire) == PA_STREAM_READY {
        0
    } else {
        u32::MAX
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_is_corked")]
pub unsafe extern "sysv64" fn pa_stream_is_corked(stream: *const pa_stream) -> c_int {
    if stream.is_null() || unsafe { (*stream).corked.load(Ordering::Acquire) } {
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_is_suspended")]
pub unsafe extern "sysv64" fn pa_stream_is_suspended(_stream: *const pa_stream) -> c_int {
    0
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_sample_spec")]
pub unsafe extern "sysv64" fn pa_stream_get_sample_spec(
    stream: *const pa_stream,
) -> *const pa_sample_spec {
    if stream.is_null() {
        ptr::null()
    } else {
        unsafe { &raw const (*stream).spec }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_buffer_attr")]
pub unsafe extern "sysv64" fn pa_stream_get_buffer_attr(
    stream: *const pa_stream,
) -> *const pa_buffer_attr {
    if stream.is_null() {
        ptr::null()
    } else {
        unsafe { &raw const (*stream).attr }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_set_buffer_attr")]
pub unsafe extern "sysv64" fn pa_stream_set_buffer_attr(
    stream: *mut pa_stream,
    attr: *const pa_buffer_attr,
    callback: Option<StreamSuccess>,
    userdata: *mut c_void,
) -> *mut pa_operation {
    let sync = unsafe { stream.as_ref() }.and_then(|value| value.sync.clone());
    let _guard = sync.as_ref().map(|sync| sync.guard());
    if stream.is_null() || attr.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*stream).attr = default_attr((*stream).spec, Some(*attr)) };
    let attr_callback = unsafe { (*stream).attr_cb.load(Ordering::Acquire) };
    let attr_userdata = unsafe { (*stream).attr_userdata.load(Ordering::Acquire) };
    if let Some(attr_callback) = unsafe { callback_from_usize::<StreamNotify>(attr_callback) } {
        unsafe { attr_callback(stream, attr_userdata as *mut c_void) };
    }
    if let Some(callback) = callback {
        unsafe { callback(stream, 1, userdata) };
    }
    operation_done()
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_get_device_name")]
pub unsafe extern "sysv64" fn pa_stream_get_device_name(stream: *const pa_stream) -> *const c_char {
    if !stream.is_null() && unsafe { (*stream).playback.load(Ordering::Acquire) } {
        SINK_NAME.as_ptr().cast()
    } else {
        SOURCE_NAME.as_ptr().cast()
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_writable_size")]
pub unsafe extern "sysv64" fn pa_stream_writable_size(stream: *const pa_stream) -> usize {
    if stream.is_null() {
        usize::MAX
    } else {
        unsafe { (*stream).attr.tlength.max((*stream).attr.minreq) as usize }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_begin_write")]
pub unsafe extern "sysv64" fn pa_stream_begin_write(
    stream: *mut pa_stream,
    data: *mut *mut c_void,
    nbytes: *mut usize,
) -> c_int {
    if stream.is_null() || data.is_null() || nbytes.is_null() {
        return -1;
    }
    let wanted = unsafe { (*nbytes).min((*stream).attr.minreq.max(64) as usize) };
    let address = unsafe { kinakaze_alloc::guest::malloc(wanted.max(1)) };
    if address.is_null() {
        return -1;
    }
    if let Some(previous) = lock(unsafe { &(*stream).pending_write }).replace(address as usize) {
        unsafe { kinakaze_alloc::guest::free(previous as *mut u8) };
    }
    unsafe {
        *data = address.cast();
        *nbytes = wanted;
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_cancel_write")]
pub unsafe extern "sysv64" fn pa_stream_cancel_write(stream: *mut pa_stream) -> c_int {
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return -1;
    };
    let Some(address) = lock(&stream.pending_write).take() else {
        return -1;
    };
    unsafe {
        kinakaze_alloc::guest::free(address as *mut u8);
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_write")]
pub unsafe extern "sysv64" fn pa_stream_write(
    stream: *mut pa_stream,
    data: *const c_void,
    nbytes: usize,
    free_callback: Option<FreeCallback>,
    _offset: i64,
    _seek: c_int,
) -> c_int {
    if stream.is_null() || data.is_null() {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), nbytes) };
    let written = lock(unsafe { &(*stream).audio })
        .as_mut()
        .is_some_and(|audio| audio.write(bytes));
    let pending = lock(unsafe { &(*stream).pending_write }).take();
    if pending == Some(data as usize) {
        unsafe { kinakaze_alloc::guest::free(data.cast_mut().cast()) };
    } else if let Some(callback) = free_callback {
        unsafe { callback(data.cast_mut()) };
    }
    if written { PA_OK } else { -1 }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_readable_size")]
pub unsafe extern "sysv64" fn pa_stream_readable_size(stream: *const pa_stream) -> usize {
    if stream.is_null() || unsafe { (*stream).corked.load(Ordering::Acquire) } {
        0
    } else {
        unsafe { (*stream).attr.fragsize as usize }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_peek")]
pub unsafe extern "sysv64" fn pa_stream_peek(
    stream: *mut pa_stream,
    data: *mut *const c_void,
    nbytes: *mut usize,
) -> c_int {
    if stream.is_null() || data.is_null() || nbytes.is_null() {
        return -1;
    }
    let len = unsafe { (*stream).attr.fragsize.max(64) as usize };
    let silent = if unsafe { (*stream).spec.format } == PA_SAMPLE_U8 {
        0x80
    } else {
        0
    };
    let packet = vec![silent; len].into_boxed_slice();
    unsafe {
        *data = packet.as_ptr().cast();
        *nbytes = packet.len();
        *lock(&(*stream).capture_packet) = Some(packet);
    }
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_stream_drop")]
pub unsafe extern "sysv64" fn pa_stream_drop(stream: *mut pa_stream) -> c_int {
    if stream.is_null() {
        return -1;
    }
    lock(unsafe { &(*stream).capture_packet }).take();
    PA_OK
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_frame_size")]
pub unsafe extern "sysv64" fn pa_frame_size(spec: *const pa_sample_spec) -> usize {
    if spec.is_null() {
        0
    } else {
        frame_size_value(unsafe { *spec })
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sample_spec_valid")]
pub unsafe extern "sysv64" fn pa_sample_spec_valid(spec: *const pa_sample_spec) -> c_int {
    if spec.is_null() {
        return 0;
    }
    let spec = unsafe { *spec };
    (sample_size(spec.format) != 0
        && (1..=32).contains(&spec.channels)
        && (1..=768_000).contains(&spec.rate)) as c_int
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_xmalloc")]
pub unsafe extern "sysv64" fn pa_xmalloc(size: usize) -> *mut c_void {
    unsafe { kinakaze_alloc::guest::malloc(size.max(1)).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_xfree")]
pub unsafe extern "sysv64" fn pa_xfree(pointer: *mut c_void) {
    unsafe { kinakaze_alloc::guest::free(pointer.cast()) };
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_strerror")]
pub unsafe extern "sysv64" fn pa_strerror(error: c_int) -> *const c_char {
    if error == PA_OK {
        ERROR_OK.as_ptr().cast()
    } else {
        ERROR_GENERIC.as_ptr().cast()
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_get_binary_name")]
pub unsafe extern "sysv64" fn pa_get_binary_name(
    output: *mut c_char,
    length: usize,
) -> *mut c_char {
    if output.is_null() || length == 0 {
        return ptr::null_mut();
    }
    let name = b"kinakaze";
    let count = name.len().min(length - 1);
    unsafe {
        ptr::copy_nonoverlapping(name.as_ptr(), output.cast(), count);
        *output.add(count) = 0;
    }
    output
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_path_get_filename")]
pub unsafe extern "sysv64" fn pa_path_get_filename(path: *const c_char) -> *const c_char {
    if path.is_null() {
        return ptr::null();
    }
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    let offset = bytes
        .iter()
        .rposition(|byte| matches!(*byte, b'/' | b'\\'))
        .map_or(0, |index| index + 1);
    unsafe { path.add(offset) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_connect_is_deferred_until_mainloop_dispatch() {
        unsafe {
            let mainloop = pa_mainloop_new();
            let context = pa_context_new(pa_mainloop_get_api(mainloop).cast(), ptr::null());
            assert_eq!(pa_context_connect(context, ptr::null(), 0, ptr::null()), 0);
            assert_eq!(pa_context_get_state(context), 1);
            assert!(pa_mainloop_iterate(mainloop, 0, ptr::null_mut()) >= 0);
            assert_eq!(pa_context_get_state(context), PA_CONTEXT_READY);
            let spec = default_spec();
            let stream = pa_stream_new(context, ptr::null(), &spec, ptr::null());
            assert!(!stream.is_null());
            // A headless CI host may not expose waveOut, so capture is used to
            // exercise the common stream state machine without a device.
            assert_eq!(
                pa_stream_connect_record(stream, ptr::null(), ptr::null(), 1),
                0
            );
            assert_eq!(pa_stream_get_state(stream), PA_STREAM_READY);
            let operation = pa_stream_cork(stream, 0, None, ptr::null_mut());
            assert_eq!(pa_operation_get_state(operation), PA_OPERATION_DONE);
            pa_operation_unref(operation);
            pa_stream_unref(stream);
            pa_context_disconnect(context);
            pa_context_unref(context);
            pa_mainloop_free(mainloop);
        }
    }

    #[test]
    fn channel_maps_cover_openal_layouts() {
        let stereo = default_map(2);
        let surround = default_map(8);
        assert_eq!(unsafe { pa_channel_map_superset(&surround, &stereo) }, 1);
        assert_eq!(unsafe { pa_frame_size(&default_spec()) }, 4);
    }
}

mod object_layout;
