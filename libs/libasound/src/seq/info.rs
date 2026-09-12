//! Opaque ALSA containers; callers obtain sizes through the matching ABI.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    pub client: u8,
    pub port: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClientInfo {
    pub client: i32,
    pub kind: i32,
    pub name: [u8; 64],
    pub ports: i32,
}
impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            client: 0,
            kind: 1,
            name: [0; 64],
            ports: 0,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PortInfo {
    pub addr: Address,
    pub client: i32,
    pub port: i32,
    pub name: [u8; 64],
    pub capability: u32,
    pub kind: u32,
    pub midi_channels: i32,
    pub timestamping: i32,
    pub timestamp_real: i32,
    pub timestamp_queue: i32,
}
impl Default for PortInfo {
    fn default() -> Self {
        Self {
            addr: Address::default(),
            client: 0,
            port: 0,
            name: [0; 64],
            capability: 0,
            kind: 0,
            midi_channels: 16,
            timestamping: 0,
            timestamp_real: 0,
            timestamp_queue: 0,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Subscription {
    pub sender: Address,
    pub dest: Address,
    pub queue: i32,
    pub exclusive: i32,
    pub time_update: i32,
    pub time_real: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tempo {
    pub tempo: u32,
    pub ppq: i32,
}
impl Default for Tempo {
    fn default() -> Self {
        Self {
            tempo: 500_000,
            ppq: 96,
        }
    }
}

macro_rules! container {
    ($ty:ty, $size:ident, $malloc:ident, $free:ident, $copy:ident) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($size)))]
        pub extern "sysv64" fn $size() -> usize {
            size_of::<$ty>()
        }
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($malloc)))]
        pub unsafe extern "sysv64" fn $malloc(out: *mut *mut $ty) -> i32 {
            if out.is_null() {
                return -22;
            }
            unsafe { params::allocate_params(out, <$ty>::default()) }
        }
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($free)))]
        pub unsafe extern "sysv64" fn $free(value: *mut $ty) {
            unsafe { kinakaze_alloc::guest::free(value.cast()) };
        }
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($copy)))]
        pub unsafe extern "sysv64" fn $copy(to: *mut $ty, from: *const $ty) {
            if !to.is_null() && !from.is_null() {
                unsafe { *to = *from };
            }
        }
    };
}
container!(
    ClientInfo,
    snd_seq_client_info_sizeof,
    snd_seq_client_info_malloc,
    snd_seq_client_info_free,
    snd_seq_client_info_copy
);
container!(
    PortInfo,
    snd_seq_port_info_sizeof,
    snd_seq_port_info_malloc,
    snd_seq_port_info_free,
    snd_seq_port_info_copy
);
container!(
    Subscription,
    snd_seq_port_subscribe_sizeof,
    snd_seq_port_subscribe_malloc,
    snd_seq_port_subscribe_free,
    snd_seq_port_subscribe_copy
);
container!(
    Tempo,
    snd_seq_queue_tempo_sizeof,
    snd_seq_queue_tempo_malloc,
    snd_seq_queue_tempo_free,
    snd_seq_queue_tempo_copy
);
macro_rules! get {
    ($name:ident,$ty:ty,$field:ident,$result:ty) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(info: *const $ty) -> $result {
            if info.is_null() {
                0
            } else {
                unsafe { (*info).$field }
            }
        }
    };
}
macro_rules! set {
    ($name:ident,$ty:ty,$field:ident,$value:ty) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(info: *mut $ty, value: $value) {
            if !info.is_null() {
                unsafe { (*info).$field = value };
            }
        }
    };
}
get!(snd_seq_client_info_get_client, ClientInfo, client, i32);
get!(snd_seq_client_info_get_type, ClientInfo, kind, i32);
get!(snd_seq_client_info_get_num_ports, ClientInfo, ports, i32);
set!(snd_seq_client_info_set_client, ClientInfo, client, i32);
get!(snd_seq_port_info_get_client, PortInfo, client, i32);
get!(snd_seq_port_info_get_port, PortInfo, port, i32);
get!(snd_seq_port_info_get_capability, PortInfo, capability, u32);
get!(snd_seq_port_info_get_type, PortInfo, kind, u32);
get!(
    snd_seq_port_info_get_midi_channels,
    PortInfo,
    midi_channels,
    i32
);
set!(snd_seq_port_info_set_capability, PortInfo, capability, u32);
set!(snd_seq_port_info_set_type, PortInfo, kind, u32);
set!(
    snd_seq_port_info_set_midi_channels,
    PortInfo,
    midi_channels,
    i32
);
set!(
    snd_seq_port_info_set_timestamping,
    PortInfo,
    timestamping,
    i32
);
set!(
    snd_seq_port_info_set_timestamp_real,
    PortInfo,
    timestamp_real,
    i32
);
set!(
    snd_seq_port_info_set_timestamp_queue,
    PortInfo,
    timestamp_queue,
    i32
);
set!(
    snd_seq_port_subscribe_set_time_real,
    Subscription,
    time_real,
    i32
);
set!(
    snd_seq_port_subscribe_set_time_update,
    Subscription,
    time_update,
    i32
);
set!(
    snd_seq_port_subscribe_set_exclusive,
    Subscription,
    exclusive,
    i32
);
set!(snd_seq_port_subscribe_set_queue, Subscription, queue, i32);
set!(snd_seq_queue_tempo_set_tempo, Tempo, tempo, u32);
set!(snd_seq_queue_tempo_set_ppq, Tempo, ppq, i32);
get!(snd_seq_queue_tempo_get_tempo, Tempo, tempo, u32);
get!(snd_seq_queue_tempo_get_ppq, Tempo, ppq, i32);

