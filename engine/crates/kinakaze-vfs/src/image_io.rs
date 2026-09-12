//! Host file access policy for guest executable images.
//!
//! A Linux reader does not lock an inode against another reader, a writer, or
//! unlink/rename. Windows share flags are therefore part of executable loading
//! correctness rather than a tuning knob. A sharing violation is returned at
//! the operation boundary: publishers use Linux-style rename/unlink instead of
//! making a reader guess when an in-place replacement might become available.

#[cfg(not(windows))]
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// An executable reader must obey the same integrity and logical-EOF boundary
/// as guest read(2). Keeping the native file private prevents magic probes or
/// dependency loading from accidentally exposing the hidden Merkle tail.
#[derive(Debug)]
pub struct GuestImage {
    #[cfg(windows)]
    file: crate::fs::object::Object,
    // Even shared length queries can issue cancellable EA I/O on this handle.
    // Keep the reader Send but not Sync so its native requests stay serialized.
    #[cfg(windows)]
    _private_io: std::marker::PhantomData<std::cell::Cell<()>>,
    #[cfg(not(windows))]
    file: File,
    offset: u64,
}

impl GuestImage {
    /// Snapshot this already-open inode through the verified logical reader.
    pub fn snapshot(mut self) -> io::Result<kinakaze_runtime::immutable::ImmutableBytes> {
        let len = usize::try_from(self.logical_length()?)
            .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "image too large"))?;
        self.seek(SeekFrom::Start(0))?;
        kinakaze_runtime::immutable::ImmutableBytes::initialize_executable(len, |bytes| {
            self.read_exact(bytes)
        })
    }

    pub fn logical_length(&self) -> io::Result<u64> {
        #[cfg(windows)]
        {
            crate::fs::verity::authoritative_size_object(&self.file).map_err(integrity_error)
        }
        #[cfg(not(windows))]
        {
            self.file.metadata().map(|metadata| metadata.len())
        }
    }
}

// Linux errno is not a Windows GetLastError value. Do not give from_raw_os_error
// EIO (5), which Windows would misrepresent as ERROR_ACCESS_DENIED.
fn integrity_error(errno: i32) -> io::Error {
    let kind = if errno == crate::EINTR {
        io::ErrorKind::Interrupted
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(
        kind,
        format!("guest image I/O failed (Linux errno {errno})"),
    )
}

impl Read for GuestImage {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(windows)]
        let count = {
            crate::fs::verity::read_object(&self.file, self.offset, buffer)
                .map_err(integrity_error)?
                .ok_or_else(|| integrity_error(crate::EIO))?
        };
        #[cfg(not(windows))]
        let count = self.file.read(buffer)?;
        self.offset = self
            .offset
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "image offset overflow"))?;
        Ok(count)
    }
}

impl Seek for GuestImage {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let (base, delta) = match position {
            SeekFrom::Start(offset) => (offset, 0),
            SeekFrom::Current(delta) => (self.offset, delta),
            SeekFrom::End(delta) => (self.logical_length()?, delta),
        };
        let offset = base
            .checked_add_signed(delta)
            .filter(|&offset| offset <= i64::MAX as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid image offset"))?;
        #[cfg(not(windows))]
        self.file.seek(SeekFrom::Start(offset))?;
        self.offset = offset;
        Ok(offset)
    }
}

/// Opens a guest executable image without imposing Windows-only sharing locks.
pub fn open_guest_image(path: &Path) -> io::Result<GuestImage> {
    let file = {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use std::os::windows::io::IntoRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };

            // This handle belongs exclusively to the reader. Reuse its native
            // open for positional data and EA queries; no guest fd shares its
            // seek position or cancellation state. GENERIC_READ grants data,
            // attributes and EA access, and the native open follows symlinks.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(path)?;
            crate::fs::object::Object::owned(file.into_raw_handle()).map_err(integrity_error)?
        }

        #[cfg(not(windows))]
        {
            File::open(path)?
        }
    };
    Ok(GuestImage {
        file,
        #[cfg(windows)]
        _private_io: std::marker::PhantomData,
        offset: 0,
    })
}

