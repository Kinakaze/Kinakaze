//! Native capabilities in a portable VFS launch frame.
//!
//! Ordinary snapshots retain their inherited HANDLE representation. A portable
//! snapshot records handles separately and places indexes in handle fields, so
//! an unused native worker need not have been created with the parent's handle
//! table. Linux descriptor numbers, open descriptions and object IDs do not
//! change. This is a launch transport, never a pathname or inode-content cache.
use crate::{EBUSY, EIO, ENOMEM};
use std::{
    cell::RefCell,
    collections::HashMap,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

pub(crate) const SECTION: u32 = 31;
const MAGIC: &[u8; 8] = b"KZHNDL01";
const TOKEN: u64 = 1 << 63;

#[derive(Default)]
struct Capture {
    sources: Vec<u64>,
    indexes: HashMap<u64, u64>,
    owned: bool,
    pins: Vec<OwnedHandle>,
    error: Option<i32>,
}

/// Keeps source capabilities alive independently of descriptor-table mutation.
pub(crate) struct OwnedFrame {
    pub(crate) bytes: Vec<u8>,
    _pins: Vec<OwnedHandle>,
}

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static RESTORE: RefCell<Option<Vec<u64>>> = const { RefCell::new(None) };
}

pub(crate) fn capturing() -> bool {
    CAPTURE.with_borrow(Option::is_some)
}

pub(crate) fn descriptor(entry: crate::FdEntry) -> Result<(u64, crate::FdFlags), i32> {
    if !capturing() {
        return Ok((entry.raw as u64, entry.flags));
    }
    // Console handles cannot be duplicated into a different process. Winsock
    // uses WSADuplicateSocket records; its table word is only a recipe key.
    if matches!(entry.kind, crate::FdKind::Console | crate::FdKind::IoRing) {
        return Err(crate::EOPNOTSUPP);
    }
    let raw = if entry.kind == crate::FdKind::Socket {
        entry.raw as u64
    } else {
        encode(entry.raw as u64)
    };
    Ok((
        raw,
        crate::FdFlags(entry.flags.0 & !crate::FdFlags::BORROWED.0),
    ))
}

/// Encode only a native handle field. Object IDs and Winsock recipe keys are
/// deliberately not handles and must retain their original representation.
pub(crate) fn encode(raw: u64) -> u64 {
    if raw == 0 {
        return 0;
    }
    CAPTURE.with_borrow_mut(|state| {
        let Some(state) = state else { return raw };
        if state.error.is_some() {
            return 0;
        }
        // Serializers call encode while their table lock or Arc owns `raw`.
        // Pin at that point, not after serialization when close/reuse could
        // already have replaced the numeric handle with an unrelated object.
        let pin = if state.owned {
            match pin_handle(raw) {
                Ok(pin) => Some(pin),
                Err(error) => {
                    state.error = Some(error);
                    return 0;
                }
            }
        } else {
            None
        };
        if let Some(index) = state.indexes.get(&raw) {
            if let Some(pin) = &pin {
                let previous = state.sources[*index as usize - 1];
                if unsafe {
                    windows_sys::Win32::Foundation::CompareObjectHandles(
                        pin.as_raw_handle(),
                        previous as _,
                    )
                } == 0
                {
                    state.error = Some(crate::EAGAIN);
                    return 0;
                }
            }
            return TOKEN | index;
        }
        let index = state.sources.len() as u64 + 1;
        state
            .sources
            .push(pin.as_ref().map_or(raw, |pin| pin.as_raw_handle() as u64));
        if let Some(pin) = pin {
            state.pins.push(pin);
        }
        state.indexes.insert(raw, index);
        TOKEN | index
    })
}

fn pin_handle(raw: u64) -> Result<OwnedHandle, i32> {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut target = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw as _,
            GetCurrentProcess(),
            &mut target,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(target) })
}

