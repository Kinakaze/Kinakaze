use super::*;
use crate::state_codec::{Reader, word};

#[test]
fn root_handoff_rejects_corruption_and_base_changes_before_mutation() {
    let before = crate::fs_context::read(Clone::clone);
    let base = default_system_root().to_path_buf();
    let frame = serialize_root().unwrap();
    let unchanged = || {
        let now = crate::fs_context::read(Clone::clone);
        assert_eq!(now.root, before.root);
        assert_eq!(now.confined, before.confined);
        assert!(now.overlay == before.overlay);
        assert_eq!(default_system_root(), base);
    };
    for length in 0..frame.len() {
        assert!(!restore_root(&frame[..length]), "accepted prefix {length}");
        unchanged();
    }
    let mut corrupt = frame.clone();
    corrupt[7] ^= 1;
    assert!(!restore_root(&corrupt));
    let mut reader = Reader(&frame[8..]);
    assert_eq!(read_path(&mut reader).unwrap(), base);
    read_path(&mut reader).unwrap();
    let flags = frame.len() - reader.0.len();
    for offset in [flags, flags + 8] {
        let mut corrupt = frame.clone();
        corrupt[offset..offset + 8].copy_from_slice(&2u64.to_le_bytes());
        assert!(!restore_root(&corrupt));
        unchanged();
    }
    let mut extra = frame.clone();
    extra.push(0);
    assert!(!restore_root(&extra));
    let mut other = STATE_MAGIC.to_vec();
    write_path(&mut other, base.parent().unwrap());
    write_path(&mut other, &base);
    word(&mut other, 1);
    word(&mut other, 0);
    assert!(!restore_root(&other), "a live namespace base cannot change");
    unchanged();
    assert!(restore_root(&frame));
    assert_eq!(default_system_root(), base);
    crate::fs_context::update(|state| *state = before);
}

#[test]
fn root_path_encoding_is_bounded_absolute_and_preserves_utf16() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let units = [b'C' as u16, b':' as u16, b'\\' as u16, 0xd800, 0x4e2d];
    let path = PathBuf::from(std::ffi::OsString::from_wide(&units));
    let mut bytes = Vec::new();
    write_path(&mut bytes, &path);
    let decoded = read_path(&mut Reader(&bytes)).unwrap();
    assert_eq!(decoded.as_os_str().encode_wide().collect::<Vec<_>>(), units);
    for path in [Path::new("relative"), Path::new("C:\\bad\0name")] {
        let mut bytes = Vec::new();
        write_path(&mut bytes, path);
        assert!(read_path(&mut Reader(&bytes)).is_err());
    }
    assert!(read_path(&mut Reader(&u64::MAX.to_le_bytes())).is_err());
}
