//! Per-worker recursive exclusion and condition signaling for audio callbacks.
//! This module owns no OS process identity or cross-process synchronization.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

#[derive(Default)]
struct State {
    owner: Option<ThreadId>,
    depth: usize,
    signal_epoch: u64,
    pending_accepts: usize,
    running: bool,
}

#[derive(Default)]
pub(crate) struct LoopLock {
    state: Mutex<State>,
    changed: Condvar,
}

pub(crate) struct Guard<'a>(&'a LoopLock);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.unlock();
    }
}

impl LoopLock {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn wait_state<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(state)
            .unwrap_or_else(|error| error.into_inner())
    }

    fn acquire(&self, mut state: MutexGuard<'_, State>, depth: usize) {
        let current = thread::current().id();
        while state.owner.is_some() && state.owner != Some(current) {
            state = self.wait_state(state);
        }
        state.owner = Some(current);
        state.depth = state
            .depth
            .checked_add(depth)
            .expect("audio loop lock recursion overflow");
    }

    pub(crate) fn lock(&self) {
        self.acquire(self.state(), 1);
    }

    pub(crate) fn guard(&self) -> Guard<'_> {
        self.lock();
        Guard(self)
    }

    pub(crate) fn unlock(&self) {
        let mut state = self.state();
        assert_eq!(
            state.owner,
            Some(thread::current().id()),
            "audio loop unlock without ownership"
        );
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = None;
            self.changed.notify_all();
        }
    }

    fn release(&self, state: &mut State) -> usize {
        assert_eq!(
            state.owner,
            Some(thread::current().id()),
            "audio loop wait without ownership"
        );
        let depth = state.depth;
        state.depth = 0;
        state.owner = None;
        self.changed.notify_all();
        depth
    }

    pub(crate) fn wait(&self) {
        let mut state = self.state();
        let epoch = state.signal_epoch;
        let depth = self.release(&mut state);
        while state.signal_epoch == epoch {
            state = self.wait_state(state);
        }
        self.acquire(state, depth);
    }

    pub(crate) fn signal(&self, wait_for_accept: bool) {
        let mut state = self.state();
        assert_eq!(
            state.owner,
            Some(thread::current().id()),
            "audio loop signal without ownership"
        );
        state.signal_epoch = state.signal_epoch.wrapping_add(1);
        if wait_for_accept {
            state.pending_accepts += 1;
        }
        self.changed.notify_all();
        if wait_for_accept {
            let depth = self.release(&mut state);
            while state.pending_accepts != 0 {
                state = self.wait_state(state);
            }
            self.acquire(state, depth);
        }
    }

    pub(crate) fn accept(&self, all: bool) {
        let mut state = self.state();
        assert_eq!(
            state.owner,
            Some(thread::current().id()),
            "audio loop accept without ownership"
        );
        assert!(
            state.pending_accepts != 0,
            "audio loop accept without pending signal"
        );
        state.pending_accepts = if all { 0 } else { state.pending_accepts - 1 };
        self.changed.notify_all();
    }

    pub(crate) fn set_running(&self, running: bool) {
        let _guard = self.guard();
        self.state().running = running;
        self.changed.notify_all();
    }

    pub(crate) fn wake(&self) {
        // Locking pairs a cancellation predicate change with the condvar wait,
        // so a worker cannot miss its stop notification before it parks.
        let _state = self.state();
        self.changed.notify_all();
    }

    pub(crate) fn callback(&self, stopped: &AtomicBool) -> Option<Guard<'_>> {
        let mut state = self.state();
        let current = thread::current().id();
        loop {
            if stopped.load(Ordering::Acquire) {
                return None;
            }
            if state.running && (state.owner.is_none() || state.owner == Some(current)) {
                state.owner = Some(current);
                state.depth += 1;
                return Some(Guard(self));
            }
            state = self.wait_state(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn recursive_wait_releases_all_depth_and_restores_it() {
        let loop_ = Arc::new(LoopLock::default());
        loop_.lock();
        loop_.lock();
        let other = Arc::clone(&loop_);
        let thread = thread::spawn(move || {
            let _guard = other.guard();
            other.signal(false);
        });
        loop_.wait();
        assert_eq!(loop_.state().depth, 2);
        loop_.unlock();
        assert_eq!(loop_.state().owner, Some(thread::current().id()));
        loop_.unlock();
        thread.join().unwrap();
    }

    #[test]
    fn signal_waits_for_accept_and_restores_recursive_ownership() {
        let loop_ = Arc::new(LoopLock::default());
        let (sent, received) = mpsc::channel();
        loop_.lock();
        let other = Arc::clone(&loop_);
        let thread = thread::spawn(move || {
            other.lock();
            other.lock();
            other.signal(true);
            assert_eq!(other.state().depth, 2);
            sent.send(()).unwrap();
            other.unlock();
            other.unlock();
        });
        loop_.wait();
        assert!(
            received.try_recv().is_err(),
            "signal returned before accept"
        );
        loop_.accept(false);
        assert!(
            received.try_recv().is_err(),
            "signaler failed to reacquire ownership"
        );
        loop_.unlock();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn callback_blocked_by_guest_lock_can_be_cancelled_without_unlocking_guest() {
        let loop_ = Arc::new(LoopLock::default());
        let stopped = Arc::new(AtomicBool::new(false));
        loop_.set_running(true);
        loop_.lock();
        let other = Arc::clone(&loop_);
        let other_stopped = Arc::clone(&stopped);
        let (sent, received) = mpsc::channel();
        let thread = thread::spawn(move || {
            assert!(other.callback(&other_stopped).is_none());
            sent.send(()).unwrap();
        });
        stopped.store(true, Ordering::Release);
        loop_.wake();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        loop_.unlock();
    }

    #[test]
    fn stopped_loop_resumes_parked_callback_when_started() {
        let loop_ = Arc::new(LoopLock::default());
        let other = Arc::clone(&loop_);
        let (sent, received) = mpsc::channel();
        let thread = thread::spawn(move || {
            let stopped = AtomicBool::new(false);
            let _guard = other.callback(&stopped).unwrap();
            sent.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(20)).is_err());
        loop_.set_running(true);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
    }
    #[test]
    fn stopping_waits_for_an_active_callback_to_release_ownership() {
        let loop_ = Arc::new(LoopLock::default());
        loop_.set_running(true);
        let callback_loop = Arc::clone(&loop_);
        let (entered, did_enter) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let callback = thread::spawn(move || {
            let stopped = AtomicBool::new(false);
            let _guard = callback_loop.callback(&stopped).unwrap();
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        did_enter.recv_timeout(Duration::from_secs(2)).unwrap();
        let stopping_loop = Arc::clone(&loop_);
        let (finished, did_finish) = mpsc::channel();
        let stopping = thread::spawn(move || {
            stopping_loop.set_running(false);
            finished.send(()).unwrap();
        });
        assert!(did_finish.recv_timeout(Duration::from_millis(20)).is_err());
        release.send(()).unwrap();
        did_finish.recv_timeout(Duration::from_secs(2)).unwrap();
        callback.join().unwrap();
        stopping.join().unwrap();
        assert!(!loop_.state().running);
    }
}
