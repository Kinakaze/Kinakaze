use super::*;

#[test]
fn initial_identity_respects_sentinel_and_namespace_transitions() {
    let original = serialize().unwrap();
    struct Restore(Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            assert!(restore(&self.0));
        }
    }
    let _restore = Restore(original.clone());
    assert!(is_initial());
    for group in [false, true] {
        for value in [0, 1, 65534, 65535, u32::MAX - 1] {
            assert_eq!(visible(value, group), value);
            assert_eq!(kernel(value, group), Ok(value));
        }
        assert_eq!(visible(u32::MAX, group), 65534);
        assert_eq!(kernel(u32::MAX, group), Err(EOVERFLOW));
    }
    assert!(groups_allowed());
    let pid = crate::job::process_id();
    assert_eq!(write_map(pid, false, b"0 1234 1\n", 0), Err(EPERM));
    assert_eq!(groups(pid, Some(b"deny")), Err(EPERM));

    prepare_unshare().unwrap().install().unwrap();
    assert!(!is_initial());
    assert_eq!(visible(0, false), 65534);
    assert_eq!(kernel(0, false), Err(EOVERFLOW));
    assert!(groups_allowed());
    assert_eq!(groups(pid, Some(b"deny")).unwrap(), b"deny\n");
    assert!(!groups_allowed());
    // Map the caller's outer ID to different visible IDs. This is the ordinary
    // unprivileged one-ID mapping, after explicitly denying setgroups.
    write_map(pid, false, b"1234 0 1\n", 0).unwrap();
    write_map(pid, true, b"5678 0 1\n", 0).unwrap();
    for (group, base) in [(false, 1234), (true, 5678)] {
        assert_eq!(visible(0, group), base);
        assert_eq!(visible(1, group), 65534);
        assert_eq!(kernel(base, group), Ok(0));
        assert_eq!(kernel(base + 1, group), Err(EOVERFLOW));
        assert_eq!(kernel(0, group), Err(EOVERFLOW));
        assert_eq!(kernel(u32::MAX, group), Err(EOVERFLOW));
    }
    assert!(restore(&original));
    assert!(is_initial());
    assert!(groups_allowed());
    assert_eq!(visible(1234, false), 1234);
    assert_eq!(kernel(5678, true), Ok(5678));
}
