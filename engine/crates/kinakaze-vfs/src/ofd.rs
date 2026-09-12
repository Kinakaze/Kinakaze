//! Open-description positions and status flags shared by SCM_RIGHTS recipients.
use crate::mount::shared::{self, Store};
use crate::{EBADF, EIO, FdEntry, FdFlags};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(test)]
mod tests;
const STATUS: u32 = FdFlags::APPEND.0 | FdFlags::NONBLOCK.0 | FdFlags::NOATIME.0;
pub(crate) struct Shared {
    store: Store,
    pin: Arc<crate::fs::object::Object>,
    pub(crate) filter_cache: Mutex<Option<(u64, Arc<crate::socket::filter::State>)>>,
}
impl std::ops::Deref for Shared {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}
impl Shared {
    pub(crate) fn open(id: u64) -> Result<Arc<Self>, i32> {
        let store = Store::user_object(id, false)?;
        let pin = Arc::new(store.pin()?);
        // A packet reader is a process-local cache, not a guest descriptor.
        // Its pin must not leak into a fork child's ambient native handles.
        Ok(Arc::new(Self {
            store,
            pin,
            filter_cache: Mutex::new(None),
        }))
    }
    fn new(store: Store) -> Result<Arc<Self>, i32> {
        let pin = Arc::new(store.pin()?);
        crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
        Ok(Arc::new(Self {
            store,
            pin,
            filter_cache: Mutex::new(None),
        }))
    }
}
struct Description {
    gate: Mutex<()>,
    shared: Mutex<Option<Arc<Shared>>>,
    object: Mutex<Option<Arc<Store>>>,
}
fn descriptions() -> &'static Mutex<HashMap<u64, Arc<Description>>> {
    static ITEMS: OnceLock<Mutex<HashMap<u64, Arc<Description>>>> = OnceLock::new();
    ITEMS.get_or_init(Default::default)
}
thread_local! {static ACTIVE:std::cell::RefCell<Vec<u64>>=const{std::cell::RefCell::new(Vec::new())};}
struct Active(u64);
impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.with(|a| a.borrow_mut().retain(|id| *id != self.0));
    }
}
pub(crate) fn signal_delivery_deferred() -> bool {
    ACTIVE.with(|a| !a.borrow().is_empty())
}
fn active(id: u64) -> bool {
    ACTIVE.with(|a| a.borrow().contains(&id))
}
fn local(id: u64) -> Result<Arc<Description>, i32> {
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .entry(id)
        .or_insert_with(|| {
            Arc::new(Description {
                gate: Mutex::new(()),
                shared: Mutex::new(None),
                object: Mutex::new(None),
            })
        })
        .clone())
}
fn encode(entry: FdEntry) -> Vec<u8> {
    let mut bytes = Vec::new();
    crate::state_codec::word(&mut bytes, entry.offset);
    crate::state_codec::word(&mut bytes, (entry.flags.0 & STATUS) as u64);
    bytes
}
fn decode(bytes: &[u8], mut entry: FdEntry) -> Result<FdEntry, i32> {
    let mut r = crate::state_codec::Reader(bytes);
    entry.offset = r.word()?;
    entry.flags.0 = (entry.flags.0 & !STATUS) | (r.word()? as u32 & STATUS);
    // Socket options occupy an optional extension after the position/status
    // prefix. Position changes must preserve it under the same transaction.
    if !r.0.is_empty() {
        crate::socket::filter::State::validate_encoding(r.0)?;
    }
    Ok(entry)
}
fn raw_entry(fd: i32) -> Result<FdEntry, i32> {
    crate::table()
        .read()
        .map_err(|_| EIO)?
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|e| *e)
        .ok_or(EBADF)
}
fn synchronize(entry: FdEntry) -> Result<(), i32> {
    let mut table = crate::table().write().map_err(|_| EIO)?;
    for e in table
        .slots
        .iter_mut()
        .flatten()
        .filter(|e| e.description_id == entry.description_id)
    {
        e.offset = entry.offset;
        e.flags.0 = (e.flags.0 & !STATUS) | (entry.flags.0 & STATUS);
    }
    Ok(())
}
pub(crate) fn refresh(entry: FdEntry) -> Result<FdEntry, i32> {
    if active(entry.description_id) {
        return Ok(entry);
    }
    let item = descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned();
    let store = match item {
        Some(item) => item.shared.lock().map_err(|_| EIO)?.clone(),
        None => None,
    };
    match store {
        Some(store) => decode(&store.read()?.1, entry),
        None => Ok(entry),
    }
}
/// Always serialize positioned I/O locally, including before the first sendmsg
/// promotes it. Promotion therefore cannot race an already in-flight read.
pub(crate) fn with<T>(fd: i32, mut operation: impl FnMut() -> Result<T, i32>) -> Result<T, i32> {
    loop {
        let result = with_once(fd, &mut operation);
        // Guest handlers may fork or access the same description. Dispatch only
        // after the local and shared position locks and temporary pins unwind.
        if !signal_delivery_deferred()
            && matches!(&result, Err(error) if *error == crate::EINTR)
            && crate::signal::deliver_pending() == crate::signal::Delivery::Restart
        {
            continue;
        }
        return result;
    }
}
fn with_once<T>(fd: i32, operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    let entry = raw_entry(fd)?;
    if active(entry.description_id) {
        return operation();
    }
    let item = local(entry.description_id)?;
    let _gate = item.gate.lock().map_err(|_| EIO)?;
    if raw_entry(fd)?.generation != entry.generation {
        return Err(EBADF);
    }
    let store = item.shared.lock().map_err(|_| EIO)?.clone();
    ACTIVE.with(|a| a.borrow_mut().push(entry.description_id));
    let _active = Active(entry.description_id);
    if let Some(store) = store {
        store.update(|bytes| {
            synchronize(decode(bytes, entry)?)?;
            let result = operation();
            let updated = crate::table()
                .read()
                .map_err(|_| EIO)?
                .slots
                .get(fd as usize)
                .and_then(|slot| *slot)
                // read/write/seek update the calling descriptor. Another dup
                // alias still contains the old offset until synchronize runs.
                // Never publish a recycled slot's unrelated description.
                .filter(|updated| updated.generation == entry.generation)
                .unwrap_or(entry);
            synchronize(updated)?;
            let mut updated_bytes = encode(updated);
            updated_bytes.extend_from_slice(&bytes[16..]);
            Ok((updated_bytes, result))
        })?
    } else {
        let result = operation();
        if let Ok(updated) = raw_entry(fd)
            && updated.generation == entry.generation
        {
            synchronize(updated)?;
        }
        result
    }
}
pub(crate) fn promote(fd: i32) -> Result<Arc<Shared>, i32> {
    let entry = raw_entry(fd)?;
    let item = local(entry.description_id)?;
    let _gate = item.gate.lock().map_err(|_| EIO)?;
    if let Some(store) = item.shared.lock().map_err(|_| EIO)?.clone() {
        return Ok(store);
    }
    let current = raw_entry(fd)?;
    if current.generation != entry.generation {
        return Err(EBADF);
    }
    let store = Shared::new(shared::new_object()?)?;
    store.update(|_| Ok((encode(current), ())))?;
    *item.shared.lock().map_err(|_| EIO)? = Some(store.clone());
    Ok(store)
}
pub(crate) fn existing(description: u64) -> Result<Option<Arc<Shared>>, i32> {
    let item = descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&description)
        .cloned();
    match item {
        Some(item) => Ok(item.shared.lock().map_err(|_| EIO)?.clone()),
        None => Ok(None),
    }
}
pub(crate) fn attach(entry: FdEntry, id: u64) -> Result<(), i32> {
    if id == 0 {
        return Ok(());
    }
    let store = Shared::new(Store::user_object(id, false)?)?;
    let item = local(entry.description_id)?;
    *item.shared.lock().map_err(|_| EIO)? = Some(store);
    Ok(())
}
pub(crate) fn closed(id: u64) {
    if let Ok(mut items) = descriptions().lock() {
        items.remove(&id);
    }
}
/// Pin a descriptor-backed section once per open description. The descriptor
/// table lock held by the caller prevents raw-handle reuse while opening it.
/// Dup shares this pin; final close releases it; fork restore rebuilds it lazily.
pub(crate) fn object_store(entry: FdEntry) -> Result<Arc<Store>, i32> {
    let description = local(entry.description_id)?;
    let mut object = description.object.lock().map_err(|_| EIO)?;
    if object.is_none() {
        *object = Some(Arc::new(shared::object_entry(entry)?));
    }
    Ok(object.as_ref().unwrap().clone())
}
pub(crate) fn auxiliary_handles() -> Result<Vec<(u64, Arc<crate::fs::object::Object>)>, i32> {
    let items = descriptions().lock().map_err(|_| EIO)?.clone();
    let mut result = Vec::new();
    for (id, item) in items {
        if let Some(shared) = item.shared.lock().map_err(|_| EIO)?.as_ref() {
            result.push((id, shared.pin.clone()));
        }
    }
    Ok(result)
}
pub(crate) fn serialize(keep: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let promote_fds: Vec<_> = crate::table()
        .read()
        .map_err(|_| EIO)?
        .slots
        .enumerated()
        .filter_map(|(fd, e)| {
            e.filter(|e| {
                keep(fd as i32)
                    && (e.flags.contains(FdFlags::SEEKABLE)
                        || matches!(
                            e.kind,
                            crate::FdKind::File
                                | crate::FdKind::Directory
                                | crate::FdKind::Pipe
                                | crate::FdKind::PtyMaster
                                | crate::FdKind::PtySlave
                                | crate::FdKind::UnixSocket
                                | crate::FdKind::Socket
                                | crate::FdKind::NetlinkSocket
                                | crate::FdKind::TimerFd
                        ))
            })
            .map(|_| fd as i32)
        })
        .collect();
    for fd in promote_fds {
        promote(fd)?;
    }
    let entries: Vec<_> = crate::table()
        .read()
        .map_err(|_| EIO)?
        .slots
        .enumerated()
        .filter_map(|(fd, e)| e.filter(|_| keep(fd as i32)))
        .collect();
    let items = descriptions().lock().map_err(|_| EIO)?.clone();
    let mut bytes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in entries {
        if seen.insert(e.description_id) {
            if let Some(item) = items.get(&e.description_id) {
                if let Some(s) = &*item.shared.lock().map_err(|_| EIO)? {
                    crate::state_codec::word(&mut bytes, e.description_id);
                    crate::state_codec::word(&mut bytes, s.id());
                    crate::state_codec::word(&mut bytes, s.pin.raw() as u64);
                }
            }
        }
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let result = (|| {
        let mut r = crate::state_codec::Reader(bytes);
        let mut items = HashMap::new();
        while !r.0.is_empty() {
            let id = r.word()?;
            let store_id = r.word()?;
            let inherited = crate::fs::object::Object::owned(r.word()? as _)?;
            let store = Shared::new(Store::user_object(store_id, false)?)?;
            drop(inherited);
            items.insert(
                id,
                Arc::new(Description {
                    gate: Mutex::new(()),
                    shared: Mutex::new(Some(store)),
                    object: Mutex::new(None),
                }),
            );
        }
        *descriptions().lock().map_err(|_| EIO)? = items;
        Ok::<_, i32>(())
    })();
    result.is_ok()
}