/// Reads a complete guest executable image using [`open_guest_image`].
pub fn read_guest_image(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_guest_image(path)?;
    let capacity = usize::try_from(file.logical_length()?)
        .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "image too large"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve(capacity)
        .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "image too large"))?;
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Capture verified logical image bytes directly into immutable fork backing.
/// A truncated input fails rather than publishing zero-filled missing bytes.
pub fn snapshot_guest_image(
    path: &Path,
) -> io::Result<kinakaze_runtime::immutable::ImmutableBytes> {
    open_guest_image(path)?.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_image(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kinakaze-image-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    /// Explicit performance probe: no timing thresholds in correctness tests.
    #[test]
    #[ignore = "run explicitly without concurrent builds or other benchmarks"]
    fn benchmark_image_io() {
        use std::hint::black_box;
        use std::time::Instant;

        let path = temporary_image("benchmark");
        for size in [64 * 1024, 2 * 1024 * 1024, 16 * 1024 * 1024] {
            let data: Vec<u8> = (0..size).map(|i| (i * 31) as u8).collect();
            std::fs::write(&path, &data).unwrap();
            for mode in ["chunks_4k", "vec", "snapshot"] {
                let mut samples = Vec::new();
                for iteration in 0..14 {
                    let started = Instant::now();
                    match mode {
                        "chunks_4k" => {
                            let mut reader = open_guest_image(&path).unwrap();
                            let mut block = [0u8; 4096];
                            let mut total = 0;
                            loop {
                                let count = reader.read(&mut block).unwrap();
                                if count == 0 {
                                    break;
                                }
                                black_box(&block[..count]);
                                total += count;
                            }
                            assert_eq!(total, size);
                        }
                        "vec" => {
                            let bytes = read_guest_image(&path).unwrap();
                            assert_eq!(bytes.len(), size);
                            black_box(bytes);
                        }
                        _ => {
                            let bytes = snapshot_guest_image(&path).unwrap();
                            assert_eq!(bytes.as_slice().len(), size);
                            black_box(bytes);
                        }
                    }
                    if iteration >= 2 {
                        samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                    }
                }
                samples.sort_by(f64::total_cmp);
                println!(
                    "image_io mode={mode} bytes={size} median_us={:.3} min_us={:.3}",
                    (samples[5] + samples[6]) / 2.0,
                    samples[0]
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn complete_image_read_preserves_bytes() {
        let path = temporary_image("read");
        std::fs::write(&path, b"\x7fELF-test-image").unwrap();
        assert_eq!(read_guest_image(&path).unwrap(), b"\x7fELF-test-image");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn snapshot_retains_open_inode_bytes_after_replacement_and_unlink() {
        let path = temporary_image("snapshot");
        let moved = path.with_extension("old");
        let original = b"\x7fELF-original-snapshot";
        std::fs::write(&path, original).unwrap();
        let mut open = open_guest_image(&path).unwrap();
        let mut magic = [0; 4];
        open.read_exact(&mut magic).unwrap();
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"new contents").unwrap();
        let snapshot = open.snapshot().unwrap();
        std::fs::remove_file(&moved).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(snapshot.as_slice(), original);
    }

    #[cfg(windows)]
    #[test]
    fn open_image_allows_linux_style_rename_while_readable() {
        let path = temporary_image("share-delete");
        let renamed = path.with_extension("renamed");
        std::fs::write(&path, b"\x7fELF").unwrap();
        let image = open_guest_image(&path).unwrap();
        std::fs::rename(&path, &renamed).unwrap();
        drop(image);
        std::fs::remove_file(renamed).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn sharing_violation_is_reported_without_timing_guesswork() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

        let path = temporary_image("sharing-error");
        std::fs::write(&path, b"\x7fELF").unwrap();
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let error = open_guest_image(&path).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        drop(blocker);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn already_open_image_observes_verity_and_never_reads_the_hidden_tail() {
        let path = temporary_image("verity-eof");
        let data: Vec<u8> = (0..16_031).map(|i| (i * 31) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut image = open_guest_image(&path).unwrap();
        let fd = crate::fs::open(&crate::to_guest_path(&path), crate::fs::O_RDONLY, 0).unwrap();
        assert_eq!(crate::write(fd, &[]), Err(crate::EBADF));
        assert_eq!(crate::write(fd, b"cannot write"), Err(crate::EBADF));
        let writer = crate::fs::open(&crate::to_guest_path(&path), crate::fs::O_WRONLY, 0).unwrap();
        assert_eq!(crate::read(writer, &mut []), Err(crate::EBADF));
        assert_eq!(crate::read(writer, &mut [0u8; 4]), Err(crate::EBADF));
        let raw = crate::get(writer).unwrap().raw as windows_sys::Win32::Foundation::HANDLE;
        // Transactional allocation/IoRing may lock a write-only inode without
        // giving that descriptor permission to read its data.
        drop(crate::fs::verity::lock(raw).unwrap());
        assert_eq!(crate::read(writer, &mut []), Err(crate::EBADF));
        crate::close(writer).unwrap();
        crate::fs::verity::enable(fd, 1, 4096, b"image").unwrap();
        crate::close(fd).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > data.len() as u64);
        assert_eq!(image.logical_length().unwrap(), data.len() as u64);
        assert_eq!(
            image.seek(SeekFrom::End(-3)).unwrap(),
            data.len() as u64 - 3
        );
        let mut suffix = [0xcc; 4096];
        assert_eq!(image.read(&mut suffix).unwrap(), 3);
        assert_eq!(&suffix[..3], &data[data.len() - 3..]);
        assert!(suffix[3..].iter().all(|&v| v == 0xcc));
        assert_eq!(image.read(&mut suffix).unwrap(), 0);
        assert_eq!(read_guest_image(&path).unwrap(), data);
        assert_eq!(snapshot_guest_image(&path).unwrap().as_slice(), data);
        drop(image);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn image_header_probe_and_full_load_reject_corrupted_data() {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        let path = temporary_image("verity-corrupt");
        std::fs::write(&path, b"\x7fELF-authenticated-image").unwrap();
        let mut image = open_guest_image(&path).unwrap();
        let fd = crate::fs::open(&crate::to_guest_path(&path), crate::fs::O_RDONLY, 0).unwrap();
        crate::fs::verity::enable(fd, 1, 4096, &[]).unwrap();
        crate::close(fd).unwrap();
        // Simulate backing-storage corruption outside the guest write policy.
        let mut native = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&path)
            .unwrap();
        native.write_all(b"evil").unwrap();
        native.sync_all().unwrap();
        drop(native);
        let mut magic = [0xcc; 4];
        assert!(
            image
                .read(&mut magic)
                .unwrap_err()
                .to_string()
                .contains("Linux errno 5")
        );
        assert_eq!(magic, [0xcc; 4]);
        assert_eq!(image.stream_position().unwrap(), 0);
        assert!(read_guest_image(&path).is_err());
        assert!(snapshot_guest_image(&path).is_err());
        drop(image);
        std::fs::remove_file(path).unwrap();
    }
}
