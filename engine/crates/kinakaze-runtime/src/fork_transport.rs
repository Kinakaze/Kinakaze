//! Explicit native capabilities for a fork child with an independent handle table.
//! Unknown participants keep the ordinary inheritance path. Contracts travel in
//! the handoff so nested forks make the same decision after provider restoration.
use super::{ForkError, ForkParticipant, ForkSnapshot, ForkStage, RegisteredParticipant};

/// Transfer a participant's native capabilities into a pinned, unpublished
/// process and update its mutable payload. The caller terminates that process
/// on any error. Source owners remain protected by the fork topology fences.
pub type ForkTransfer = unsafe extern "system" fn(usize, *mut u8, usize) -> i32;

#[derive(Clone, Copy)]
pub struct ForkTransport {
    /// An alternate snapshot format; `None` retains the ordinary callback.
    pub snapshot: Option<ForkSnapshot>,
    /// `None` declares that the payload contains no ambient inherited handles.
    pub transfer: Option<ForkTransfer>,
}

/// Register a participant that does not depend on ambient native inheritance.
///
/// # Safety
/// Every native capability required by the child must be transferred by the
/// callback, an existing registered arena handle slot, or reopening an object
/// with a stable identity. This includes owners reached through copied pointers,
/// not just integers in the serialized payload. The callbacks must remain loaded
/// at their registered addresses throughout the process's fork lifetime.
pub unsafe fn register_fork_participant_with_transport(
    hooks: ForkParticipant,
    transport: ForkTransport,
) -> bool {
    if hooks.abi != super::FORK_PARTICIPANT_ABI || hooks.key == 0 {
        return false;
    }
    let Ok(mut registry) = super::participants().lock() else {
        return false;
    };
    registry.insert_with_transport(hooks, Some(transport))
}

/// Register a participant whose state uses no ambient native handles.
///
/// # Safety
/// The complete contract of [`register_fork_participant_with_transport`] applies.
pub unsafe fn register_fork_participant_without_inherited_handles(hooks: ForkParticipant) -> bool {
    unsafe {
        register_fork_participant_with_transport(
            hooks,
            ForkTransport {
                snapshot: None,
                transfer: None,
            },
        )
    }
}

pub(super) const HANDOFF_KEY: u64 = u64::MAX - 3;

fn error() -> ForkError {
    ForkError {
        stage: ForkStage::HandoffStage,
        os_code: 5,
    }
}

