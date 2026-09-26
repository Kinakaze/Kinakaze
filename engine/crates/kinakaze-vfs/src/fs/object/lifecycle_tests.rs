//! Native experiments for the namespace-reference design. These deliberately
//! distinguish an inode identifier, an owning reference and a new file open.

use super::*;
use std::io::Write;
use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::process::Command;
use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, ExtendedFileIdType, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_ID_DESCRIPTOR,
    FILE_ID_DESCRIPTOR_0, FILE_ID_INFO, FileDispositionInfoEx, FileIdInfo,
    GetFileInformationByHandleEx, OpenFileById, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kinakaze-object-lifetime-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // This fixture owns this freshly created directory and nothing else.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity(object: &Object) -> FILE_ID_INFO {
    let mut info = unsafe { zeroed::<FILE_ID_INFO>() };
    assert_ne!(
        unsafe {
            GetFileInformationByHandleEx(
                object.raw(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        },
        0,
        "identity query: {}",
        unsafe { GetLastError() }
    );
    info
}

fn by_id(volume_hint: &Object, info: &FILE_ID_INFO, access: u32) -> Result<Object, i32> {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            ExtendedFileId: info.FileId,
        },
    };
    Object::owned(unsafe {
        OpenFileById(
            volume_hint.raw(),
            &descriptor,
            access | SYNCHRONIZE,
            SHARE,
            ptr::null(),
            FLAGS,
        )
    })
}

fn delete_name(path: &Path) {
    let delete = Object::open(path, DELETE).unwrap();
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    assert_ne!(
        unsafe {
            SetFileInformationByHandle(
                delete.raw(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        },
        0,
        "POSIX deletion: {}",
        unsafe { GetLastError() }
    );
}

#[test]
fn identifier_reopen_observes_native_unlink_lifetime() {
    for (directory, reopen_unlinked) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let fixture = Fixture::new();
        let path = fixture.0.join("original");
        if directory {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, b"original").unwrap();
        }
        let volume = Object::open(&fixture.0, FILE_READ_ATTRIBUTES).unwrap();
        let pin = Object::open(&path, FILE_READ_ATTRIBUTES).unwrap();
        let info = identity(&pin);
        let moved = fixture.0.join("moved");
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        let reopened = by_id(&volume, &info, FILE_READ_ATTRIBUTES).unwrap();
        assert_eq!(
            identity(&reopened).FileId.Identifier,
            info.FileId.Identifier
        );
        drop(reopened);
        delete_name(&moved);
        assert!(!moved.exists());
        if reopen_unlinked {
            let reopened = by_id(&volume, &info, FILE_READ_ATTRIBUTES);
            eprintln!("directory={directory} POSIX-unlinked by-ID: {reopened:?}");
            if let Ok(ref reopened) = reopened {
                assert_eq!(identity(reopened).FileId.Identifier, info.FileId.Identifier);
            }
            // Empty-name reopen is a different operation from opening a saved ID.
            let direct = Object::reopen(pin.raw(), FILE_READ_ATTRIBUTES).unwrap();
            assert_eq!(identity(&direct).FileId.Identifier, info.FileId.Identifier);
        }
        drop(pin);
        let after_close = by_id(&volume, &info, FILE_READ_ATTRIBUTES);
        eprintln!(
            "directory={directory} reopened_unlinked={reopen_unlinked} after owned references close: {after_close:?}"
        );
        // Closing our handles does not establish that every kernel/filter
        // reference has gone away. Immediate success is not a lifetime lease,
        // and immediate failure is not a portable timing requirement either.
        if let Ok(ref after_close) = after_close {
            assert_eq!(
                identity(after_close).FileId.Identifier,
                info.FileId.Identifier
            );
        }
        assert_eq!(std::fs::read(path).unwrap(), b"replacement");
    }
}

#[test]
fn reference_sender_helper() {
    let Ok(path) = std::env::var("KINAKAZE_OBJECT_TRANSFER_PATH") else {
        return;
    };
    let receiver: u32 = std::env::var("KINAKAZE_OBJECT_TRANSFER_PID")
        .unwrap()
        .parse()
        .unwrap();
    let path = PathBuf::from(path);
    std::fs::write(&path, b"pinned content").unwrap();
    let file = Object::open(&path, GENERIC_READ).unwrap();
    let info = identity(&file);
    let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, receiver) };
    assert!(!process.is_null());
    let mut remote = ptr::null_mut();
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.raw(),
            process,
            &mut remote,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    unsafe { CloseHandle(process) };
    assert_ne!(result, 0);
    delete_name(&path);
    std::fs::write(&path, b"replacement").unwrap();
    println!(
        "TRANSFER {} {:032x}",
        remote as usize,
        u128::from_le_bytes(info.FileId.Identifier)
    );
    std::io::stdout().flush().unwrap();
    // No RAII teardown: the OS closes the sender's complete handle table.
    std::process::exit(23);
}

#[test]
fn transferred_reference_survives_sender_exit_and_posix_unlink() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fs::object::lifecycle_tests::reference_sender_helper",
            "--nocapture",
        ])
        .env("KINAKAZE_OBJECT_TRANSFER_PATH", &path)
        .env(
            "KINAKAZE_OBJECT_TRANSFER_PID",
            std::process::id().to_string(),
        )
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let record = text
        .lines()
        .find_map(|line| line.strip_prefix("TRANSFER "))
        .unwrap();
    let mut fields = record.split_whitespace();
    // The helper put this handle in this process's table; it is owned here,
    // not a numeric handle borrowed from the exited process.
    let received =
        Object::owned(fields.next().unwrap().parse::<usize>().unwrap() as HANDLE).unwrap();
    let expected = u128::from_str_radix(fields.next().unwrap(), 16).unwrap();
    assert_eq!(
        u128::from_le_bytes(identity(&received).FileId.Identifier),
        expected
    );
    let mut bytes = [0u8; 32];
    let count = received.read_at(0, &mut bytes).unwrap();
    assert_eq!(&bytes[..count], b"pinned content");
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
}

#[test]
#[ignore = "manual native open cost experiment, not a timing assertion"]
fn compare_native_open_costs() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file");
    std::fs::write(&path, b"data").unwrap();
    let pin = Object::open(&path, FILE_READ_ATTRIBUTES).unwrap();
    let volume = Object::open(&fixture.0, FILE_READ_ATTRIBUTES).unwrap();
    let info = identity(&pin);
    const COUNT: u32 = 10_000;
    for mode in 0..4 {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..COUNT {
                let opened = match mode {
                    0 => Object::open(&path, FILE_READ_ATTRIBUTES).unwrap(),
                    1 => by_id(&volume, &info, FILE_READ_ATTRIBUTES).unwrap(),
                    2 => Object::reopen(pin.raw(), FILE_READ_ATTRIBUTES).unwrap(),
                    _ => {
                        let mut duplicate = ptr::null_mut();
                        assert_ne!(
                            unsafe {
                                DuplicateHandle(
                                    GetCurrentProcess(),
                                    pin.raw(),
                                    GetCurrentProcess(),
                                    &mut duplicate,
                                    0,
                                    0,
                                    DUPLICATE_SAME_ACCESS,
                                )
                            },
                            0
                        );
                        Object::owned(duplicate).unwrap()
                    }
                };
                std::hint::black_box(opened);
            }
            samples.push(start.elapsed().as_nanos() / u128::from(COUNT));
        }
        eprintln!("open mode={mode} ns/open+close samples={samples:?}");
    }
}
