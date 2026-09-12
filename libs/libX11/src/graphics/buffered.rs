//! Window-linked drawing surfaces. Multiple names share one physical surface;
//! presentation exchanges the surfaces while keeping drawable names stable.
use super::*;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
pub struct SwapRequest {
    pub window: Window,
    pub action: u8,
}

struct Alternate {
    buffer: BackBuffer,
    references: usize,
}
#[derive(Default)]
struct State {
    names: HashMap<usize, (Window, usize)>,
    windows: HashMap<Window, Alternate>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn extent(window: Window) -> Option<(i32, i32)> {
    let mut rect: RECT = unsafe { core::mem::zeroed() };
    (unsafe { GetClientRect(window as HWND, &mut rect) } != 0 && rect.right > 0 && rect.bottom > 0)
        .then_some((rect.right, rect.bottom))
}
fn background_buffer(window: Window, width: i32, height: i32) -> Option<BackBuffer> {
    let mut buffer = create_pixmap_buffer(width as u32, height as u32, 24)?;
    buffer.resize_background = true;
    clear(
        &buffer,
        window,
        RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        },
    );
    Some(buffer)
}
fn clear(buffer: &BackBuffer, window: Window, rect: RECT) {
    synchronize_buffer(buffer);
    let pixels = unsafe {
        core::slice::from_raw_parts_mut(
            buffer.bits as *mut u32,
            buffer.width as usize * buffer.height as usize,
        )
    };
    background::paint(window, pixels, buffer.width, buffer.height, rect);
}
fn resize(window: Window, alternate: &mut Alternate) -> bool {
    let Some((width, height)) = extent(window) else {
        return false;
    };
    if (width, height) != (alternate.buffer.width, alternate.buffer.height) {
        let Some(replacement) = background_buffer(window, width, height) else {
            return false;
        };
        let old = core::mem::replace(&mut alternate.buffer, replacement);
        unsafe {
            delete_back_buffer(old);
        }
    }
    true
}

/// Return the closure unchanged for ordinary drawables. This keeps callers'
/// borrowed image storage on the stack and avoids cloning a complete frame.
pub(super) fn access<T, F: FnOnce(&mut BackBuffer) -> T>(
    name: usize,
    apply: F,
) -> Result<Option<T>, F> {
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return Err(apply);
    }
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    let Some(&(window, _)) = state.names.get(&name) else {
        return Err(apply);
    };
    let alternate = state.windows.get_mut(&window).unwrap();
    if !resize(window, alternate) {
        return Ok(None);
    }
    Ok(Some(apply(&mut alternate.buffer)))
}

