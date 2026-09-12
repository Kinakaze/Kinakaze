use super::*;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAllocEx};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForSingleObject,
};

static SERIAL: Mutex<()> = Mutex::new(());
static SEQUENCE: AtomicUsize = AtomicUsize::new(1);

struct Native(HANDLE);
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct Slot(*mut AtomicUsize);
impl Slot {
    fn new(value: usize) -> Self {
        let slot = unsafe { kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<AtomicUsize>()) }
            .cast::<AtomicUsize>();
        assert!(!slot.is_null());
        unsafe { slot.write(AtomicUsize::new(value)) };
        Self(slot)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        let _transaction = begin_fork_mapping_transaction().unwrap();
        unregister_fork_handle_slot(self.0);
        unsafe {
            kinakaze_alloc::ManagedAllocator.dealloc(self.0.cast(), Layout::new::<AtomicUsize>())
        };
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn event(name: Option<&str>) -> Native {
    let name = name.map(wide);
    let handle = unsafe {
        CreateEventW(
            ptr::null(),
            1,
            0,
            name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
        )
    };
    assert!(!handle.is_null());
    Native(handle)
}

#[test]
fn registration_rejects_unowned_addresses_and_inheritable_handles() {
    let _serial = SERIAL.lock().unwrap();
    let _transaction = begin_fork_mapping_transaction().unwrap();
    let local = AtomicUsize::new(0);
    assert!(!unsafe { register_fork_handle_slot(&local) });
    assert!(!unsafe { register_fork_handle_slot(ptr::null()) });
    assert!(!valid_slot_address(kinakaze_alloc::ARENA_BASE));
    let native = event(None);
    let slot = Slot::new(native.0 as usize);
    assert!(unsafe { register_fork_handle_slot(slot.0) });
    assert!(unsafe { register_fork_handle_slot(slot.0) });
    assert_eq!(
        mappings()
            .lock()
            .unwrap()
            .handle_slots
            .iter()
            .filter(|&&address| address == slot.0 as usize)
            .count(),
        1
    );
    assert!(unregister_fork_handle_slot(slot.0));
    assert!(!unregister_fork_handle_slot(slot.0));
    assert_ne!(
        unsafe {
            windows_sys::Win32::Foundation::SetHandleInformation(
                native.0,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        },
        0
    );
    assert!(!unsafe { register_fork_handle_slot(slot.0) });
    assert_ne!(
        unsafe {
            windows_sys::Win32::Foundation::SetHandleInformation(native.0, HANDLE_FLAG_INHERIT, 0)
        },
        0
    );
    unsafe { (*slot.0).store(usize::MAX, Ordering::Release) };
    assert!(!unsafe { register_fork_handle_slot(slot.0) });
}

#[test]
#[ignore = "driven by retained_handle_is_installed_before_native_child_admission"]
fn handle_slot_child() {
    let prefix = std::env::var("KINAKAZE_HANDLE_SLOT_TEST").unwrap();
    let address = std::env::var("KINAKAZE_HANDLE_SLOT_ADDRESS")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    kinakaze_alloc::initialize().unwrap();
    let open = |suffix: &str| {
        let handle = unsafe {
            OpenEventW(
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                0,
                wide(&format!("{prefix}.{suffix}")).as_ptr(),
            )
        };
        assert!(!handle.is_null());
        Native(handle)
    };
    let ready = open("ready");
    let resume = open("resume");
    assert_ne!(unsafe { SetEvent(ready.0) }, 0);
    assert_eq!(
        unsafe { WaitForSingleObject(resume.0, 10_000) },
        WAIT_OBJECT_0
    );
    let value = unsafe { (*(address as *const AtomicUsize)).load(Ordering::Acquire) };
    let owned = Native(value as HANDLE);
    let mut flags = 0;
    assert_ne!(unsafe { GetHandleInformation(owned.0, &mut flags) }, 0);
    assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    assert_eq!(unsafe { WaitForSingleObject(owned.0, 0) }, WAIT_OBJECT_0);
    // A child can register the patched value for a subsequent fork. The parent
    // process's numeric HANDLE is never reused as the child ownership token.
    assert!(unsafe { register_fork_handle_slot(address as *const AtomicUsize) });
    assert!(unregister_fork_handle_slot(address as *const AtomicUsize));
}

#[test]
fn retained_handle_is_installed_before_native_child_admission() {
    let _serial = SERIAL.lock().unwrap();
    let _transaction = begin_fork_mapping_transaction().unwrap();
    let prefix = format!(
        "Local\\kinakaze.handle-slot.{}.{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let ready = event(Some(&format!("{prefix}.ready")));
    let resume = event(Some(&format!("{prefix}.resume")));
    let source = event(None);
    assert_ne!(unsafe { SetEvent(source.0) }, 0);
    let slot = Slot::new(source.0 as usize);
    assert!(unsafe { register_fork_handle_slot(slot.0) });
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "handle_slots::tests::handle_slot_child",
            "--ignored",
            "--nocapture",
        ])
        .env("KINAKAZE_HANDLE_SLOT_TEST", &prefix)
        .env(
            "KINAKAZE_HANDLE_SLOT_ADDRESS",
            (slot.0 as usize).to_string(),
        )
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = OwnedChild(child);
    assert_eq!(
        unsafe { WaitForSingleObject(ready.0, 10_000) },
        WAIT_OBJECT_0
    );
    let process = child.0.as_raw_handle();
    let values = unsafe { duplicate_into(process) }.unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].address, slot.0 as usize);
    let page = (slot.0 as usize & !4095) as *const core::ffi::c_void;
    assert!(!unsafe { VirtualAllocEx(process, page, 4096, MEM_COMMIT, PAGE_READWRITE) }.is_null());
    let mut written = 0;
    assert_ne!(
        unsafe {
            WriteProcessMemory(
                process,
                slot.0.cast(),
                ptr::addr_of!(values[0].value).cast(),
                core::mem::size_of::<usize>(),
                &mut written,
            )
        },
        0
    );
    assert_eq!(written, core::mem::size_of::<usize>());
    assert!(unregister_fork_handle_slot(slot.0));
    drop(source); // child must keep the kernel object alive on its own
    assert_ne!(unsafe { SetEvent(resume.0) }, 0);
    assert_eq!(
        unsafe { WaitForSingleObject(process, 10_000) },
        WAIT_OBJECT_0
    );
    assert!(child.0.wait().unwrap().success());
}

