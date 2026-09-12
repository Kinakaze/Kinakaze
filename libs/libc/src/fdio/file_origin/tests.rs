use super::*;

struct Fixture {
    path: PathBuf,
    fd: Option<i32>,
}
impl Fixture {
    fn new(pages: usize, enabled: bool) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kinakaze-vma-origin-{}-{stamp}",
            std::process::id()
        ));
        let page = memory_geometry().0;
        let bytes: Vec<u8> = (0..pages * page)
            .map(|at| 0x40 + (at / page) as u8)
            .collect();
        std::fs::write(&path, bytes).unwrap();
        let fd = fs::open(&kinakaze_vfs::path::to_guest_path(&path), fs::O_RDONLY, 0).unwrap();
        if enabled {
            fs::verity::enable(fd, 1, 4096, &[]).unwrap();
        }
        Self { path, fd: Some(fd) }
    }
    fn close(&mut self) {
        if let Some(fd) = self.fd.take() {
            kinakaze_vfs::close(fd).unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.close();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn info(address: usize) -> Info {
    mappings()
        .lock()
        .unwrap()
        .iter()
        .find_map(|(&at, mapping)| {
            (at <= address && address < at + mapping.length)
                .then(|| mapping.file_origin.clone())
                .flatten()
        })
        .expect("file origin retained")
}
fn live(handle: *mut c_void) -> bool {
    let mut flags = 0;
    unsafe { windows_sys::Win32::Foundation::GetHandleInformation(handle, &mut flags) != 0 }
}

#[test]
fn retained_origin_survives_fd_close_and_native_partial_protection() {
    let page = memory_geometry().0;
    let mut fixture = Fixture::new(4, false);
    let mapped = map(
        ptr::null_mut(),
        page * 3,
        PROT_READ,
        MAP_PRIVATE,
        fixture.fd.unwrap(),
        page as i64,
    )
    .unwrap();
    let owner = info(mapped as usize);
    let handle = owner.owner.handle();
    let mut flags = 0;
    assert_ne!(
        unsafe { windows_sys::Win32::Foundation::GetHandleInformation(handle, &mut flags) },
        0
    );
    assert_eq!(
        flags & windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
        0
    );
    assert_eq!(
        owner.owner.offset(mapped as usize + page).unwrap(),
        (page * 2) as u64
    );
    assert_eq!(owner.owner.initial_protection(), PROT_READ);
    fixture.close();
    fs::unlink(&kinakaze_vfs::path::to_guest_path(&fixture.path)).unwrap();
    std::fs::write(&fixture.path, vec![0xcc; page * 4]).unwrap();
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x41);
    let middle = unsafe { mapped.cast::<u8>().add(page) }.cast();
    assert_eq!(unsafe { kinakaze_abi_mprotect(middle, page, PROT_NONE) }, 0);
    assert_eq!(owner.owner.protection(mapped as usize).unwrap(), PROT_READ);
    assert_eq!(owner.owner.protection(middle as usize).unwrap(), PROT_NONE);
    assert_eq!(
        owner.owner.protection(mapped as usize + 2 * page).unwrap(),
        PROT_READ
    );
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(middle, page, PROT_READ | PROT_WRITE) },
        0
    );
    unsafe {
        *middle.cast::<u8>() = 0x17;
    }
    assert_eq!(owner.owner.initial_protection(), PROT_READ);
    assert_eq!(
        unmap(middle, page),
        Err(EINVAL),
        "unsupported native split must preserve its owner"
    );
    assert_eq!(
        info(mapped as usize).owner.identity(),
        owner.owner.identity()
    );
    assert_eq!(unsafe { *middle.cast::<u8>() }, 0x17);
    unmap(mapped, page * 3).unwrap();
    assert!(live(handle), "explicit inspection pin still owns the inode");
    drop(owner);
    assert!(
        !live(handle),
        "last origin reference closes its registered inode handle"
    );
}

