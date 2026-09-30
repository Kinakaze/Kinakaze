use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[repr(C)]
struct Entry {
    next: usize,
    word: AtomicI32,
}

#[test]
fn robust_raw_syscalls_dispatch_registration_and_query() {
    exit_current();
    let mut reported = usize::MAX;
    let mut length = 0usize;
    unsafe {
        assert_eq!(
            kinakaze_abi_syscall_raw(SYS_SET_ROBUST_LIST, 0, 24, 0, 0, 0, 0),
            0
        );
        assert_eq!(
            kinakaze_abi_syscall_raw(
                SYS_GET_ROBUST_LIST,
                0,
                &raw mut reported as u64,
                &raw mut length as u64,
                0,
                0,
                0
            ),
            0
        );
    }
    assert_eq!((reported, length), (0, 24));
    exit_current();
}

#[test]
fn robust_pthread_return_runs_kernel_cleanup_before_stack_retirement() {
    unsafe extern "sysv64" fn owner(argument: *mut c_void) -> *mut c_void {
        let head = unsafe { &*(argument.cast::<Head>()) };
        let entry = unsafe { &*(head.next as *const Entry) };
        entry.word.store(current_tid(), Ordering::Release);
        assert_eq!(set(argument as usize, 24), 0);
        core::ptr::null_mut()
    }
    let mut head = Box::new(Head {
        next: 0,
        offset: 8,
        pending: 0,
    });
    let sentinel = &raw mut *head as usize;
    let entry = Box::new(Entry {
        next: sentinel,
        word: AtomicI32::new(0),
    });
    head.next = &*entry as *const _ as usize;
    let mut thread = 0usize;
    assert_eq!(
        unsafe {
            libpthread::pthread_create(
                &raw mut thread,
                core::ptr::null(),
                Some(owner),
                sentinel as *mut c_void,
            )
        },
        0
    );
    assert_eq!(
        unsafe { libpthread::pthread_join(thread, core::ptr::null_mut()) },
        0
    );
    assert_eq!(entry.word.load(Ordering::Acquire) as u32, OWNER_DIED);
}

#[test]
fn robust_registration_validates_size_and_output_order() {
    exit_current();
    assert_eq!(set(1, 23), -i64::from(EINVAL));
    assert_eq!(set(1, 24), 0); // registration never dereferences the head
    let mut head = 0usize;
    let mut length = 0usize;
    assert_eq!(get(0, &raw mut head as usize, &raw mut length as usize), 0);
    assert_eq!((head, length), (1, 24));
    length = 0;
    assert_eq!(get(0, 1, &raw mut length as usize), -i64::from(EFAULT));
    assert_eq!(length, 24);
    assert_eq!(
        get(-1, &raw mut head as usize, &raw mut length as usize),
        -i64::from(ESRCH)
    );
    exit_current(); // unreadable list is harmless
    assert_eq!(get(0, &raw mut head as usize, &raw mut length as usize), 0);
    assert_eq!(head, 0);
}

#[test]
fn robust_walk_handles_pending_owner_checks_and_signed_offset() {
    let tid = current_tid() as u32;
    let mut head = Box::new(Head {
        next: 0,
        offset: 8,
        pending: 0,
    });
    let sentinel = &raw mut *head as usize;
    let pending = Box::new(Entry {
        next: sentinel,
        word: AtomicI32::new((tid | WAITERS) as i32),
    });
    let foreign = Box::new(Entry {
        next: &*pending as *const _ as usize,
        word: AtomicI32::new((tid + 1) as i32),
    });
    head.next = &*foreign as *const _ as usize;
    head.pending = &*pending as *const _ as usize;
    assert_eq!(set(sentinel, 24), 0);
    exit_current();
    assert_eq!(
        pending.word.load(Ordering::Acquire) as u32,
        WAITERS | OWNER_DIED
    );
    assert_eq!(foreign.word.load(Ordering::Acquire) as u32, tid + 1);

    #[repr(C)]
    struct Negative {
        word: AtomicI32,
        padding: u32,
        next: usize,
    }
    let item = Negative {
        word: AtomicI32::new(tid as i32),
        padding: 0,
        next: sentinel,
    };
    head.next = &item.next as *const _ as usize;
    head.offset = -8;
    head.pending = 0;
    walk(sentinel, tid);
    assert_eq!(item.word.load(Ordering::Acquire) as u32, OWNER_DIED);

    // Cycles stop at the ABI limit; no unbounded exit loop.
    let mut cycle = Box::new(Entry {
        next: 0,
        word: AtomicI32::new(tid as i32),
    });
    cycle.next = &*cycle as *const _ as usize;
    head.next = cycle.next;
    head.offset = 8;
    walk(sentinel, tid);
    assert_eq!(cycle.word.load(Ordering::Acquire) as u32, OWNER_DIED);

    let pi = Box::new(Entry {
        next: sentinel,
        word: AtomicI32::new((tid | WAITERS) as i32),
    });
    head.next = &*pi as *const _ as usize | 1;
    walk(sentinel, tid);
    assert_eq!(pi.word.load(Ordering::Acquire) as u32, WAITERS | OWNER_DIED);
}

#[test]
fn robust_thread_teardown_marks_owner_and_wakes_private_waiter() {
    let mut head = Box::new(Head {
        next: 0,
        offset: 8,
        pending: 0,
    });
    let sentinel = &raw mut *head as usize;
    let entry = Arc::new(Entry {
        next: sentinel,
        word: AtomicI32::new(0),
    });
    head.next = &*entry as *const _ as usize;
    let (ready, observe) = mpsc::channel();
    let (release, retire) = mpsc::channel();
    let owner_entry = Arc::clone(&entry);
    let owner = std::thread::spawn(move || {
        let tid = current_tid();
        owner_entry
            .word
            .store((tid as u32 | WAITERS) as i32, Ordering::Release);
        assert_eq!(set(sentinel, 24), 0);
        ready.send(tid).unwrap();
        retire.recv().unwrap();
        // TASK::drop handles native return; pthread uses the explicit hook.
    });
    let tid = observe.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut reported = 0usize;
    let mut length = 0usize;
    assert_eq!(
        get(tid, &raw mut reported as usize, &raw mut length as usize),
        0
    );
    assert_eq!((reported, length), (sentinel, 24));
    let target = Arc::clone(&entry);
    let expected = entry.word.load(Ordering::Acquire);
    let waiter = std::thread::spawn(move || {
        let timeout = KernelTimespec {
            tv_sec: 5,
            tv_nsec: 0,
        };
        futex_wait(
            target.word.as_ptr(),
            expected,
            &timeout,
            false,
            false,
            u32::MAX,
        )
    });
    let address = entry.word.as_ptr() as usize;
    let started = Instant::now();
    while !futex_queues()
        .lock()
        .unwrap()
        .get(&address)
        .is_some_and(|q| !q.is_empty())
    {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    owner.join().unwrap();
    assert_eq!(waiter.join().unwrap(), 0);
    assert_eq!(
        entry.word.load(Ordering::Acquire) as u32,
        WAITERS | OWNER_DIED
    );
    assert_eq!(
        get(tid, &raw mut reported as usize, &raw mut length as usize),
        -i64::from(ESRCH)
    );
}
