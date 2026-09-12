//! Optimistic fd metadata snapshots. Path lookup can block, so no fd-table
//! guard is held while resolving names. Publication accepts only an unchanged
//! set of descriptor identities, never a partial list after close or reuse.
use crate::{EAGAIN, EIO, FdEntry};

fn entries() -> Result<Vec<(i32, FdEntry)>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    Ok(table
        .slots
        .enumerated()
        .filter_map(|(fd, slot)| slot.map(|entry| (fd as i32, entry)))
        .collect())
}

fn same(left: &[(i32, FdEntry)], right: &[(i32, FdEntry)]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|((lfd, l), (rfd, r))| {
            lfd == rfd
                && l.generation == r.generation
                && l.description_id == r.description_id
                && l.kind == r.kind
                && l.raw == r.raw
                && l.flags == r.flags
            // File position changes do not change a proc-fd link's identity.
        })
}

pub(super) fn capture(
    mut resolve: impl FnMut(i32) -> Result<String, i32>,
) -> Result<Vec<(i32, String)>, i32> {
    // Continuous open/close activity must not turn fork into an unbounded busy
    // loop. Contention may fail with EAGAIN; a stable descriptor error is kept.
    const ATTEMPTS: usize = 16;
    for attempt in 0..ATTEMPTS {
        let before = entries()?;
        let links = before
            .iter()
            .map(|(fd, _)| resolve(*fd).map(|name| (*fd, name)))
            .collect::<Result<Vec<_>, _>>();
        let after = entries()?;
        if same(&before, &after) {
            return links;
        }
        #[cfg(windows)]
        kinakaze_runtime::fork_diagnostic(format_args!(
            "kinakaze: proc fd snapshot changed attempt={} before={} after={} error={:?}",
            attempt + 1,
            before.len(),
            after.len(),
            links.as_ref().err(),
        ));
        if attempt + 1 != ATTEMPTS {
            std::thread::yield_now();
        }
    }
    Err(EAGAIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EBADF, fs};

    fn replace(fd: i32, source: i32) {
        let entry = crate::get(source).unwrap();
        assert!(entry.kind.allows_missing_host_handle());
        crate::close(fd).unwrap();
        assert_eq!(
            crate::install_duplicate_exact(0, entry.kind, entry.flags, fd, entry),
            Ok(fd)
        );
    }

    #[test]
    fn close_during_resolution_rebuilds_the_complete_snapshot() {
        let directory = crate::to_guest_path(&std::env::current_dir().unwrap());
        let fd = fs::open(&directory, fs::O_RDONLY | fs::O_DIRECTORY, 0).unwrap();
        let survivor = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
        let mut closed = false;
        let links = capture(|item| {
            if item == fd && !closed {
                std::thread::spawn(move || crate::close(fd).unwrap())
                    .join()
                    .unwrap();
                closed = true;
            }
            super::super::local_fd_link_target(item)
        })
        .unwrap();
        assert!(closed);
        assert!(!links.iter().any(|(item, _)| *item == fd));
        assert!(links.contains(&(survivor, "/dev/zero".to_owned())));
        crate::close(survivor).unwrap();
    }

    #[test]
    fn descriptor_added_during_lookup_is_included_after_retry() {
        let fd = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
        let mut added = None;
        let links = capture(|item| {
            let name = super::super::local_fd_link_target(item)?;
            if item == fd && added.is_none() {
                added = Some(fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap());
            }
            Ok(name)
        })
        .unwrap();
        let added = added.unwrap();
        assert!(links.contains(&(fd, "/dev/null".to_owned())));
        assert!(links.contains(&(added, "/dev/zero".to_owned())));
        for item in [fd, added] {
            crate::close(item).unwrap();
        }
    }

    #[test]
    fn reuse_after_successful_lookup_discards_the_old_name() {
        let fd = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
        let source = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
        let mut replaced = false;
        let links = capture(|item| {
            let name = super::super::local_fd_link_target(item)?;
            if item == fd && !replaced {
                replace(fd, source);
                replaced = true;
            }
            Ok(name)
        })
        .unwrap();
        assert!(replaced);
        assert!(links.contains(&(fd, "/dev/zero".to_owned())));
        assert!(!links.contains(&(fd, "/dev/null".to_owned())));
        for item in [fd, source] {
            crate::close(item).unwrap();
        }
    }

    #[test]
    fn stable_descriptor_error_is_not_hidden() {
        let fd = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
        assert_eq!(
            capture(|item| if item == fd {
                Err(EBADF)
            } else {
                super::super::local_fd_link_target(item)
            }),
            Err(EBADF)
        );
        crate::close(fd).unwrap();
    }

    #[test]
    fn continuous_reuse_has_a_bounded_failure() {
        let fd = fs::open("/dev/null", fs::O_RDONLY, 0).unwrap();
        let source = fs::open("/dev/zero", fs::O_RDONLY, 0).unwrap();
        let mut replacements = 0;
        let result = capture(|item| {
            let name = super::super::local_fd_link_target(item)?;
            if item == fd {
                replace(fd, source);
                replacements += 1;
            }
            Ok(name)
        });
        assert_eq!(result, Err(EAGAIN));
        assert_eq!(replacements, 16);
        for item in [fd, source] {
            crate::close(item).unwrap();
        }
    }
}
