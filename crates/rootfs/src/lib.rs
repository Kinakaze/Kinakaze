//! First-run configuration from a distribution-owned, versioned data manifest.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
const STATE: &str = ".kinakaze-rootfs.sha256";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    directories: Vec<String>,
    files: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Payload {
    Text { content: String },
    Copy { source: String, sha256: String },
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn relative(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains(['\\', ':', '\0'])
        || value.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part.to_ascii_lowercase().starts_with(".kinakaze-")
                || part.chars().any(|c| c < ' ' || "<>\"|?*".contains(c))
                || matches!(
                    part.split('.')
                        .next()
                        .unwrap()
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
    {
        return Err(format!("invalid rootfs manifest path: {value}").into());
    }
    Ok(())
}

fn reject_redirect(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(
                        format!("rootfs path is a reparse point: {}", path.display()).into(),
                    );
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(format!("rootfs path is a symlink: {}", path.display()).into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn checked_path(base: &Path, value: &str) -> Result<PathBuf> {
    relative(value)?;
    let mut path = base.to_owned();
    for part in value.split('/') {
        path.push(part);
        reject_redirect(&path)?;
    }
    Ok(path)
}

fn exclusive_lock(path: &Path) -> Result<File> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        reject_redirect(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        match options.open(path) {
            Ok(file) => return Ok(file),
            Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

struct Staging(PathBuf);
impl Staging {
    fn new(parent: &Path) -> Result<Self> {
        for attempt in 0..1000 {
            let path = parent.join(format!(
                ".kinakaze-rootfs-stage-{}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err("cannot reserve rootfs staging directory".into())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        // This exact sibling directory was create_dir-owned by this invocation;
        // manifest paths cannot escape it or install links/reparse points.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn nonempty(root: &Path) -> Result<bool> {
    match fs::read_dir(root) {
        Ok(mut entries) => Ok(entries.next().transpose()?.is_some()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Missing default manifests preserve compatibility with developer roots.
/// External manifests take precedence. A nonempty root is never initialized,
/// even when a supplied manifest is missing, invalid, or changed.
pub fn prepare(root: &Path, dist: &Path, explicit: Option<&Path>) -> Result<()> {
    if nonempty(root)? {
        return Ok(());
    }
    let manifest_path = explicit
        .map(Path::to_owned)
        .unwrap_or_else(|| dist.join("rootfs.manifest.json"));
    if explicit.is_none() && !manifest_path.exists() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    File::open(&manifest_path)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("rootfs manifest exceeds 4 MiB".into());
    }
    let manifest_hash = digest(&bytes);
    let absolute = std::path::absolute(root)?;
    let parent = absolute.parent().ok_or("rootfs has no parent directory")?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let name = absolute.file_name().ok_or("rootfs has no directory name")?;
    let destination = parent.join(name);
    reject_redirect(&destination)?;
    // The lock lives outside rootfs so an empty destination remains empty on failure.
    let lock_name = format!(
        ".kinakaze-rootfs-{}.lock",
        digest(name.to_string_lossy().to_lowercase().as_bytes())
    );
    let _lock = exclusive_lock(&parent.join(lock_name))?;
    if nonempty(&destination)? {
        return Ok(());
    }
    let staging = Staging::new(&parent)?;
    let root = &staging.0;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != 1 {
        return Err("unsupported rootfs manifest schema".into());
    }
    let source_base = manifest_path.canonicalize()?.parent().unwrap().to_owned();
    let mut paths = BTreeSet::new();
    let mut directories = Vec::new();
    let mut files = Vec::new();
    // Validate the complete plan and all source hashes before installing files.
    for name in &manifest.directories {
        let path = checked_path(root, name)?;
        if !paths.insert(name.to_ascii_lowercase()) || path.exists() && !path.is_dir() {
            return Err(format!("duplicate or conflicting rootfs directory: {name}").into());
        }
        directories.push(path);
    }
    for entry in &manifest.files {
        let path = checked_path(root, &entry.path)?;
        if !paths.insert(entry.path.to_ascii_lowercase()) || path.exists() && !path.is_file() {
            return Err(format!("duplicate or conflicting rootfs file: {}", entry.path).into());
        }
        let content = match &entry.payload {
            Payload::Text { content } => content.as_bytes().to_vec(),
            Payload::Copy { source, sha256 } => {
                let source = checked_path(&source_base, source)?;
                let bytes = fs::read(&source)?;
                if digest(&bytes) != *sha256 {
                    return Err(format!("rootfs source hash mismatch: {}", source.display()).into());
                }
                bytes
            }
        };
        files.push((path, content));
    }
    for entry in &manifest.files {
        let prefix = format!("{}/", entry.path.to_ascii_lowercase());
        if paths.iter().any(|path| path.starts_with(&prefix)) {
            return Err(format!("rootfs file is also a parent directory: {}", entry.path).into());
        }
    }
    for directory in directories {
        fs::create_dir_all(directory)?;
    }
    for (path, content) in files {
        fs::create_dir_all(path.parent().unwrap())?;
        // Every staged file is new. Let the filesystem reject aliases that its
        // case rules consider identical even beyond ASCII preflight checks.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&content)?;
        file.sync_all()?;
    }
    fs::write(root.join(STATE), manifest_hash.as_bytes())?;
    // Both paths are verified children of the caller-selected canonical parent.
    // Only an empty destination can be removed; never merge into an existing root.
    if nonempty(&destination)? {
        return Ok(());
    }
    match fs::remove_dir(&destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::rename(root, &destination)?;
    Ok(())
}
