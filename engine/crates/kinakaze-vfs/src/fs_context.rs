//! Linux fs_struct: shared by CLONE_FS, copied by unshare(CLONE_FS).
//! Native process cwd is never the authority for guest path resolution.
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub(crate) struct State {
    pub cwd: Option<String>,
    #[cfg(windows)]
    pub cwd_object: Option<Arc<crate::fs::cwd::Directory>>,
    pub root: Option<PathBuf>,
    #[cfg(windows)]
    pub root_object: Option<Arc<crate::fs::object::Object>>,
    pub confined: bool,
    pub overlay: Option<crate::path::OverlayRoot>,
    pub umask: u32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            cwd: None,
            #[cfg(windows)]
            cwd_object: None,
            root: None,
            #[cfg(windows)]
            root_object: None,
            confined: false,
            overlay: None,
            umask: 0o022,
        }
    }
}
type Shared = Arc<Mutex<State>>;
static INITIAL: OnceLock<Shared> = OnceLock::new();
thread_local! { static CURRENT: RefCell<Option<Shared>> = const { RefCell::new(None) }; }

fn current() -> Shared {
    CURRENT.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| {
                INITIAL
                    .get_or_init(|| Arc::new(Mutex::new(State::default())))
                    .clone()
            })
            .clone()
    })
}
pub(crate) fn read<T>(action: impl FnOnce(&State) -> T) -> T {
    action(&current().lock().unwrap_or_else(|p| p.into_inner()))
}
pub(crate) fn update<T>(action: impl FnOnce(&mut State) -> T) -> T {
    action(&mut current().lock().unwrap_or_else(|p| p.into_inner()))
}

pub fn unshare() {
    let copy = read(Clone::clone);
    CURRENT.with(|slot| *slot.borrow_mut() = Some(Arc::new(Mutex::new(copy))));
}
pub fn is_private() -> bool {
    let value = current();
    Arc::strong_count(&value) == 2 || (kinakaze_runtime::process_thread_count() == 1)
}
pub fn umask() -> u32 {
    read(|s| s.umask)
}
pub fn set_umask(value: u32) -> u32 {
    update(|s| std::mem::replace(&mut s.umask, value & 0o777))
}

/// The creating thread owns this packet until its child acknowledges adoption.
pub struct Inheritance {
    fs: Shared,
    #[cfg(windows)]
    keys: crate::keyring::Task,
    #[cfg(windows)]
    mount: Arc<crate::mount::shared::Store>,
}
pub fn capture(shared: bool) -> Result<Inheritance, i32> {
    Ok(Inheritance {
        #[cfg(windows)]
        keys: crate::keyring::capture_thread(),
        fs: if shared {
            current()
        } else {
            Arc::new(Mutex::new(read(Clone::clone)))
        },
        #[cfg(windows)]
        mount: crate::mount::shared::get()?,
    })
}
impl Inheritance {
    pub fn adopt(&self) {
        #[cfg(windows)]
        crate::keyring::adopt(self.keys);
        CURRENT.with(|slot| *slot.borrow_mut() = Some(self.fs.clone()));
        #[cfg(windows)]
        crate::mount::shared::inherit(self.mount.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloned_fs_shares_then_unshare_separates_root_cwd_and_umask() {
        unshare();
        update(|s| {
            s.cwd = Some("/parent".into());
            s.root = Some(PathBuf::from("parent"));
            s.umask = 0o022;
        });
        let inherited = capture(true).unwrap();
        std::thread::spawn(move || {
            inherited.adopt();
            set_umask(0o027);
            unshare();
            update(|s| {
                s.cwd = Some("/child".into());
                s.root = Some(PathBuf::from("child"));
                s.umask = 0o077;
            });
            assert_eq!(read(|s| s.cwd.clone()), Some("/child".into()));
        })
        .join()
        .unwrap();
        assert_eq!(umask(), 0o027);
        assert_eq!(read(|s| s.cwd.clone()), Some("/parent".into()));
        assert_eq!(read(|s| s.root.clone()), Some(PathBuf::from("parent")));
    }
}