pub(super) fn name_bytes(out: &mut [u8; 64], name: &[u8]) {
    out.fill(0);
    let count = name.len().min(63);
    out[..count].copy_from_slice(&name[..count]);
}
macro_rules! names {
    ($ty:ty,$getter:ident,$setter:ident) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($getter)))]
        pub unsafe extern "sysv64" fn $getter(info: *const $ty) -> *const c_char {
            if info.is_null() {
                ptr::null()
            } else {
                unsafe { (&raw const (*info).name).cast() }
            }
        }
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($setter)))]
        pub unsafe extern "sysv64" fn $setter(info: *mut $ty, name: *const c_char) {
            if !info.is_null() && !name.is_null() {
                unsafe { name_bytes(&mut (*info).name, CStr::from_ptr(name).to_bytes()) };
            }
        }
    };
}
names!(
    ClientInfo,
    snd_seq_client_info_get_name,
    snd_seq_client_info_set_name
);
names!(
    PortInfo,
    snd_seq_port_info_get_name,
    snd_seq_port_info_set_name
);
macro_rules! addr_field {
    ($field:ident,$getter:ident,$setter:ident) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($getter)))]
        pub unsafe extern "sysv64" fn $getter(info: *const Subscription) -> *const Address {
            if info.is_null() {
                ptr::null()
            } else {
                unsafe { &raw const (*info).$field }
            }
        }
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($setter)))]
        pub unsafe extern "sysv64" fn $setter(info: *mut Subscription, value: *const Address) {
            if !info.is_null() && !value.is_null() {
                unsafe { (*info).$field = *value };
            }
        }
    };
}
addr_field!(
    sender,
    snd_seq_port_subscribe_get_sender,
    snd_seq_port_subscribe_set_sender
);
addr_field!(
    dest,
    snd_seq_port_subscribe_get_dest,
    snd_seq_port_subscribe_set_dest
);
#[unsafe(export_name = "kinakaze_engine_libasound_snd_seq_port_info_get_addr")]
pub unsafe extern "sysv64" fn snd_seq_port_info_get_addr(info: *const PortInfo) -> *const Address {
    if info.is_null() {
        ptr::null()
    } else {
        unsafe { &raw const (*info).addr }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_seq_port_info_set_client")]
pub unsafe extern "sysv64" fn snd_seq_port_info_set_client(info: *mut PortInfo, value: i32) {
    if !info.is_null() {
        unsafe {
            (*info).client = value;
            (*info).addr.client = value as u8;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_seq_port_info_set_port")]
pub unsafe extern "sysv64" fn snd_seq_port_info_set_port(info: *mut PortInfo, value: i32) {
    if !info.is_null() {
        unsafe {
            (*info).port = value;
            (*info).addr.port = value as u8;
        }
    }
}
