use kinakaze_v2_rootfs::prepare;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-rootfs-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn manifest(&self, value: serde_json::Value) {
        fs::write(
            self.0.join("rootfs.manifest.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    fn run(&self) -> bool {
        match prepare(&self.0.join("root"), &self.0, None) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("{e}");
                false
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn concurrent_first_run_preserves_nonempty_roots_even_when_manifest_changes() {
    let fixture = Fixture::new();
    fixture.manifest(json!({"schema": 1, "directories": ["etc", "tmp"], "files": [{"path": "etc/hostname", "content": "initial\n"}]}));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| assert!(fixture.run()));
        }
    });
    let hostname = fixture.0.join("root/etc/hostname");
    assert_eq!(fs::read_to_string(&hostname).unwrap(), "initial\n");
    fs::write(&hostname, "custom\n").unwrap();
    fixture.manifest(json!({"schema": 1, "directories": ["etc"], "files": [{"path": "etc/hostname", "content": "replacement\n"}, {"path": "etc/new", "content": "added"}]}));
    assert!(fixture.run());
    assert_eq!(fs::read_to_string(hostname).unwrap(), "custom\n");
    assert!(!fixture.0.join("root/etc/new").exists());
}

#[test]
fn bad_hash_never_publishes_a_completion_marker_or_partial_configuration() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("seed"), b"hello").unwrap();
    let mut manifest = json!({"schema": 1, "directories": ["etc"], "files": [
        {"path": "etc/config", "content": "default"},
        {"path": "bin/program", "source": "seed", "sha256": "wrong"}
    ]});
    fixture.manifest(manifest.clone());
    assert!(!fixture.run());
    assert!(!fixture.0.join("root/etc/config").exists());
    assert!(!fixture.0.join("root/.kinakaze-rootfs.sha256").exists());
    manifest["files"][1]["sha256"] =
        json!("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    fixture.manifest(manifest);
    assert!(fixture.run());
    assert_eq!(
        fs::read(fixture.0.join("root/bin/program")).unwrap(),
        b"hello"
    );
}

#[test]
fn rejects_path_escape_unknown_fields_duplicates_and_unsupported_schema() {
    for path in [
        "../outside",
        "/absolute",
        "C:/escape",
        "etc/../outside",
        "etc\\outside",
        "etc/file:stream",
        "etc/CON",
        "etc/file.",
        ".KINAKAZE-rootfs.sha256",
    ] {
        let fixture = Fixture::new();
        fixture.manifest(
            json!({"schema": 1, "directories": [], "files": [{"path": path, "content": "bad"}]}),
        );
        assert!(!fixture.run(), "accepted {path}");
    }
    for manifest in [
        json!({"schema": 2, "directories": [], "files": []}),
        json!({"schema": 1, "directories": [], "files": [], "typo": true}),
        json!({"schema": 1, "directories": [], "files": [{"path": "etc/test", "content": "bad", "typo": true}]}),
        json!({"schema": 1, "directories": ["etc", "ETC"], "files": []}),
    ] {
        let fixture = Fixture::new();
        fixture.manifest(manifest);
        assert!(!fixture.run());
    }
}

#[test]
fn missing_explicit_manifest_is_an_error_but_old_distributions_still_work() {
    let fixture = Fixture::new();
    assert!(fixture.run());
    assert!(
        prepare(
            &fixture.0.join("root"),
            &fixture.0,
            Some(&fixture.0.join("missing"))
        )
        .is_err()
    );
}

#[test]
fn external_manifest_wins_and_an_existing_empty_directory_is_initialized() {
    let fixture = Fixture::new();
    fixture.manifest(json!({"schema": 999}));
    let external = fixture.0.join("external.json");
    fs::write(
        &external,
        br#"{"schema":1,"directories":[],"files":[{"path":"selected","content":"external"}]}"#,
    )
    .unwrap();
    let root = fixture.0.join("root");
    fs::create_dir(&root).unwrap();
    prepare(&root, &fixture.0, Some(&external)).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("selected")).unwrap(),
        "external"
    );
}

#[test]
fn any_existing_entry_skips_initialization_without_reading_the_manifest() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    fs::create_dir_all(root.join("empty-child-directory")).unwrap();
    prepare(&root, &fixture.0, Some(&fixture.0.join("missing.json"))).unwrap();
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    assert!(!root.join(".kinakaze-rootfs.sha256").exists());
}

#[test]
fn failed_initialization_leaves_an_empty_root_empty_for_retry() {
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    fs::create_dir(&root).unwrap();
    fixture.manifest(json!({"schema": 1, "directories": [], "files": [
        {"path": "first", "content": "valid"}, {"path": "a", "content": "invalid parent"},
        {"path": "a/child", "content": "conflict"}
    ]}));
    assert!(!fixture.run());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fixture.manifest(
        json!({"schema": 1, "directories": [], "files": [{"path": "first", "content": "valid"}]}),
    );
    assert!(fixture.run());
    assert_eq!(fs::read_to_string(root.join("first")).unwrap(), "valid");
}

#[test]
#[cfg(windows)]
fn filesystem_case_aliases_fail_without_a_partial_root() {
    let fixture = Fixture::new();
    fixture.manifest(json!({"schema": 1, "directories": [], "files": [
        {"path": "é", "content": "first"}, {"path": "É", "content": "alias"}
    ]}));
    assert!(!fixture.run());
    assert!(!fixture.0.join("root").exists());
}
