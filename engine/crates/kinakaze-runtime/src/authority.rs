//! Process ownership supplied by the enclosing runtime DLL.
//!
//! This module has no RPC transport or process-local context pointer. A copied
//! fork stack carries only a bounded reservation; the child callback must open
//! a fresh manager session before restoring any VFS participant.

use std::sync::atomic::{AtomicPtr, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub epoch: u64,
    pub pid: u32,
    pub parent_pid: u32,
}

/// An opaque, single-use manager reservation. Never log the token.
#[derive(Clone, Copy)]
pub struct ForkReservation {
    pub transaction: u64,
    pub child: Identity,
    pub token: [u8; 64],
}

/// All callbacks return a positive Linux errno on failure. The table and its
/// functions reside in the one runtime DLL and live until process teardown.
pub struct ProcessAuthority {
    pub identity: fn() -> Result<Identity, i32>,
    pub memory_target: fn(u32, bool) -> Result<(u32, u64), i32>,
    pub prepare_fork: fn(Option<u32>) -> Result<ForkReservation, i32>,
    pub adopt_fork: fn(&ForkReservation) -> Result<(), i32>,
    pub mark_ready: fn() -> Result<(), i32>,
    pub await_activation: fn() -> Result<(), i32>,
    pub commit_fork: fn(u64) -> Result<(), i32>,
    pub abort_fork: fn(u64) -> Result<(), i32>,
    pub prepare_exec: fn(u32) -> Result<u64, i32>,
    pub commit_exec: fn(u64) -> Result<(), i32>,
    pub abort_exec: fn(u64) -> Result<(), i32>,
}

static AUTHORITY: AtomicPtr<ProcessAuthority> = AtomicPtr::new(core::ptr::null_mut());
static RESERVED_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PROCESS_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static DOMAIN_EPOCH: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// Shared kernel objects belong to a manager instance, not its root PID
/// namespace (whose local ID is always 1). Keep every epoch bit and retain the
/// immutable value across fork. Zero identifies the standalone native domain;
/// an installed authority never falls back to that domain after an RPC error.
pub fn domain_id() -> u64 {
    if let Some(epoch) = DOMAIN_EPOCH.get() {
        return *epoch;
    }
    if get().is_none() {
        return 0;
    }
    *DOMAIN_EPOCH.get_or_init(|| {
        let epoch = required_identity().epoch;
        assert_ne!(epoch, 0, "process manager supplied a zero domain epoch");
        epoch
    })
}

pub fn install(authority: &'static ProcessAuthority) -> Result<(), i32> {
    if get().is_none() && DOMAIN_EPOCH.get().is_some() {
        return Err(16); // A helper cannot turn into a Linux process.
    }
    let pointer = core::ptr::from_ref(authority).cast_mut();
    match AUTHORITY.compare_exchange(
        core::ptr::null_mut(),
        pointer,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(()),
        Err(existing) if existing == pointer => Ok(()),
        Err(_) => Err(16), // EBUSY: never silently replace the process owner.
    }
}

/// Bind a fresh internal helper to the authenticated manager's object domain.
/// Called once before starting helper threads; no Linux identity is installed.
pub fn install_helper_domain(epoch: u64) -> Result<(), i32> {
    if epoch == 0 || get().is_some() {
        return Err(22);
    }
    DOMAIN_EPOCH.set(epoch).map_err(|_| 16)
}

pub fn get() -> Option<&'static ProcessAuthority> {
    let pointer = AUTHORITY.load(Ordering::Acquire);
    // SAFETY: install accepts only a static table, published before guest use.
    unsafe { pointer.as_ref() }
}

pub fn identity() -> Option<Result<Identity, i32>> {
    get().map(|authority| (authority.identity)())
}

/// Authorize a logical PID and return its native identity. Callers must pin an
/// OS process handle and compare its creation time before accessing memory.
pub fn memory_target(pid: u32, write: bool) -> Result<(u32, u64), i32> {
    (get().ok_or(3)?.memory_target)(pid, write)
}

/// The native fork coordinator serializes transactions before setting this.
pub(crate) fn reserve_pid(pid: u32) {
    RESERVED_PID.store(pid, Ordering::Release);
}

pub(crate) fn reserved_pid() -> Option<u32> {
    let pid = RESERVED_PID.load(Ordering::Acquire);
    (pid != 0).then_some(pid)
}

/// Logical identity is never replaced by a host PID after manager failure.
pub fn required_identity() -> Identity {
    match identity() {
        Some(Ok(identity)) => {
            PROCESS_PID.store(identity.pid, Ordering::Release);
            identity
        }
        _ => {
            eprintln!("kinakaze: process manager identity is unavailable");
            std::process::abort()
        }
    }
}

/// getpid is immutable within this native worker. Linux parent relationships
/// are maintained by the shared process registry, including subreaper adoption.
pub fn process_id() -> u32 {
    let pid = PROCESS_PID.load(Ordering::Acquire);
    if pid == 0 {
        required_identity().pid
    } else {
        pid
    }
}

pub(crate) fn reset_after_fork() {
    PROCESS_PID.store(0, Ordering::Release);
}

/// Final image activation barrier, before guest constructors or entry. Initial
/// workers are already active; an exec candidate waits for ownership transfer.
pub fn activate_image() -> Result<(), i32> {
    if let Some(authority) = get() {
        (authority.mark_ready)()?;
        (authority.await_activation)()?;
    }
    Ok(())
}

/// Parent-owned reservation guard. A native clone copies this value but never
/// runs its parent's destructor against the child's new connection.
pub struct ForkTransaction {
    owner: u32,
    reservation: Option<ForkReservation>,
}

impl ForkTransaction {
    pub fn prepare(parent: Option<u32>) -> Result<Self, i32> {
        let reservation = get()
            .map(|authority| (authority.prepare_fork)(parent))
            .transpose()?;
        reserve_pid(reservation.map_or(0, |reservation| reservation.child.pid));
        Ok(Self {
            owner: std::process::id(),
            reservation,
        })
    }

    pub(crate) fn adopt(&self) -> Result<(), i32> {
        if let Some(reservation) = &self.reservation {
            (get().ok_or(5)?.adopt_fork)(reservation)?;
            let identity = required_identity();
            // CLONE_PARENT's logical parent may die while the reservation is
            // being materialized. PID/epoch are stable; parent_pid is mutable.
            if identity.epoch != reservation.child.epoch || identity.pid != reservation.child.pid {
                return Err(5);
            }
        }
        Ok(())
    }

    pub(crate) fn ready(&self) -> Result<(), i32> {
        if self.reservation.is_some() {
            (get().ok_or(5)?.mark_ready)()?;
        }
        Ok(())
    }

    pub(crate) fn await_activation(&self) -> Result<(), i32> {
        if self.reservation.is_some() {
            (get().ok_or(5)?.await_activation)()?;
        }
        Ok(())
    }

    /// Reservation for a fresh image launch; credentials stay in the host environment.
    pub fn reservation(&self) -> Option<&ForkReservation> {
        self.reservation.as_ref()
    }

    pub fn commit(&mut self) -> Result<(), i32> {
        if let Some(reservation) = self.reservation {
            (get().ok_or(5)?.commit_fork)(reservation.transaction)?;
        }
        self.reservation = None;
        reserve_pid(0);
        Ok(())
    }
}

impl Drop for ForkTransaction {
    fn drop(&mut self) {
        if self.owner != std::process::id() {
            return;
        }
        if let Some(reservation) = self.reservation {
            if let Some(authority) = get() {
                let _ = (authority.abort_fork)(reservation.transaction);
            }
        }
        reserve_pid(0);
    }
}
