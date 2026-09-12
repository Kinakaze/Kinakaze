//! Stable guest window ids and the process-local HWND mapping.
//!
//! An HWND belongs to one process and to the UI thread that created it.  It is
//! therefore a rebuildable host object, not state that can be copied at fork.
//! Guest ids stay stable while this table creates a new HWND in the child.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::event::{self, EVENT_REBOUND, Event};
use crate::ui;

pub mod x11;

const FORK_MAGIC: u64 = 0x4352_5957_494e_4633; // "CRYWINF3"
const FORK_KEY: u64 = 0x4449_5350_4c41_5932; // "DISPLAY2"

#[derive(Clone)]
struct WindowState {
    native: usize,
    parent: Option<u64>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: String,
    visible: bool,
}

fn windows() -> &'static Mutex<HashMap<u64, WindowState>> {
    static WINDOWS: OnceLock<Mutex<HashMap<u64, WindowState>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn create(width: i32, height: i32, title: &str) -> Option<u64> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).max(1);
    create_with_id(id, None, 0, 0, width, height, title, true).then_some(id)
}

/// Creates a child whose geometry is relative to the parent's client area.
pub fn create_child(
    parent: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
) -> Option<u64> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).max(1);
    create_with_id(id, Some(parent), x, y, width, height, title, true).then_some(id)
}

/// X11 creates windows unmapped; other frontends choose their own visibility.
pub fn create_unmapped(
    parent: Option<u64>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
) -> Option<u64> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).max(1);
    create_with_id(id, parent, x, y, width, height, title, false).then_some(id)
}

fn create_with_id(
    id: u64,
    parent: Option<u64>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
    visible: bool,
) -> bool {
    let Ok(mut registry) = windows().lock() else {
        return false;
    };
    let parent_native = match parent {
        Some(parent) => match registry.get(&parent) {
            Some(state) => Some(state.native),
            None => return false,
        },
        None => None,
    };
    let native = match parent_native {
        Some(parent_native) => {
            ui::create_child_for_logical(id, parent_native, x, y, width, height, title, visible)
        }
        None => ui::create_for_logical_at(id, x, y, width, height, title, visible),
    };
    let Some(native) = native else { return false };
    registry.insert(
        id,
        WindowState {
            native,
            parent,
            x,
            y,
            width,
            height,
            title: title.to_owned(),
            visible,
        },
    );
    true
}

pub fn destroy(id: u64) -> bool {
    fn take_subtree(
        registry: &mut HashMap<u64, WindowState>,
        id: u64,
        removed: &mut Vec<WindowState>,
    ) {
        let children = registry
            .iter()
            .filter_map(|(child, state)| (state.parent == Some(id)).then_some(*child))
            .collect::<Vec<_>>();
        for child in children {
            take_subtree(registry, child, removed);
        }
        if let Some(state) = registry.remove(&id) {
            removed.push(state);
        }
    }

    let Ok(mut registry) = windows().lock() else {
        return false;
    };
    let mut removed = Vec::new();
    take_subtree(&mut registry, id, &mut removed);
    x11::retain(&registry);
    drop(registry);
    !removed.is_empty() && removed.into_iter().all(|state| ui::destroy(state.native))
}

/// Rebuildable ownership follows the native parent, including after fork.
pub fn reparent(id: u64, parent: Option<u64>, x: i32, y: i32) -> Result<(), u8> {
    let mut registry = windows().lock().map_err(|_| 17u8)?;
    let native = registry.get(&id).ok_or(3u8)?.native;
    let mut cursor = parent;
    while let Some(ancestor) = cursor {
        if ancestor == id {
            return Err(8);
        }
        cursor = registry.get(&ancestor).ok_or(3u8)?.parent;
    }
    let target = parent.map(|parent| registry[&parent].native).unwrap_or(0);
    if !ui::reparent(native, target, x, y) {
        return Err(8);
    }
    let state = registry.get_mut(&id).unwrap();
    state.parent = parent;
    state.x = x;
    state.y = y;
    Ok(())
}

