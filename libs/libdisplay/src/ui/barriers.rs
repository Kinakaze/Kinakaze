//! Axis-aligned XFixes barriers applied to relative mouse motion in screen space.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    },
};
#[derive(Clone, Copy)]
pub struct Barrier {
    pub window: usize,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub directions: u32,
}
struct Tracked {
    barrier: Barrier,
    event: u32,
    last_time: u64,
    released: bool,
}
#[derive(Clone, Copy)]
pub struct Notice {
    pub barrier: u32,
    pub event: u32,
    pub window: usize,
    pub leave: bool,
    pub time: u64,
    pub dtime: i32,
    pub flags: i32,
    pub x: i32,
    pub y: i32,
    pub dx: i32,
    pub dy: i32,
}
static NEXT: AtomicU32 = AtomicU32::new(1);
static BARRIERS: Mutex<BTreeMap<u32, Tracked>> = Mutex::new(BTreeMap::new());
static NOTICES: Mutex<VecDeque<Notice>> = Mutex::new(VecDeque::new());
static LAST: Mutex<Option<(i32, i32)>> = Mutex::new(None);
fn emit(notice: Notice) {
    let mut queue = NOTICES.lock().unwrap_or_else(|e| e.into_inner());
    if queue.len() >= crate::event::CAPACITY {
        queue.pop_front();
    }
    queue.push_back(notice);
}
pub fn notices() -> Vec<Notice> {
    NOTICES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect()
}
pub fn create(id: u32, barrier: Barrier) -> bool {
    let mut all = BARRIERS.lock().unwrap_or_else(|e| e.into_inner());
    if id == 0 || all.contains_key(&id) {
        return false;
    }
    all.insert(
        id,
        Tracked {
            barrier,
            event: 0,
            last_time: 0,
            released: false,
        },
    );
    true
}
pub fn destroy(id: u32) -> bool {
    let old = BARRIERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    if let Some(b) = &old {
        if b.event != 0 {
            let (x, y) = LAST
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_default();
            emit(Notice {
                barrier: id,
                event: b.event,
                window: b.barrier.window,
                leave: true,
                time: crate::event::timestamp(),
                dtime: 0,
                flags: 2,
                x,
                y,
                dx: 0,
                dy: 0,
            });
            crate::event::wake();
        }
    }
    old.is_some()
}
pub fn release(id: u32, event: u32) -> Result<(), ()> {
    let mut all = BARRIERS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(b) = all.get_mut(&id) else {
        return Err(());
    };
    if b.event == event && event != 0 {
        b.released = true;
    }
    Ok(())
}
pub fn clear() {
    BARRIERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    NOTICES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
pub fn warped(x: i32, y: i32) {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((x, y));
    let mut all = BARRIERS.lock().unwrap_or_else(|e| e.into_inner());
    for (&id, b) in all.iter_mut() {
        if b.event != 0 {
            emit(Notice {
                barrier: id,
                event: b.event,
                window: b.barrier.window,
                leave: true,
                time: crate::event::timestamp(),
                dtime: 0,
                flags: 0,
                x,
                y,
                dx: 0,
                dy: 0,
            });
            b.event = 0;
            b.released = false;
        }
    }
    drop(all);
    crate::event::wake();
}
fn constrain(b: Barrier, old: (i32, i32), next: (i32, i32)) -> (i32, i32) {
    let (mut x, mut y) = next;
    let (px, py) = old;
    if b.x1 == b.x2 && x != px {
        let positive = px < b.x1 && x >= b.x1;
        let negative = px >= b.x1 && x < b.x1;
        if (positive && b.directions & 1 == 0) || (negative && b.directions & 4 == 0) {
            let t = (b.x1 - px) as f64 / (x - px) as f64;
            let cross = py as f64 + t * (y - py) as f64;
            if cross >= b.y1.min(b.y2) as f64 && cross < b.y1.max(b.y2) as f64 {
                x = b.x1 - i32::from(positive);
            }
        }
    } else if b.y1 == b.y2 && y != py {
        let positive = py < b.y1 && y >= b.y1;
        let negative = py >= b.y1 && y < b.y1;
        if (positive && b.directions & 2 == 0) || (negative && b.directions & 8 == 0) {
            let t = (b.y1 - py) as f64 / (y - py) as f64;
            let cross = px as f64 + t * (x - px) as f64;
            if cross >= b.x1.min(b.x2) as f64 && cross < b.x1.max(b.x2) as f64 {
                y = b.y1 - i32::from(positive);
            }
        }
    }
    (x, y)
}
pub(super) fn motion(x: i32, y: i32) -> (i32, i32) {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let mut next = (x, y);
    if let Some(old) = *last {
        let now = crate::event::timestamp();
        for (&id, b) in BARRIERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
        {
            let constrained = constrain(b.barrier, old, next);
            if constrained != next && !b.released {
                if b.event == 0 {
                    b.event = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
                    b.last_time = now;
                }
                emit(Notice {
                    barrier: id,
                    event: b.event,
                    window: b.barrier.window,
                    leave: false,
                    time: now,
                    dtime: now.saturating_sub(b.last_time).min(i32::MAX as u64) as i32,
                    flags: 0,
                    x: constrained.0,
                    y: constrained.1,
                    dx: x - old.0,
                    dy: y - old.1,
                });
                b.last_time = now;
                next = constrained;
            } else if b.event != 0 && (next != old || b.released) {
                emit(Notice {
                    barrier: id,
                    event: b.event,
                    window: b.barrier.window,
                    leave: true,
                    time: now,
                    dtime: now.saturating_sub(b.last_time).min(i32::MAX as u64) as i32,
                    flags: if b.released { 2 } else { 0 },
                    x: next.0,
                    y: next.1,
                    dx: x - old.0,
                    dy: y - old.1,
                });
                b.event = 0;
                b.released = false;
            }
        }
    }
    *last = Some(next);
    drop(last);
    crate::event::wake();
    next
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn barriers_block_crossings_but_allow_direction_and_endpoints() {
        let mut b = Barrier {
            window: 1,
            x1: 10,
            y1: 0,
            x2: 10,
            y2: 20,
            directions: 0,
        };
        assert_eq!(constrain(b, (0, 5), (30, 5)), (9, 5));
        assert_eq!(constrain(b, (30, 5), (0, 5)), (10, 5));
        assert_eq!(constrain(b, (0, 20), (30, 20)), (30, 20));
        b.directions = 1;
        assert_eq!(constrain(b, (0, 5), (30, 5)), (30, 5));
        let b = Barrier {
            window: 1,
            x1: 0,
            y1: 10,
            x2: 20,
            y2: 10,
            directions: 0,
        };
        assert_eq!(constrain(b, (5, 0), (5, 30)), (5, 9));
    }
    #[test]
    fn barrier_hit_release_leave_and_stale_release() {
        clear();
        assert!(create(
            99,
            Barrier {
                window: 1,
                x1: 10,
                y1: 0,
                x2: 10,
                y2: 20,
                directions: 0
            }
        ));
        warped(0, 5);
        assert_eq!(motion(30, 5), (9, 5));
        let hit = notices();
        assert_eq!(hit.len(), 1);
        assert!(!hit[0].leave);
        assert_eq!((hit[0].dx, hit[0].x), (30, 9));
        release(99, hit[0].event.wrapping_add(1)).unwrap();
        assert_eq!(motion(20, 5), (9, 5));
        assert_eq!(notices()[0].event, hit[0].event);
        release(99, hit[0].event).unwrap();
        assert_eq!(motion(20, 5), (20, 5));
        let leave = notices();
        assert_eq!(leave.len(), 1);
        assert!(leave[0].leave);
        assert_eq!(leave[0].flags, 2);
        assert!(destroy(99));
        clear();
    }
}
