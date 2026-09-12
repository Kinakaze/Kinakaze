//! Each blocking consumer has its own wake descriptor. Draining the public X
//! connection in another thread must not consume this consumer's notification.
use libc::fdio;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

static WAITERS: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
fn waiters() -> MutexGuard<'static, Vec<i32>> {
    WAITERS.lock().unwrap_or_else(|error| error.into_inner())
}
pub struct Waiter(i32);
impl Waiter {
    pub fn new() -> Option<Self> {
        let mut active = waiters();
        let fd = unsafe { fdio::kinakaze_abi_eventfd(0, fdio::EFD_NONBLOCK | fdio::EFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        active.push(fd);
        ACTIVE.store(active.len(), Ordering::Release);
        Some(Self(fd))
    }
    pub fn drain(&self) {
        let mut value = 0;
        unsafe {
            fdio::kinakaze_abi_eventfd_read(self.0, &raw mut value);
        }
    }
    pub fn wait(&self) -> bool {
        let mut fd = fdio::PollFd {
            fd: self.0,
            events: fdio::POLLIN,
            revents: 0,
        };
        let result = unsafe { fdio::kinakaze_abi_poll(&raw mut fd, 1, -1) };
        (result >= 0 || libc::kinakaze_errno() == 4)
            && fd.revents & (fdio::POLLNVAL | fdio::POLLERR | fdio::POLLHUP) == 0
    }
}
impl Drop for Waiter {
    fn drop(&mut self) {
        // Exclude notification and fork until the owned descriptor is closed.
        let mut active = waiters();
        libc::kinakaze_abi_close(self.0);
        active.retain(|fd| *fd != self.0);
        ACTIVE.store(active.len(), Ordering::Release);
    }
}
pub(crate) fn notify() {
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return;
    }
    for &fd in waiters().iter() {
        unsafe {
            fdio::kinakaze_abi_eventfd_write(fd, 1);
        }
    }
}

// A suspended native wait frame cannot yet be restored in a fork child.
const KEY: u64 = u64::from_le_bytes(*b"CYXES001");
thread_local! { static FROZEN: RefCell<Option<MutexGuard<'static, Vec<i32>>>> = const { RefCell::new(None) }; }
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        if slot.borrow().is_some() {
            return 35;
        }
        let active = waiters();
        if !active.is_empty() {
            return 11;
        }
        *slot.borrow_mut() = Some(active);
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if !FROZEN.with(|slot| slot.borrow().is_some()) {
        return -22;
    }
    if output.is_null() {
        return 8;
    }
    if capacity < 8 {
        return -22;
    }
    unsafe {
        output.cast::<u64>().write_unaligned(KEY);
    }
    8
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if length != 8 || input.is_null() || unsafe { input.cast::<u64>().read_unaligned() } != KEY {
        return 22;
    }
    0
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 448,
        key: KEY,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