#[test]
fn retained_origin_follows_verified_splits_discard_and_anonymous_replacement() {
    let page = memory_geometry().0;
    let mut fixture = Fixture::new(3, true);
    let mapped = map(
        ptr::null_mut(),
        page * 3,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    let original = info(mapped as usize);
    let handle = original.owner.handle();
    fixture.close();
    unsafe {
        *mapped.cast::<u8>() = 0x19;
    }
    madvise_impl(mapped, page, MADV_DONTNEED).unwrap();
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x40);
    let middle = unsafe { mapped.cast::<u8>().add(page) }.cast();
    assert_eq!(unsafe { kinakaze_abi_mprotect(middle, page, PROT_READ) }, 0);
    assert_eq!(
        original.owner.protection(middle as usize).unwrap(),
        PROT_READ
    );
    map(
        middle,
        page,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
        -1,
        0,
    )
    .unwrap();
    assert!(
        mappings()
            .lock()
            .unwrap()
            .get(&(middle as usize))
            .unwrap()
            .file_origin
            .is_none()
    );
    assert_eq!(
        info(mapped as usize).owner.identity(),
        original.owner.identity()
    );
    assert_eq!(
        info(mapped as usize + page * 2)
            .owner
            .offset(mapped as usize + page * 2)
            .unwrap(),
        (page * 2) as u64
    );
    unmap(mapped, page).unwrap();
    unmap(unsafe { mapped.cast::<u8>().add(page * 2) }.cast(), page).unwrap();
    assert!(live(handle));
    drop(original);
    assert!(!live(handle));
    unmap(middle, page).unwrap();
}

#[test]
fn copied_file_origin_is_not_anonymous_for_discard_and_releases_all_handles() {
    let page = memory_geometry().0;
    let fixture = Fixture::new(3, false);
    let mut baseline = 0;
    // Warm allocator/coordinator state before checking repeated VMA retirement.
    let warm = map(
        ptr::null_mut(),
        page,
        PROT_READ,
        MAP_PRIVATE,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    unmap(warm, page).unwrap();
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::GetProcessHandleCount(
                GetCurrentProcess(),
                &mut baseline,
            )
        },
        0
    );
    for _ in 0..20 {
        let reserved = map(
            ptr::null_mut(),
            page * 3,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
        .unwrap();
        let mapped = map(
            reserved,
            page * 3,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_FIXED,
            fixture.fd.unwrap(),
            0,
        )
        .unwrap();
        // Anonymous VMAs now use COW sections, so MAP_FIXED can install a
        // native file view. Explicitly materialize it to exercise the copied
        // representation whose discard and handle retirement this test owns.
        {
            let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
            let mut registry = mappings().lock().unwrap();
            let mapping = registry.get(&(mapped as usize)).unwrap().clone();
            cow_materialize::make_remappable(&mut registry, mapped as usize, mapping).unwrap();
        }
        let origin = info(mapped as usize);
        assert_eq!(origin.representation, Representation::CopiedUnclassified);
        unsafe {
            *mapped.cast::<u8>() = 0x28;
        }
        assert_eq!(
            madvise_impl(mapped, page, MADV_DONTNEED),
            Err(kinakaze_vfs::EOPNOTSUPP)
        );
        assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x28);
        let middle = unsafe { mapped.cast::<u8>().add(page) }.cast();
        map(
            middle,
            page,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
            -1,
            0,
        )
        .unwrap();
        assert!(
            mappings()
                .lock()
                .unwrap()
                .get(&(middle as usize))
                .unwrap()
                .file_origin
                .is_none()
        );
        assert_eq!(
            info(mapped as usize + 2 * page).owner.identity(),
            origin.owner.identity()
        );
        drop(origin);
        unmap(mapped, page * 3).unwrap();
    }
    let mut after = 0;
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::GetProcessHandleCount(
                GetCurrentProcess(),
                &mut after,
            )
        },
        0
    );
    assert_eq!(
        after, baseline,
        "repeated replacement/split/unmap must retire handle slots"
    );
}

