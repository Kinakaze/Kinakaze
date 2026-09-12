//! A queued signal must reach its handler after the overlay lock unwinds.
use super::*;
use kinakaze_vfs::{ENOENT, mount, signal};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::Threading::*};

static RELEASE: AtomicUsize = AtomicUsize::new(0);
static DELIVERED: AtomicUsize = AtomicUsize::new(0);
unsafe extern "sysv64" fn handler(_: i32) {
    DELIVERED.fetch_add(1, Ordering::SeqCst);
    unsafe { SetEvent(RELEASE.load(Ordering::Acquire) as _) };
}

struct Fixture {
    native: std::path::PathBuf,
    guest: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        match mount::unmount(&self.guest, 0) {
            Ok(()) | Err(kinakaze_vfs::EINVAL) => {}
            Err(error) => panic!("fixture unmount: {error}"),
        }
        std::fs::remove_dir_all(&self.native).unwrap();
    }
}

// Both tests alter the process-wide action and use this native release event.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn open_dispatches_once_and_retries_only_restart_or_stale_interruptions() {
    let _serial = TEST_LOCK.lock().unwrap();
    let previous_mask = signal::sigprocmask(signal::SIG_SETMASK, 0).unwrap();
    for restart in [false, true] {
        for delivered_inside in [false, true] {
            DELIVERED.store(0, Ordering::Release);
            let previous = signal::sigaction(
                signal::SIGUSR1,
                Some(signal::Action {
                    disposition: signal::Disposition::Handle(handler, 0),
                    flags: if restart { signal::SA_RESTART } else { 0 },
                    ..signal::Action::default()
                }),
            )
            .unwrap();
            let mut calls = 0;
            let result = crate::fs::open_with_restart(0, || {
                calls += 1;
                if calls > 1 {
                    return Ok(731);
                }
                signal::raise_thread_signal(
                    kinakaze_vfs::interrupt::current_thread_id(),
                    signal::SIGUSR1,
                )
                .unwrap();
                if delivered_inside {
                    assert_eq!(
                        signal::deliver_pending(),
                        if restart {
                            signal::Delivery::Restart
                        } else {
                            signal::Delivery::Interrupted
                        }
                    );
                    assert_eq!(DELIVERED.load(Ordering::Acquire), 1);
                }
                Err(kinakaze_vfs::EINTR)
            });
            assert_eq!(
                result,
                if restart {
                    Ok(731)
                } else {
                    Err(kinakaze_vfs::EINTR)
                }
            );
            assert_eq!(calls, if restart { 2 } else { 1 });
            assert_eq!(DELIVERED.load(Ordering::Acquire), 1);
            signal::sigaction(signal::SIGUSR1, Some(previous)).unwrap();
        }
    }
    let mut calls = 0;
    assert_eq!(
        crate::fs::open_with_restart(0, || {
            calls += 1;
            if calls == 1 {
                Err(kinakaze_vfs::EINTR)
            } else {
                Ok(731)
            }
        }),
        Ok(731)
    );
    assert_eq!(calls, 2);
    signal::sigprocmask(signal::SIG_SETMASK, previous_mask).unwrap();
}

fn interrupted(lock_name: &str, restart: bool, operation: impl FnOnce() -> i32) -> (i32, i32) {
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    assert!(!event.is_null());
    RELEASE.store(event as usize, Ordering::Release);
    DELIVERED.store(0, Ordering::Release);
    let old_mask = signal::sigprocmask(signal::SIG_SETMASK, 0).unwrap();
    let old_action = signal::sigaction(
        signal::SIGUSR1,
        Some(signal::Action {
            disposition: signal::Disposition::Handle(handler, 0),
            flags: if restart { signal::SA_RESTART } else { 0 },
            mask: 0,
            restorer: 0,
        }),
    )
    .unwrap();
    let lock_name: Vec<u16> = lock_name.encode_utf16().chain(Some(0)).collect();
    let event_bits = event as usize;
    let (sender, receiver) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || unsafe {
        let mutex = CreateMutexW(ptr::null(), 0, lock_name.as_ptr());
        assert!(!mutex.is_null());
        assert_eq!(WaitForSingleObject(mutex, 5000), WAIT_OBJECT_0);
        sender.send(()).unwrap();
        let released = WaitForSingleObject(event_bits as _, 5000);
        ReleaseMutex(mutex);
        CloseHandle(mutex);
        released
    });
    receiver.recv().unwrap();
    signal::raise_thread_signal(
        kinakaze_vfs::interrupt::current_thread_id(),
        signal::SIGUSR1,
    )
    .unwrap();
    let result = operation();
    let error = kinakaze_tls::errno();
    let delivered = DELIVERED.load(Ordering::Acquire);
    let pending = signal::pending();
    // Release before asserting, so the old implementation fails without
    // leaving a native mutex holder or queued default-action signal.
    unsafe { SetEvent(event) };
    let released = holder.join().unwrap();
    signal::deliver_pending();
    signal::sigaction(signal::SIGUSR1, Some(old_action)).unwrap();
    signal::sigprocmask(signal::SIG_SETMASK, old_mask).unwrap();
    unsafe { CloseHandle(event) };
    assert_eq!(released, WAIT_OBJECT_0);
    assert_eq!(
        delivered, 1,
        "restart={restart}: pending signal was not dispatched"
    );
    assert_eq!(pending & (1 << (signal::SIGUSR1 - 1)), 0);
    (result, error)
}

