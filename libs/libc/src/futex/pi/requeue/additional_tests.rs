use super::*;

#[test]
fn source_target_journal_roles_are_metadata_without_changing_record_size() {
    let identity = Identity::current().unwrap();
    let key = Key::private(0x4000).unwrap();
    let source = identity.record(key, 42, 1, REQUEUE | hybrid::PRIVATE_PARK);
    assert!(source.special_wait());
    assert!(!source.pi_wait());
    assert!(!source.metadata());
    for role in [TARGET, SOURCE_JOURNAL] {
        assert!(identity.record(key, 42, 1, role).metadata());
    }
    assert_eq!(size_of::<Record>(), 72);
}
