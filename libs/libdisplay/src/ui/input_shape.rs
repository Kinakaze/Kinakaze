//! Optional input regions; an empty region makes a compositor overlay click through.
use std::{collections::BTreeMap, sync::Mutex};
type Rectangles = Vec<(i32, i32, i32, i32)>;
static REGIONS: Mutex<BTreeMap<usize, Rectangles>> = Mutex::new(BTreeMap::new());
pub fn set(window: usize, rects: Option<Rectangles>) {
    let mut all = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = rects {
        all.insert(window, r);
    } else {
        all.remove(&window);
    }
    drop(all);
    super::desktop::input_region_changed(window);
}
pub fn contains(window: usize, x: i32, y: i32) -> bool {
    REGIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&window)
        .is_none_or(|r| {
            r.iter()
                .any(|&(l, t, r, b)| x >= l && x < r && y >= t && y < b)
        })
}
pub fn clear() {
    REGIONS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