#[test]
fn symlink_and_symlinkat_dispatch_pending_signals_after_overlay_lock_unwinds() {
    let _serial = TEST_LOCK.lock().unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let native = std::env::temp_dir().join(format!(
        "kinakaze-symlink-signal-{}-{nonce}",
        std::process::id()
    ));
    for part in ["lower", "upper", "work", "merged"] {
        std::fs::create_dir_all(native.join(part)).unwrap();
    }
    let guest = kinakaze_vfs::to_guest_path(&native.join("merged"));
    mount::overlay(
        &guest,
        &[kinakaze_vfs::to_guest_path(&native.join("lower"))],
        Some(&kinakaze_vfs::to_guest_path(&native.join("upper"))),
        Some(&kinakaze_vfs::to_guest_path(&native.join("work"))),
        0,
    )
    .unwrap();
    let fixture = Fixture { native, guest };
    let upper = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(fixture.native.join("upper"))
        .unwrap();
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { core::mem::zeroed() };
    assert_ne!(
        unsafe { GetFileInformationByHandle(upper.as_raw_handle(), &mut info) },
        0
    );
    let lock_name = format!(
        r"Local\kinakaze.xattr.v1.{:08x}.{:08x}{:08x}",
        info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
    );
    let fd = fs::open(&fixture.guest, fs::O_PATH | fs::O_DIRECTORY, 0).unwrap();
    assert!(!kinakaze_vfs::interrupt::current().is_null());
    for at in [false, true] {
        for restart in [false, true] {
            let name = format!("link-{at}-{restart}");
            let path = std::ffi::CString::new(if at {
                name.clone()
            } else {
                format!("{}/{name}", fixture.guest)
            })
            .unwrap();
            let (result, error) = interrupted(&lock_name, restart, || unsafe {
                if at {
                    kinakaze_abi_symlinkat(c"/proc/mounts".as_ptr(), fd, path.as_ptr())
                } else {
                    kinakaze_abi_symlink(c"/proc/mounts".as_ptr(), path.as_ptr())
                }
            });
            assert_eq!(result, if restart { 0 } else { -1 });
            if !restart {
                assert_eq!(error, kinakaze_vfs::EINTR);
                assert_eq!(
                    fs::lstat(&format!("{}/{name}", fixture.guest)).err(),
                    Some(ENOENT)
                );
                assert_eq!(
                    unsafe {
                        if at {
                            kinakaze_abi_symlinkat(c"/proc/mounts".as_ptr(), fd, path.as_ptr())
                        } else {
                            kinakaze_abi_symlink(c"/proc/mounts".as_ptr(), path.as_ptr())
                        }
                    },
                    0
                );
            }
            assert_eq!(
                link_target(&format!("{}/{name}", fixture.guest)).unwrap(),
                "/proc/mounts"
            );
            assert_eq!(
                std::fs::read_dir(fixture.native.join("work"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }
    kinakaze_vfs::close(fd).unwrap();
}

#[test]
fn overlay_mount_dispatches_pending_signals_without_duplicate_publication() {
    let _serial = TEST_LOCK.lock().unwrap();
    assert!(!kinakaze_vfs::interrupt::current().is_null());
    for volatile in [false, true] {
        for restart in [false, true] {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let native = std::env::temp_dir().join(format!(
                "kinakaze-mount-signal-{}-{nonce}",
                std::process::id()
            ));
            for part in ["lower", "upper", "work", "merged"] {
                std::fs::create_dir_all(native.join(part)).unwrap();
            }
            let fixture = Fixture {
                guest: kinakaze_vfs::to_guest_path(&native.join("merged")),
                native,
            };
            let upper = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(fixture.native.join("upper"))
                .unwrap();
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { core::mem::zeroed() };
            assert_ne!(
                unsafe { GetFileInformationByHandle(upper.as_raw_handle(), &mut info) },
                0
            );
            let lock_name = format!(
                r"Local\kinakaze.xattr.v1.{:08x}.{:08x}{:08x}",
                info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
            );
            let target = std::ffi::CString::new(fixture.guest.clone()).unwrap();
            let data = std::ffi::CString::new(format!(
                "lowerdir={},upperdir={},workdir={}{}",
                kinakaze_vfs::to_guest_path(&fixture.native.join("lower")),
                kinakaze_vfs::to_guest_path(&fixture.native.join("upper")),
                kinakaze_vfs::to_guest_path(&fixture.native.join("work")),
                if volatile { ",volatile" } else { "" },
            ))
            .unwrap();
            let operation = || unsafe {
                crate::sysadmin::kinakaze_abi_mount(
                    c"overlay".as_ptr(),
                    target.as_ptr(),
                    c"overlay".as_ptr(),
                    0,
                    data.as_ptr().cast(),
                )
            };
            let before = mount::snapshot_list().unwrap();
            let (result, error) = interrupted(&lock_name, restart, operation);
            // Volatile workdir markers persist. Dispatch but do not implicitly
            // replay that mount, even for a SA_RESTART handler.
            if restart && !volatile {
                assert_eq!(result, 0);
            } else {
                assert_eq!(result, -1);
                assert_eq!(error, kinakaze_vfs::EINTR);
                assert_eq!(mount::snapshot_list().unwrap(), before);
                assert_eq!(
                    std::fs::read_dir(fixture.native.join("work"))
                        .unwrap()
                        .count(),
                    0
                );
                assert_eq!(operation(), 0);
            }
            let after = mount::snapshot_list().unwrap();
            assert_eq!(
                after.len(),
                before.len() + 1,
                "exactly one mount must be published"
            );
            mount::unmount(&fixture.guest, 0).unwrap();
            assert_eq!(mount::snapshot_list().unwrap(), before);
            assert_eq!(
                fixture.native.join("work/work/incompat/volatile").is_dir(),
                volatile
            );
        }
    }
}
