//! Sequencer clients, ports, subscriptions and bounded event delivery.
//! WinMM output devices are queried lazily; opening a client makes no sound.
//! Software ports currently route within this process. Inherited handles are
//! rejected in a fork child instead of dereferencing copied native state.
//! ABI: https://www.alsa-project.org/alsa-doc/alsa-lib/group___sequencer.html
use super::*;
use core::{ffi::CStr, ptr};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, OnceLock};
mod info;
use info::*;
mod events;
use events::Packet;
mod schedule;

macro_rules! api {
    ($name:ident($($arg:ident:$ty:ty),*) -> $result:ty $body:block) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libasound_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name($($arg:$ty),*) -> $result { unsafe { $body } }
    }
}
use api;
macro_rules! get_client {
    ($seq:expr) => {
        match client($seq) {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
}
use get_client;

#[repr(C)]
pub struct Seq {
    id: u8,
    owner: u32,
    event: [u8; 28],
    payload: *mut u8,
    capacity: usize,
}
struct Input {
    packets: VecDeque<Packet>,
    bytes: usize,
    overflow: bool,
    capacity: usize,
}
struct Client {
    id: u8,
    streams: i32,
    nonblock: AtomicBool,
    closed: AtomicBool,
    info: Mutex<ClientInfo>,
    ports: Mutex<BTreeMap<u8, PortInfo>>,
    input: Mutex<Input>,
    fd: i32,
    output: Mutex<(VecDeque<Packet>, usize, usize)>,
    native: Mutex<BTreeMap<u8, usize>>,
    schedule: Mutex<schedule::State>,
    wake: Condvar,
    worker: AtomicBool,
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.fd);
        if let Ok(outputs) = self.native.get_mut() {
            for (_, &mut pointer) in outputs.iter_mut() {
                unsafe { rawmidi::snd_rawmidi_close(pointer as *mut rawmidi::Midi) };
            }
        }
    }
}
#[derive(Default)]
struct Bus {
    clients: BTreeMap<u8, Arc<Client>>,
    subscriptions: Vec<Subscription>,
    queues: BTreeMap<u8, u8>,
}
fn bus() -> &'static Mutex<Bus> {
    static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();
    BUS.get_or_init(|| Mutex::new(Bus::default()))
}
unsafe fn client(seq: *mut Seq) -> Result<Arc<Client>, i32> {
    if seq.is_null() {
        return Err(-22);
    }
    if unsafe { (*seq).owner } != std::process::id() {
        return Err(-130);
    }
    bus()
        .lock()
        .map_err(|_| -5)?
        .clients
        .get(&unsafe { (*seq).id })
        .cloned()
        .ok_or(-9)
}
fn device_count() -> u32 {
    unsafe { windows_sys::Win32::Media::Audio::midiOutGetNumDevs() }.min(253)
}
fn port_info(address: Address) -> Result<PortInfo, i32> {
    let mut info = PortInfo {
        addr: address,
        client: address.client as i32,
        port: address.port as i32,
        ..Default::default()
    };
    match address.client {
        0 if address.port == 1 => {
            info.capability = 1 | 32;
            info.kind = 1;
            name_bytes(&mut info.name, b"Announce");
        }
        16 if u32::from(address.port) < device_count() => {
            use windows_sys::Win32::Media::Audio::{MIDIOUTCAPSW, midiOutGetDevCapsW};
            let mut caps = MIDIOUTCAPSW::default();
            if unsafe {
                midiOutGetDevCapsW(
                    address.port as usize,
                    &raw mut caps,
                    size_of::<MIDIOUTCAPSW>() as u32,
                )
            } != 0
            {
                return Err(-19);
            }
            let name = caps.szPname;
            let length = name.iter().position(|&v| v == 0).unwrap_or(name.len());
            name_bytes(
                &mut info.name,
                String::from_utf16_lossy(&name[..length]).as_bytes(),
            );
            info.capability = 2 | 64;
            info.kind = 2 | (1 << 16);
        }
        id => {
            let state = bus()
                .lock()
                .map_err(|_| -5)?
                .clients
                .get(&id)
                .cloned()
                .ok_or(-2)?;
            return state
                .ports
                .lock()
                .map_err(|_| -5)?
                .get(&address.port)
                .copied()
                .ok_or(-2);
        }
    }
    Ok(info)
}
fn client_info(id: i32) -> Result<ClientInfo, i32> {
    let mut info = ClientInfo {
        client: id,
        kind: 2,
        ..Default::default()
    };
    match id {
        0 => {
            name_bytes(&mut info.name, b"System");
            info.ports = 1;
        }
        16 if device_count() != 0 => {
            name_bytes(&mut info.name, b"Windows MIDI");
            info.ports = device_count() as i32;
        }
        128..=252 => {
            let state = bus()
                .lock()
                .map_err(|_| -5)?
                .clients
                .get(&(id as u8))
                .cloned()
                .ok_or(-2)?;
            info = *state.info.lock().map_err(|_| -5)?;
            info.ports = state.ports.lock().map_err(|_| -5)?.len() as i32;
        }
        _ => return Err(-2),
    }
    Ok(info)
}
fn announce(kind: u8, address: Address) {
    let mut event = Packet::empty(kind);
    event.bytes[12] = 0;
    event.bytes[13] = 1;
    event.bytes[14] = 254;
    event.bytes[16] = address.client;
    event.bytes[17] = address.port;
    let _ = events::route(None, event);
}
api!(snd_seq_open(out:*mut *mut Seq,name:*const c_char,streams:i32,mode:i32)->i32 {
    if out.is_null() { return -22; } *out=ptr::null_mut();
    if name.is_null() || !(1..=3).contains(&streams) || mode & !1 !=0 { return -22; }
    if !matches!(CStr::from_ptr(name).to_bytes(),b"default"|b"hw"|b"hw:0") { return -2; }
    let fd=match kinakaze_vfs::eventfd::create_eventfd(0,0x80800) { Ok(fd)=>fd,Err(e)=>return -e };
    let mut bus=bus().lock().unwrap();
    let Some(id)=(128..=252).find(|id|!bus.clients.contains_key(id)) else { let _=kinakaze_vfs::close(fd); return -28; };
    let result=params::allocate_params(out,Seq { id,owner:std::process::id(),event:[0;28],payload:ptr::null_mut(),capacity:0 });
    if result<0 { let _=kinakaze_vfs::close(fd); return result; }
    let mut info=ClientInfo { client:id as i32,..Default::default() }; name_bytes(&mut info.name,b"ALSA client");
    bus.clients.insert(id,Arc::new(Client { id,streams,nonblock:AtomicBool::new(mode!=0),closed:AtomicBool::new(false),
        info:Mutex::new(info),ports:Mutex::new(BTreeMap::new()),fd,
        input:Mutex::new(Input { packets:VecDeque::new(),bytes:0,overflow:false,capacity:65536 }),
        output:Mutex::new((VecDeque::new(),0,65536)),native:Mutex::new(BTreeMap::new()),
        schedule:Mutex::new(schedule::State::default()),wake:Condvar::new(),worker:AtomicBool::new(false) }));
    drop(bus); announce(60,Address {client:id,port:0}); 0
});
api!(snd_seq_open_lconf(out:*mut *mut Seq,name:*const c_char,streams:i32,mode:i32,_config:*mut c_void)->i32 {
    snd_seq_open(out,name,streams,mode)
});
api!(snd_seq_close(seq:*mut Seq)->i32 {
    let state=get_client!(seq);
    { let mut bus=bus().lock().unwrap(); bus.clients.remove(&state.id);
      bus.subscriptions.retain(|s|s.sender.client!=state.id && s.dest.client!=state.id);
      bus.queues.retain(|_,owner|*owner!=state.id); }
    { let _schedule=state.schedule.lock().unwrap(); state.closed.store(true,Ordering::Release); }
    state.wake.notify_all(); polling::signal(state.fd);
    announce(61,Address {client:state.id,port:0});
    kinakaze_alloc::guest::free((*seq).payload); kinakaze_alloc::guest::free(seq.cast()); 0
});
api!(snd_seq_client_id(seq:*mut Seq)->i32 { get_client!(seq).id as i32 });
api!(snd_seq_name(seq:*mut Seq)->*const c_char { if client(seq).is_err() { ptr::null() } else { c"default".as_ptr() } });
api!(snd_seq_nonblock(seq:*mut Seq,value:i32)->i32 { get_client!(seq).nonblock.store(value!=0,Ordering::Relaxed); 0 });
api!(snd_seq_set_client_name(seq:*mut Seq,name:*const c_char)->i32 {
    let state=get_client!(seq); if name.is_null() { return -22; }
    name_bytes(&mut state.info.lock().unwrap().name,CStr::from_ptr(name).to_bytes()); 0
});
api!(snd_seq_get_any_client_info(seq:*mut Seq,id:i32,out:*mut ClientInfo)->i32 {
    let _=get_client!(seq); if out.is_null() { return -22; }
    match client_info(id) { Ok(value)=>{*out=value;0},Err(e)=>e }
});
api!(snd_seq_get_client_info(seq:*mut Seq,out:*mut ClientInfo)->i32 {
    let state=get_client!(seq); snd_seq_get_any_client_info(seq,state.id as i32,out)
});
api!(snd_seq_query_next_client(seq:*mut Seq,out:*mut ClientInfo)->i32 {
    let _=get_client!(seq); if out.is_null() { return -22; }
    for id in ((*out).client.saturating_add(1).max(0))..=252 { if let Ok(value)=client_info(id) { *out=value; return 0; } } -2
});
api!(snd_seq_get_any_port_info(seq:*mut Seq,id:i32,port:i32,out:*mut PortInfo)->i32 {
    let _=get_client!(seq); if out.is_null() || !(0..=252).contains(&id) || !(0..=255).contains(&port) { return -22; }
    match port_info(Address {client:id as u8,port:port as u8}) { Ok(value)=>{*out=value;0},Err(e)=>e }
});
api!(snd_seq_query_next_port(seq:*mut Seq,out:*mut PortInfo)->i32 {
    let _=get_client!(seq); if out.is_null() || !(0..=252).contains(&(*out).client) { return -22; }
    for id in ((*out).port.saturating_add(1).max(0))..=255 {
        if let Ok(value)=port_info(Address {client:(*out).client as u8,port:id as u8}) { *out=value; return 0; }
    } -2
});
api!(snd_seq_create_port(seq:*mut Seq,info:*mut PortInfo)->i32 {
    let state=get_client!(seq); if info.is_null() { return -22; }
    if (*info).timestamping!=0 && !state.schedule.lock().unwrap().queues.contains_key(&(*info).timestamp_queue) { return -22; }
    let mut ports=state.ports.lock().unwrap();
    let Some(id)=(0..=255).find(|id|!ports.contains_key(id)) else { return -28; };
    (*info).client=state.id as i32; (*info).port=id as i32; (*info).addr=Address {client:state.id,port:id};
    ports.insert(id,*info); drop(ports); announce(63,(*info).addr); 0
});
api!(snd_seq_create_simple_port(seq:*mut Seq,name:*const c_char,capability:u32,kind:u32)->i32 {
    if name.is_null() { return -22; }
    let mut info=PortInfo { capability,kind,..Default::default() }; name_bytes(&mut info.name,CStr::from_ptr(name).to_bytes());
    let result=snd_seq_create_port(seq,&raw mut info); if result<0 { result } else { info.port }
});
api!(snd_seq_delete_port(seq:*mut Seq,port:i32)->i32 {
    let state=get_client!(seq); if !(0..=255).contains(&port) { return -22; }
    if state.ports.lock().unwrap().remove(&(port as u8)).is_none() { return -2; }
    let address=Address {client:state.id,port:port as u8};
    bus().lock().unwrap().subscriptions.retain(|s|s.sender!=address && s.dest!=address);
    announce(64,address); 0
});
api!(snd_seq_delete_simple_port(seq:*mut Seq,port:i32)->i32 { snd_seq_delete_port(seq,port) });
api!(snd_seq_subscribe_port(seq:*mut Seq,subscription:*const Subscription)->i32 {
    let _=get_client!(seq); if subscription.is_null() { return -22; } let value=*subscription;
    let sender=match port_info(value.sender) { Ok(p)=>p,Err(e)=>return e };
    let dest=match port_info(value.dest) { Ok(p)=>p,Err(e)=>return e };
    if sender.capability & 33 !=33 || dest.capability & 66 !=66 { return -1; }
    if value.time_update!=0 {
        let bus=bus().lock().unwrap(); let Some(receiver)=bus.clients.get(&value.dest.client) else { return -95; };
        if !receiver.schedule.lock().unwrap().queues.contains_key(&value.queue) { return -22; }
    }
    let mut bus=bus().lock().unwrap();
    if bus.subscriptions.iter().any(|s|s.sender==value.sender && s.dest==value.dest) { return -16; }
    if bus.subscriptions.iter().any(|s|(s.sender==value.sender || s.dest==value.dest) && (s.exclusive!=0 || value.exclusive!=0)) { return -16; }
    bus.subscriptions.push(value); 0
});
api!(snd_seq_unsubscribe_port(seq:*mut Seq,subscription:*const Subscription)->i32 {
    let _=get_client!(seq); if subscription.is_null() { return -22; }
    let mut bus=bus().lock().unwrap(); let before=bus.subscriptions.len();
    bus.subscriptions.retain(|s|s.sender!=(*subscription).sender || s.dest!=(*subscription).dest);
    if before==bus.subscriptions.len() { -2 } else { 0 }
});
api!(snd_seq_connect_to(seq:*mut Seq,port:i32,dest:i32,dest_port:i32)->i32 {
    let state=get_client!(seq); if !(0..=255).contains(&port) || !(0..=252).contains(&dest) || !(0..=255).contains(&dest_port) {return -22;}
    snd_seq_subscribe_port(seq,&Subscription {sender:Address {client:state.id,port:port as u8},dest:Address {client:dest as u8,port:dest_port as u8},..Default::default()})
});
api!(snd_seq_connect_from(seq:*mut Seq,port:i32,source:i32,source_port:i32)->i32 {
    let state=get_client!(seq); if !(0..=255).contains(&port) || !(0..=252).contains(&source) || !(0..=255).contains(&source_port) {return -22;}
    snd_seq_subscribe_port(seq,&Subscription {sender:Address {client:source as u8,port:source_port as u8},dest:Address {client:state.id,port:port as u8},..Default::default()})
});
