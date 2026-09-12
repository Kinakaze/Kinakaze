//! Session-scoped X server state. Native pointers and GDI handles never cross
//! process boundaries. A committed journal updates private read caches; named
//! events wake receivers only when another client actually sends an event.
use core::ffi::c_void;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
type Handle = *mut c_void;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingW(
        file: Handle,
        attributes: *const c_void,
        protection: u32,
        high: u32,
        low: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(mapping: Handle, access: u32, high: u32, low: u32, size: usize)
    -> *mut c_void;
    fn UnmapViewOfFile(base: *const c_void) -> i32;
    fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> Handle;
    fn ReleaseMutex(mutex: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
}
const BANK: usize = 16 * 1024 * 1024;

pub fn name(suffix: &str) -> Vec<u16> {
    format!(
        "Local\\kinakaze.x11.v1.{:016x}.{suffix}",
        kinakaze_runtime::authority::domain_id()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect()
}
pub fn pid() -> u32 {
    std::process::id()
}
pub struct Mapping {
    pub handle: usize,
    pub base: usize,
}
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    pub fn new(suffix: &str, size: usize) -> Option<Self> {
        let handle = unsafe {
            CreateFileMappingW(
                -1isize as Handle,
                core::ptr::null(),
                4,
                (size as u64 >> 32) as u32,
                size as u32,
                name(suffix).as_ptr(),
            )
        };
        if handle.is_null() {
            return None;
        }
        let base = unsafe { MapViewOfFile(handle, 0xf001f, 0, 0, size) };
        if base.is_null() {
            unsafe {
                CloseHandle(handle);
            }
            return None;
        }
        Some(Self {
            handle: handle as usize,
            base: base as usize,
        })
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.base as _);
            CloseHandle(self.handle as _);
        }
    }
}
struct Database {
    section: Mapping,
    lock: usize,
    generation: u32,
    cursor: usize,
    values: BTreeMap<String, Vec<u8>>,
}
impl Database {
    fn stamp(&self) -> &AtomicU64 {
        // Page-aligned shared memory; writers publish only after committing the
        // journal. Readers may reuse their private cache at this sequence.
        unsafe { &*(self.section.base as *const AtomicU64) }
    }
    fn new() -> Option<Self> {
        let section = Mapping::new("state", 64 + 2 * BANK)?;
        let lock = unsafe { CreateMutexW(core::ptr::null(), 0, name("lock").as_ptr()) };
        if lock.is_null() {
            return None;
        }
        Some(Self {
            section,
            lock: lock as usize,
            generation: 0,
            cursor: 0,
            values: BTreeMap::new(),
        })
    }
    fn refresh(&mut self) -> Result<(), u8> {
        let stamp = self.stamp().load(Ordering::Acquire);
        let generation = (stamp >> 32) as u32;
        let end = stamp as u32 as usize;
        if end > BANK {
            return Err(11);
        }
        if self.generation != generation {
            self.values.clear();
            self.cursor = 0;
            self.generation = generation;
        }
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (self.section.base + 64 + (generation as usize & 1) * BANK) as *const u8,
                end,
            )
        };
        while self.cursor < end {
            let mut cursor = self.cursor;
            let header = bytes.get(cursor..cursor + 8).ok_or(11u8)?;
            let key_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
            let value_len = u32::from_le_bytes(header[4..].try_into().unwrap());
            cursor += 8;
            let key = std::str::from_utf8(bytes.get(cursor..cursor + key_len).ok_or(11u8)?)
                .map_err(|_| 11u8)?
                .to_owned();
            cursor += key_len;
            if value_len == u32::MAX {
                self.values.remove(&key);
            } else {
                let value = bytes
                    .get(cursor..cursor + value_len as usize)
                    .ok_or(11u8)?
                    .to_vec();
                cursor += value_len as usize;
                self.values.insert(key, value);
            }
            self.cursor = cursor;
        }
        Ok(())
    }
    fn commit(&mut self, pending: BTreeMap<String, Option<Vec<u8>>>) -> Result<(), u8> {
        if pending.is_empty() {
            return Ok(());
        }
        fn encode(out: &mut Vec<u8>, key: &str, value: Option<&[u8]>) {
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(&value.map_or(u32::MAX, |v| v.len() as u32).to_le_bytes());
            out.extend_from_slice(key.as_bytes());
            if let Some(value) = value {
                out.extend_from_slice(value);
            }
        }
        let mut bytes = Vec::new();
        for (key, value) in &pending {
            encode(&mut bytes, key, value.as_deref());
        }
        let compact = self.cursor + bytes.len() > BANK;
        if compact {
            bytes.clear();
            for (key, value) in &self.values {
                if !pending.contains_key(key) {
                    encode(&mut bytes, key, Some(value));
                }
            }
            for (key, value) in &pending {
                if let Some(value) = value {
                    encode(&mut bytes, key, Some(value));
                }
            }
            if bytes.len() > BANK {
                return Err(11);
            }
        }
        let generation = self.generation.wrapping_add(u32::from(compact));
        let start = if compact { 0 } else { self.cursor };
        let end = start + bytes.len();
        // The last committed bank is untouched during compaction. An abandoned
        // writer cannot expose a partially written transaction to another client.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.section.base + 64 + (generation as usize & 1) * BANK + start) as *mut u8,
                bytes.len(),
            );
            self.stamp()
                .store(((generation as u64) << 32) | end as u64, Ordering::Release);
        }
        for (key, value) in pending {
            if let Some(value) = value {
                self.values.insert(key, value);
            } else {
                self.values.remove(&key);
            }
        }
        self.generation = generation;
        self.cursor = end;
        Ok(())
    }
}
pub struct Transaction<'a> {
    values: &'a BTreeMap<String, Vec<u8>>,
    pending: BTreeMap<String, Option<Vec<u8>>>,
}
impl Transaction<'_> {
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        match self.pending.get(key) {
            Some(value) => value.as_deref(),
            None => self.values.get(key).map(Vec::as_slice),
        }
    }
    pub fn set(&mut self, key: String, value: Vec<u8>) {
        if self.get(&key) != Some(value.as_slice()) {
            self.pending.insert(key, Some(value));
        }
    }
    pub fn remove(&mut self, key: &str) {
        if self.get(key).is_some() {
            self.pending.insert(key.to_owned(), None);
        }
    }
    pub fn prefix(&self, prefix: &str) -> Vec<(String, Vec<u8>)> {
        self.values
            .range(prefix.to_owned()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .filter_map(|(key, _)| self.get(key).map(|value| (key.clone(), value.to_vec())))
            .collect()
    }
}
static DATABASE: OnceLock<Mutex<Option<Database>>> = OnceLock::new();
pub fn transaction<T>(apply: impl FnOnce(&mut Transaction<'_>) -> Result<T, u8>) -> Result<T, u8> {
    let mut slot = DATABASE
        .get_or_init(|| Mutex::new(Database::new()))
        .lock()
        .map_err(|_| 11u8)?;
    let db = slot.as_mut().ok_or(11u8)?;
    if !matches!(unsafe { WaitForSingleObject(db.lock as _, 5000) }, 0 | 0x80) {
        return Err(11);
    }
    struct Unlock(usize);
    impl Drop for Unlock {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0 as _);
            }
        }
    }
    let _unlock = Unlock(db.lock);
    db.refresh()?;
    let mut tx = Transaction {
        values: &db.values,
        pending: BTreeMap::new(),
    };
    let result = apply(&mut tx)?;
    let pending = tx.pending;
    db.commit(pending)?;
    Ok(result)
}
fn read<T>(read: impl FnOnce(&BTreeMap<String, Vec<u8>>) -> T) -> Option<T> {
    {
        let mut slot = DATABASE
            .get_or_init(|| Mutex::new(Database::new()))
            .lock()
            .ok()?;
        let db = slot.as_mut()?;
        if db.stamp().load(Ordering::Acquire) == (((db.generation as u64) << 32) | db.cursor as u64)
        {
            return Some(read(&db.values));
        }
    }
    transaction(|tx| Ok(read(tx.values))).ok()
}
pub fn get(key: &str) -> Option<Vec<u8>> {
    read(|values| values.get(key).cloned()).flatten()
}
pub(crate) fn prefix(prefix: &str) -> Vec<(String, Vec<u8>)> {
    read(|values| {
        values
            .range(prefix.to_owned()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}
pub fn set(key: String, value: Vec<u8>) -> Result<(), u8> {
    transaction(|tx| {
        tx.set(key, value);
        Ok(())
    })
}
pub fn remove(key: &str) {
    let _ = transaction(|tx| {
        tx.remove(key);
        Ok(())
    });
}

pub fn window_owner(window: usize) -> Option<u32> {
    let value = get(&format!("w/{window}"))?;
    let owner = u32::from_le_bytes(value.get(..4)?.try_into().ok()?);
    let mut actual = 0;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            window as _,
            &raw mut actual,
        );
    }
    (owner == actual && owner != 0).then_some(owner)
}
pub fn valid(window: usize) -> bool {
    window == 1 || window_owner(window).is_some()
}
pub fn register_window(window: usize, parent: usize) {
    let mut value = pid().to_le_bytes().to_vec();
    value.extend_from_slice(&(parent as u64).to_le_bytes());
    let _ = set(format!("w/{window}"), value);
}
pub fn parent(window: usize) -> usize {
    get(&format!("w/{window}"))
        .and_then(|v| Some(u64::from_le_bytes(v.get(4..12)?.try_into().ok()?) as usize))
        .unwrap_or(1)
}
pub fn override_redirect(window: usize) -> bool {
    unsafe {
        !windows_sys::Win32::UI::WindowsAndMessaging::GetPropW(
            window as _,
            "KinakazeOverrideRedirect\0"
                .encode_utf16()
                .collect::<Vec<_>>()
                .as_ptr(),
        )
        .is_null()
    }
}
pub fn redirect_request(display: *mut crate::Display, window: usize, mut event: [u8; 32]) -> bool {
    if override_redirect(window) {
        return false;
    }
    let parent = parent(window);
    event[4..8].copy_from_slice(&(parent as u32).to_le_bytes());
    event[8..12].copy_from_slice(&(window as u32).to_le_bytes());
    if crate::connection::selected(display, parent) & (1 << 20) == 0
        && !crate::connection::recipients(parent, 1 << 20).is_empty()
    {
        crate::xext::protocol::queue_event(event);
        return true;
    }
    for owner in recipients(parent, 1 << 20) {
        if owner != pid() {
            return send(owner, event);
        }
    }
    false
}
pub fn structure(window: usize, kind: u8) {
    let parent = parent(window);
    let mut wire = [0u8; 32];
    wire[0] = kind;
    wire[8..12].copy_from_slice(&(window as u32).to_le_bytes());
    if kind == 16 || kind == 22 {
        if kind == 22 {
            wire[12..16]
                .copy_from_slice(&(crate::window_tree::sibling_below(window) as u32).to_le_bytes());
        }
        if let (Some((x, y)), Some((width, height))) = (
            crate::window_tree::origin(window),
            kinakaze_libdisplay::ui::client_size(window),
        ) {
            let start = if kind == 16 { 12 } else { 16 };
            for (index, value) in [x, y, width, height, 0].into_iter().enumerate() {
                wire[start + index * 2..start + index * 2 + 2]
                    .copy_from_slice(&(value as i16).to_le_bytes());
            }
            wire[start + 10] = u8::from(override_redirect(window));
        }
    } else if kind == 19 {
        wire[12] = u8::from(override_redirect(window));
    }
    if kind != 16 {
        wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
        broadcast(window, 1 << 17, wire, false);
    }
    wire[4..8].copy_from_slice(&(parent as u32).to_le_bytes());
    broadcast(parent, 1 << 19, wire, false);
}
pub fn forget(window: usize) {
    let _ = transaction(|tx| {
        for prefix in [
            format!("p/{window}/"),
            format!("m/{window}/"),
            format!("d/{window}/"),
            format!("shape/{window}/"),
            format!("shape-watch/{window}/"),
        ] {
            for (key, _) in tx.prefix(&prefix) {
                tx.remove(&key);
            }
        }
        for key in [
            format!("w/{window}"),
            format!("surface/{window}"),
            format!("r/{window}"),
        ] {
            tx.remove(&key);
        }
        Ok(())
    });
}
#[repr(C)]
pub struct CreateEvent {
    pub kind: i32,
    pub serial: u64,
    pub sent: i32,
    pub display: *mut crate::Display,
    pub parent: usize,
    pub window: usize,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub border: i32,
    pub override_redirect: i32,
}
#[repr(C)]
pub struct ConfigureRequest {
    pub kind: i32,
    pub serial: u64,
    pub sent: i32,
    pub display: *mut crate::Display,
    pub parent: usize,
    pub window: usize,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub border: i32,
    pub above: usize,
    pub detail: i32,
    pub mask: u64,
}
pub fn hierarchy(window: usize) -> Option<(Option<usize>, Vec<usize>)> {
    if !valid(window) {
        return None;
    }
    let parent = get(&format!("w/{window}"))
        .and_then(|v| Some(u64::from_le_bytes(v.get(4..12)?.try_into().ok()?) as usize));
    let rows = prefix("w/");
    let children = rows
        .into_iter()
        .filter_map(|(key, value)| {
            let id = key[2..].parse::<usize>().ok()?;
            let parent = u64::from_le_bytes(value.get(4..12)?.try_into().ok()?) as usize;
            (parent == window && valid(id)).then_some(id)
        })
        .collect();
    Some((parent.filter(|p| *p != 1), children))
}
pub fn subscribe(window: usize, mask: u32) -> Result<(), u8> {
    transaction(|tx| {
        let prefix = format!("m/{window}/");
        if mask & ((1 << 20) | (1 << 18) | (1 << 2)) != 0 {
            for (key, value) in tx.prefix(&prefix) {
                let owner = key[prefix.len()..].parse::<u32>().unwrap_or(0);
                let other = u32::from_le_bytes(value[..4].try_into().unwrap());
                if owner != pid()
                    && kinakaze_runtime::job::alive(owner)
                    && mask & other & ((1 << 20) | (1 << 18) | (1 << 2)) != 0
                {
                    return Err(10);
                }
            }
        }
        tx.set(format!("{prefix}{}", pid()), mask.to_le_bytes().to_vec());
        Ok(())
    })
}
pub fn selected_masks(window: usize) -> u32 {
    let prefix = format!("m/{window}/");
    self::prefix(&prefix)
        .into_iter()
        .fold(0, |mask, (key, value)| {
            let owner = key[prefix.len()..].parse::<u32>().unwrap_or(0);
            if kinakaze_runtime::job::alive(owner) {
                mask | u32::from_le_bytes(value[..4].try_into().unwrap())
            } else {
                mask
            }
        })
}
pub fn mapped(window: usize) -> bool {
    window == 1
        || unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetPropA(
                window as _,
                c"KinakazeXMapped".as_ptr().cast(),
            ) as usize
                == 1
        }
}
pub fn set_mapped(window: usize, mapped: bool) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetPropA(
            window as _,
            c"KinakazeXMapped".as_ptr().cast(),
            usize::from(mapped) as _,
        );
    }
}
pub fn recipients(window: usize, mask: u32) -> Vec<u32> {
    let prefix = format!("m/{window}/");
    self::prefix(&prefix)
        .into_iter()
        .filter_map(|(key, value)| {
            let owner = key[prefix.len()..].parse::<u32>().ok()?;
            let selected = u32::from_le_bytes(value.get(..4)?.try_into().ok()?);
            (selected & mask != 0 && kinakaze_runtime::job::alive(owner)).then_some(owner)
        })
        .collect()
}
pub fn send(owner: u32, wire: [u8; 32]) -> bool {
    send_delivery(owner, wire, None)
}
pub fn send_delivery(owner: u32, wire: [u8; 32], delivery: Option<(usize, u32)>) -> bool {
    if crate::trace_enabled() && matches!(wire[0], 17 | 20 | 23) {
        crate::diagnostic!(
            "[x11-shared] send pid={} to={owner} kind={} parent={} window={}",
            pid(),
            wire[0],
            u32::from_le_bytes(wire[4..8].try_into().unwrap()),
            u32::from_le_bytes(wire[8..12].try_into().unwrap())
        );
    }
    if owner == pid() {
        crate::xext::protocol::queue_delivery(wire, delivery);
        return true;
    }
    if !kinakaze_runtime::job::alive(owner) {
        return false;
    }
    let sent = transaction(|tx| {
        let next = format!("q/{owner}");
        let sequence = tx
            .get(&next)
            .map_or(0, |v| u64::from_le_bytes(v.try_into().unwrap()))
            .wrapping_add(1);
        let mut payload = wire.to_vec();
        if let Some((window, mask)) = delivery {
            payload.extend_from_slice(&(window as u32).to_le_bytes());
            payload.extend_from_slice(&mask.to_le_bytes());
        }
        tx.set(format!("e/{owner}/{sequence:020}"), payload);
        tx.set(next, sequence.to_le_bytes().to_vec());
        Ok(())
    })
    .is_ok();
    if sent {
        unsafe {
            let event = OpenEventW(2, 0, name(&format!("wake/{owner}")).as_ptr());
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
        }
    }
    sent
}
pub fn broadcast(window: usize, mask: u32, wire: [u8; 32], include_self: bool) {
    for owner in recipients(window, mask) {
        if include_self || owner != pid() {
            send(owner, wire);
        }
    }
}
pub fn damage(window: usize, rect: crate::XRectangle, width: u16, height: u16) {
    if window_owner(window) != Some(pid()) {
        return;
    }
    damage_from_child(window, rect, width, height);
}
/// A client can modify a foreign ancestor's backing surface through its child.
pub(crate) fn damage_from_child(window: usize, rect: crate::XRectangle, width: u16, height: u16) {
    let prefix = format!("d/{window}/");
    let owners = self::prefix(&prefix);
    let mut wire = [0u8; 32];
    wire[0] = 255;
    wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
    crate::xfixes::encode(&mut wire, 8, rect);
    wire[16..18].copy_from_slice(&width.to_le_bytes());
    wire[18..20].copy_from_slice(&height.to_le_bytes());
    for (key, _) in owners {
        if let Ok(owner) = key[prefix.len()..].parse::<u32>() {
            if owner != pid() {
                send(owner, wire);
            }
        }
    }
}
pub fn send_xevent(
    display: *mut crate::Display,
    recipient: usize,
    event: &mut crate::XEvent,
) -> bool {
    let Some(owner) = window_owner(recipient) else {
        return false;
    };
    let mut wire = [0u8; 32];
    if unsafe { crate::event_wire::_XEventToWire(display, event, wire.as_mut_ptr()) } == 0 {
        return false;
    }
    send(owner, wire)
}
struct Receiver {
    event: usize,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
static RECEIVER: Mutex<Option<Receiver>> = Mutex::new(None);
pub fn open() {
    let mut receiver = RECEIVER.lock().unwrap();
    if receiver.is_some() {
        return;
    }
    let event = unsafe {
        CreateEventW(
            core::ptr::null(),
            0,
            1,
            name(&format!("wake/{}", pid())).as_ptr(),
        )
    } as usize;
    if event == 0 {
        return;
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled = stop.clone();
    let thread = std::thread::Builder::new()
        .name("kinakaze-x11-receive".into())
        .spawn(move || {
            loop {
                unsafe {
                    WaitForSingleObject(event as _, u32::MAX);
                }
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let events = transaction(|tx| {
                    let rows = tx.prefix(&format!("e/{}/", pid()));
                    for (key, _) in &rows {
                        tx.remove(key);
                    }
                    Ok(rows)
                })
                .unwrap_or_default();
                for (_, bytes) in events {
                    if let Some(wire) = bytes
                        .get(..32)
                        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                    {
                        if crate::trace_enabled() && matches!(wire[0], 17 | 20 | 23) {
                            crate::diagnostic!(
                                "[x11-shared] receive pid={} kind={} window={}",
                                pid(),
                                wire[0],
                                u32::from_le_bytes(wire[8..12].try_into().unwrap())
                            );
                        }
                        if wire[0] == 253 {
                            crate::shape::receive(&wire);
                        } else if wire[0] == 254 {
                            crate::notify_event();
                        } else if wire[0] == 255 {
                            let n = |offset| {
                                u16::from_le_bytes(wire[offset..offset + 2].try_into().unwrap())
                            };
                            crate::damage::changed(
                                u32::from_le_bytes(wire[4..8].try_into().unwrap()) as usize,
                                crate::XRectangle {
                                    x: n(8) as i16,
                                    y: n(10) as i16,
                                    width: n(12),
                                    height: n(14),
                                },
                                n(16),
                                n(18),
                            );
                        } else {
                            let delivery = (bytes.len() == 40).then(|| {
                                (
                                    u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize,
                                    u32::from_le_bytes(bytes[36..40].try_into().unwrap()),
                                )
                            });
                            crate::xext::protocol::queue_delivery(wire, delivery);
                        }
                    }
                }
            }
        })
        .ok();
    *receiver = Some(Receiver {
        event,
        stop,
        thread,
    });
}
pub fn close() {
    let Some(mut receiver) = RECEIVER.lock().unwrap().take() else {
        return;
    };
    receiver
        .stop
        .store(true, std::sync::atomic::Ordering::Release);
    unsafe {
        SetEvent(receiver.event as _);
    }
    if let Some(thread) = receiver.thread.take() {
        let _ = thread.join();
    }
    unsafe {
        CloseHandle(receiver.event as _);
    }
    let _ = transaction(|tx| {
        for (key, _) in tx.prefix("m/") {
            if key.ends_with(&format!("/{}", pid())) {
                tx.remove(&key);
            }
        }
        for (key, _) in tx.prefix(&format!("e/{}/", pid())) {
            tx.remove(&key);
        }
        tx.remove(&format!("q/{}", pid()));
        Ok(())
    });
}
