use super::*;
use crate::ParticipantRegistry;

unsafe extern "system" fn snapshot(_: *mut u8, _: usize) -> isize {
    0
}
unsafe extern "system" fn alternate(_: *mut u8, _: usize) -> isize {
    1
}
unsafe extern "system" fn transfer(_: usize, bytes: *mut u8, len: usize) -> i32 {
    if len != 8 {
        return 22;
    }
    unsafe { bytes.cast::<u64>().write_unaligned(0x12345678) };
    0
}
fn hooks(key: u64, priority: i32) -> ForkParticipant {
    ForkParticipant {
        abi: crate::FORK_PARTICIPANT_ABI,
        priority,
        key,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: None,
    }
}
fn transport() -> ForkTransport {
    ForkTransport {
        snapshot: Some(alternate),
        transfer: Some(transfer),
    }
}

#[test]
fn transport_belongs_to_the_selected_participant_owner() {
    let mut registry = ParticipantRegistry::default();
    assert!(registry.insert(hooks(7, 10)));
    assert!(registry.insert_with_transport(hooks(7, 20), Some(transport())));
    assert!(registry.entries[0].transport.is_none());
    assert!(registry.insert_with_transport(hooks(7, 5), Some(transport())));
    assert!(registry.entries[0].transport.is_some());
    assert!(registry.insert(hooks(7, 0)));
    assert!(registry.entries[0].transport.is_none());
}

#[test]
fn nested_fork_contracts_round_trip_and_reject_wrong_owners_atomically() {
    let mut source = ParticipantRegistry::default();
    source.insert_with_transport(hooks(7, 10), Some(transport()));
    source.insert(hooks(8, 10));
    let bytes = encode_contracts(&source.entries);
    assert_eq!(bytes.len(), 32);
    let mut destination = ParticipantRegistry::default();
    destination.insert(hooks(7, 10));
    destination.insert(hooks(8, 10));
    restore_contracts_into(&mut destination, &bytes).unwrap();
    assert_eq!(encode_contracts(&destination.entries), bytes);
    assert!(destination.entries[1].transport.is_none());
    for invalid in [
        bytes[..31].to_vec(),
        [bytes.clone(), bytes.clone()].concat(),
    ] {
        let mut fresh = ParticipantRegistry::default();
        fresh.insert(hooks(7, 10));
        assert!(restore_contracts_into(&mut fresh, &invalid).is_err());
        assert!(fresh.entries[0].transport.is_none());
    }
    let mut fresh = ParticipantRegistry::default();
    let mut changed = hooks(7, 10);
    changed.snapshot = Some(alternate);
    fresh.insert(changed);
    assert!(restore_contracts_into(&mut fresh, &bytes).is_err());
    assert!(fresh.entries[0].transport.is_none());
}

#[cfg(windows)]
fn frame(records: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = crate::HANDOFF_MAGIC.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for (key, payload) in records {
        bytes.extend_from_slice(&key.to_le_bytes());
        bytes.extend_from_slice(&[0; 40]);
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(payload);
        while !bytes.len().is_multiple_of(8) {
            bytes.push(0);
        }
    }
    bytes
}

#[cfg(windows)]
#[test]
fn frame_is_validated_before_any_transfer_callback() {
    use std::os::windows::io::BorrowedHandle;
    let process = unsafe {
        BorrowedHandle::borrow_raw(windows_sys::Win32::System::Threading::GetCurrentProcess())
    };
    let mut registry = ParticipantRegistry::default();
    registry.insert_with_transport(hooks(7, 10), Some(transport()));
    let original = frame(&[(7, vec![0; 8])]);
    let mut valid = original.clone();
    native::transfer_frame(&mut valid, &registry.entries, process).unwrap();
    assert_eq!(&valid[72..80], &0x12345678u64.to_le_bytes());
    for mut invalid in [
        original[..79].to_vec(),
        frame(&[(7, vec![0; 8]), (7, vec![0; 8])]),
        frame(&[(7, vec![0; 8]), (8, vec![])]),
    ] {
        let before = invalid.clone();
        assert!(native::transfer_frame(&mut invalid, &registry.entries, process).is_err());
        assert_eq!(invalid, before);
    }
}

#[cfg(windows)]
#[test]
fn vfork_rendezvous_events_are_explicit_duplicates() {
    use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::Threading::{CreateEventW, GetCurrentProcess, SetEvent, WaitForSingleObject},
    };
    let originals: Vec<OwnedHandle> = (0..3)
        .map(|_| {
            let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            assert!(!raw.is_null());
            unsafe { OwnedHandle::from_raw_handle(raw) }
        })
        .collect();
    let payload = originals
        .iter()
        .flat_map(|event| (event.as_raw_handle() as u64).to_le_bytes())
        .collect();
    let original = frame(&[(crate::VFORK_HANDOFF_KEY, payload)]);
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) };
    for _ in 0..2 {
        let mut candidate = original.clone();
        native::transfer_frame(&mut candidate, &[], process).unwrap();
        for (source, word) in originals.iter().zip(candidate[72..96].chunks_exact(8)) {
            let raw = u64::from_le_bytes(word.try_into().unwrap()) as usize;
            let target = unsafe { OwnedHandle::from_raw_handle(raw as _) };
            assert_ne!(target.as_raw_handle(), source.as_raw_handle());
            assert_ne!(unsafe { SetEvent(target.as_raw_handle()) }, 0);
            assert_eq!(
                unsafe { WaitForSingleObject(source.as_raw_handle(), 0) },
                WAIT_OBJECT_0
            );
        }
    }
}
