//! Fetch locked Debian archives and read only manifest-selected regular files.
use super::{Entry, Payload, Result, Staging, digest, exclusive_lock, reject_redirect};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Archive {
    url: String,
    sha256: String,
    size: u64,
}

type Contents = BTreeMap<(String, String), Vec<u8>>;

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn validate(archive: &Archive) -> Result<()> {
    if !valid_hash(&archive.sha256) || archive.size == 0 || archive.size > 512 * 1024 * 1024 {
        return Err("invalid Debian archive size or SHA-256".into());
    }
    super::download::validate_url(&archive.url)?;
    Ok(())
}

fn fetch(
    name: &str,
    archive: &Archive,
    url: &str,
    cache: &Path,
    client: &super::download::Client,
) -> Result<PathBuf> {
    let path = cache.join(format!("{}.deb", archive.sha256));
    let _lock = exclusive_lock(&cache.join(format!("{}.lock", archive.sha256)))?;
    reject_redirect(&path)?;
    if path.is_file() {
        let bytes = fs::read(&path)?;
        if bytes.len() as u64 == archive.size && digest(&bytes) == archive.sha256 {
            eprintln!("Using verified cached package: {name}");
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }
    let mut failure = String::new();
    for attempt in 0..3 {
        let result: Result<Vec<u8>> = (|| {
            eprintln!(
                "Downloading {name}: 0 / {} bytes (0%; attempt {}/3)",
                archive.size,
                attempt + 1
            );
            let mut last_progress = Instant::now();
            let bytes = client.read(url, archive.size + 1, |received| {
                if received >= archive.size || last_progress.elapsed() >= Duration::from_secs(1) {
                    eprintln!(
                        "Downloading {name}: {received} / {} bytes ({}%)",
                        archive.size,
                        (received * 100 / archive.size).min(100)
                    );
                    last_progress = Instant::now();
                }
            })?;
            if bytes.len() as u64 != archive.size || digest(&bytes) != archive.sha256 {
                return Err(format!(
                    "Debian archive hash/size mismatch: {url}; received {} of {} bytes, SHA-256 {} (expected {})",
                    bytes.len(), archive.size, digest(&bytes), archive.sha256
                ).into());
            }
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => {
                let staging = Staging::new(cache)?;
                let temporary = staging.0.join("download.deb");
                fs::write(&temporary, bytes)?;
                fs::rename(temporary, &path)?;
                return Ok(path);
            }
            Err(error) => {
                failure = error.to_string();
                eprintln!(
                    "Download attempt {} failed: {}: {failure}",
                    attempt + 1,
                    url
                );
            }
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_secs(attempt + 1));
        }
    }
    Err(format!("cannot download {url}: {failure}; start again to retry").into())
}

// Bound decompression independently from the compressed archive size.
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > 512 * 1024 * 1024 {
            return Err(io::Error::other("Debian payload exceeds 512 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn deb_payload(bytes: &[u8]) -> Result<Vec<u8>> {
    if !bytes.starts_with(b"!<arch>\n") {
        return Err("invalid Debian ar archive".into());
    }
    let mut position = 8;
    while position < bytes.len() {
        let header = bytes
            .get(position..position + 60)
            .ok_or("truncated Debian ar header")?;
        if &header[58..] != b"`\n" {
            return Err("invalid Debian ar header".into());
        }
        let name = std::str::from_utf8(&header[..16])?
            .trim()
            .trim_end_matches('/');
        let size: usize = std::str::from_utf8(&header[48..58])?.trim().parse()?;
        position += 60;
        let end = position
            .checked_add(size)
            .ok_or("Debian ar size overflow")?;
        let data = bytes
            .get(position..end)
            .ok_or("truncated Debian ar member")?;
        let mut output = Bounded(Vec::new());
        match name {
            "data.tar.xz" => {
                lzma_rs::xz_decompress(&mut Cursor::new(data), &mut output)?;
            }
            "data.tar.gz" => {
                io::copy(&mut flate2::read::GzDecoder::new(data), &mut output)?;
            }
            "data.tar" => {
                output.write_all(data)?;
            }
            _ => {
                position = end + size % 2;
                continue;
            }
        }
        return Ok(output.0);
    }
    Err("Debian archive has no supported data.tar payload".into())
}

pub(super) fn load(
    archives: &BTreeMap<String, Archive>,
    entries: &[Entry],
    parent: &Path,
    settings: &super::download::Settings,
) -> Result<Contents> {
    let mut requested: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for entry in entries {
        if let Payload::Archive {
            archive,
            member,
            sha256,
        } = &entry.payload
        {
            if !valid_hash(sha256) || !archives.contains_key(archive) {
                return Err(format!("invalid Debian archive reference: {archive}").into());
            }
            requested.entry(archive).or_default().insert(member);
        }
    }
    for archive in archives.values() {
        validate(archive)?;
    }
    if requested.is_empty() {
        return Ok(BTreeMap::new());
    }
    let settings = settings.resolve()?;
    let requests = requested
        .into_iter()
        .map(|(name, members)| Ok((name, members, settings.url(&archives[name].url)?)))
        .collect::<Result<Vec<_>>>()?;
    let cache = std::env::var_os("KINAKAZE_ROOTFS_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| parent.join(".kinakaze-downloads"));
    reject_redirect(&cache)?;
    fs::create_dir_all(&cache)?;
    let count = requests.len();
    let completed = Mutex::new(0usize);
    let client = super::download::Client::new(&settings.proxy)?;
    eprintln!("Preparing {count} Debian packages; verified downloads are reused.");
    let contents = super::parallel(&requests, |(name, members, url)| {
        eprintln!("Preparing Debian package {name}");
        let path = fetch(name, &archives[*name], url, &cache, &client)?;
        eprintln!("Extracting Debian package {name}...");
        let bytes = fs::read(path)?;
        let payload = deb_payload(&bytes)?;
        let mut output = BTreeMap::new();
        let mut tar = tar::Archive::new(Cursor::new(payload));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let member = String::from_utf8(entry.path_bytes().to_vec())?;
            if members.contains(member.as_str()) {
                if !entry.header().entry_type().is_file() {
                    return Err(
                        format!("Debian payload is not a regular file: {name}: {member}").into(),
                    );
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                if output.insert(((*name).to_owned(), member), bytes).is_some() {
                    return Err("duplicate Debian archive member".into());
                }
            }
        }
        if output.len() != members.len() {
            return Err(format!("missing Debian files in {name}").into());
        }
        let mut completed = completed.lock().unwrap();
        *completed += 1;
        eprintln!("Packages ready: {completed}/{count} ({name})");
        Ok(output)
    })?;
    Ok(contents.into_iter().flatten().collect())
}