pub(super) fn encode_contracts(entries: &[RegisteredParticipant]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for entry in entries {
        if let Some(transport) = entry.transport {
            for word in [
                entry.hooks.key,
                entry.hooks.snapshot.map_or(0, |f| f as usize as u64),
                transport.snapshot.map_or(0, |f| f as usize as u64),
                transport.transfer.map_or(0, |f| f as usize as u64),
            ] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
    }
    bytes
}

pub(super) fn restore_contracts(bytes: &[u8]) -> Result<(), ForkError> {
    let mut registry = super::participants().lock().map_err(|_| error())?;
    restore_contracts_into(&mut registry, bytes)
}

fn restore_contracts_into(
    registry: &mut super::ParticipantRegistry,
    bytes: &[u8],
) -> Result<(), ForkError> {
    if !bytes.len().is_multiple_of(32) {
        return Err(error());
    }
    let mut seen = std::collections::HashSet::new();
    let mut restored = Vec::new();
    for row in bytes.chunks_exact(32) {
        let word = |offset| u64::from_le_bytes(row[offset..offset + 8].try_into().unwrap());
        let key = word(0);
        let index = registry
            .entries
            .iter()
            .position(|entry| entry.hooks.key == key)
            .ok_or_else(error)?;
        if !seen.insert(key)
            || registry.entries[index]
                .hooks
                .snapshot
                .map_or(0, |f| f as usize as u64)
                != word(8)
        {
            return Err(error());
        }
        restored.push((
            index,
            ForkTransport {
                snapshot: super::callback_from_address(word(16) as usize),
                transfer: super::callback_from_address(word(24) as usize),
            },
        ));
    }
    for (index, transport) in restored {
        registry.entries[index].transport = Some(transport);
    }
    Ok(())
}

pub(super) fn selected(entries: &[RegisteredParticipant]) -> bool {
    #[cfg(windows)]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ENABLED.get_or_init(|| {
            std::env::var_os("KINAKAZE_FORK_TRANSFER").is_some_and(|value| value == "1")
        }) {
            return false;
        }
        if super::STAGE_HOOK.get().is_some() {
            return false;
        }
        if let Some(entry) = entries.iter().find(|entry| entry.transport.is_none()) {
            if super::fork_trace_enabled() {
                eprintln!(
                    "kinakaze: ordinary fork required by participant={:#x}",
                    entry.hooks.key
                );
            }
            return false;
        }
        true
    }
    #[cfg(not(windows))]
    {
        let _ = entries;
        false
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        cell::RefCell,
        os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
        System::Threading::GetCurrentProcess,
    };

    struct Plan {
        original: Vec<u8>,
        entries: Vec<RegisteredParticipant>,
    }
    thread_local! { static PLAN: RefCell<Option<Plan>> = const { RefCell::new(None) }; }

    pub(crate) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            PLAN.with_borrow_mut(|plan| *plan = None);
        }
    }

    pub(crate) fn begin(entries: &[RegisteredParticipant]) -> Result<Guard, ForkError> {
        if PLAN.with_borrow(Option::is_some) {
            return Err(error());
        }
        let original = kinakaze_alloc::take_handoff().ok_or_else(error)?;
        if !kinakaze_alloc::stage_handoff(&original) {
            return Err(error());
        }
        PLAN.with_borrow_mut(|plan| {
            *plan = Some(Plan {
                original,
                entries: entries.to_vec(),
            })
        });
        Ok(Guard)
    }

    pub(crate) fn active() -> bool {
        PLAN.with_borrow(Option::is_some)
    }

    pub(crate) fn materialize(process: BorrowedHandle<'_>) -> Result<(), ForkError> {
        PLAN.with_borrow(|plan| {
            let Some(plan) = plan else { return Ok(()) };
            // Always start with the original source capabilities. A failed
            // address-layout attempt may already have replaced the staged frame.
            let mut frame = plan.original.clone();
            transfer_frame(&mut frame, &plan.entries, process)?;
            if !kinakaze_alloc::stage_handoff(&frame) {
                return Err(error());
            }
            Ok(())
        })
    }

    pub(super) fn transfer_frame(
        frame: &mut [u8],
        entries: &[RegisteredParticipant],
        process: BorrowedHandle<'_>,
    ) -> Result<(), ForkError> {
        if frame.len() < 16
            || u64::from_le_bytes(frame[..8].try_into().unwrap()) != super::super::HANDOFF_MAGIC
        {
            return Err(error());
        }
        let count = u32::from_le_bytes(frame[8..12].try_into().unwrap());
        let mut cursor = 16usize;
        let mut sections = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..count {
            let end = cursor.checked_add(56).ok_or_else(error)?;
            let header = frame.get(cursor..end).ok_or_else(error)?;
            let key = u64::from_le_bytes(header[..8].try_into().unwrap());
            let len = usize::try_from(u64::from_le_bytes(header[48..56].try_into().unwrap()))
                .map_err(|_| error())?;
            let payload_end = end.checked_add(len).ok_or_else(error)?;
            frame.get(end..payload_end).ok_or_else(error)?;
            if !seen.insert(key) {
                return Err(error());
            }
            if key == super::super::VFORK_HANDOFF_KEY {
                if len != 24 {
                    return Err(error());
                }
            } else if key != super::super::MAPPING_HANDOFF_KEY && key != HANDOFF_KEY {
                if !entries
                    .iter()
                    .any(|entry| entry.hooks.key == key && entry.transport.is_some())
                {
                    return Err(error());
                }
            }
            sections.push((key, end..payload_end));
            cursor = payload_end.checked_next_multiple_of(8).ok_or_else(error)?;
        }
        if cursor != frame.len() {
            return Err(error());
        }
        for (key, range) in sections {
            let payload = &mut frame[range];
            if key == super::super::VFORK_HANDOFF_KEY {
                for word in payload.chunks_exact_mut(8) {
                    let source = u64::from_le_bytes(word.try_into().unwrap()) as usize;
                    let destination = duplicate_remote(source, process)?;
                    word.copy_from_slice(&(destination as u64).to_le_bytes());
                }
            } else if let Some(transfer) = entries
                .iter()
                .find(|entry| entry.hooks.key == key)
                .and_then(|entry| entry.transport)
                .and_then(|transport| transport.transfer)
            {
                let status = unsafe {
                    transfer(
                        process.as_raw_handle() as usize,
                        payload.as_mut_ptr(),
                        payload.len(),
                    )
                };
                if status != 0 {
                    return Err(ForkError {
                        stage: ForkStage::HandoffStage,
                        os_code: status.unsigned_abs(),
                    });
                }
            }
        }
        Ok(())
    }

    fn duplicate_remote(source: usize, process: BorrowedHandle<'_>) -> Result<usize, ForkError> {
        let mut target = std::ptr::null_mut();
        if source == 0
            || unsafe {
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
            return Err(error());
        }
        Ok(target as usize)
    }

    /// Native bootstrap sees only these explicitly listed handles. All guest
    /// capabilities are duplicated later while its main thread is suspended.
    pub(crate) struct BootstrapHandles {
        storage: Vec<usize>,
        _standards: Vec<OwnedHandle>,
        handles: Vec<usize>,
        standards: [usize; 3],
    }
    impl BootstrapHandles {
        pub(crate) fn new(
            ready: usize,
            manifest: usize,
            standards: [usize; 3],
        ) -> Result<Self, ForkError> {
            use windows_sys::Win32::System::Threading::{
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                UpdateProcThreadAttribute,
            };
            let mut handles = vec![ready, manifest];
            let mut pinned = Vec::new();
            let mut child_standards = [0; 3];
            for (index, raw) in standards.into_iter().enumerate() {
                if raw == 0 || raw == usize::MAX {
                    continue;
                }
                let target = duplicate_remote(raw, unsafe {
                    BorrowedHandle::borrow_raw(GetCurrentProcess())
                })?;
                pinned.push(unsafe { OwnedHandle::from_raw_handle(target as _) });
                handles.push(target);
                child_standards[index] = target;
            }
            let mut bytes = 0;
            unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes) };
            if bytes == 0 {
                return Err(error());
            }
            let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
            if unsafe {
                InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
            } == 0
            {
                return Err(error());
            }
            let mut result = Self {
                storage,
                _standards: pinned,
                handles,
                standards: child_standards,
            };
            if unsafe {
                UpdateProcThreadAttribute(
                    result.storage.as_mut_ptr().cast(),
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    result.handles.as_mut_ptr().cast(),
                    result.handles.len() * std::mem::size_of::<usize>(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(error());
            }
            Ok(result)
        }
        pub(crate) fn configure(
            &mut self,
            startup: &mut windows_sys::Win32::System::Threading::STARTUPINFOEXW,
        ) {
            startup.StartupInfo.cb = std::mem::size_of_val(startup) as u32;
            startup.lpAttributeList = self.storage.as_mut_ptr().cast();
            startup.StartupInfo.hStdInput = self.standards[0] as _;
            startup.StartupInfo.hStdOutput = self.standards[1] as _;
            startup.StartupInfo.hStdError = self.standards[2] as _;
        }
    }
    impl Drop for BootstrapHandles {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(
                    self.storage.as_mut_ptr().cast(),
                )
            };
        }
    }
}

#[cfg(windows)]
pub(super) use native::{BootstrapHandles, active, begin, materialize};

#[cfg(test)]
mod tests;
