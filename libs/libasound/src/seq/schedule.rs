//! Lazy deadline worker for sequencer tick and real-time queues.
use super::*;
pub(super) struct Queue {
    tempo: Tempo,
    start: Option<Instant>,
    nanoseconds: u64,
    ticks: u128,
}
impl Queue {
    fn new() -> Self {
        Self {
            tempo: Tempo::default(),
            start: None,
            nanoseconds: 0,
            ticks: 0,
        }
    }
    fn elapsed(&self) -> u64 {
        self.start
            .map_or(0, |at| at.elapsed().as_nanos().min(u64::MAX as u128) as u64)
    }
    pub fn position(&self, real: bool) -> u64 {
        if real {
            self.nanoseconds.saturating_add(self.elapsed())
        } else {
            ((self.ticks
                + ((self.elapsed() as u128 * self.tempo.ppq as u128) << 32)
                    / (self.tempo.tempo as u128 * 1000))
                >> 32)
                .min(u64::MAX as u128) as u64
        }
    }
    fn settle(&mut self) {
        let elapsed = self.elapsed();
        self.nanoseconds = self.nanoseconds.saturating_add(elapsed);
        self.ticks +=
            ((elapsed as u128 * self.tempo.ppq as u128) << 32) / (self.tempo.tempo as u128 * 1000);
        if self.start.is_some() {
            self.start = Some(Instant::now());
        }
    }
    fn delay(&self, real: bool, target: u64) -> Option<Duration> {
        self.start?;
        let remaining = target.saturating_sub(self.position(real));
        let ns = if real {
            remaining as u128
        } else {
            (remaining as u128 * self.tempo.tempo as u128 * 1000).div_ceil(self.tempo.ppq as u128)
        };
        Some(Duration::from_nanos(ns.min(u64::MAX as u128) as u64))
    }
}
pub(super) struct Pending {
    queue: i32,
    real: bool,
    when: u64,
    packet: Packet,
}
#[derive(Default)]
pub(super) struct State {
    pub queues: BTreeMap<i32, Queue>,
    pending: Vec<Pending>,
    bytes: usize,
}
impl State {
    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.bytes = 0;
    }
}
pub(super) fn submit(client: &Arc<Client>, packet: Packet) -> Result<(), i32> {
    if packet.bytes[3] == 253 {
        return events::route(Some(client), packet);
    }
    let queue = packet.bytes[3] as i32;
    let real = packet.bytes[1] & 1 != 0;
    let word =
        |offset| u32::from_ne_bytes(packet.bytes[offset..offset + 4].try_into().unwrap()) as u64;
    let mut when = if real {
        if word(8) >= 1_000_000_000 {
            return Err(-22);
        }
        word(4) * 1_000_000_000 + word(8)
    } else {
        word(4)
    };
    let mut schedule = client.schedule.lock().unwrap();
    let q = schedule.queues.get(&queue).ok_or(-22)?;
    if packet.bytes[1] & 2 != 0 {
        when = when.checked_add(q.position(real)).ok_or(-22)?;
    }
    if schedule.pending.len() >= 4096
        || packet.size() > (4 * 1024 * 1024usize).saturating_sub(schedule.bytes)
    {
        return Err(-11);
    }
    if !client.worker.swap(true, Ordering::AcqRel) {
        let state = Arc::clone(client);
        if std::thread::Builder::new()
            .name("alsa-sequencer".into())
            .spawn(move || run(state))
            .is_err()
        {
            client.worker.store(false, Ordering::Release);
            return Err(-11);
        }
    }
    schedule.bytes += packet.size();
    schedule.pending.push(Pending {
        queue,
        real,
        when,
        packet,
    });
    drop(schedule);
    client.wake.notify_all();
    Ok(())
}
fn run(client: Arc<Client>) {
    let mut state = client.schedule.lock().unwrap();
    loop {
        if client.closed.load(Ordering::Acquire) {
            return;
        }
        let next = state
            .pending
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                state
                    .queues
                    .get(&event.queue)?
                    .delay(event.real, event.when)
                    .map(|delay| (index, delay))
            })
            .min_by_key(|(_, delay)| *delay);
        match next {
            Some((index, delay)) if delay.is_zero() => {
                let event = state.pending.remove(index);
                state.bytes -= event.packet.size();
                drop(state);
                let _ = events::route(Some(&client), event.packet);
                state = client.schedule.lock().unwrap();
            }
            Some((_, delay)) => {
                state = client.wake.wait_timeout(state, delay).unwrap().0;
            }
            None => {
                state = client.wake.wait(state).unwrap();
            }
        }
    }
}
api!(snd_seq_alloc_named_queue(seq:*mut Seq,_name:*const c_char)->i32 {
    let state=get_client!(seq);let mut bus=bus().lock().unwrap();
    let Some(id)=(0..32).find(|id|!bus.queues.contains_key(id))else{return -28;};
    bus.queues.insert(id,state.id);state.schedule.lock().unwrap().queues.insert(id as i32,Queue::new());id as i32
});
api!(snd_seq_alloc_queue(seq:*mut Seq)->i32 {snd_seq_alloc_named_queue(seq,ptr::null())});
api!(snd_seq_free_queue(seq:*mut Seq,queue:i32)->i32 {
    let state=get_client!(seq);let mut bus=bus().lock().unwrap();
    if queue<0||queue>=32||bus.queues.get(&(queue as u8))!=Some(&state.id){return -22;}
    bus.queues.remove(&(queue as u8));let mut schedule=state.schedule.lock().unwrap();schedule.queues.remove(&queue);
    schedule.pending.retain(|e|e.queue!=queue);
    schedule.bytes=schedule.pending.iter().map(|e|e.packet.size()).sum();state.wake.notify_all();0
});
api!(snd_seq_get_queue_tempo(seq:*mut Seq,queue:i32,out:*mut Tempo)->i32 {
    let state=get_client!(seq);if out.is_null(){return -22;}let schedule=state.schedule.lock().unwrap();
    let Some(q)=schedule.queues.get(&queue)else{return -22;};*out=q.tempo;0
});
api!(snd_seq_set_queue_tempo(seq:*mut Seq,queue:i32,tempo:*const Tempo)->i32 {
    let state=get_client!(seq);if tempo.is_null()||(*tempo).tempo==0||(*tempo).ppq<=0{return -22;}
    let mut schedule=state.schedule.lock().unwrap();let Some(q)=schedule.queues.get_mut(&queue)else{return -22;};
    q.settle();q.tempo=*tempo;state.wake.notify_all();0
});
pub(super) fn control(state: &Arc<Client>, packet: &Packet) -> Result<(), i32> {
    let queue = packet.bytes[16] as i32;
    let kind = packet.bytes[0];
    let value = u32::from_ne_bytes(packet.bytes[20..24].try_into().unwrap());
    let mut schedule = state.schedule.lock().unwrap();
    let q = schedule.queues.get_mut(&queue).ok_or(-22)?;
    match kind {
        30 => {
            q.nanoseconds = 0;
            q.ticks = 0;
            q.start = Some(Instant::now());
        }
        31 => {
            if q.start.is_none() {
                q.start = Some(Instant::now());
            }
        }
        32 => {
            q.settle();
            q.start = None;
        }
        33 => {
            q.settle();
            q.ticks = (value as u128) << 32;
        }
        34 => {
            let ns = u32::from_ne_bytes(packet.bytes[24..28].try_into().unwrap());
            if ns >= 1_000_000_000 {
                return Err(-22);
            }
            q.settle();
            q.nanoseconds = u64::from(value) * 1_000_000_000 + u64::from(ns);
        }
        35 if value > 0 => {
            q.settle();
            q.tempo.tempo = value as u32;
        }
        _ => return Err(-95),
    }
    state.wake.notify_all();
    Ok(())
}
api!(snd_seq_control_queue(seq:*mut Seq,queue:i32,kind:i32,value:i32,event:*mut u8)->i32 {
    if !(0..32).contains(&queue) || !(0..=255).contains(&kind){return -22;}
    let mut local=Packet::empty(0);
    let event=if event.is_null(){local.bytes.as_mut_ptr()}else{event};
    *event=kind as u8;*event.add(1)&=!12;*event.add(14)=0;*event.add(15)=0;
    *event.add(16)=queue as u8;
    event.add(20).cast::<i32>().write_unaligned(value);
    events::snd_seq_event_output(seq,event)
});
