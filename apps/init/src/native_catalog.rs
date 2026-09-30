//! Session-owned validation. Existing images stay pinned; additions rescan.
use kinakaze_v2_bridge::native::{self, CatalogSnapshot, ModuleCatalog};
use kinakaze_v2_host_win::{ProcessHandle, RemoteTransfer, StartupSpan};
use kinakaze_v2_manager::PeerIdentity;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

struct Entry {
    directory: PathBuf,
    _pin: File,
    names: Vec<String>,
    snapshot: CatalogSnapshot,
}
#[derive(Default)]
pub(super) struct Cache {
    entry: Option<Entry>,
}

fn names(directory: &Path) -> io::Result<Option<Vec<String>>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !native::is_shared_object(name) {
            continue;
        }
        let kind = entry.file_type()?;
        // A link itself could be replaced while its target is pinned. Let the
        // ordinary fresh discovery retain that distribution's existing rules.
        if kind.is_symlink() {
            return Ok(None);
        }
        if kind.is_file() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(Some(names))
}
impl Cache {
    pub fn get(
        &mut self,
        dist: &Path,
        peer: PeerIdentity,
        requested: &str,
    ) -> io::Result<Option<(usize, RemoteTransfer)>> {
        // Never let a guest make init pin arbitrary host paths. Only this
        // session's configured native distribution can use the optimization.
        let directory = native::directory(dist).canonicalize()?;
        if directory != Path::new(requested) {
            return Ok(None);
        }
        let Some(observed) = names(&directory)? else {
            return Ok(None);
        };
        if !self
            .entry
            .as_ref()
            .is_some_and(|entry| entry.directory == directory && entry.names == observed)
        {
            let _miss = StartupSpan::begin("native-catalog-miss");
            let pin = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&directory)?;
            let catalog = ModuleCatalog::discover(&directory).map_err(io::Error::other)?;
            let snapshot = CatalogSnapshot::new(&directory, catalog).map_err(io::Error::other)?;
            // Mutable ordinary ELF libraries cannot share this immutable native
            // proof. A mixed directory continues to use ordinary discovery.
            if snapshot.image_count() != observed.len() {
                return Ok(None);
            }
            if names(&directory)?.as_ref() != Some(&observed) {
                return Ok(None);
            }
            self.entry = Some(Entry {
                directory,
                _pin: pin,
                names: observed,
                snapshot,
            });
        } else {
            let _hit = StartupSpan::begin("native-catalog-hit");
        }
        let process = ProcessHandle::open(peer.host_pid)?;
        if process.birth() != peer.birth {
            return Err(io::Error::other("native catalog peer identity mismatch"));
        }
        let entry = self.entry.as_ref().unwrap();
        let mut transfer = RemoteTransfer::new(process);
        entry
            .snapshot
            .transfer(&mut transfer)
            .map_err(io::Error::other)?;
        Ok(Some((entry.snapshot.length(), transfer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "kinakaze-init-catalog-{}-{}",
                std::process::id(),
                kinakaze_v2_host_win::random_token().unwrap()
            ));
            fs::create_dir_all(root.join("native")).unwrap();
            fs::write(
                root.join("native/libruntime.so"),
                image("kinakaze_runtime_open_v1"),
            )
            .unwrap();
            Self(root)
        }
        fn directory(&self) -> PathBuf {
            self.0.join("native").canonicalize().unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn image(symbol: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x1200];
        let w16 = |b: &mut [u8], p: usize, v: u16| b[p..p + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |b: &mut [u8], p: usize, v: u32| b[p..p + 4].copy_from_slice(&v.to_le_bytes());
        bytes[..2].copy_from_slice(b"MZ");
        w32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        w16(&mut bytes, 0x84, 0x8664);
        w16(&mut bytes, 0x86, 1);
        w16(&mut bytes, 0x94, 240);
        w16(&mut bytes, 0x96, 0x2000);
        w16(&mut bytes, 0x98, 0x20b);
        w32(&mut bytes, 0x98 + 108, 1);
        w32(&mut bytes, 0x98 + 112, 0x1000);
        w32(&mut bytes, 0x98 + 116, 0x800);
        for (at, value) in [
            (8, 0x1000),
            (12, 0x1000),
            (16, 0x1000),
            (20, 0x200),
            (36, 0x6000_0020),
        ] {
            w32(&mut bytes, 0x188 + at, value);
        }
        for (at, value) in [
            (12, 0x1200),
            (20, 1),
            (24, 1),
            (28, 0x1040),
            (32, 0x1080),
            (36, 0x10c0),
        ] {
            w32(&mut bytes, 0x200 + at, value);
        }
        w32(&mut bytes, 0x240, 0x1900);
        w32(&mut bytes, 0x280, 0x1280);
        bytes[0x400..0x40d].copy_from_slice(b"libfixture.so");
        bytes[0x480..0x480 + symbol.len()].copy_from_slice(symbol.as_bytes());
        bytes
    }
    fn peer() -> PeerIdentity {
        let process = ProcessHandle::open(std::process::id()).unwrap();
        PeerIdentity {
            host_pid: process.pid(),
            birth: process.birth(),
        }
    }
    fn receive(length: usize, transfer: RemoteTransfer, directory: &Path) -> ModuleCatalog {
        let handles = transfer.handles().to_vec();
        transfer.commit();
        let view = unsafe {
            kinakaze_v2_host_win::ReadOnlySectionView::adopt(&handles, length, 2 * 1024 * 1024)
        }
        .unwrap();
        unsafe { ModuleCatalog::from_snapshot(directory, view) }.unwrap()
    }
    #[test]
    fn catalog_cache_tracks_additions_and_preserves_earlier_pins() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        let mut cache = Cache::default();
        assert!(
            cache
                .get(&fixture.0, peer(), "C:\\unrelated")
                .unwrap()
                .is_none()
        );
        let (length, transfer) = cache
            .get(&fixture.0, peer(), directory.to_str().unwrap())
            .unwrap()
            .unwrap();
        let first = receive(length, transfer, &directory);
        assert!(first.shared_libraries.is_empty());
        fs::write(directory.join("libshared.so"), image("_Rinternal")).unwrap();
        let (length, transfer) = cache
            .get(&fixture.0, peer(), directory.to_str().unwrap())
            .unwrap()
            .unwrap();
        let second = receive(length, transfer, &directory);
        assert_eq!(second.shared_libraries, ["libshared.so"]);
        assert!(first.shared_libraries.is_empty());
        assert!(fs::write(directory.join("libruntime.so"), b"changed").is_err());
        let (length, transfer) = cache
            .get(&fixture.0, peer(), directory.to_str().unwrap())
            .unwrap()
            .unwrap();
        drop(receive(length, transfer, &directory));
        // A newly added malformed image must force discovery and fail.
        fs::write(directory.join("libbroken.so"), b"MZ malformed").unwrap();
        assert!(
            cache
                .get(&fixture.0, peer(), directory.to_str().unwrap())
                .is_err()
        );
        drop(cache);
        drop(second);
        assert!(fs::write(directory.join("libruntime.so"), b"changed").is_err());
        drop(first);
        assert!(fs::write(directory.join("libruntime.so"), b"changed").is_ok());
    }
    #[test]
    fn mixed_elf_directory_falls_back_and_stale_peer_cannot_receive_handles() {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        let mut cache = Cache::default();
        let mut stale = peer();
        stale.birth += 1;
        assert!(
            cache
                .get(&fixture.0, stale, directory.to_str().unwrap())
                .is_err()
        );
        fs::write(
            directory.join("libguest.so"),
            b"\x7fELF writable guest library",
        )
        .unwrap();
        assert!(
            cache
                .get(&fixture.0, peer(), directory.to_str().unwrap())
                .unwrap()
                .is_none()
        );
        assert!(fs::write(directory.join("libguest.so"), b"\x7fELF replaced").is_ok());
    }
}
