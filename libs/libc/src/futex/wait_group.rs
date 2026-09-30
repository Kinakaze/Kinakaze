//! One native event for up to 128 vector entries, including other processes.
//! The existing value-only record ABI and event names stay compatible: a
//! group's records share a token and identify their entry in `reserved`.

use super::*;

pub(crate) struct WaitGroup {
    shared: &'static Shared,
    record: Record,
    event: Handle,
    members: u128,
    retired: Cell<Option<Option<usize>>>,
}

impl WaitGroup {
    pub(crate) fn new() -> Result<Self, i32> {
        let shared = shared()?;
        let record = Record {
            key: Key([0; 5]),
            token: shared.header().next_token.fetch_add(1, Ordering::Relaxed),
            born: current_thread_birth()?,
            host: std::process::id(),
            thread: unsafe { GetCurrentThreadId() },
            bitset: u32::MAX,
            reserved: 0,
        };
        let event = Handle::new(unsafe {
            CreateEventW(ptr::null(), 0, 0, record.event_name(shared.domain).as_ptr())
        })?;
        Ok(Self {
            shared,
            record,
            event,
            members: 0,
            retired: Cell::new(None),
        })
    }

    pub(crate) fn event(&self) -> HANDLE {
        self.event.0
    }

    /// Serialize each value check with wake, as Linux's multiple-wait setup
    /// does. On a later failure the caller retires all earlier registrations;
    /// a wake already selected during setup takes precedence over the error.
    pub(crate) fn enqueue(
        &mut self,
        index: usize,
        expected: i32,
        load: impl FnOnce() -> Result<(Key, i32), i32>,
    ) -> Result<(), i32> {
        assert!(index < 128 && self.members & (1u128 << index) == 0);
        debug_assert!(self.retired.get().is_none());
        let _guard = self.shared.acquire()?;
        let (key, value) = load()?;
        if value != expected {
            return Err(EAGAIN);
        }
        let mut records = self.shared.load()?;
        self.shared.reserve(&mut records, 1)?;
        records.push(Record { key, reserved: index as u32 + 1, ..self.record });
        self.shared.commit(&records);
        self.members |= 1u128 << index;
        Ok(())
    }

    /// Publish a whole vector in one double-bank transaction. The callback
    /// validates values and publishes process-local members under their queue
    /// mutex. It runs under the domain mutex after native allocation/capacity
    /// checks; no fallible operation remains once it publishes local members.
    /// Lock order is domain mutex then process-local queue mutex.
    pub(crate) fn enqueue_batch(
        indices: &[usize],
        publish: impl FnOnce() -> Result<Vec<Key>, i32>,
    ) -> Result<Self, i32> {
        assert!(!indices.is_empty());
        let mut members = 0u128;
        for &index in indices {
            assert!(index < 128 && members & (1u128 << index) == 0);
            members |= 1u128 << index;
        }
        let mut group = Self::new()?;
        let _guard = group.shared.acquire()?;
        let mut records = group.shared.load()?;
        group.shared.reserve(&mut records, indices.len())?;
        records.reserve(indices.len());
        let keys = publish()?;
        assert_eq!(keys.len(), indices.len());
        records.extend(keys.into_iter().zip(indices).map(|(key, &index)| Record {
            key,
            reserved: index as u32 + 1,
            ..group.record
        }));
        group.shared.commit(&records);
        group.members = members;
        Ok(group)
    }

    /// Queue presence, including requeued entries, is authoritative. A set
    /// event before an abandoned commit is merely a spurious notification.
    /// Retire the entire group in one commit when canceled or selected.
    pub(crate) fn finish(&self, cancel: bool) -> Result<Option<usize>, i32> {
        if let Some(result) = self.retired.get() {
            return Ok(result);
        }
        let _guard = self.shared.acquire()?;
        let records = self.shared.records()?;
        let mut present = 0u128;
        for record in records.iter().filter(|r| r.token == self.record.token) {
            if !(1..=128).contains(&record.reserved) {
                return Err(EIO);
            }
            present |= 1u128 << (record.reserved - 1);
        }
        let missing = self.members & !present;
        let woken = (missing != 0).then(|| 127 - missing.leading_zeros() as usize);
        if cancel || woken.is_some() {
            if present != 0 {
                let mut records = records.to_vec();
                records.retain(|r| r.token != self.record.token);
                self.shared.commit(&records);
            }
            self.retired.set(Some(woken));
        }
        Ok(woken)
    }
}

impl Drop for WaitGroup {
    fn drop(&mut self) {
        if self.retired.get().is_none() {
            let _ = self.finish(true);
        }
    }
}

#[cfg(test)]
#[path = "wait_group/batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    #[test]
    #[ignore = "subprocess helper for vector futex wake"]
    fn vector_wake_child() {
        let key: Vec<u64> = std::env::var("KINAKAZE_FUTEX_VECTOR_TEST_KEY")
            .unwrap()
            .split(',')
            .map(|part| part.parse().unwrap())
            .collect();
        assert_eq!(wake(Key(key.try_into().unwrap()), 1, u32::MAX), Ok(1));
    }

    #[test]
    fn vector_group_receives_committed_crossprocess_wake() {
        let key = Key::anonymous(
            [u64::from(std::process::id()), process_birth().unwrap(), 91],
            92,
        );
        let mut group = WaitGroup::new().unwrap();
        for i in 0..128 {
            group.enqueue(i, 0, || Ok((key, 0))).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "futex::wait_group::tests::vector_wake_child",
                "--ignored",
                "--nocapture",
            ])
            .env(
                "KINAKAZE_FUTEX_VECTOR_TEST_KEY",
                key.0.map(|part| part.to_string()).join(","),
            )
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            unsafe { WaitForSingleObject(group.event(), 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(group.finish(false), Ok(Some(0)));
        assert_eq!(wake(key, 128, u32::MAX), Ok(0));
    }

    #[test]
    fn vector_group_handles_all_128_entries_duplicates_and_requeue() {
        let key = Key::anonymous([71, 72, 73], 74);
        let target = Key::anonymous([71, 72, 73], 75);
        let mut group = WaitGroup::new().unwrap();
        for i in 0..128 {
            group.enqueue(i, 0, || Ok((key, 0))).unwrap();
        }
        assert_eq!(requeue(key, target, 0, 128, None), Ok(128));
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
        assert_eq!(group.finish(false), Ok(None));
        assert_eq!(wake(target, 1, u32::MAX), Ok(1));
        assert_eq!(group.finish(false), Ok(Some(0)));
        assert_eq!(wake(target, 128, u32::MAX), Ok(0));
    }

    #[test]
    fn vector_group_ignores_uncommitted_event_and_cancellation_is_final() {
        let key = Key::anonymous([81, 82, 83], 84);
        let mut group = WaitGroup::new().unwrap();
        group.enqueue(127, 0, || Ok((key, 0))).unwrap();
        // Simulate SetEvent followed by waker death before bank publication.
        assert_ne!(unsafe { SetEvent(group.event()) }, 0);
        assert_eq!(group.finish(false), Ok(None));
        assert_eq!(group.finish(true), Ok(None));
        assert_eq!(group.finish(false), Ok(None));
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    }
}