/// Decode a native handle field under the frame's restoration scope. Reject
/// unindexed handles in a portable frame rather than accidentally using a
/// numerically equal bootstrap handle in the receiving process.
pub(crate) fn decode(value: u64) -> Result<u64, i32> {
    if value == 0 {
        return Ok(0);
    }
    RESTORE.with_borrow(|state| match state {
        None if value < TOKEN => Ok(value),
        Some(handles) if value & TOKEN != 0 => value
            .checked_sub(TOKEN + 1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| handles.get(index).copied())
            .ok_or(EIO),
        _ => Err(EIO),
    })
}

struct CaptureGuard;
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURE.with_borrow_mut(|state| *state = None);
    }
}

pub(crate) fn capture(build: impl FnOnce() -> Result<Vec<u8>, i32>) -> Result<Vec<u8>, i32> {
    capture_with_ownership(false, build).map(|frame| frame.bytes)
}

pub(crate) fn capture_owned(
    build: impl FnOnce() -> Result<Vec<u8>, i32>,
) -> Result<OwnedFrame, i32> {
    capture_with_ownership(true, build)
}

fn capture_with_ownership(
    owned: bool,
    build: impl FnOnce() -> Result<Vec<u8>, i32>,
) -> Result<OwnedFrame, i32> {
    CAPTURE.with_borrow_mut(|state| {
        if state.is_some() {
            return Err(EBUSY);
        }
        *state = Some(Capture {
            owned,
            ..Capture::default()
        });
        Ok(())
    })?;
    let guard = CaptureGuard;
    let mut frame = build()?;
    let state = CAPTURE.with_borrow_mut(Option::take).ok_or(EIO)?;
    drop(guard);
    if let Some(error) = state.error {
        return Err(error);
    }
    if frame.len() < 16
        || u64::from_le_bytes(frame[..8].try_into().unwrap()) != crate::FORK_STATE_MAGIC
        || table_range(&frame)?.is_some()
    {
        return Err(EIO);
    }
    let count = u32::from_le_bytes(frame[8..12].try_into().unwrap())
        .checked_add(1)
        .ok_or(EIO)?;
    let mut table = MAGIC.to_vec();
    crate::state_codec::word(&mut table, state.sources.len() as u64);
    for source in state.sources {
        if source == 0 || source >= TOKEN {
            return Err(EIO);
        }
        crate::state_codec::word(&mut table, source);
        // Identity destinations permit explicit same-process round trips. A
        // cross-process sender replaces them only after all duplicates succeed.
        crate::state_codec::word(&mut table, source);
    }
    crate::append_fork_section(&mut frame, SECTION, &table)?;
    frame[8..12].copy_from_slice(&count.to_le_bytes());
    Ok(OwnedFrame {
        bytes: frame,
        _pins: state.pins,
    })
}

/// Parses the complete frame before any remote capability is created.
fn table_range(frame: &[u8]) -> Result<Option<std::ops::Range<usize>>, i32> {
    if frame.len() < 16
        || u64::from_le_bytes(frame[..8].try_into().unwrap()) != crate::FORK_STATE_MAGIC
    {
        return Err(EIO);
    }
    let count = u32::from_le_bytes(frame[8..12].try_into().unwrap());
    let mut cursor = 16usize;
    let mut table = None;
    for _ in 0..count {
        let header = frame
            .get(cursor..cursor.checked_add(8).ok_or(EIO)?)
            .ok_or(EIO)?;
        let tag = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        let start = cursor + 8;
        let end = start.checked_add(length).ok_or(EIO)?;
        frame.get(start..end).ok_or(EIO)?;
        if tag == SECTION && table.replace(start..end).is_some() {
            return Err(EIO);
        }
        cursor = end.checked_next_multiple_of(8).ok_or(EIO)?;
    }
    if cursor != frame.len() {
        return Err(EIO);
    }
    Ok(table)
}

