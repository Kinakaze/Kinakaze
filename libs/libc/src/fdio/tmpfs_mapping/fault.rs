//! The native fault entry first checks an immutable integer-only index. It
//! releases its reader before acquiring any VMA transaction or doing I/O.
use super::*;
static INDEX: AtomicUsize = AtomicUsize::new(0);
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static READERS: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

pub(super) fn publish(registry: &HashMap<usize, Mapping>) {
    let mut ranges: Vec<(usize, usize)> = registry
        .iter()
        .filter(|(_, m)| m.tmpfs.is_some())
        .map(|(&start, m)| (start, start + m.length))
        .collect();
    ranges.sort_unstable();
    let next = if ranges.is_empty() {
        0
    } else {
        Box::into_raw(Box::new(ranges)) as usize
    };
    let epoch = EPOCH.load(Ordering::SeqCst);
    let previous = INDEX.swap(next, Ordering::SeqCst);
    EPOCH.fetch_add(1, Ordering::SeqCst);
    while READERS[epoch & 1].load(Ordering::SeqCst) != 0 {
        std::thread::yield_now();
    }
    if previous != 0 {
        unsafe { drop(Box::from_raw(previous as *mut Vec<(usize, usize)>)) };
    }
}

pub(crate) fn classify(address: usize, access: usize) -> i32 {
    let epoch = loop {
        let epoch = EPOCH.load(Ordering::SeqCst);
        READERS[epoch & 1].fetch_add(1, Ordering::SeqCst);
        if EPOCH.load(Ordering::SeqCst) == epoch {
            break epoch;
        }
        READERS[epoch & 1].fetch_sub(1, Ordering::SeqCst);
    };
    let index = INDEX.load(Ordering::SeqCst);
    let found = if index == 0 {
        false
    } else {
        let ranges = unsafe { &*(index as *const Vec<(usize, usize)>) };
        let at = ranges.partition_point(|range| range.0 <= address);
        at != 0 && address < ranges[at - 1].1
    };
    READERS[epoch & 1].fetch_sub(1, Ordering::SeqCst);
    if found {
        resolve(address, access).unwrap_or(7)
    } else {
        0
    }
}