pub fn set_visible(id: u64, visible: bool) -> bool {
    let Ok(mut windows) = windows().lock() else {
        return false;
    };
    let Some(state) = windows.get_mut(&id) else {
        return false;
    };
    if !ui::set_visible(state.native, visible) {
        return false;
    }
    state.visible = visible;
    true
}

pub fn set_title(id: u64, title: &str) -> bool {
    let Ok(mut windows) = windows().lock() else {
        return false;
    };
    let Some(state) = windows.get_mut(&id) else {
        return false;
    };
    if !ui::set_title(state.native, title) {
        return false;
    }
    state.title = title.to_owned();
    true
}

pub fn client_size(id: u64) -> Option<(i32, i32)> {
    let native = windows().lock().ok()?.get(&id)?.native;
    ui::client_size(native)
}

pub fn native_handle(id: u64) -> Option<usize> {
    windows().lock().ok()?.get(&id).map(|state| state.native)
}

pub fn logical_for_native(native: usize) -> Option<u64> {
    windows()
        .lock()
        .ok()?
        .iter()
        .find_map(|(id, state)| (state.native == native).then_some(*id))
}

/// Snapshot the owned hierarchy; the caller can order children by native Z order.
/// `None` identifies the desktop, without exposing an X11 root id here.
pub fn hierarchy(native: Option<usize>) -> Option<(Option<usize>, Vec<usize>)> {
    let registry = windows().lock().ok()?;
    let (id, parent) = if let Some(native) = native {
        let (id, state) = registry.iter().find(|(_, state)| state.native == native)?;
        (Some(*id), state.parent.map(|id| registry[&id].native))
    } else {
        (None, None)
    };
    let children = registry
        .values()
        .filter_map(|state| (state.parent == id).then_some(state.native))
        .collect();
    Some((parent, children))
}

fn serialize() -> Option<Vec<u8>> {
    let mut registry = windows().lock().ok()?;
    let pending = event::fork_snapshot();
    let mut out = Vec::new();
    out.extend_from_slice(&FORK_MAGIC.to_le_bytes());
    out.extend_from_slice(&(registry.len() as u32).to_le_bytes());
    out.extend_from_slice(&(pending.0.len() as u32).to_le_bytes());
    out.extend_from_slice(&pending.1.to_le_bytes());
    for (id, state) in registry.iter_mut() {
        if let Some((width, height)) = ui::client_size(state.native) {
            state.width = width;
            state.height = height;
        }
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&state.parent.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&state.x.to_le_bytes());
        out.extend_from_slice(&state.y.to_le_bytes());
        out.extend_from_slice(&state.width.to_le_bytes());
        out.extend_from_slice(&state.height.to_le_bytes());
        let flags =
            state.visible as u32 | ((ui::input_only::is_input_only(state.native) as u32) << 1);
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&(state.title.len() as u32).to_le_bytes());
        out.extend_from_slice(state.title.as_bytes());
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }
    }
    for item in pending.0 {
        // Event is a flat repr(C) ABI record with no padding carrying references.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&item as *const Event).cast::<u8>(),
                core::mem::size_of::<Event>(),
            )
        };
        out.extend_from_slice(bytes);
    }
    Some(out)
}

unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let Some(payload) = serialize() else {
        return -1;
    };
    if buffer.is_null() {
        return payload.len() as isize;
    }
    if capacity < payload.len() {
        return -1;
    }
    unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len()) };
    payload.len() as isize
}

