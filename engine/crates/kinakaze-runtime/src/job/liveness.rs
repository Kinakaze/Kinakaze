//! Reuse process capabilities, never cached answers about whether they are alive.
use super::{current_host_pid, start_token};
use std::{
    cell::{Cell, RefCell},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, WAIT_TIMEOUT},
    System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        WaitForSingleObject,
    },
};

const CAPACITY: usize = 64;
struct Owner {
    pid: u32,
    token: u64,
    handle: OwnedHandle,
}

impl Owner {
    fn open(pid: u32, token: u64) -> Option<Self> {
        // Non-inheritable: each freshly bootstrapped native worker owns its own
        // TLS cache. These capabilities are not guest descriptors or fork state.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return None;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut created: FILETIME = unsafe { std::mem::zeroed() };
        let mut exited: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) };
        let observed = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        (ok != 0 && observed == token).then_some(Self { pid, token, handle })
    }

    fn alive(&self) -> bool {
        // A retained handle continues to refer to the same kernel object after
        // exit; its signalled state, including exit code 259, must be checked on
        // every use. No timeout or negative liveness cache is involved.
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == WAIT_TIMEOUT }
    }
}

thread_local! {
    static OWN_TOKEN: Cell<(u32, u64)> = const { Cell::new((0, 0)) };
    static OWNERS: RefCell<[Option<Owner>; CAPACITY]> = const {
        RefCell::new([const { None }; CAPACITY])
    };
}

pub(super) fn live_owner(pid: u32, token: u64) -> bool {
    if pid == 0 || token == 0 {
        return false;
    }
    if pid == current_host_pid() {
        return OWN_TOKEN.with(|cache| {
            let (owner, mut observed) = cache.get();
            if owner != pid || observed == 0 {
                observed = start_token(pid);
                cache.set((pid, observed));
            }
            observed == token
        });
    }
    OWNERS
        .try_with(|cache| {
            // Direct mapping bounds both retained handles and lookup work. A hash
            // collision only causes another identity-checked open.
            let index = ((pid >> 2).wrapping_mul(0x9e37_79b1) as usize) & (CAPACITY - 1);
            let Ok(mut cache) = cache.try_borrow_mut() else {
                return Owner::open(pid, token).is_some_and(|owner| owner.alive());
            };
            let slot = &mut cache[index];
            if let Some(owner) = slot.as_ref()
                && owner.pid == pid
                && owner.token == token
            {
                if owner.alive() {
                    return true;
                }
                *slot = None;
                return false;
            }
            let Some(owner) = Owner::open(pid, token) else {
                return false;
            };
            if !owner.alive() {
                return false;
            }
            *slot = Some(owner);
            true
        })
        // Another native TLS destructor can still query a process after this
        // thread's handle cache has retired. Preserve the uncached operation.
        .unwrap_or_else(|_| Owner::open(pid, token).is_some_and(|owner| owner.alive()))
}
