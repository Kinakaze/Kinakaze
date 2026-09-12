//! Owned event packets, readiness and lazy WinMM output. Variable payloads are
//! copied when queued so callers may immediately reuse their encoder storage.
use super::*;
use libc::fdio::PollFd;

#[derive(Clone)]
pub(super) struct Packet {
    pub bytes: [u8; 28],
    pub payload: Vec<u8>,
}
impl Packet {
    pub fn empty(kind: u8) -> Self {
        let mut bytes = [0; 28];
        bytes[0] = kind;
        bytes[3] = 253;
        Self {
            bytes,
            payload: Vec::new(),
        }
    }
    pub(super) fn size(&self) -> usize {
        28 + self.payload.len()
    }
    unsafe fn read(event: *const u8) -> Result<Self, i32> {
        if event.is_null() {
            return Err(-22);
        }
        let mut packet = Self::empty(0);
        unsafe { ptr::copy_nonoverlapping(event, packet.bytes.as_mut_ptr(), 28) };
        match packet.bytes[1] & 12 {
            0 => {}
            4 | 8 => {
                let length = u32::from_ne_bytes(packet.bytes[16..20].try_into().unwrap()) as usize;
                if length > 256 * 1024 {
                    return Err(-90);
                }
                let data = unsafe { event.add(20).cast::<*const u8>().read_unaligned() };
                if length != 0 {
                    if data.is_null() {
                        return Err(-22);
                    }
                    packet.payload = unsafe { core::slice::from_raw_parts(data, length) }.to_vec();
                }
                packet.bytes[1] = (packet.bytes[1] & !12) | 4;
            }
            _ => return Err(-22),
        }
        Ok(packet)
    }
}

fn deliver(
    target: &Arc<Client>,
    port: PortInfo,
    subscription: Option<Subscription>,
    mut packet: Packet,
) -> Result<(), i32> {
    if target.streams & 2 == 0 || port.capability & 2 == 0 || target.closed.load(Ordering::Acquire)
    {
        return Err(-1);
    }
    let stamp = subscription
        .filter(|s| s.time_update != 0)
        .map(|s| (s.queue, s.time_real != 0))
        .or_else(|| {
            (port.timestamping != 0).then_some((port.timestamp_queue, port.timestamp_real != 0))
        });
    if let Some((queue, real)) = stamp {
        let state = target.schedule.lock().unwrap();
        let q = state.queues.get(&queue).ok_or(-22)?;
        let value = q.position(real);
        packet.bytes[1] = (packet.bytes[1] & !3) | u8::from(real);
        if real {
            packet.bytes[4..8].copy_from_slice(&((value / 1_000_000_000) as u32).to_ne_bytes());
            packet.bytes[8..12].copy_from_slice(&((value % 1_000_000_000) as u32).to_ne_bytes());
        } else {
            packet.bytes[4..8].copy_from_slice(&(value as u32).to_ne_bytes());
        }
    }
    let mut input = target.input.lock().unwrap();
    if packet.size() > input.capacity.saturating_sub(input.bytes) {
        input.packets.clear();
        input.bytes = 0;
        input.overflow = true;
        polling::signal(target.fd);
        return Err(-28);
    }
    packet.bytes[14] = target.id;
    packet.bytes[15] = port.addr.port;
    let was_empty = input.packets.is_empty();
    input.bytes += packet.size();
    input.packets.push_back(packet);
    if was_empty {
        polling::signal(target.fd);
    }
    Ok(())
}

fn native_output(sender: &Arc<Client>, port: u8, packet: &Packet) -> Result<(), i32> {
    // SysEx is retained for software routes, but the current native raw-MIDI
    // transport accepts short messages only. Do not acknowledge dropped data.
    if !packet.payload.is_empty() {
        return Err(-95);
    }
    let mut bytes = [0u8; 12];
    let count = unsafe { midi_event::decode_short(packet.bytes.as_ptr(), &mut bytes) };
    if count < 0 {
        return Err(count as i32);
    }
    let mut handles = sender.native.lock().unwrap();
    let handle = if let Some(&pointer) = handles.get(&port) {
        pointer
    } else {
        let name = std::ffi::CString::new(format!("hw:0,{port}")).unwrap();
        let mut output = ptr::null_mut();
        let result = unsafe {
            rawmidi::snd_rawmidi_open(ptr::null_mut(), &raw mut output, name.as_ptr(), 0)
        };
        if result < 0 {
            return Err(result);
        }
        handles.insert(port, output as usize);
        output as usize
    };
    let result =
        unsafe { rawmidi::snd_rawmidi_write(handle as _, bytes.as_ptr().cast(), count as usize) };
    if result == count {
        Ok(())
    } else {
        Err(if result < 0 { result as i32 } else { -5 })
    }
}

