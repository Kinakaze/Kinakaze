//! Windows scheduling classes for identity-checked guest process targets.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, FILETIME, GetLastError},
    System::Threading::{
        GetPriorityClass, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SET_INFORMATION, SetPriorityClass,
    },
};

use super::{EPERM, ESRCH, class_to_nice, nice_to_class};

struct Target {
    handle: OwnedHandle,
    pid: u32,
}

fn open(who: u32, write: bool) -> Result<Target, i32> {
    let pid = kinakaze_runtime::job::namespaces::resolve(who).ok_or(ESRCH)?;
    let entry = kinakaze_runtime::job::lookup(pid).ok_or(ESRCH)?;
    if entry.flags & kinakaze_runtime::job::FLAG_ZOMBIE != 0 {
        return Err(ESRCH);
    }
    let access =
        PROCESS_QUERY_LIMITED_INFORMATION | if write { PROCESS_SET_INFORMATION } else { 0 };
    // A retained, non-inheritable handle cannot change identity when a PID is reused.
    let raw = unsafe { OpenProcess(access, 0, entry.pid) };
    if raw.is_null() {
        return Err(if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
            EPERM
        } else {
            ESRCH
        });
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) };
    let token = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if ok == 0
        || token != entry.token
        || !kinakaze_runtime::job::lookup(pid)
            .is_some_and(|current| current.pid == entry.pid && current.token == token)
    {
        return Err(ESRCH);
    }
    Ok(Target { handle, pid })
}

pub(super) fn get(who: u32) -> Result<i32, i32> {
    if who == 0 || who == super::own_pid() as u32 {
        return Ok(super::current_nice());
    }
    let target = open(who, false)?;
    let class = unsafe { GetPriorityClass(target.handle.as_raw_handle()) };
    if class == 0 {
        Err(EPERM)
    } else {
        Ok(class_to_nice(class))
    }
}

pub(super) fn set(who: u32, value: i32) -> Result<i32, i32> {
    if who == 0 || who == super::own_pid() as u32 {
        return super::apply_nice(value);
    }
    let target = open(who, true)?;
    let ids = kinakaze_vfs::credentials::process_identity(target.pid).map_err(|_| ESRCH)?;
    let caller = kinakaze_vfs::credentials::identity().uids[1];
    let namespace = kinakaze_vfs::user_namespace::id(target.pid).map_err(|_| ESRCH)?;
    let privileged = kinakaze_vfs::user_namespace::capable(namespace, 23);
    if caller != ids.uids[0] && caller != ids.uids[1] && !privileged {
        return Err(EPERM);
    }
    let class = unsafe { GetPriorityClass(target.handle.as_raw_handle()) };
    if class == 0 {
        return Err(EPERM);
    }
    let nice = value.clamp(-20, 19);
    if nice < class_to_nice(class) && !privileged {
        return Err(EPERM);
    }
    let ok = unsafe { SetPriorityClass(target.handle.as_raw_handle(), nice_to_class(nice)) };
    if ok == 0 { Err(EPERM) } else { Ok(nice) }
}
