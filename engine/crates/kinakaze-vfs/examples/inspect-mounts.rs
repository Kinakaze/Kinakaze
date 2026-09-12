//! Read the actual shared mount table without changing its published entries.
use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let require_empty = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--require-empty" => true,
        [argument] if argument == "--help" || argument == "-h" => {
            println!(
                "Usage: inspect-mounts [--require-empty]\nReads the actual shared mount table. Never changes or clears mounts.\nExit: 0 success, 1 read failure, 2 invalid arguments, 3 table is not empty."
            );
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("Usage: inspect-mounts [--require-empty]");
            return ExitCode::from(2);
        }
    };
    inspect(require_empty)
}

#[cfg(windows)]
fn inspect(require_empty: bool) -> ExitCode {
    match kinakaze_vfs::mount::snapshot_list() {
        Ok(points) => {
            println!("mount_count={}", points.len());
            for (index, point) in points.iter().enumerate() {
                // Debug string formatting escapes control characters, so a
                // pathname cannot forge additional diagnostic records.
                println!(
                    "mount[{index}] source={:?} target={:?} flags={:#x}",
                    point.source, point.target, point.flags
                );
            }
            if require_empty && !points.is_empty() {
                eprintln!("Mount table is not empty; no mounts were changed.");
                ExitCode::from(3)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("Cannot inspect mount table: Linux errno {error}. No mounts were changed.");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("Native shared mount inspection requires Windows.");
    ExitCode::FAILURE
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use kinakaze_vfs::mount::{MS_BIND, bind, snapshot_list, unmount};

    #[test]
    fn checker_rejects_nonempty_table_without_removing_live_mounts() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("/kinakaze-inspector-test-{}-{nonce}", std::process::id());
        let source = format!("{prefix}/source");
        let target = format!("{prefix}/target");
        struct OwnedMount<'a> {
            source: &'a str,
            target: &'a str,
        }
        impl Drop for OwnedMount<'_> {
            fn drop(&mut self) {
                let owned_top = snapshot_list().ok().is_some_and(|points| {
                    points
                        .iter()
                        .rev()
                        .find(|point| point.target == self.target)
                        .is_some_and(|point| point.source == self.source)
                });
                if owned_top {
                    let _ = unmount(self.target, 0);
                }
            }
        }
        assert!(
            !snapshot_list()
                .unwrap()
                .iter()
                .any(|point| point.target == target)
        );
        bind(&source, &target, MS_BIND).unwrap();
        let owned = OwnedMount {
            source: &source,
            target: &target,
        };
        assert_eq!(inspect(false), ExitCode::SUCCESS);
        assert_eq!(inspect(true), ExitCode::from(3));
        assert!(snapshot_list().unwrap().iter().any(|point| {
            point.source == source && point.target == target && point.flags == MS_BIND
        }));
        drop(owned);
        assert!(
            !snapshot_list()
                .unwrap()
                .iter()
                .any(|point| point.target == target)
        );
    }
}