#[test]
fn handoff_rejects_truncated_or_duplicate_handle_slots() {
    let _serial = SERIAL.lock().unwrap();
    let _transaction = begin_fork_mapping_transaction().unwrap();
    let mut frame = vec![0u8; 16];
    frame[12..16].copy_from_slice(&super::super::MAPPING_HANDOFF_VERSION.to_le_bytes());
    frame[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert!(super::super::restore_mapping_registry(&frame).is_err());
    frame.extend_from_slice(&0u64.to_le_bytes());
    assert!(super::super::restore_mapping_registry(&frame).is_err());
    let source = event(None);
    let slot = Slot::new(source.0 as usize);
    frame[8..12].copy_from_slice(&2u32.to_le_bytes());
    frame[16..24].copy_from_slice(&(slot.0 as u64).to_le_bytes());
    frame.extend_from_slice(&(slot.0 as u64).to_le_bytes());
    assert!(super::super::restore_mapping_registry(&frame).is_err());
    // Invalid input is parsed in a local registry; it does not replace live state.
    assert!(mappings().lock().unwrap().handle_slots.is_empty());

    frame.truncate(24);
    frame[8..12].copy_from_slice(&1u32.to_le_bytes());
    super::super::restore_mapping_registry(&frame).unwrap();
    assert_eq!(mappings().lock().unwrap().handle_slots, [slot.0 as usize]);
    assert!(unregister_fork_handle_slot(slot.0));
}

#[test]
fn retained_mapping_wire_requires_registered_owner_and_known_memory_domain() {
    let _serial = SERIAL.lock().unwrap();
    let _transaction = begin_fork_mapping_transaction().unwrap();
    let source = event(None);
    let slot = Slot::new(source.0 as usize);
    let mut frame = vec![0u8; 16];
    frame[12..16].copy_from_slice(&super::super::MAPPING_HANDOFF_VERSION.to_le_bytes());
    frame[0..4].copy_from_slice(&1u32.to_le_bytes());
    frame.extend_from_slice(&0x10000u64.to_le_bytes());
    frame.extend_from_slice(&0x1000u64.to_le_bytes());
    frame.extend_from_slice(&1u32.to_le_bytes());
    frame.extend_from_slice(&3u32.to_le_bytes());
    frame.extend_from_slice(&(slot.0 as u64).to_le_bytes());
    frame.extend_from_slice(&0x3000u64.to_le_bytes());
    frame.extend_from_slice(&4u32.to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    assert!(super::super::restore_mapping_registry(&frame).is_err());
    frame[8..12].copy_from_slice(&1u32.to_le_bytes());
    frame.extend_from_slice(&(slot.0 as u64).to_le_bytes());
    frame[60] = 2;
    assert!(super::super::restore_mapping_registry(&frame).is_err());
    frame[60] = 1;
    super::super::restore_mapping_registry(&frame).unwrap();
    {
        let mut registry = mappings().lock().unwrap();
        assert_eq!(registry.entries[0].view_protection, 4);
        assert_eq!(registry.entries[0].backing_offset, 0x3000);
        assert_eq!(
            registry.entries[0].domain,
            super::super::ForkMappingDomain::GuestMm
        );
        registry.clear();
    }
}

#[test]
fn failed_target_duplication_does_not_publish_a_child_slot() {
    let _serial = SERIAL.lock().unwrap();
    let _transaction = begin_fork_mapping_transaction().unwrap();
    let source = event(None);
    let not_a_process = event(None);
    let slot = Slot::new(source.0 as usize);
    assert!(unsafe { register_fork_handle_slot(slot.0) });
    let error = match unsafe { duplicate_into(not_a_process.0) } {
        Err(error) => error,
        Ok(_) => panic!("an event handle cannot receive a child handle table"),
    };
    assert_eq!(error.stage, ForkStage::GuestMapping);
    assert_ne!(error.os_code, 0);
    assert_eq!(
        unsafe { (*slot.0).load(Ordering::Acquire) },
        source.0 as usize
    );
    let mut flags = 0;
    assert_ne!(unsafe { GetHandleInformation(source.0, &mut flags) }, 0);
    assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
}
