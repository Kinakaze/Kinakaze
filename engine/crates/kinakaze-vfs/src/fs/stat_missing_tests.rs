use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-stat-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_observation_expires_and_symlink_rules_survive() {
    let fixture = Fixture::new();
    let leaf = fixture.0.join("leaf");
    let guest = crate::to_guest_path(&leaf);
    assert!(matches!(stat(&guest), Err(ENOENT)));
    assert!(matches!(lstat(&guest), Err(ENOENT)));
    std::fs::write(&leaf, b"created after the missing observation").unwrap();
    assert_eq!(stat(&guest).unwrap().st_size, 37);
    assert_eq!(lstat(&guest).unwrap().st_mode & S_IFMT, S_IFREG);
    assert!(matches!(stat(&format!("{guest}/child")), Err(ENOTDIR)));
    std::fs::remove_file(&leaf).unwrap();
    crate::create_emulated_symlink(&leaf, "missing-target").unwrap();
    assert_eq!(lstat(&guest).unwrap().st_mode & S_IFMT, S_IFLNK);
    assert!(matches!(stat(&guest), Err(ENOENT)));
}

#[test]
fn absent_native_leaves_preserve_synthetic_configuration_files() {
    let fixture = Fixture::new();
    for path in ["/etc/hosts", "/etc/resolv.conf", "/etc/environment"] {
        for follow in [false, true] {
            let result =
                stat_path_resolved(path, follow, Some((fixture.0.join("absent"), true))).unwrap();
            assert_eq!(result.st_mode & S_IFMT, S_IFREG);
        }
    }
}
