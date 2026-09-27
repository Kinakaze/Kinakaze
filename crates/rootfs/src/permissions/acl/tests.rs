use super::*;
use crate::permissions::set_case_sensitive;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Security::PROTECTED_DACL_SECURITY_INFORMATION,
    Storage::FileSystem::{DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE},
};

#[test]
fn modify_only_parent_allows_installation_without_changing_parent_or_overriding_denies() {
    for deny in [false, true] {
        let directory = std::env::temp_dir().join(format!(
            "kinakaze-modify-acl-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let security = Security::read(&directory).unwrap();
        let modify = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
        let mut entries = vec![security.entry(modify, GRANT_ACCESS)];
        if deny {
            entries.push(security.entry(FILE_DELETE_CHILD, DENY_ACCESS));
        }
        security
            .apply(&entries, null_mut(), PROTECTED_DACL_SECURITY_INFORMATION)
            .unwrap();
        assert_eq!(
            set_case_sensitive(&directory).unwrap_err().raw_os_error(),
            Some(5)
        );
        fs::write(directory.join("rootfs.manifest.json"), serde_json::to_vec(&serde_json::json!({
            "schema":1, "case_sensitive":true, "directories":["Term", "term"],
            "files":[{"path":"Term/Name","content":"upper"}, {"path":"term/name","content":"lower"}]
        })).unwrap()).unwrap();
        let root = directory.join("root");
        let result = crate::prepare(&root, &directory, None);
        if deny {
            assert!(result.is_err());
            assert!(!root.exists());
        } else {
            result.unwrap();
            assert_eq!(fs::read(root.join("Term/Name")).unwrap(), b"upper");
            assert_eq!(fs::read(root.join("term/name")).unwrap(), b"lower");
        }
        // The release directory retains its original permissions and case behavior.
        assert_eq!(
            set_case_sensitive(&directory).unwrap_err().raw_os_error(),
            Some(5)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