pub(super) fn route(sender: Option<&Arc<Client>>, packet: Packet) -> Result<(), i32> {
    if packet.bytes[14] == 0 && packet.bytes[15] == 0 {
        return schedule::control(sender.ok_or(-1)?, &packet);
    }
    let source = Address {
        client: packet.bytes[12],
        port: packet.bytes[13],
    };
    let destinations = if packet.bytes[14] == 254 {
        bus()
            .lock()
            .unwrap()
            .subscriptions
            .iter()
            .filter(|s| s.sender == source)
            .map(|s| (s.dest, Some(*s)))
            .collect::<Vec<_>>()
    } else if packet.bytes[14] == 255 {
        return Err(-95);
    } else {
        vec![(
            Address {
                client: packet.bytes[14],
                port: packet.bytes[15],
            },
            None,
        )]
    };
    let mut delivered = false;
    let mut error = None;
    for (address, subscription) in destinations {
        let result = (|| {
            let port = port_info(address)?;
            if port.capability & 2 == 0 {
                return Err(-1);
            }
            if address.client == 16 {
                return native_output(sender.ok_or(-1)?, address.port, &packet);
            }
            let target = bus()
                .lock()
                .unwrap()
                .clients
                .get(&address.client)
                .cloned()
                .ok_or(-2)?;
            deliver(&target, port, subscription, packet.clone())
        })();
        match result {
            Ok(()) => delivered = true,
            Err(e) => error = Some(e),
        }
    }
    if delivered || error.is_none() {
        Ok(())
    } else {
        Err(error.unwrap())
    }
}

