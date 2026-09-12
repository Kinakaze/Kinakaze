//! Fault-contained Linux key-management syscall boundary.
use super::mount_api::{read_user, text, write_user};
use kinakaze_vfs::{EINVAL, keyring};
fn payload(address: usize, length: usize) -> Result<Vec<u8>, i32> {
    if length > 32767 {
        return Err(EINVAL);
    }
    let mut data = vec![0; length];
    read_user(address, &mut data)?;
    Ok(data)
}
pub(super) fn syscall(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> Result<i64, i32> {
    if number == 248 {
        return keyring::add(
            &text(a as usize, 32)?,
            &text(b as usize, 4096)?,
            &payload(c as usize, d as usize)?,
            e as i32,
        )
        .map(i64::from);
    }
    if number == 249 {
        // A cached key can be found without a request-key userspace upcall.
        let kind = text(a as usize, 32)?;
        let name = text(b as usize, 4096)?;
        for ring in [-1, -2, -3, -4, -5] {
            match keyring::search(ring, &kind, &name, d as i32) {
                Ok(id) => return Ok(id as i64),
                Err(126 | 13) => {}
                Err(e) => return Err(e),
            }
        }
        return Err(126);
    }
    let op = a as u32;
    if op == 1 {
        let name = if b == 0 {
            None
        } else {
            Some(text(b as usize, 4096)?)
        };
        return keyring::join(name.as_deref()).map(i64::from);
    }
    if op == 10 {
        return keyring::search(
            b as i32,
            &text(c as usize, 32)?,
            &text(d as usize, 4096)?,
            e as i32,
        )
        .map(i64::from);
    }
    let input = if op == 2 {
        payload(c as usize, d as usize)?
    } else {
        Vec::new()
    };
    let (result, output) = keyring::command(op, b as i32, c, d, &input)?;
    if matches!(op, 6 | 11 | 17 | 31) {
        let (address, length) = if op == 31 { (b, c) } else { (c, d) };
        if address != 0 {
            write_user(
                address as usize,
                &output[..output.len().min(length as usize)],
            )?;
        }
    }
    Ok(result)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn keyctl(op: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    match syscall(250, op, a, b, c, d) {
        Ok(n) => n,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn add_key(kind: u64, name: u64, payload: u64, length: u64, ring: i32) -> i32 {
    match syscall(248, kind, name, payload, length, ring as u64) {
        Ok(n) => n as i32,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn request_key(kind: u64, name: u64, callout: u64, ring: i32) -> i32 {
    match syscall(249, kind, name, callout, ring as u64, 0) {
        Ok(n) => n as i32,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}
