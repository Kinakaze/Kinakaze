//! Reuse a decoded volume only within one synchronous filesystem operation.
//! Shared publications invalidate it immediately; the outer scope always drops
//! it, so native sysfs observations refresh on the next operation without a TTL.
use super::{State, Store};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
struct Snapshot {
    revision: u64,
    refreshed: bool,
    state: Arc<State>,
}
#[derive(Default)]
struct Observations {
    depth: usize,
    snapshots: BTreeMap<u64, Snapshot>,
}
thread_local! { static CURRENT: RefCell<Observations> = RefCell::new(Observations::default()); }

pub(crate) struct Scope(std::marker::PhantomData<std::rc::Rc<()>>);
impl Scope {
    pub(crate) fn enter() -> Self {
        CURRENT.with(|current| current.borrow_mut().depth += 1);
        Self(std::marker::PhantomData)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            current.depth -= 1;
            if current.depth == 0 {
                current.snapshots.clear();
            }
        });
    }
}
pub(super) fn get(store: &Store, refreshed: bool) -> Option<Arc<State>> {
    CURRENT.with(|current| {
        let current = current.borrow();
        if current.depth == 0 {
            return None;
        }
        let snapshot = current.snapshots.get(&store.id())?;
        if (refreshed && !snapshot.refreshed) || store.revision() != snapshot.revision {
            return None;
        }
        Some(snapshot.state.clone())
    })
}
pub(super) fn put(store: &Store, revision: u64, state: &Arc<State>, refreshed: bool) {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        if current.depth != 0 {
            current.snapshots.insert(
                store.id(),
                Snapshot {
                    revision,
                    refreshed,
                    state: state.clone(),
                },
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmpfs::{Volume, parse, prepare_unkept, volume};

    fn fixture() -> Arc<Volume> {
        let source = prepare_unkept("size=64k").unwrap();
        volume(parse(&source).unwrap().0).unwrap()
    }

    #[test]
    fn nested_reads_reuse_one_observation_and_outer_exit_releases_it() {
        let volume = fixture();
        let first;
        {
            let _scope = Scope::enter();
            first = volume.snapshot().unwrap();
            let second = volume.snapshot().unwrap();
            assert!(Arc::ptr_eq(&first, &second));
            {
                let _nested = Scope::enter();
                assert!(Arc::ptr_eq(&first, &volume.snapshot().unwrap()));
            }
            assert!(Arc::ptr_eq(&first, &volume.snapshot().unwrap()));
        }
        let _scope = Scope::enter();
        assert!(!Arc::ptr_eq(&first, &volume.snapshot().unwrap()));
    }

    #[test]
    fn independent_publisher_invalidates_an_active_read_scope() {
        let volume = fixture();
        let id = volume.meta.id();
        let _scope = Scope::enter();
        let before = volume.snapshot().unwrap();
        std::thread::spawn(move || {
            // A separate mapping and thread do not share the observation cache.
            let remote = Volume::map(Store::user_object(id, false).unwrap(), false).unwrap();
            remote
                .change(|state| {
                    state.nodes.get_mut(&1).unwrap().mode = crate::fs::S_IFDIR | 0o700;
                    Ok(())
                })
                .unwrap();
        })
        .join()
        .unwrap();
        let after = volume.snapshot().unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_ne!(before.nodes[&1].mode, after.nodes[&1].mode);
        assert_eq!(after.nodes[&1].mode, crate::fs::S_IFDIR | 0o700);
    }

    #[test]
    fn failed_update_does_not_publish_mutated_state_to_observers() {
        let volume = fixture();
        let _scope = Scope::enter();
        let before = volume.snapshot().unwrap();
        let publication = volume.meta.revision();
        assert_eq!(
            volume.change::<()>(|state| {
                state.nodes.get_mut(&1).unwrap().mode = 0;
                Err(crate::EACCES)
            }),
            Err(crate::EACCES)
        );
        assert_eq!(volume.meta.revision(), publication);
        assert!(Arc::ptr_eq(&before, &volume.snapshot().unwrap()));
        assert_ne!(volume.snapshot().unwrap().nodes[&1].mode, 0);
    }
}
