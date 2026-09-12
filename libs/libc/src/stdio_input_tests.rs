use super::*;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::IntoRawHandle;
use std::sync::atomic::{AtomicU64, Ordering};

struct Input {
    file: *mut File,
    path: std::path::PathBuf,
}
impl Input {
    fn new(data: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kinakaze-stdio-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, data).unwrap();
        let native = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x40000000)
            .open(&path)
            .unwrap();
        let fd = kinakaze_vfs::install(
            native.into_raw_handle() as usize,
            kinakaze_vfs::FdKind::File,
            kinakaze_vfs::FdFlags::READ_ACCESS
                .union(kinakaze_vfs::FdFlags::WRITE_ACCESS)
                .union(kinakaze_vfs::FdFlags::SEEKABLE)
                .union(kinakaze_vfs::FdFlags::OVERLAPPED),
        )
        .unwrap();
        Self {
            file: publish(Stream::new(fd, Buffering::Full)),
            path,
        }
    }
    fn position(&self) -> u64 {
        kinakaze_vfs::fs::lseek(fileno(self.file), 0, 1).unwrap()
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        unsafe {
            fclose(self.file);
        }
        std::fs::remove_file(&self.path).unwrap();
    }
}

#[test]
fn fgets_refills_in_blocks_and_preserves_every_byte() {
    let data: Vec<u8> = (0..20003)
        .map(|i| {
            if i % 137 == 136 {
                b'\n'
            } else {
                b'a' + (i % 26) as u8
            }
        })
        .collect();
    for mode in [2, 0] {
        let mut timings = Vec::new();
        let mut positions = Vec::new();
        for _ in 0..3 {
            let input = Input::new(&data);
            assert_eq!(setvbuf(input.file, ptr::null_mut(), mode, 0), 0);
            let started = std::time::Instant::now();
            let mut output = vec![fgetc(input.file) as u8];
            positions.push(input.position());
            let mut line = [0i8; 257];
            while !unsafe { fgets(line.as_mut_ptr(), line.len() as i32, input.file) }.is_null() {
                output.extend_from_slice(
                    unsafe { std::ffi::CStr::from_ptr(line.as_ptr()) }.to_bytes(),
                );
            }
            timings.push(started.elapsed().as_micros());
            assert_eq!(output, data);
            assert_eq!(ftell(input.file), data.len() as i64);
            assert_eq!(feof(input.file), 1);
            assert_eq!(ferror(input.file), 0);
        }
        eprintln!(
            "stdio_fgets mode={mode} bytes={} rounds_us={timings:?} initial_native_positions={positions:?}",
            data.len()
        );
        let expected = if mode == 0 { BUFSIZ as u64 } else { 1 };
        assert!(
            positions.iter().all(|p| *p == expected),
            "the input discipline must control read-ahead"
        );
    }
}

#[test]
fn input_seek_tell_pushback_and_failed_seek_keep_logical_position() {
    let input = Input::new(&vec![b'a'; BUFSIZ + 50]);
    for _ in 0..3 {
        assert_eq!(fgetc(input.file), b'a' as i32);
    }
    assert_eq!(ftell(input.file), 3);
    assert_eq!(ungetc(b'Z' as i32, input.file), b'Z' as i32);
    assert_eq!(ftell(input.file), 2);
    assert_eq!(fseek(input.file, 0, 999), EOF);
    assert_eq!(fgetc(input.file), b'Z' as i32);
    assert_eq!(fseek(input.file, 5, 1), 0);
    assert_eq!(ftell(input.file), 8);
    assert_eq!(input.position(), 8);
    assert_eq!(fgetc(input.file), b'a' as i32);
    assert_eq!(fseek(input.file, -1, 2), 0);
    assert_eq!(ftell(input.file), (BUFSIZ + 49) as i64);
}

#[test]
fn fflush_and_buffering_changes_synchronize_input_before_descriptor_handoff() {
    let input = Input::new(b"0123456789abcdefghijklmnopqrstuvwxyz");
    assert_eq!(fgetc(input.file), b'0' as i32);
    assert_eq!(unsafe { fflush(input.file) }, 0);
    assert_eq!(input.position(), 1);
    let mut raw = [0; 2];
    assert_eq!(kinakaze_vfs::read(fileno(input.file), &mut raw), Ok(2));
    assert_eq!(&raw, b"12");
    assert_eq!(fgetc(input.file), b'3' as i32);
    assert_eq!(setvbuf(input.file, ptr::null_mut(), 2, 0), 0);
    assert_eq!(input.position(), 4);
    assert_eq!(fgetc(input.file), b'4' as i32);
    assert_eq!(input.position(), 5);
    assert_eq!(fputc(b'X' as i32, input.file), b'X' as i32);
    assert_eq!(fseek(input.file, 5, 0), 0);
    assert_eq!(fgetc(input.file), b'X' as i32);
}

#[test]
fn nonseekable_input_keeps_read_ahead_across_flush_and_handles_eof_pushback() {
    let (reader, writer) =
        kinakaze_vfs::unix::socketpair(kinakaze_vfs::socket::SOCK_STREAM).unwrap();
    let file = publish(Stream::new(reader, Buffering::Full));
    kinakaze_vfs::write(writer, b"first\nsecond\n").unwrap();
    let mut line = [0i8; 32];
    assert!(!unsafe { fgets(line.as_mut_ptr(), 32, file) }.is_null());
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(line.as_ptr()) }.to_bytes(),
        b"first\n"
    );
    assert_eq!(unsafe { fflush(file) }, 0);
    assert_eq!(setvbuf(file, ptr::null_mut(), 2, 0), 0);
    assert!(!unsafe { fgets(line.as_mut_ptr(), 32, file) }.is_null());
    assert_eq!(
        unsafe { std::ffi::CStr::from_ptr(line.as_ptr()) }.to_bytes(),
        b"second\n"
    );
    kinakaze_vfs::unix::shutdown(writer, kinakaze_vfs::socket::SHUT_WR).unwrap();
    assert_eq!(fgetc(file), EOF);
    assert_eq!(feof(file), 1);
    assert_eq!(ungetc(b'Z' as i32, file), b'Z' as i32);
    assert_eq!(feof(file), 0);
    assert_eq!(fgetc(file), b'Z' as i32);
    assert_eq!(fgetc(file), EOF);
    unsafe {
        fclose(file);
    }
    kinakaze_vfs::close(writer).unwrap();
}
