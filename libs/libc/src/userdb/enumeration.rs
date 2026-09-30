//! One owned database snapshot per passwd/group enumeration, released at EOF
//! or set/end. Name/ID lookups do not consult this state. No host file handle or
//! pointer into the native heap crosses fork; the existing cursor handoff lets
//! the fresh child rebuild its enumeration at the inherited record position.
use super::{GROUP_CURSOR, GroupAccount, PASSWD_CURSOR, UserAccount};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Enumeration<T> {
    records: Option<Vec<T>>,
    ended: bool,
}

impl<T> Enumeration<T> {
    const fn new() -> Self {
        Self {
            records: None,
            ended: false,
        }
    }

    fn next<R>(
        &mut self,
        cursor: &AtomicUsize,
        load: impl FnOnce() -> Vec<T>,
        publish: impl FnOnce(&T) -> Result<R, i32>,
    ) -> Result<Option<R>, i32> {
        if self.ended {
            return Ok(None);
        }
        let records = self.records.get_or_insert_with(load);
        let index = cursor.load(Ordering::SeqCst);
        let Some(record) = records.get(index) else {
            // Repeated EOF queries must neither reopen the database nor retain
            // all its strings. A new set/end operation starts a fresh stream.
            self.records = None;
            self.ended = true;
            return Ok(None);
        };
        let result = publish(record)?;
        // ERANGE leaves this record available to both reentrant and ordinary
        // callers. Reset and next are serialized by the same database mutex.
        cursor.store(index + 1, Ordering::SeqCst);
        Ok(Some(result))
    }
}

static USERS: Mutex<Enumeration<UserAccount>> = Mutex::new(Enumeration::new());
static GROUPS: Mutex<Enumeration<GroupAccount>> = Mutex::new(Enumeration::new());

pub(super) fn user<R>(
    publish: impl FnOnce(&UserAccount) -> Result<R, i32>,
) -> Result<Option<R>, i32> {
    USERS.lock().unwrap_or_else(|e| e.into_inner()).next(
        &PASSWD_CURSOR,
        super::load_user_accounts,
        publish,
    )
}

pub(super) fn group<R>(
    publish: impl FnOnce(&GroupAccount) -> Result<R, i32>,
) -> Result<Option<R>, i32> {
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).next(
        &GROUP_CURSOR,
        super::load_group_accounts,
        publish,
    )
}

pub(super) fn reset_users() {
    let mut state = USERS.lock().unwrap_or_else(|e| e.into_inner());
    *state = Enumeration::new();
    PASSWD_CURSOR.store(0, Ordering::SeqCst);
}

pub(super) fn reset_groups() {
    let mut state = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
    *state = Enumeration::new();
    GROUP_CURSOR.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_load_retry_same_record_and_release_at_eof() {
        let cursor = AtomicUsize::new(0);
        let mut state = Enumeration::new();
        assert_eq!(
            state.next(&cursor, || vec![17, 23], |_| Err::<(), _>(34)),
            Err(34)
        );
        assert_eq!(cursor.load(Ordering::SeqCst), 0);
        for expected in [17, 23] {
            assert_eq!(
                state.next(&cursor, || panic!("reopened enumeration"), |v| Ok(*v)),
                Ok(Some(expected))
            );
        }
        assert_eq!(
            state.next(&cursor, || panic!("reopened at EOF"), |v| Ok(*v)),
            Ok(None)
        );
        assert!(state.records.is_none());
        assert_eq!(
            state.next(&cursor, || panic!("reopened after EOF"), |v| Ok(*v)),
            Ok(None)
        );
    }
}
