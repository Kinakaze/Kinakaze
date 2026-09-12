//! The loader owns its state; runtime only coordinates callback ordering.
use super::{
    lock_dynamic_loader_for_fork, restore_active_linker, snapshot_active_linker,
    unlock_dynamic_loader_after_fork,
};

pub(super) fn register() -> Result<(), crate::LinkError> {
    if kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        // Engine ABI/TLS restoration (0) precedes private linker reconstruction.
        priority: 1,
        key: 0x4c44_5354_4154_4531, // LDSTATE1
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(restore),
    }) {
        Ok(())
    } else {
        Err(crate::LinkError::InvalidProvider(
            "cannot register loader fork state".into(),
        ))
    }
}

unsafe extern "system" fn prepare() -> i32 {
    i32::from(!lock_dynamic_loader_for_fork())
}

unsafe extern "system" fn parent(_status: i32) {
    unlock_dynamic_loader_after_fork();
}

unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let bytes = match snapshot_active_linker() {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("kinakaze: snapshot linker: {error}");
            return -5;
        }
    };
    let Ok(length) = isize::try_from(bytes.len()) else {
        return -75;
    };
    if buffer.is_null() {
        return length;
    }
    if capacity < bytes.len() {
        return -22;
    }
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    length
}

unsafe extern "system" fn restore(payload: *const u8, length: usize) -> i32 {
    if payload.is_null() || length > isize::MAX as usize {
        return 22;
    }
    match unsafe { restore_active_linker(core::slice::from_raw_parts(payload, length)) } {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("kinakaze: restore linker: {error}");
            5
        }
    }
}
