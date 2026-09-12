use super::*;

static REMAINDERS: Mutex<Option<HashMap<usize, [i32; 2]>>> = Mutex::new(None);

pub(super) fn accumulate(window: usize, horizontal: bool, delta: i32) -> i32 {
    let mut state = REMAINDERS.lock().unwrap_or_else(|e| e.into_inner());
    let remainder = &mut state
        .get_or_insert_with(HashMap::new)
        .entry(window)
        .or_default()[usize::from(horizontal)];
    *remainder += delta;
    let notches = *remainder / WHEEL_DELTA;
    *remainder %= WHEEL_DELTA;
    notches
}

pub(super) fn forget(window: usize) {
    if let Some(state) = REMAINDERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        state.remove(&window);
    }
}
