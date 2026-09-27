use kinakaze_v2_rootfs::prepare;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-download-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn manifest(&self, url: &str, deb: &[u8], file_hash: &str, member: &str) {
        fs::write(self.0.join("rootfs.manifest.json"), serde_json::to_vec(&json!({
            "schema": 1,
            "directories": ["etc"],
            "archives": {"probe": {"url": url, "size": deb.len(), "sha256": hash(deb)}},
            "files": [{"path": "etc/probe", "archive": "probe", "member": member, "sha256": file_hash}],
            "permissions": {"etc/probe": 420}
        })).unwrap()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn deb(content: &[u8], symlink: bool) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    if symlink {
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_link_name("/outside").unwrap();
    } else {
        header.set_size(content.len() as u64);
    }
    header.set_cksum();
    tar.append_data(
        &mut header,
        "./etc/probe",
        if symlink { &[][..] } else { content },
    )
    .unwrap();
    let tar = tar.into_inner().unwrap();
    let mut compressed = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(tar), &mut compressed).unwrap();
    let mut bytes = b"!<arch>\n".to_vec();
    bytes.extend_from_slice(
        format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "data.tar.xz",
            0,
            0,
            0,
            "100644",
            compressed.len()
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(&compressed);
    if compressed.len() % 2 != 0 {
        bytes.push(b'\n');
    }
    bytes
}

fn server(bytes: Vec<u8>, requests: usize) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/probe.deb", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            assert!(size > 0);
            paths.push(
                String::from_utf8_lossy(&request[..size])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        }
        paths
    });
    (url, handle)
}

#[test]
fn manifest_mirrors_download_both_repositories_directly_and_verify_payloads() {
    for (repository, setting) in [
        ("debian", "debian_mirror"),
        ("debian-security", "security_mirror"),
    ] {
        let fixture = Fixture::new();
        let content = b"mirror payload";
        let deb = deb(content, false);
        let (url, server) = server(deb.clone(), 1);
        fixture.manifest(
            &format!("https://deb.debian.org/{repository}/pool/probe.deb"),
            &deb,
            &hash(content),
            "etc/probe",
        );
        let path = fixture.0.join("rootfs.manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["download"] =
            json!({"proxy": "direct", setting: url.trim_end_matches("/probe.deb")});
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let root = fixture.0.join("root");
        prepare(&root, &fixture.0, None).unwrap();
        assert_eq!(fs::read(root.join("etc/probe")).unwrap(), content);
        assert_eq!(server.join().unwrap(), ["GET /pool/probe.deb HTTP/1.1"]);
    }
}

#[test]
fn download_failure_does_not_publish_and_retry_reuses_verified_archive_without_network() {
    let fixture = Fixture::new();
    let content = b"downloaded Debian file\n";
    let deb = deb(content, false);
    let (url, server) = server(deb.clone(), 1);
    // tar normalizes its header path; this is the exact member the manifest selects.
    fixture.manifest(&url, &deb, &hash(b"wrong file"), "etc/probe");
    let root = fixture.0.join("root");
    assert!(prepare(&root, &fixture.0, None).is_err());
    server.join().unwrap();
    assert!(!root.exists());
    fixture.manifest(&url, &deb, &hash(content), "etc/probe");
    prepare(&root, &fixture.0, None).unwrap();
    assert_eq!(fs::read(root.join("etc/probe")).unwrap(), content);
    assert!(root.join(".kinakaze-rootfs.sha256").is_file());
    fs::write(root.join("etc/probe"), b"user configuration").unwrap();
    fs::write(fixture.0.join("rootfs.manifest.json"), b"invalid manifest").unwrap();
    prepare(&root, &fixture.0, None).unwrap();
    assert_eq!(
        fs::read(root.join("etc/probe")).unwrap(),
        b"user configuration"
    );
}

#[test]
fn corrupted_archive_is_rejected_before_installation() {
    let fixture = Fixture::new();
    let deb = deb(b"expected", false);
    let (url, server) = server(vec![0; deb.len()], 3);
    fixture.manifest(&url, &deb, &hash(b"expected"), "etc/probe");
    assert!(prepare(&fixture.0.join("root"), &fixture.0, None).is_err());
    server.join().unwrap();
    assert!(!fixture.0.join("root").exists());
    assert!(
        !fixture
            .0
            .join(format!(".kinakaze-downloads/{}.deb", hash(&deb)))
            .exists()
    );
}

#[test]
fn missing_members_and_archive_symlinks_cannot_be_installed_as_files() {
    for (symlink, member) in [(false, "etc/missing"), (true, "etc/probe")] {
        let fixture = Fixture::new();
        let deb = deb(b"content", symlink);
        let (url, server) = server(deb.clone(), 1);
        fixture.manifest(&url, &deb, &hash(b"content"), member);
        assert!(prepare(&fixture.0.join("root"), &fixture.0, None).is_err());
        server.join().unwrap();
        assert!(!fixture.0.join("root").exists());
    }
}
