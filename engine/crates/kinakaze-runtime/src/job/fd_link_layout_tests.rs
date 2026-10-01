use super::*;

unsafe fn value(base: *mut u8, pid: u32, fd: u32) -> Option<Vec<u8>> {
    let record = unsafe { find_fd_link(base, pid, fd)? };
    let length = unsafe { load32(record, FD_LINK_LENGTH) } as usize;
    assert!(length <= FD_LINK_TARGET_CAPACITY);
    Some(unsafe { core::slice::from_raw_parts(fd_link_target(base, record), length).to_vec() })
}

#[test]
fn both_layouts_preserve_targets_neighbors_and_fork_copies() {
    for stride in [FD_LINK_SIZE, FD_LINK_LEGACY_SIZE] {
        let mut storage = vec![0u64; SECTION_SIZE.div_ceil(8)];
        let base = storage.as_mut_ptr().cast();
        let targets: Vec<Vec<u8>> = [0, 1, 511, FD_LINK_TARGET_CAPACITY]
            .into_iter()
            .enumerate()
            .map(|(n, len)| vec![b'A' + n as u8; len])
            .collect();
        unsafe {
            store32(base, HEADER_FD_LINK_STRIDE, stride as u32);
            store64(base, PID_NAMESPACES_OFFSET, 0x0123_4567_89ab_cdef);
            for (fd, target) in targets.iter().enumerate() {
                assert!(write_fd_link(base, 7, fd as u32, target));
            }
            assert!(clone_fd_links(base, 7, 8));
            clear_fd_links(base, 7);
            for (fd, target) in targets.iter().enumerate() {
                assert_eq!(value(base, 7, fd as u32), None);
                assert_eq!(value(base, 8, fd as u32).as_ref(), Some(target));
            }
            assert!(write_fd_link(base, 8, 3, b"short"));
            let last = find_fd_link(base, 8, 3).unwrap();
            let raw =
                core::slice::from_raw_parts(fd_link_target(base, last), FD_LINK_TARGET_CAPACITY);
            assert_eq!(&raw[..5], b"short");
            assert!(raw[5..].iter().all(|b| *b == 0));
            assert_eq!(value(base, 8, 2).as_deref(), Some(targets[2].as_slice()));
            assert_eq!(load64(base, PID_NAMESPACES_OFFSET), 0x0123_4567_89ab_cdef);
        }
    }
}

#[test]
fn colliding_headers_keep_distinct_targets_through_replacement_and_deletion() {
    for stride in [FD_LINK_SIZE, FD_LINK_LEGACY_SIZE] {
        let mut storage = vec![0u64; SECTION_SIZE.div_ceil(8)];
        let base = storage.as_mut_ptr().cast();
        let pid = 42;
        let keys: Vec<_> = (0..u32::MAX)
            .filter(|fd| fd_link_hash(pid, *fd) == FD_LINK_CAPACITY - 1)
            .take(3)
            .collect();
        unsafe {
            store32(base, HEADER_FD_LINK_STRIDE, stride as u32);
            for (n, fd) in keys.iter().enumerate() {
                assert!(write_fd_link(
                    base,
                    pid,
                    *fd,
                    &vec![b'0' + n as u8; FD_LINK_TARGET_CAPACITY]
                ));
            }
            let middle = find_fd_link(base, pid, keys[1]).unwrap();
            store32(middle, FD_LINK_STATE, FD_LINK_TOMBSTONE);
            assert_eq!(
                value(base, pid, keys[2]),
                Some(vec![b'2'; FD_LINK_TARGET_CAPACITY])
            );
            assert!(write_fd_link(base, pid, keys[1], b"reused"));
            assert!(write_fd_link(base, pid, keys[0], b"updated"));
            assert_eq!(
                value(base, pid, keys[0]).as_deref(),
                Some(b"updated" as &[u8])
            );
            assert_eq!(
                value(base, pid, keys[1]).as_deref(),
                Some(b"reused" as &[u8])
            );
            assert_eq!(
                value(base, pid, keys[2]),
                Some(vec![b'2'; FD_LINK_TARGET_CAPACITY])
            );
            assert!(clone_fd_links(base, pid, pid + 1));
            clear_fd_links(base, pid);
            assert_eq!(
                value(base, pid + 1, keys[2]),
                Some(vec![b'2'; FD_LINK_TARGET_CAPACITY])
            );
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct Counters {
    size: u32,
    faults: u32,
    peak_working_set: usize,
    working_set: usize,
    peak_paged: usize,
    paged: usize,
    peak_nonpaged: usize,
    nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(process: HANDLE, info: *mut Counters, size: u32) -> i32;
}
fn counters() -> (u32, u64, u64) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut info = Counters::default();
    info.size = core::mem::size_of::<Counters>() as u32;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        assert_ne!(
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut info, info.size),
            0
        );
        assert_ne!(
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user
            ),
            0
        );
    }
    let ticks = |t: FILETIME| u64::from(t.dwLowDateTime) | (u64::from(t.dwHighDateTime) << 32);
    (info.faults, ticks(kernel), ticks(user))
}

#[test]
#[ignore = "fresh-view layout diagnostic, not a complete install benchmark"]
fn compare_cold_fd_link_scans() {
    for (run, stride) in [
        FD_LINK_LEGACY_SIZE,
        FD_LINK_SIZE,
        FD_LINK_SIZE,
        FD_LINK_LEGACY_SIZE,
    ]
    .into_iter()
    .enumerate()
    {
        unsafe {
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                SECTION_SIZE as u32,
                core::ptr::null(),
            );
            assert!(!section.is_null());
            let initial = MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE);
            assert!(!initial.Value.is_null());
            core::ptr::write_bytes(initial.Value, 0, SECTION_SIZE);
            let base = initial.Value.cast::<u8>();
            store32(base, HEADER_FD_LINK_STRIDE, stride as u32);
            for fd in 0..6 {
                assert!(write_fd_link(base, 1, fd, b"retained descriptor"));
            }
            let before = counters();
            let clock = std::time::Instant::now();
            for _ in 0..500 {
                let view = MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE);
                assert!(!view.Value.is_null());
                // This unnamed mapping has one test owner and no concurrent
                // mutation; each new view models a process's first table scan.
                clear_fd_links(
                    std::hint::black_box(view.Value.cast()),
                    std::hint::black_box(9999),
                );
                assert_ne!(UnmapViewOfFile(view), 0);
            }
            let ms = clock.elapsed().as_secs_f64() * 1000.0;
            let after = counters();
            for fd in 0..6 {
                assert_eq!(
                    value(base, 1, fd).as_deref(),
                    Some(b"retained descriptor" as &[u8])
                );
            }
            println!(
                "FDLINK_LAYOUT {{\"run\":{run},\"stride\":{stride},\"views\":500,\"ms\":{ms},\"page_faults\":{},\"kernel_ms\":{},\"user_ms\":{}}}",
                after.0 - before.0,
                (after.1 - before.1) as f64 / 10000.0,
                (after.2 - before.2) as f64 / 10000.0
            );
            assert_ne!(UnmapViewOfFile(initial), 0);
            assert_ne!(CloseHandle(section), 0);
        }
    }
}