#[test]
fn retained_origin_refreshes_eof_through_another_fd_and_keeps_tail_protection() {
    let page = memory_geometry().0;
    let mut fixture = Fixture::new(1, false);
    let mapped = map(
        ptr::null_mut(),
        page * 3,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    let owner = info(mapped as usize);
    fixture.close();
    let alias = fixture.path.with_extension("renamed");
    std::fs::rename(&fixture.path, &alias).unwrap();
    let original_path = std::mem::replace(&mut fixture.path, alias);
    std::fs::write(&original_path, vec![0xce; page * 3]).unwrap();
    let unrelated = fs::open(
        &kinakaze_vfs::path::to_guest_path(&original_path),
        fs::O_RDONLY,
        0,
    )
    .unwrap();
    let alias = fs::open(
        &kinakaze_vfs::path::to_guest_path(&fixture.path),
        fs::O_RDWR,
        0,
    )
    .unwrap();
    let tail = unsafe { mapped.cast::<u8>().add(page) }.cast();
    assert_eq!(unsafe { kinakaze_abi_mprotect(tail, page, PROT_NONE) }, 0);
    assert_eq!(crate::fs::ftruncate(alias, (page * 3) as i64), 0);
    let mut state: MemoryBasicInformation = unsafe { core::mem::zeroed() };
    assert_ne!(
        unsafe { VirtualQuery(tail, &mut state, size_of::<MemoryBasicInformation>()) },
        0
    );
    assert!(state.state == MEM_RESERVE_STATE || state.protect == PAGE_NOACCESS);
    assert_eq!(owner.owner.protection(tail as usize).unwrap(), PROT_NONE);
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(tail, page, PROT_READ | PROT_WRITE) },
        0,
        "errno={} representation={:?}",
        crate::kinakaze_errno(),
        info(tail as usize).representation,
    );
    assert_eq!(unsafe { *tail.cast::<u8>() }, 0);
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x40);
    assert_eq!(info(tail as usize).owner.identity(), owner.owner.identity());
    kinakaze_vfs::close(alias).unwrap();
    kinakaze_vfs::close(unrelated).unwrap();
    unmap(mapped, page * 3).unwrap();
    drop(owner);
    std::fs::remove_file(original_path).unwrap();
}

#[test]
fn short_file_origin_keeps_the_complete_last_linux_page() {
    let mut fixture = Fixture::new(1, false);
    fixture.close();
    std::fs::write(&fixture.path, b"x").unwrap();
    let fd = fs::open(
        &kinakaze_vfs::path::to_guest_path(&fixture.path),
        fs::O_RDONLY,
        0,
    )
    .unwrap();
    let page = memory_geometry().0;
    let mapped = map(ptr::null_mut(), 1, PROT_READ, MAP_PRIVATE, fd, 0).unwrap();
    kinakaze_vfs::close(fd).unwrap();
    assert_eq!(info(mapped as usize).owner.length(), page);
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ | PROT_WRITE) },
        0
    );
    assert_eq!(unsafe { *mapped.cast::<u8>() }, b'x');
    assert_eq!(unsafe { *mapped.cast::<u8>().add(page - 1) }, 0);
    unmap(mapped, page).unwrap();
}

#[test]
fn shared_origin_retains_the_real_section_and_readonly_permissions() {
    use std::io::{Seek, SeekFrom, Write};
    let page = memory_geometry().0;
    let mut fixture = Fixture::new(4, false);
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .unwrap();
    let mapped = map(
        ptr::null_mut(),
        page * 2,
        PROT_READ,
        MAP_SHARED,
        fixture.fd.unwrap(),
        page as i64,
    )
    .unwrap();
    let owner = info(mapped as usize);
    assert!(owner.owner.shared());
    assert_eq!(owner.representation, Representation::NativeSection);
    let section = {
        let registry = mappings().lock().unwrap();
        let mapping = registry.get(&(mapped as usize)).unwrap();
        let backing = mapping.backing.as_ref().unwrap();
        assert!(mapping.shared && backing.is_retained_shared());
        assert!(
            !backing.is_remappable(),
            "no whole-view neighbour access gap"
        );
        assert_eq!(backing.maximum_protection(), PAGE_READONLY);
        assert_eq!(mapping.backing_offset, page as u64);
        backing.handle()
    };
    fixture.close();
    fs::unlink(&kinakaze_vfs::path::to_guest_path(&fixture.path)).unwrap();
    std::fs::write(&fixture.path, vec![0xcc; page * 4]).unwrap();
    writer.seek(SeekFrom::Start(page as u64)).unwrap();
    writer.write_all(&[0x72]).unwrap();
    writer.flush().unwrap();
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x72);
    assert_eq!(unsafe { kinakaze_abi_mprotect(mapped, page, PROT_NONE) }, 0);
    assert_eq!(unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ) }, 0);
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ | PROT_WRITE) },
        -1
    );
    assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EACCES);
    assert_eq!(owner.owner.protection(mapped as usize).unwrap(), PROT_READ);
    assert_eq!(unmap(mapped, page), Err(EINVAL));
    assert!(live(section));
    unmap(mapped, page * 2).unwrap();
    assert!(
        !live(section),
        "last view closes its registered shared section"
    );
    assert!(live(owner.owner.handle()));
}

