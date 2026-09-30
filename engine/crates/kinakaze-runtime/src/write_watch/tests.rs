use super::*;
use std::io::{Read, Write};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::SystemServices::MEM_WRITE_WATCH;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

struct Allocation(usize, usize);
impl Allocation {
    fn new(len: usize, watch: bool) -> Self {
        let address = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_RESERVE | MEM_COMMIT | if watch { MEM_WRITE_WATCH } else { 0 },
                PAGE_READWRITE,
            )
        };
        assert!(!address.is_null());
        Self(address as usize, len)
    }
    fn mapping(&self) -> ForkMapping {
        ForkMapping {
            base: self.0,
            len: self.1,
            behavior: ForkMappingBehavior::Copy,
            storage: ForkMappingStorage::Ordinary,
            backing_slot: 0,
            backing_offset: 0,
            view_protection: 0,
            domain: crate::ForkMappingDomain::GuestMm,
        }
    }
    fn plan(&self) -> Plan {
        Plan {
            base: self.0,
            len: self.1,
            pages: vec![0; self.1 / PAGE],
            used: None,
        }
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        let _transaction = crate::begin_fork_mapping_transaction().unwrap();
        crate::unregister_fork_mapping(self.0);
        assert_ne!(unsafe { VirtualFree(self.0 as _, 0, MEM_RELEASE) }, 0);
    }
}

#[test]
fn cpu_kernel_io_and_remote_writes_remain_cumulative_without_scanning_clean_pages() {
    let source = Allocation::new(8 * 1024 * 1024, true);
    let mut plan = source.plan();
    assert!(unsafe { plan.capture() });
    assert_eq!(plan.used, Some(0));
    unsafe { ((source.0 + 3 * PAGE + 17) as *mut u8).write(0x35) };
    let bytes = [0x7a; 37];
    let mut written = 0;
    assert_ne!(
        unsafe {
            WriteProcessMemory(
                GetCurrentProcess(),
                (source.0 + 7 * PAGE + 13) as _,
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut written,
            )
        },
        0
    );
    assert_eq!(written, bytes.len());
    let path =
        std::env::temp_dir().join(format!("kinakaze-write-watch-{}.bin", std::process::id()));
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&[0x19; 29]).unwrap();
    drop(file);
    let target =
        unsafe { std::slice::from_raw_parts_mut((source.0 + 11 * PAGE + 5) as *mut u8, 29) };
    std::fs::File::open(&path)
        .unwrap()
        .read_exact(target)
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(unsafe { plan.capture() });
    for page in [3, 7, 11] {
        assert!(plan.pages[..plan.used.unwrap()].contains(&(source.0 + page * PAGE)));
    }
    assert!(
        plan.used.unwrap() < 10,
        "fresh unused pages must remain unscanned"
    );
    let destination = Allocation::new(source.1, false);
    let counts = plan
        .copy(source.0, source.1, |start, length| {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    start as *const u8,
                    (destination.0 + start - source.0) as *mut u8,
                    length,
                )
            };
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(counts.0 < 10 * PAGE);
    assert_eq!(
        unsafe { *((destination.0 + 3 * PAGE + 17) as *const u8) },
        0x35
    );
    assert_eq!(
        unsafe { *((destination.0 + 7 * PAGE + 13) as *const u8) },
        0x7a
    );
    assert_eq!(
        unsafe { *((destination.0 + 11 * PAGE + 5) as *const u8) },
        0x19
    );
    assert_eq!(unsafe { *((destination.0 + 100 * PAGE) as *const u8) }, 0);
    unsafe { ((source.0 + 15 * PAGE + 8) as *mut u8).write(0x62) };
    assert!(unsafe { plan.capture() });
    for page in [3, 7, 11, 15] {
        assert!(plan.pages[..plan.used.unwrap()].contains(&(source.0 + page * PAGE)));
    }
}

#[test]
fn unsupported_history_and_replaced_registration_drop_the_hint() {
    let ordinary = Allocation::new(65536, false);
    assert!(!unsafe { ordinary.plan().capture() });
    let source = Allocation::new(65536, true);
    let _transaction = crate::begin_fork_mapping_transaction().unwrap();
    assert!(unsafe { crate::register_zero_write_watch_mapping(source.mapping()) });
    assert!(origins().lock().unwrap().contains(&(source.0, source.1)));
    let mut replacement = source.mapping();
    replacement.base += PAGE;
    replacement.len = PAGE;
    assert!(crate::register_fork_mapping(replacement));
    assert!(!origins().lock().unwrap().contains(&(source.0, source.1)));
    crate::unregister_fork_mapping(replacement.base);
    crate::unregister_fork_mapping(replacement.base + replacement.len);
}

#[test]
fn dirty_runs_intersect_protection_boundaries_and_propagate_failure() {
    let plan = Plan {
        base: 0x10000,
        len: 8 * PAGE,
        pages: vec![0x10000, 0x11000, 0x14000, 0x15000, 0x17000],
        used: Some(5),
    };
    let mut runs = Vec::new();
    assert_eq!(
        plan.copy(0x11000, 4 * PAGE, |start, len| {
            runs.push((start, len));
            Ok::<_, ()>(())
        })
        .unwrap(),
        (2 * PAGE, 2)
    );
    assert_eq!(runs, [(0x11000, PAGE), (0x14000, PAGE)]);
    assert_eq!(plan.copy(plan.base, plan.len, |_, _| Err(17)), Err(17));
}

#[test]
fn copied_destination_history_covers_old_new_and_protected_writes() {
    use windows_sys::Win32::System::Memory::{PAGE_NOACCESS, VirtualProtect};
    let source = Allocation::new(65536, true);
    let _transaction = crate::begin_fork_mapping_transaction().unwrap();
    assert!(crate::register_fork_mapping(source.mapping()));
    let payload = [0x71u8; 17];
    let mut written = 0;
    assert_ne!(
        unsafe {
            WriteProcessMemory(
                GetCurrentProcess(),
                (source.0 + PAGE) as _,
                payload.as_ptr().cast(),
                payload.len(),
                &mut written,
            )
        },
        0
    );
    let mut previous = 0;
    assert_ne!(
        unsafe { VirtualProtect((source.0 + PAGE) as _, PAGE, PAGE_NOACCESS, &mut previous) },
        0
    );
    assert!(unsafe { adopt_destination(source.mapping()) });
    assert!(origins().lock().unwrap().contains(&(source.0, source.1)));
    let mut plan = source.plan();
    assert!(unsafe { plan.capture() });
    assert!(plan.pages[..plan.used.unwrap()].contains(&(source.0 + PAGE)));
    unsafe { ((source.0 + 5 * PAGE) as *mut u8).write(0x52) };
    assert!(unsafe { plan.capture() });
    for page in [1, 5] {
        assert!(plan.pages[..plan.used.unwrap()].contains(&(source.0 + page * PAGE)));
    }
    assert_ne!(
        unsafe { VirtualProtect((source.0 + PAGE) as _, PAGE, previous, &mut previous) },
        0
    );
    let ordinary = Allocation::new(65536, false);
    assert!(!unsafe { adopt_destination(ordinary.mapping()) });
    let mut oversized = source.mapping();
    oversized.len = MAX_REGION + PAGE;
    assert!(!unsafe { adopt_destination(oversized) });
}