/// Errors are X11 BadWindow, BadMatch or BadAlloc, independent of any extension.
pub fn allocate(window: Window, owner: usize) -> Result<usize, u8> {
    if kinakaze_libdisplay::window::logical_for_native(window).is_none() {
        return Err(3);
    }
    let Some((width, height)) = extent(window) else {
        return Err(3);
    };
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    if !state.windows.contains_key(&window) {
        let buffer = background_buffer(window, width, height).ok_or(11u8)?;
        if !prepare_back_buffer(window, false) {
            unsafe {
                delete_back_buffer(buffer);
            }
            return Err(11);
        }
        if let Some(front) = back_buffers()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&window)
        {
            front.resize_background = true;
        }
        state.windows.insert(
            window,
            Alternate {
                buffer,
                references: 0,
            },
        );
    }
    let id = allocate_drawable_id();
    state.windows.get_mut(&window).unwrap().references += 1;
    state.names.insert(id, (window, owner));
    ACTIVE.store(state.names.len(), Ordering::Release);
    Ok(id)
}
fn release(state: &mut State, id: usize) {
    let Some((window, _)) = state.names.remove(&id) else {
        return;
    };
    let alternate = state.windows.get_mut(&window).unwrap();
    alternate.references -= 1;
    if alternate.references == 0 {
        let alternate = state.windows.remove(&window).unwrap();
        unsafe {
            delete_back_buffer(alternate.buffer);
        }
        if let Some(front) = back_buffers()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&window)
        {
            front.resize_background = false;
        }
    }
    ACTIVE.store(state.names.len(), Ordering::Release);
}
pub fn deallocate(id: usize, owner: usize) -> bool {
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    if !state
        .names
        .get(&id)
        .is_some_and(|&(_, current)| current == owner)
    {
        return false;
    }
    release(&mut state, id);
    true
}
pub fn window(id: usize, owner: usize) -> Option<Window> {
    let state = state().lock().unwrap_or_else(|e| e.into_inner());
    state
        .names
        .get(&id)
        .and_then(|&(window, current)| (current == owner).then_some(window))
}
pub fn close(owner: usize) {
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    let ids: Vec<_> = state
        .names
        .iter()
        .filter_map(|(&id, &(_, current))| (current == owner).then_some(id))
        .collect();
    for id in ids {
        release(&mut state, id);
    }
}
pub(super) fn forget(window: Window) {
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    state.names.retain(|_, (current, _)| *current != window);
    if let Some(alternate) = state.windows.remove(&window) {
        unsafe {
            delete_back_buffer(alternate.buffer);
        }
    }
    ACTIVE.store(state.names.len(), Ordering::Release);
}
pub(super) fn clear_area(window: Window, rect: RECT) {
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(alternate) = state.windows.get_mut(&window)
        && resize(window, alternate)
    {
        clear(&alternate.buffer, window, rect);
    }
}

/// Validate the whole request before exchanging any surface. Holding the front
/// map excludes the presenter for the entire batch. Undefined/Untouched swap
/// ownership in constant space; only Copied copies pixel storage.
pub fn swap(requests: &[SwapRequest]) -> Result<(), (u8, usize)> {
    let mut state = state().lock().unwrap_or_else(|e| e.into_inner());
    for (index, request) in requests.iter().enumerate() {
        let (window, action) = (request.window, request.action);
        if action > 3 {
            return Err((2, action as usize));
        }
        if requests[..index].iter().any(|other| other.window == window) {
            return Err((8, window));
        }
        let alternate = state.windows.get_mut(&window).ok_or((3, window))?;
        if !resize(window, alternate) || !prepare_back_buffer(window, false) {
            return Err((11, window));
        }
    }
    let mut fronts = back_buffers().lock().unwrap_or_else(|e| e.into_inner());
    for request in requests {
        let window = request.window;
        let front = fronts.get(&window).ok_or((3, window))?;
        let back = &state.windows[&window].buffer;
        if (front.width, front.height) != (back.width, back.height) {
            return Err((8, window));
        }
    }
    for request in requests {
        let (window, action) = (request.window, request.action);
        let front = fronts.get_mut(&window).unwrap();
        let back = &mut state.windows.get_mut(&window).unwrap().buffer;
        synchronize_buffer(front);
        synchronize_buffer(back);
        core::mem::swap(front, back);
        let rect = RECT {
            left: 0,
            top: 0,
            right: front.width,
            bottom: front.height,
        };
        match action {
            1 => clear(back, window, rect),
            3 => unsafe {
                core::ptr::copy_nonoverlapping(
                    front.bits as *const u32,
                    back.bits as *mut u32,
                    front.width as usize * front.height as usize,
                );
            },
            _ => {}
        }
        front.dirty = Some(rect);
        back.dirty = None;
    }
    drop(fronts);
    drop(state);
    if !requests.is_empty() {
        request_present();
    }
    Ok(())
}

/// Native GDI resources cannot be copied into a forked worker. Keep the empty
/// resource table frozen through the snapshot, or reject before creating it.
pub struct ForkGuard {
    _state: MutexGuard<'static, State>,
}
pub fn freeze() -> Result<ForkGuard, i32> {
    let state = state().try_lock().map_err(|_| 11)?;
    if !state.names.is_empty() {
        return Err(11);
    }
    Ok(ForkGuard { _state: state })
}