#[test]
fn shared_origin_initial_noaccess_restores_access_without_copying() {
    let fixture = Fixture::new(2, false);
    let page = memory_geometry().0;
    let mapped = map(
        ptr::null_mut(),
        page * 2,
        PROT_NONE,
        MAP_SHARED,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    let mut state: MemoryBasicInformation = unsafe { core::mem::zeroed() };
    assert_ne!(
        unsafe { VirtualQuery(mapped, &mut state, size_of::<MemoryBasicInformation>()) },
        0
    );
    assert_eq!(state.protect, PAGE_NOACCESS);
    assert_eq!(state.type_, MEM_MAPPED_TYPE);
    assert_eq!(unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ) }, 0);
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x40);
    assert!(
        mappings()
            .lock()
            .unwrap()
            .get(&(mapped as usize))
            .unwrap()
            .backing
            .as_ref()
            .unwrap()
            .is_retained_shared()
    );
    unmap(mapped, page * 2).unwrap();
}

#[test]
fn shared_origin_retires_both_handle_slots_without_native_leaks() {
    let fixture = Fixture::new(2, false);
    let page = memory_geometry().0;
    let once = || {
        let mapped = map(
            ptr::null_mut(),
            page * 2,
            PROT_READ,
            MAP_SHARED,
            fixture.fd.unwrap(),
            0,
        )
        .unwrap();
        unmap(mapped, page * 2).unwrap();
    };
    once();
    let count = || {
        let mut result = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Threading::GetProcessHandleCount(
                    GetCurrentProcess(),
                    &mut result,
                )
            },
            0
        );
        result
    };
    let before = count();
    for _ in 0..20 {
        once();
    }
    assert_eq!(count(), before);
}

#[test]
fn shared_origin_read_mapping_keeps_the_original_maywrite_right() {
    let mut fixture = Fixture::new(2, false);
    fixture.close();
    fixture.fd = Some(
        fs::open(
            &kinakaze_vfs::path::to_guest_path(&fixture.path),
            fs::O_RDWR,
            0,
        )
        .unwrap(),
    );
    let page = memory_geometry().0;
    let mapped = map(
        ptr::null_mut(),
        page * 2,
        PROT_READ,
        MAP_SHARED,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    fixture.close();
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ | PROT_WRITE) },
        0
    );
    unsafe {
        *mapped.cast::<u8>() = 0x91;
    }
    assert_eq!(std::fs::read(&fixture.path).unwrap()[0], 0x91);
    unmap(mapped, page * 2).unwrap();
}

#[test]
fn shared_origin_fixed_native_maywrite_limit_preserves_the_readonly_view() {
    let mut fixture = Fixture::new(2, false);
    fixture.close();
    fixture.fd = Some(
        fs::open(
            &kinakaze_vfs::path::to_guest_path(&fixture.path),
            fs::O_RDWR,
            0,
        )
        .unwrap(),
    );
    let page = memory_geometry().0;
    let reservation = map(
        ptr::null_mut(),
        page * 3,
        PROT_NONE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )
    .unwrap();
    let mapped = map(
        reservation,
        page * 2,
        PROT_READ,
        MAP_SHARED | MAP_FIXED,
        fixture.fd.unwrap(),
        0,
    )
    .unwrap();
    assert_eq!(
        unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ | PROT_WRITE) },
        -1,
        "errno={}",
        crate::kinakaze_errno()
    );
    // This is a documented host limitation, not a passing Linux MAYWRITE
    // implementation. Do not introduce a transient writable published VMA.
    assert_eq!(crate::kinakaze_errno(), EINVAL);
    assert_eq!(
        info(mapped as usize)
            .owner
            .protection(mapped as usize)
            .unwrap(),
        PROT_READ
    );
    assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x40);
    unmap(reservation, page * 3).unwrap();
}