fn entries(table: &[u8]) -> Result<Vec<(u64, u64)>, i32> {
    let mut reader = crate::state_codec::Reader(table.strip_prefix(MAGIC).ok_or(EIO)?);
    let count = usize::try_from(reader.word()?).map_err(|_| EIO)?;
    if reader.0.len() != count.checked_mul(16).ok_or(EIO)? {
        return Err(EIO);
    }
    let mut sources = std::collections::HashSet::new();
    let mut destinations = std::collections::HashSet::new();
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    for _ in 0..count {
        let source = reader.word()?;
        let destination = reader.word()?;
        if source == 0
            || source >= TOKEN
            || destination == 0
            || destination >= TOKEN
            || !sources.insert(source)
            || !destinations.insert(destination)
        {
            return Err(EIO);
        }
        result.push((source, destination));
    }
    Ok(result)
}

pub(crate) struct RestoreGuard(bool);
impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.0 {
            RESTORE.with_borrow_mut(|state| *state = None);
        }
    }
}

pub(crate) fn restore_scope(frame: &[u8]) -> Result<RestoreGuard, i32> {
    let Some(range) = table_range(frame)? else {
        return if RESTORE.with_borrow(Option::is_some) {
            Err(EBUSY)
        } else {
            Ok(RestoreGuard(false))
        };
    };
    let handles = entries(&frame[range])?
        .into_iter()
        .map(|(_, target)| target)
        .collect();
    RESTORE.with_borrow_mut(|state| {
        if state.is_some() {
            return Err(EBUSY);
        }
        *state = Some(handles);
        Ok(RestoreGuard(true))
    })
}

/// Copy all captured native capabilities into a pinned, unpublished process.
/// The caller keeps the serialized source owners stable throughout this call.
/// Success transfers ownership to the target; it must either restore the frame
/// or terminate. Failure retires every duplicate created by this invocation and
/// leaves the original frame unchanged, including when retrying another target.
pub(crate) fn transfer(
    frame: &mut [u8],
    process: std::os::windows::io::BorrowedHandle<'_>,
) -> Result<(), i32> {
    use windows_sys::Win32::Foundation::{
        DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    struct Remote<'a> {
        process: std::os::windows::io::BorrowedHandle<'a>,
        handles: Vec<usize>,
        committed: bool,
    }
    impl Drop for Remote<'_> {
        fn drop(&mut self) {
            if !self.committed {
                for raw in self.handles.drain(..) {
                    // DUPLICATE_CLOSE_SOURCE closes the source even when the
                    // optional destination is omitted. The pinned target cannot
                    // become an unrelated process after native PID reuse.
                    unsafe {
                        DuplicateHandle(
                            self.process.as_raw_handle(),
                            raw as _,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            0,
                            0,
                            DUPLICATE_CLOSE_SOURCE,
                        );
                    }
                }
            }
        }
    }
    let range = table_range(frame)?.ok_or(EIO)?;
    let entries = entries(&frame[range.clone()])?;
    let mut remote = Remote {
        process,
        handles: Vec::new(),
        committed: false,
    };
    remote
        .handles
        .try_reserve_exact(entries.len())
        .map_err(|_| ENOMEM)?;
    for (source, _) in entries {
        let mut target = std::ptr::null_mut();
        // Preserve ordinary nested-fork inheritance. The destination process
        // object is owned by the caller until this complete transfer returns.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source as _,
                process.as_raw_handle(),
                &mut target,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(crate::errno_from_win32(unsafe {
                windows_sys::Win32::Foundation::GetLastError()
            }));
        }
        remote.handles.push(target as usize);
    }
    for (index, raw) in remote.handles.iter().enumerate() {
        let start = range.start + 16 + index * 16 + 8;
        frame[start..start + 8].copy_from_slice(&(*raw as u64).to_le_bytes());
    }
    remote.committed = true;
    Ok(())
}

#[cfg(test)]
mod tests;
