//! Account records may be retained by guest code over fork (for example newgrp).
//! Reuse database-record ownership instead of publishing pointers into a PE heap.
use super::{Group, GroupAccount, Passwd, UserAccount, fill_group_for, fill_passwd_for};
use crate::netdb::records::returned::{Record, returned_record};
use core::{ffi::c_char, ptr};
use kinakaze_vfs::ENOMEM;

fn strings_size<'a>(
    mut strings: impl Iterator<Item = &'a str>,
    initial: usize,
) -> Result<usize, i32> {
    strings.try_fold(initial, |size, text| {
        size.checked_add(text.len())
            .and_then(|n| n.checked_add(1))
            .ok_or(ENOMEM)
    })
}

fn create<T>(
    length: usize,
    fill: impl FnOnce(*mut T, *mut c_char, usize) -> Result<(), i32>,
) -> Result<Record, i32> {
    let size = size_of::<T>().checked_add(length).ok_or(ENOMEM)?;
    let base = unsafe { kinakaze_alloc::guest::malloc(size) };
    if base.is_null() {
        return Err(ENOMEM);
    }
    let allocation = Record(base as usize);
    fill(
        base.cast(),
        unsafe { base.add(size_of::<T>()).cast() },
        length,
    )?;
    Ok(allocation)
}

mod passwd {
    use super::*;
    returned_record!(*b"CYPWDR01");

    pub(super) fn publish(account: &UserAccount) -> Result<*mut Passwd, i32> {
        register_returned()?;
        let length = strings_size(
            [
                account.name.as_str(),
                account.password.as_str(),
                account.gecos.as_str(),
                account.dir.as_str(),
                account.shell.as_str(),
            ]
            .into_iter(),
            0,
        )?;
        let allocation = create(length, |entry, buffer, length| unsafe {
            fill_passwd_for(account, entry, buffer, length)
        })?;
        let address = allocation.0 as *mut Passwd;
        RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
        Ok(address)
    }
}
mod group {
    use super::*;
    returned_record!(*b"CYGRPR01");

    pub(super) fn publish(account: &GroupAccount) -> Result<*mut Group, i32> {
        register_returned()?;
        let table = account
            .members
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_mul(size_of::<*mut c_char>()))
            .and_then(|n| n.checked_add(align_of::<*mut c_char>() - 1))
            .ok_or(ENOMEM)?;
        let length = strings_size(
            [account.name.as_str(), account.password.as_str()]
                .into_iter()
                .chain(account.members.iter().map(String::as_str)),
            table,
        )?;
        let allocation = create(length, |entry, buffer, length| unsafe {
            fill_group_for(account, entry, buffer, length)
        })?;
        let address = allocation.0 as *mut Group;
        RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
        Ok(address)
    }
}
fn result<T>(value: Result<*mut T, i32>) -> *mut T {
    value.unwrap_or_else(|error| {
        crate::set_errno(error);
        ptr::null_mut()
    })
}
pub(super) fn passwd(account: &UserAccount) -> *mut Passwd {
    result(passwd::publish(account))
}
pub(super) fn group(account: &GroupAccount) -> *mut Group {
    result(group::publish(account))
}
