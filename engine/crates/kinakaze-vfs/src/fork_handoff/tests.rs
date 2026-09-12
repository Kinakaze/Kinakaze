use super::*;
use crate::fs;

struct Descriptors(Vec<i32>);
impl Drop for Descriptors {
    fn drop(&mut self) {
        for fd in self.0.drain(..) {
            let _ = crate::close(fd);
        }
    }
}
struct Prepared;
impl Prepared {
    fn new() -> Self {
        assert_eq!(unsafe { prepare() }, 0);
        Self
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        unsafe { parent(-1) };
    }
}

fn query() -> usize {
    let length = unsafe { snapshot(std::ptr::null_mut(), 0) };
    assert!(length > 0, "snapshot query failed: {length}");
    length as usize
}
fn copy(length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    assert_eq!(
        unsafe { snapshot(bytes.as_mut_ptr(), bytes.len()) },
        length as isize
    );
    bytes
}
fn contains_fd(frame: &[u8], fd: i32) -> bool {
    let range = crate::fork_section_range(frame, 1).unwrap();
    let table = &frame[range];
    table[4..]
        .chunks_exact(36)
        .any(|entry| i32::from_le_bytes(entry[..4].try_into().unwrap()) == fd)
}

#[test]
fn query_and_copy_use_one_frame_when_descriptors_change() {
    let source = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
    let mut descriptors = Descriptors(vec![source]);
    let _prepared = Prepared::new();
    let length = query();
    let added = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
    descriptors.0.push(added);
    let frame = copy(length);
    assert!(contains_fd(&frame, source));
    assert!(!contains_fd(&frame, added));
}

#[test]
fn aborted_query_does_not_leak_into_the_next_handoff() {
    let mut descriptors = Descriptors(Vec::new());
    {
        let _prepared = Prepared::new();
        query();
    }
    let mut scratch = [0u8; 1];
    assert_eq!(
        unsafe { snapshot(scratch.as_mut_ptr(), scratch.len()) },
        -(crate::EINVAL as isize)
    );
    let fd = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
    descriptors.0.push(fd);
    let _prepared = Prepared::new();
    let frame = copy(query());
    assert!(contains_fd(&frame, fd));
    assert_eq!(
        unsafe { snapshot(scratch.as_mut_ptr(), scratch.len()) },
        -(crate::EINVAL as isize)
    );
}

#[test]
fn a_new_length_query_replaces_the_uncopied_frame() {
    let mut descriptors = Descriptors(Vec::new());
    let _prepared = Prepared::new();
    let first = query();
    let fd = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
    descriptors.0.push(fd);
    let second = query();
    assert!(second > first);
    assert!(contains_fd(&copy(second), fd));
}

#[test]
fn a_short_buffer_discards_its_frame_and_requires_a_new_query() {
    let _prepared = Prepared::new();
    let length = query();
    let mut output = vec![0u8; length];
    assert_eq!(
        unsafe { snapshot(output.as_mut_ptr(), length - 1) },
        -(crate::ENOMEM as isize)
    );
    assert_eq!(
        unsafe { snapshot(output.as_mut_ptr(), length) },
        -(crate::EINVAL as isize)
    );
    copy(query());
}

#[test]
#[ignore = "serial fork participant benchmark; stop the live service stack first"]
fn fork_frame_benchmark() {
    let path = crate::to_guest_path(&std::env::current_exe().unwrap());
    let mut descriptors = Descriptors(Vec::new());
    for _ in 0..64 {
        descriptors
            .0
            .push(fs::open(&path, fs::O_RDONLY, 0).unwrap());
    }
    for batch in 0..5 {
        let mut prepare_us = 0;
        let mut snapshot_us = 0;
        let mut bytes = 0;
        for _ in 0..16 {
            let began = std::time::Instant::now();
            let prepared = Prepared::new();
            prepare_us += began.elapsed().as_micros();
            let began = std::time::Instant::now();
            let frame = copy(query());
            snapshot_us += began.elapsed().as_micros();
            bytes = frame.len();
            assert!(contains_fd(&frame, descriptors.0[0]));
            drop(prepared);
        }
        println!(
            "FORK_FRAME batch={batch} iterations=16 native_fds=64 bytes={bytes} prepare_us={prepare_us} snapshot_us={snapshot_us}"
        );
    }
}
