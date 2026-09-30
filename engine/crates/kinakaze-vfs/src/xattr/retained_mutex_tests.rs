use super::*;
use std::sync::Arc;

fn fixture() -> (std::fs::File, InodeKey, std::path::PathBuf) {
    use std::os::windows::io::AsRawHandle;
    let path = std::env::temp_dir().join(format!(
        "kinakaze-retained-mutex-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let file = std::fs::File::create(&path).unwrap();
    let key = InodeKey::from_handle(file.as_raw_handle().cast()).unwrap();
    (file, key, path)
}

fn other_thread_can_acquire(name: Vec<u16>) -> bool {
    std::thread::spawn(move || {
        let mutex = Handle::new(unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) }).unwrap();
        match unsafe { WaitForSingleObject(mutex.0, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                assert_ne!(unsafe { ReleaseMutex(mutex.0) }, 0);
                true
            }
            WAIT_TIMEOUT => false,
            other => panic!("unexpected wait status: {other:#x}"),
        }
    })
    .join()
    .unwrap()
}

#[test]
fn retained_mutex_synchronizes_fresh_openers_and_recursive_guards() {
    let (file, key, path) = fixture();
    let mutex = InodeMutex::open(&key).unwrap();
    assert!(other_thread_can_acquire(key.0.clone()));
    for _ in 0..3 {
        let first = mutex.acquire().unwrap();
        let second = key.acquire().unwrap();
        assert!(!other_thread_can_acquire(key.0.clone()));
        drop(second);
        assert!(!other_thread_can_acquire(key.0.clone()));
        drop(first);
        assert!(other_thread_can_acquire(key.0.clone()));
    }
    drop(mutex);
    drop(file);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn retained_mutex_recovers_a_terminated_native_owner() {
    let (file, key, path) = fixture();
    let mutex = Arc::new(InodeMutex::open(&key).unwrap());
    let other = mutex.clone();
    std::thread::spawn(move || {
        // The owner exits without unlocking; the surviving handle must still
        // admit a later transaction after Windows reports an abandoned mutex.
        std::mem::forget(other.acquire().unwrap());
    })
    .join()
    .unwrap();
    let guard = mutex.acquire().unwrap();
    assert!(!other_thread_can_acquire(key.0.clone()));
    drop(guard);
    assert!(other_thread_can_acquire(key.0.clone()));
    drop(mutex);
    drop(file);
    std::fs::remove_file(path).unwrap();
}
