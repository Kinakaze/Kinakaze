//! Transfer pending cookies and protocol errors without sharing native containers.
use super::{STORE, Store};
use core::{cell::RefCell, ptr};
use std::sync::MutexGuard;
const MAGIC: u64 = u64::from_le_bytes(*b"CYXCBRP1");
struct Frozen {
    _store: MutexGuard<'static, Store>,
    bytes: Vec<u8>,
}
thread_local! { static FROZEN: RefCell<Option<Frozen>> = const { RefCell::new(None) }; }
fn word(output: &mut Vec<u8>, n: usize) {
    output.extend_from_slice(&(n as u64).to_le_bytes());
}
fn blob(output: &mut Vec<u8>, b: &[u8]) {
    word(output, b.len());
    output.extend_from_slice(b);
}
fn encode(store: &Store) -> Vec<u8> {
    let mut b = Vec::new();
    word(&mut b, MAGIC as usize);
    word(&mut b, store.next as usize);
    word(&mut b, store.pending.len());
    for (sequence, bytes) in &store.pending {
        word(&mut b, *sequence as usize);
        blob(&mut b, bytes);
    }
    word(&mut b, store.events.len());
    for event in &store.events {
        blob(&mut b, event);
    }
    b
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], i32> {
        let (a, b) = self.0.split_at_checked(n).ok_or(22)?;
        self.0 = b;
        Ok(a)
    }
    fn word(&mut self) -> Result<usize, i32> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()) as usize)
    }
    fn blob(&mut self) -> Result<Vec<u8>, i32> {
        let n = self.word()?;
        Ok(self.take(n)?.to_vec())
    }
}
fn decode(bytes: &[u8]) -> Result<Store, i32> {
    let mut r = Reader(bytes);
    if r.word()? != MAGIC as usize {
        return Err(22);
    }
    let next = u32::try_from(r.word()?).map_err(|_| 22)?;
    if next == 0 {
        return Err(22);
    }
    let mut store = Store {
        next,
        pending: Default::default(),
        events: Default::default(),
    };
    for _ in 0..r.word()? {
        let seq = u32::try_from(r.word()?).map_err(|_| 22)?;
        let b = r.blob()?;
        if seq == 0
            || (!b.is_empty() && (b.len() < 32 || b[0] > 1))
            || store.pending.insert(seq, b).is_some()
        {
            return Err(22);
        }
    }
    for _ in 0..r.word()? {
        let b = r.blob()?;
        if b.len() != 36 || b[0] != 0 {
            return Err(22);
        }
        store.events.push_back(b);
    }
    if !r.0.is_empty() {
        return Err(22);
    }
    Ok(store)
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(store) = STORE.try_lock() else {
            return 11;
        };
        let bytes = encode(&store);
        *slot = Some(Frozen {
            _store: store,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(frozen) = slot.as_ref() else {
            return -22;
        };
        if !output.is_null() {
            if capacity < frozen.bytes.len() {
                return -22;
            }
            unsafe {
                ptr::copy_nonoverlapping(frozen.bytes.as_ptr(), output, frozen.bytes.len());
            }
        }
        frozen.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length > isize::MAX as usize {
        return 22;
    }
    match decode(unsafe { core::slice::from_raw_parts(input, length) }) {
        Ok(store) => {
            *STORE.lock().unwrap() = store;
            0
        }
        Err(error) => error,
    }
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 448,
        key: MAGIC,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
