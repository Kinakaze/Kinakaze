//! Linux x86-64 robust-list registration and bounded task-exit traversal.
//! Never invoke guest code, hold the registration table, or trust list pointers
//! while traversing. The pending entry covers interrupted insertion/removal.

use super::*;
use core::cell::Cell;

const WAITERS: u32 = 0x8000_0000;
const OWNER_DIED: u32 = 0x4000_0000;
const TID_MASK: u32 = 0x3fff_ffff;
const WALK_LIMIT: usize = 2048; // Linux 6.12 ROBUST_LIST_LIMIT

#[repr(C)]
#[derive(Clone, Copy)]
struct Head {
    next: usize,
    offset: isize,
    pending: usize,
}

static TABLE: AtomicPtr<Mutex<HashMap<i32, usize>>> = AtomicPtr::new(core::ptr::null_mut());

fn table() -> &'static Mutex<HashMap<i32, usize>> {
    let mut pointer = TABLE.load(Ordering::Acquire);
    if pointer.is_null() {
        let candidate = Box::into_raw(Box::new(Mutex::new(HashMap::new())));
        pointer = match TABLE.compare_exchange(
            core::ptr::null_mut(),
            candidate,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => candidate,
            Err(existing) => {
                unsafe { drop(Box::from_raw(candidate)) };
                existing
            }
        };
    }
    unsafe { &*pointer }
}

#[derive(Clone, Copy, Default)]
struct Registration {
    head: usize,
    tid: i32,
}

struct Task(Cell<Registration>);
impl Drop for Task {
    fn drop(&mut self) {
        finish(self.0.replace(Registration::default()));
    }
}

thread_local! {
    static TASK: Task = const { Task(Cell::new(Registration { head: 0, tid: 0 })) };
}

pub(super) fn set(head: usize, length: usize) -> i64 {
    if length != size_of::<Head>() {
        return -i64::from(EINVAL);
    }
    // Linux registers even null/unreadable pointers; exit traversal validates.
    let tid = current_tid();
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(tid, head);
    TASK.with(|task| task.0.set(Registration { head, tid }));
    libpthread::install_kernel_thread_exit(exit_hook);
    0
}

pub(super) fn get(tid: i32, output: usize, length: usize) -> i64 {
    let tid = if tid == 0 { current_tid() } else { tid };
    let head = table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&tid)
        .copied();
    let head = match head {
        Some(head) => head,
        None if tid == current_tid()
            || kinakaze_vfs::interrupt::thread_exists(crate::fsextra::native_signal_tid(tid)) =>
        {
            0
        }
        None => return -i64::from(ESRCH),
    };
    // Linux writes the length first, even when the head output then faults.
    let result = crate::ptrace::write_value(length, size_of::<Head>())
        .and_then(|()| crate::ptrace::write_value(output, head));
    result.map_or_else(|e| -i64::from(e), |()| 0)
}

extern "sysv64" fn exit_hook() {
    exit_current();
}

pub(crate) fn install_exit_hook() {
    libpthread::install_kernel_thread_exit(exit_hook);
}

pub(crate) fn exit_current() {
    crate::futex::pi::pthread::exit_current();
    let _ = TASK.try_with(|task| finish(task.0.replace(Registration::default())));
    crate::futex::pi::exit_current();
}

pub(super) fn reset_after_fork() {
    TABLE.store(core::ptr::null_mut(), Ordering::Release);
    TASK.with(|task| task.0.set(Registration::default()));
}

fn finish(registration: Registration) {
    if registration.tid == 0 {
        return;
    }
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&registration.tid);
    if registration.head != 0 {
        walk(registration.head, registration.tid as u32);
    }
}

fn owner_died(entry: usize, offset: isize, tid: u32, pending: bool) -> Result<(), i32> {
    // PI-tagged entries still get OWNER_DIED, but their PI state cleanup must
    // not be substituted with an ordinary futex wake.
    let pi = entry & 1 != 0;
    let address = (entry & !1).checked_add_signed(offset).ok_or(EFAULT)?;
    let word = futex_word(address as _).map_err(|e| -e as i32)?;
    futex_access(address, size_of::<i32>(), true).map_err(|e| -e as i32)?;
    let mut old = word.load(Ordering::Acquire) as u32;
    loop {
        // A thread can die after clearing a pending lock but before its wake.
        if pending && old == 0 {
            if !pi {
                wake(address);
            }
            return Ok(());
        }
        if old & TID_MASK != tid {
            return Ok(());
        }
        let new = (old & WAITERS) | OWNER_DIED;
        match word.compare_exchange(old as i32, new as i32, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                if old & WAITERS != 0 && !pi {
                    wake(address);
                }
                return Ok(());
            }
            Err(value) => old = value as u32,
        }
    }
}

fn wake(address: usize) {
    if let Ok(shared) = FutexAddress::resolve(address as _, false)
        && futex_wake(shared, 1, u32::MAX) > 0
    {
        return;
    }
    let _ = futex_wake(address as *mut c_int, 1, u32::MAX);
}

fn walk(address: usize, tid: u32) {
    let Ok(head) = crate::ptrace::read_value::<Head>(address) else {
        return;
    };
    let pending = head.pending & !1;
    let mut entry = head.next;
    for _ in 0..WALK_LIMIT {
        let pointer = entry & !1;
        if pointer == address {
            break;
        }
        let next = crate::ptrace::read_value::<usize>(pointer);
        if pointer != pending && owner_died(entry, head.offset, tid, false).is_err() {
            return;
        }
        let Ok(next) = next else { return };
        entry = next;
    }
    // Handle pending once, including when it was also encountered on the list.
    if pending != 0 {
        let _ = owner_died(head.pending, head.offset, tid, true);
    }
}

#[cfg(test)]
mod tests;