unsafe fn prepare(seq: *mut Seq, event: *const u8) -> Result<(Arc<Client>, Packet), i32> {
    let state = unsafe { client(seq) }?;
    if state.streams & 1 == 0 {
        return Err(-9);
    }
    let mut packet = unsafe { Packet::read(event) }?;
    packet.bytes[12] = state.id;
    if !(packet.bytes[14] == 0 && packet.bytes[15] == 0)
        && !state.ports.lock().unwrap().contains_key(&packet.bytes[13])
    {
        return Err(-2);
    }
    Ok((state, packet))
}
api!(snd_seq_event_output_direct(seq:*mut Seq,event:*const u8)->i32 {
    let (state,packet)=match prepare(seq,event) {Ok(value)=>value,Err(e)=>return e};
    let length=packet.size() as i32;
    match schedule::submit(&state,packet) {Ok(())=>length,Err(e)=>e}
});
api!(snd_seq_event_output_buffer(seq:*mut Seq,event:*const u8)->i32 {
    let (state,packet)=match prepare(seq,event) {Ok(value)=>value,Err(e)=>return e};
    let mut output=state.output.lock().unwrap();
    if packet.size()>output.2 {return -90;}
    if packet.size()>output.2.saturating_sub(output.1) {return -11;}
    output.1+=packet.size(); output.0.push_back(packet); output.1 as i32
});
api!(snd_seq_event_output(seq:*mut Seq,event:*const u8)->i32 {
    let result=snd_seq_event_output_buffer(seq,event); if result!=-11 {return result;}
    let drained=snd_seq_drain_output(seq); if drained<0 {return drained;}
    snd_seq_event_output_buffer(seq,event)
});
api!(snd_seq_drain_output(seq:*mut Seq)->i32 {
    let state=get_client!(seq);
    loop {
        let mut output=state.output.lock().unwrap();
        let Some(packet)=output.0.front().cloned() else {return 0;};
        match schedule::submit(&state,packet.clone()) {Ok(())=>{output.0.pop_front();output.1-=packet.size();},Err(e)=>return e}
    }
});
api!(snd_seq_drop_output(seq:*mut Seq)->i32 {
    let state=get_client!(seq); let mut output=state.output.lock().unwrap(); output.0.clear();output.1=0;
    state.schedule.lock().unwrap().clear_pending();state.wake.notify_all();0
});
api!(snd_seq_drop_input(seq:*mut Seq)->i32 {
    let state=get_client!(seq);let mut input=state.input.lock().unwrap();input.packets.clear();input.bytes=0;input.overflow=false;polling::clear(state.fd);0
});
api!(snd_seq_event_input_pending(seq:*mut Seq,_fetch:i32)->i32 {get_client!(seq).input.lock().unwrap().packets.len() as i32});
api!(snd_seq_event_output_pending(seq:*mut Seq)->i32 {get_client!(seq).output.lock().unwrap().1 as i32});
api!(snd_seq_event_input(seq:*mut Seq,out:*mut *mut u8)->i32 {
    if out.is_null() {return -22;} *out=ptr::null_mut();let state=get_client!(seq);if state.streams&2==0{return -9;}
    loop {
        if state.closed.load(Ordering::Acquire) {return -9;}
        let mut input=state.input.lock().unwrap();
        if input.overflow {input.overflow=false;if input.packets.is_empty(){polling::clear(state.fd);}return -28;}
        if let Some(packet)=input.packets.front() {
            if packet.payload.len()>(*seq).capacity {
                let memory=kinakaze_alloc::guest::reallocate((*seq).payload,16,packet.payload.len());
                if memory.is_null() {return -12;} (*seq).payload=memory;(*seq).capacity=packet.payload.len();
            }
            let packet=input.packets.pop_front().unwrap();input.bytes-=packet.size();(*seq).event=packet.bytes;
            if !packet.payload.is_empty() {ptr::copy_nonoverlapping(packet.payload.as_ptr(),(*seq).payload,packet.payload.len());}
            if packet.bytes[1]&12!=0 {(&raw mut (*seq).event).cast::<u8>().add(20).cast::<*mut u8>().write_unaligned((*seq).payload);}
            if input.packets.is_empty() {polling::clear(state.fd);}
            *out=(&raw mut (*seq).event).cast();return input.bytes as i32;
        }
        drop(input);if state.nonblock.load(Ordering::Relaxed){return -11;}
        let mut fd=PollFd {fd:state.fd,events:1,revents:0};
        if libc::fdio::kinakaze_abi_poll(&raw mut fd,1,-1)<0 {return -kinakaze_tls::errno();}
    }
});
api!(snd_seq_free_event(_event:*mut u8)->i32 {0}); // Input storage is owned by the handle.
fn allowed(state: &Client, events: i16) -> i16 {
    events
        & (if state.streams & 1 != 0 { 4 } else { 0 } | if state.streams & 2 != 0 { 1 } else { 0 })
}
api!(snd_seq_poll_descriptors_count(seq:*mut Seq,events:i16)->i32 {let state=get_client!(seq); i32::from(allowed(&state,events)!=0)});
api!(snd_seq_poll_descriptors(seq:*mut Seq,out:*mut PollFd,space:u32,events:i16)->i32 {
    let state=get_client!(seq);let events=allowed(&state,events);if events==0{return 0;}if out.is_null()||space<1{return -22;}
    *out=PollFd{fd:state.fd,events,revents:0};1
});
api!(snd_seq_poll_descriptors_revents(seq:*mut Seq,fds:*const PollFd,count:u32,out:*mut u16)->i32 {
    let state=get_client!(seq);if fds.is_null()||out.is_null()||count!=1||(*fds).fd!=state.fd{return -22;}
    *out=(*fds).revents as u16;0
});
api!(snd_seq_get_input_buffer_size(seq:*mut Seq)->usize {client(seq).map_or(0,|s|s.input.lock().unwrap().capacity)});
api!(snd_seq_get_output_buffer_size(seq:*mut Seq)->usize {client(seq).map_or(0,|s|s.output.lock().unwrap().2)});
api!(snd_seq_set_input_buffer_size(seq:*mut Seq,size:usize)->i32 {
    let state=get_client!(seq);if !(28..=16*1024*1024).contains(&size){return -22;}
    let mut input=state.input.lock().unwrap();if size<input.bytes{return -16;}input.capacity=size;0
});
api!(snd_seq_set_output_buffer_size(seq:*mut Seq,size:usize)->i32 {
    let state=get_client!(seq);if !(28..=16*1024*1024).contains(&size){return -22;}
    let mut output=state.output.lock().unwrap();if size<output.1{return -16;}output.2=size;0
});
