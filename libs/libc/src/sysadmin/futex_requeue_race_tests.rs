//! Deterministic publication races run in fresh processes, where the bridge
//! hint starts false and no earlier test can mask the local-only fast path.
use super::*;
use core::ptr;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Pause {
    reached: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

static PAUSE_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static PAUSE: Mutex<Option<Arc<Pause>>> = Mutex::new(None);

pub(super) fn pause_after_hint(address: usize) {
    let paused = PAUSE_ADDRESS.load(Ordering::Acquire);
    if paused == 0 || paused != address {
        return;
    }
    let pause = PAUSE.lock().unwrap().as_ref().cloned().unwrap();
    pause.reached.send(()).unwrap();
    pause
        .release
        .lock()
        .unwrap()
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
}

#[repr(C)]
struct Entry {
    value: u64,
    address: u64,
    flags: u32,
    reserved: u32,
}

fn migrate(
    source: &AtomicI32,
    source_private: bool,
    target: &AtomicI32,
    target_private: bool,
) -> i64 {
    let entry = |word: &AtomicI32, private| Entry {
        value: 0,
        address: word.as_ptr() as u64,
        flags: 2 | if private { FUTEX_PRIVATE_FLAG } else { 0 },
        reserved: 0,
    };
    let entries = [entry(source, source_private), entry(target, target_private)];
    unsafe { kinakaze_abi_syscall_raw(456, entries.as_ptr() as u64, 0, 0, 1, 0, 0) }
}

fn wake(word: &AtomicI32, private: bool) -> i64 {
    futex_syscall(
        word.as_ptr(),
        (FUTEX_WAKE | if private { FUTEX_PRIVATE_FLAG } else { 0 }) as i32,
        1,
        ptr::null(),
        ptr::null_mut(),
        0,
    )
}

fn spawn_wait(
    word: &Arc<AtomicI32>,
    private: bool,
    milliseconds: i64,
) -> std::thread::JoinHandle<i64> {
    let word = Arc::clone(word);
    std::thread::spawn(move || {
        let timeout = KernelTimespec {
            tv_sec: milliseconds / 1000,
            tv_nsec: milliseconds % 1000 * 1_000_000,
        };
        futex_wait(
            FutexAddress::resolve(word.as_ptr(), private).unwrap(),
            0,
            &timeout,
            false,
            false,
            u32::MAX,
        )
    })
}

fn local_waiter(word: &AtomicI32) -> Arc<FutexWaiter> {
    let started = Instant::now();
    loop {
        if let Some(waiter) = futex_queues()
            .lock()
            .unwrap()
            .get(&(word.as_ptr() as usize))
            .and_then(|queue| queue.front().cloned())
        {
            return waiter;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
}

fn shared_queued(word: &AtomicI32, count: usize) {
    let key = crate::fdio::futex_key(word.as_ptr() as usize).unwrap();
    let started = Instant::now();
    while crate::futex::count_for_test(key) != count {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
}

fn child(name: &str, mode: &str) {
    let mut process = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env("KINAKAZE_FUTEX_HINT_RACE", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let timed_out = loop {
        if process.try_wait().unwrap().is_some() {
            break false;
        }
        if started.elapsed() > Duration::from_secs(30) {
            process.kill().unwrap();
            break true;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = process.wait_with_output().unwrap();
    assert!(
        !timed_out && output.status.success(),
        "{mode}, timeout={timed_out}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bridge_publication_is_rechecked_for_all_three_private_fast_paths() {
    for mode in ["wake", "wake-op", "requeue"] {
        child("sysadmin::futex_requeue_race_tests::hint_race_child", mode);
    }
}

#[test]
#[ignore = "fresh-process helper for bridge publication races"]
fn hint_race_child() {
    let mode = std::env::var("KINAKAZE_FUTEX_HINT_RACE").unwrap();
    assert!(!FUTEX_PRIVATE_BRIDGED.load(Ordering::Acquire));
    let source = Arc::new(AtomicI32::new(0));
    let target = Arc::new(AtomicI32::new(0));
    let destination = Arc::new(AtomicI32::new(0));
    let sleeper = spawn_wait(&source, false, 10_000);
    shared_queued(&source, 1);
    let (reached_tx, reached_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    *PAUSE.lock().unwrap() = Some(Arc::new(Pause {
        reached: reached_tx,
        release: Mutex::new(release_rx),
    }));
    PAUSE_ADDRESS.store(target.as_ptr() as usize, Ordering::Release);
    let waker_target = Arc::clone(&target);
    let waker_destination = Arc::clone(&destination);
    let waker_mode = mode.clone();
    let waker = std::thread::spawn(move || match waker_mode.as_str() {
        "wake" => wake(&waker_target, true),
        "wake-op" => futex_syscall(
            waker_target.as_ptr(),
            (FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG) as i32,
            1,
            1usize as _,
            waker_destination.as_ptr(),
            0,
        ),
        "requeue" => futex_syscall(
            waker_target.as_ptr(),
            (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
            0,
            1usize as _,
            waker_destination.as_ptr(),
            0,
        ),
        _ => unreachable!(),
    });
    reached_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The first hint read was false. Publish a mixed transfer before the
    // paused fast path acquires the local mutex and checks the hint again.
    let migrated = migrate(&source, false, &target, true);
    PAUSE_ADDRESS.store(0, Ordering::Release);
    release_tx.send(()).unwrap();
    let selected = waker.join().unwrap();
    // Clean up before assertions even when testing a regressed implementation.
    wake(&target, true);
    wake(&destination, true);
    let result = sleeper.join().unwrap();
    assert_eq!(migrated, 1);
    assert_eq!(selected, 1, "{mode} missed the newly published bridge");
    assert_eq!(result, 0);
    shared_queued(&source, 0);
    assert_eq!(wake(&target, true), 0);
    assert_eq!(wake(&destination, true), 0);
}

#[test]
fn first_private_bridge_token_in_a_fresh_domain_is_not_the_local_sentinel() {
    child(
        "sysadmin::futex_requeue_race_tests::fresh_domain_token_child",
        "first-token",
    );
}

#[test]
#[ignore = "fresh-domain helper for the first bridge token"]
fn fresh_domain_token_child() {
    let domain = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        ^ (u64::from(std::process::id()) << 32);
    kinakaze_runtime::authority::install_helper_domain(domain.max(1)).unwrap();
    let source = Arc::new(AtomicI32::new(0));
    let target = AtomicI32::new(0);
    let sleeper = spawn_wait(&source, true, 500);
    let waiter = local_waiter(&source);
    assert_eq!(waiter.token.load(Ordering::Acquire), 0);
    assert_eq!(migrate(&source, true, &target, false), 1);
    let token = waiter.token.load(Ordering::Acquire);
    let result = sleeper.join().unwrap();
    let remaining = wake(&target, false);
    assert_ne!(token, 0, "migration published the local-only sentinel");
    assert_eq!(result, -i64::from(ETIMEDOUT));
    assert_eq!(remaining, 0);
    shared_queued(&target, 0);
}
