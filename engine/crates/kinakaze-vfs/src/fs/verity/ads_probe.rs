//! Explicit native capability probes, not a production metadata fallback.
//! These never infer inode identity from a saved pathname after opening it.

use super::*;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, ExtendedFileIdType, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OVERLAPPED, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_ID_INFO,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfoEx, FileIdInfo,
    GetFileInformationByHandleEx, OpenFileById, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const STREAM: &str = ":kinakaze-verity-lifetime-probe";
const PAYLOAD: &[u8] = b"the original inode's named stream";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-verity-ads-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self) -> PathBuf {
        let path = self.0.join("original");
        std::fs::write(&path, b"ordinary unnamed data").unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // The fixture created this unique directory and owns all its contents.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity(object: &Object) -> FILE_ID_INFO {
    let mut id = unsafe { zeroed::<FILE_ID_INFO>() };
    assert_ne!(
        unsafe {
            GetFileInformationByHandleEx(
                object.raw(),
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        },
        0,
        "file identity: {}",
        unsafe { GetLastError() }
    );
    id
}

fn delete_name(path: &Path) {
    let deletion = Object::open(path, DELETE).unwrap();
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    assert_ne!(
        unsafe {
            SetFileInformationByHandle(
                deletion.raw(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        },
        0,
        "POSIX unlink: {}",
        unsafe { GetLastError() }
    );
}

fn stream_result(label: &str, result: Result<Object, i32>, expected_id: &FILE_ID_INFO) {
    eprintln!("{label}: {result:?}");
    if let Ok(stream) = result {
        assert_eq!(
            identity(&stream).FileId.Identifier,
            expected_id.FileId.Identifier
        );
        let mut bytes = [0; 100];
        let count = stream.read_at(0, &mut bytes).unwrap();
        assert_eq!(
            &bytes[..count],
            PAYLOAD,
            "{label} must not reopen replacement/default data"
        );
    }
}

#[test]
#[ignore = "native ADS lifetime capability probe; run explicitly without service tests"]
fn native_ads_lifecycle_probe() {
    for retain_stream in [false, true] {
        for retain_link in [false, true] {
            let fixture = Fixture::new();
            let original = fixture.file();
            let base = Object::open(&original, GENERIC_READ | QUERY_ACCESS).unwrap();
            let id = identity(&base);
            let stream = unsafe {
                Object::create_stream(base.raw(), OsStr::new(STREAM), GENERIC_READ | GENERIC_WRITE)
            }
            .unwrap();
            exact_write(&stream, 0, PAYLOAD).unwrap();
            stream.flush().unwrap();
            let mut retained = Some(stream);
            if !retain_stream {
                retained.take();
            }
            let renamed = fixture.0.join("renamed");
            std::fs::rename(&original, &renamed).unwrap();
            std::fs::write(&original, b"replacement inode").unwrap();
            stream_result(
                "after rename",
                unsafe { Object::stream(base.raw(), OsStr::new(STREAM), GENERIC_READ) },
                &id,
            );
            if retain_link {
                std::fs::hard_link(&renamed, fixture.0.join("remaining-link")).unwrap();
            }
            delete_name(&renamed);
            assert!(!renamed.exists());
            std::fs::write(&renamed, b"replacement of removed name").unwrap();
            eprintln!("CASE retained_stream={retain_stream} retained_link={retain_link}");
            stream_result(
                "late ADS via original base",
                unsafe { Object::stream(base.raw(), OsStr::new(STREAM), GENERIC_READ) },
                &id,
            );
            let reopen = Object::reopen(base.raw(), GENERIC_READ | QUERY_ACCESS).unwrap();
            assert_eq!(identity(&reopen).FileId.Identifier, id.FileId.Identifier);
            stream_result(
                "late ADS via empty-name reopened base",
                unsafe { Object::stream(reopen.raw(), OsStr::new(STREAM), GENERIC_READ) },
                &id,
            );
            let volume_hint = Object::open(&fixture.0, FILE_READ_ATTRIBUTES).unwrap();
            let descriptor = FILE_ID_DESCRIPTOR {
                dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
                Type: ExtendedFileIdType,
                Anonymous: FILE_ID_DESCRIPTOR_0 {
                    ExtendedFileId: id.FileId,
                },
            };
            let by_id = unsafe {
                OpenFileById(
                    volume_hint.raw(),
                    &descriptor,
                    GENERIC_READ | SYNCHRONIZE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                )
            };
            if by_id != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE && !by_id.is_null() {
                stream_result(
                    "late ADS via file-ID reopened base",
                    unsafe { Object::stream(by_id, OsStr::new(STREAM), GENERIC_READ) },
                    &id,
                );
                unsafe { CloseHandle(by_id) };
            } else {
                eprintln!("by-ID base open win32={}", unsafe { GetLastError() });
            }
            if let Some(stream) = retained.as_ref() {
                let mut data = [0; 100];
                let count = stream.read_at(0, &mut data).unwrap();
                assert_eq!(&data[..count], PAYLOAD);
                eprintln!("retained ADS direct read: {count} bytes");
                stream_result(
                    "empty-name reopen of retained ADS",
                    Object::reopen(stream.raw(), GENERIC_READ),
                    &id,
                );
            }
            let new_stream = unsafe {
                Object::create_stream(
                    base.raw(),
                    OsStr::new(":kinakaze-late-created"),
                    GENERIC_WRITE,
                )
            };
            eprintln!("create new ADS through retained base: {new_stream:?}");
            assert_eq!(std::fs::read(&original).unwrap(), b"replacement inode");
            assert_eq!(
                std::fs::read(&renamed).unwrap(),
                b"replacement of removed name"
            );
            assert_eq!(
                native_size(&base).unwrap(),
                b"ordinary unnamed data".len() as u64
            );
        }
    }
}

#[test]
fn ads_transferred_handle_helper() {
    if std::env::var_os("KINAKAZE_VERITY_ADS_RECEIVER").is_none() {
        return;
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let raw = input.trim().parse::<usize>().unwrap() as HANDLE;
    let stream = Object::reopen(raw, GENERIC_READ).unwrap();
    unsafe { CloseHandle(raw) };
    let mut data = [0; 100];
    let count = stream.read_at(0, &mut data).unwrap();
    assert_eq!(&data[..count], PAYLOAD);
    println!("TRANSFERRED_ADS_OK {count}");
}

#[test]
#[ignore = "native ADS cross-process capability probe; run explicitly without service tests"]
fn native_ads_transferred_handle_survives_unlink_and_sender_close() {
    let fixture = Fixture::new();
    let path = fixture.file();
    let base = Object::open(&path, GENERIC_READ | QUERY_ACCESS).unwrap();
    let stream = unsafe {
        Object::create_stream(base.raw(), OsStr::new(STREAM), GENERIC_READ | GENERIC_WRITE)
    }
    .unwrap();
    exact_write(&stream, 0, PAYLOAD).unwrap();
    stream.flush().unwrap();
    delete_name(&path);
    std::fs::write(&path, b"replacement inode").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "fs::verity::ads_probe::ads_transferred_handle_helper",
            "--nocapture",
        ])
        .env("KINAKAZE_VERITY_ADS_RECEIVER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000);
    let mut child = crate::with_exec_handle_filter(|| command.spawn())
        .unwrap()
        .unwrap();
    let mut remote = std::ptr::null_mut();
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                stream.raw(),
                child.as_raw_handle(),
                &mut remote,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        },
        0,
        "duplicate ADS: {}",
        unsafe { GetLastError() }
    );
    drop(stream);
    drop(base);
    writeln!(child.stdin.take().unwrap(), "{}", remote as usize).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            panic!(
                "ADS receiver exceeded bounded deadline: {:?}",
                child.wait_with_output()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("TRANSFERRED_ADS_OK"), "{output}");
    eprintln!("{output}");
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement inode");
}