unsafe extern "system" fn child(payload: *const u8, len: usize) -> i32 {
    if payload.is_null() || len < 24 {
        return 22;
    }
    let bytes = unsafe { std::slice::from_raw_parts(payload, len) };
    if u64::from_le_bytes(bytes[0..8].try_into().unwrap_or_default()) != FORK_MAGIC {
        return 22;
    }
    let window_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap_or_default()) as usize;
    let event_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap_or_default()) as usize;
    let dropped = u64::from_le_bytes(bytes[16..24].try_into().unwrap_or_default());
    let mut cursor = 24usize;
    let mut descriptions = Vec::with_capacity(window_count);
    for _ in 0..window_count {
        if cursor + 40 > bytes.len() {
            return 22;
        }
        let id = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap_or_default());
        let parent = u64::from_le_bytes(
            bytes[cursor + 8..cursor + 16]
                .try_into()
                .unwrap_or_default(),
        );
        let parent = (parent != 0).then_some(parent);
        let x = i32::from_le_bytes(
            bytes[cursor + 16..cursor + 20]
                .try_into()
                .unwrap_or_default(),
        );
        let y = i32::from_le_bytes(
            bytes[cursor + 20..cursor + 24]
                .try_into()
                .unwrap_or_default(),
        );
        let width = i32::from_le_bytes(
            bytes[cursor + 24..cursor + 28]
                .try_into()
                .unwrap_or_default(),
        );
        let height = i32::from_le_bytes(
            bytes[cursor + 28..cursor + 32]
                .try_into()
                .unwrap_or_default(),
        );
        let flags = u32::from_le_bytes(
            bytes[cursor + 32..cursor + 36]
                .try_into()
                .unwrap_or_default(),
        );
        let visible = flags & 1 != 0;
        let input_only = flags & 2 != 0;
        let title_len = u32::from_le_bytes(
            bytes[cursor + 36..cursor + 40]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let start = cursor + 40;
        let Some(end) = start.checked_add(title_len) else {
            return 22;
        };
        let Some(title) = bytes
            .get(start..end)
            .and_then(|title| std::str::from_utf8(title).ok())
        else {
            return 22;
        };
        descriptions.push((
            id,
            parent,
            x,
            y,
            width,
            height,
            title.to_owned(),
            visible,
            input_only,
        ));
        cursor = end.next_multiple_of(8);
    }
    let event_bytes = match event_count
        .checked_mul(core::mem::size_of::<Event>())
        .and_then(|size| cursor.checked_add(size))
    {
        Some(end) if end == bytes.len() => &bytes[cursor..end],
        _ => return 22,
    };
    if let Ok(mut registry) = windows().lock() {
        registry.clear();
        x11::clear();
    } else {
        return 11;
    }
    let mut max_id = 0u64;
    let mut restored = HashSet::with_capacity(window_count);
    let mut remaining = descriptions;
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for (id, parent, x, y, width, height, title, visible, input_only) in remaining {
            if parent.is_some_and(|parent| !restored.contains(&parent)) {
                deferred.push((id, parent, x, y, width, height, title, visible, input_only));
                continue;
            }
            if !create_with_id(id, parent, x, y, width, height, &title, false) {
                return 5;
            }
            if input_only {
                ui::input_only::set(native_handle(id).unwrap());
            }
            if visible {
                let mut registry = windows().lock().unwrap_or_else(|e| e.into_inner());
                let state = registry.get_mut(&id).unwrap();
                state.visible = true;
                ui::presentation::defer_until_frame(state.native);
            }
            restored.insert(id);
            max_id = max_id.max(id);
        }
        if deferred.len() == before {
            // A missing parent or a cycle is not a restorable Linux window tree.
            return 22;
        }
        remaining = deferred;
    }
    NEXT_ID.store(max_id.saturating_add(1).max(1), Ordering::Release);
    let mut restored_events = Vec::with_capacity(event_count + window_count);
    for chunk in event_bytes.chunks_exact(core::mem::size_of::<Event>()) {
        // SAFETY: chunks are exactly one repr(C) Event and read_unaligned avoids
        // imposing alignment on the byte frame.
        restored_events.push(unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<Event>()) });
    }
    for id in windows()
        .lock()
        .map(|windows| windows.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        restored_events.push(Event {
            kind: EVENT_REBOUND,
            window: id,
            ..Event::default()
        });
    }
    event::fork_restore(restored_events, dropped);
    0
}

fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 200,
        key: FORK_KEY,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(child),
    });
}

extern "C" fn initializer() {
    register();
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = initializer;