#[test]
fn native_shared_section_readonly_view_cannot_gain_write_despite_section_rights() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Memory::{
        MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3, UnmapViewOfFile2,
    };
    let mut fixture = Fixture::new(2, false);
    fixture.close();
    fixture.fd = Some(
        fs::open(
            &kinakaze_vfs::path::to_guest_path(&fixture.path),
            fs::O_RDWR,
            0,
        )
        .unwrap(),
    );
    let opened = fs::verity::Opened::from_fd(fixture.fd.unwrap()).unwrap();
    let page = memory_geometry().0;
    let section = unsafe {
        CreateFileMappingW(
            opened.borrowed_handle(),
            ptr::null(),
            PAGE_READWRITE,
            0,
            0,
            ptr::null(),
        )
    };
    assert!(!section.is_null());
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryObject(
            handle: *mut c_void,
            class: u32,
            output: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    let mut object_basic = [0u64; 7];
    assert!(
        unsafe {
            NtQueryObject(
                section,
                0,
                object_basic.as_mut_ptr().cast(),
                size_of_val(&object_basic) as u32,
                ptr::null_mut(),
            )
        } >= 0
    );
    let section_access = (object_basic[0] >> 32) as u32;
    assert_ne!(section_access & 2, 0, "SECTION_MAP_WRITE really is granted");
    let reservation = unsafe { reserve_placeholder_at(ptr::null_mut(), page * 3) };
    assert!(!reservation.is_null());
    assert_ne!(
        unsafe {
            VirtualFree(
                reservation,
                page * 2,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            )
        },
        0
    );
    let view: MEMORY_MAPPED_VIEW_ADDRESS = unsafe {
        MapViewOfFile3(
            section,
            GetCurrentProcess(),
            reservation,
            0,
            page * 2,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_READONLY,
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(view.Value, reservation);
    let mut ignored = 0;
    assert_eq!(
        unsafe { VirtualProtect(reservation, page, PAGE_READWRITE, &mut ignored) },
        0
    );
    let error = unsafe { GetLastError() };
    assert_eq!(
        error, 87,
        "the native syscall rejects the protection, not the file open"
    );
    let mut information: MemoryBasicInformation = unsafe { core::mem::zeroed() };
    assert_ne!(
        unsafe {
            VirtualQuery(
                reservation,
                &mut information,
                size_of::<MemoryBasicInformation>(),
            )
        },
        0
    );
    assert_eq!(information.protect, PAGE_READONLY);
    assert_eq!(information.allocation_protect, PAGE_READONLY);
    eprintln!(
        "SHARED_VIEW_RIGHTS section_create={PAGE_READWRITE:#x} section_granted={section_access:#x} MapViewOfFile3_initial={:#x} VirtualProtect_write_error={error}",
        information.allocation_protect
    );
    assert_ne!(
        unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) },
        0
    );
    assert_ne!(unsafe { VirtualFree(reservation, 0, MEM_RELEASE) }, 0);
    let tail = unsafe { reservation.cast::<u8>().add(page * 2) }.cast();
    assert_ne!(unsafe { VirtualFree(tail, 0, MEM_RELEASE) }, 0);

    // The same section supports write when the view itself starts with the
    // requisite access. This view is unpublished scratch, never a guest VMA.
    let writable = unsafe {
        MapViewOfFileEx(
            section,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            page * 2,
            ptr::null_mut(),
        )
    };
    assert!(!writable.is_null());
    assert_ne!(
        unsafe { VirtualProtect(writable, page, PAGE_READONLY, &mut ignored) },
        0
    );
    assert_ne!(
        unsafe { VirtualProtect(writable, page, PAGE_READWRITE, &mut ignored) },
        0
    );
    assert_ne!(unsafe { UnmapViewOfFile(writable) }, 0);
    assert_ne!(unsafe { CloseHandle(section) }, 0);
}

#[test]
fn positional_read_pin_survives_close_and_exact_fd_reuse() {
    let mut first = Fixture::new(1, false);
    let mut second = Fixture::new(1, false);
    second.close();
    std::fs::write(&second.path, vec![0x7a; memory_geometry().0]).unwrap();
    let fd = first.fd.unwrap();
    let (entry, pin) = kinakaze_vfs::pin_native_fd(fd, |_| Ok(())).unwrap();
    first.close();
    fs::unlink(&kinakaze_vfs::path::to_guest_path(&first.path)).unwrap();
    let reused = fs::open(
        &kinakaze_vfs::path::to_guest_path(&second.path),
        fs::O_RDONLY,
        0,
    )
    .unwrap();
    assert_eq!(reused, fd);
    second.fd = Some(reused);
    let mut bytes = [0; 8];
    assert_eq!(
        unsafe { read_pinned_at(entry, bytes.as_mut_ptr(), bytes.len(), 0) }.unwrap(),
        bytes.len()
    );
    assert_eq!(
        bytes, [0x40; 8],
        "pinned pread must not switch to the reused descriptor"
    );
    drop(pin);
    assert_eq!(
        unsafe { read_at(fd, bytes.as_mut_ptr(), bytes.len(), 0) }.unwrap(),
        bytes.len()
    );
    assert_eq!(bytes, [0x7a; 8]);
    assert_eq!(kinakaze_vfs::get(fd).unwrap().offset, 0);
}
