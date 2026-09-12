//! Opt-in, bounded path-resolution samples for real container workloads.
//! No path cache is used: this only locates repeated or expensive resolution.
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

thread_local! { static CALLS: Cell<u64> = const { Cell::new(0) }; }

pub(super) struct Resolution<'a> {
    path: &'a str,
    count: u64,
    start: Instant,
    caller: &'static std::panic::Location<'static>,
    directory: &'static Path,
}

#[track_caller]
pub(super) fn resolution(path: &str) -> Option<Resolution<'_>> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
    let directory = DIRECTORY
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os("KINAKAZE_OVERLAY_PROFILE")?);
            (path.is_absolute() && path.is_dir()).then_some(path)
        })
        .as_deref()?;
    let count = CALLS.with(|calls| {
        let value = calls.get().saturating_add(1);
        calls.set(value);
        value
    });
    Some(Resolution {
        path,
        count,
        start: Instant::now(),
        caller: std::panic::Location::caller(),
        directory,
    })
}

impl Drop for Resolution<'_> {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_micros();
        if self.count <= 4 || self.count % 256 == 0 || elapsed >= 50_000 {
            // Child stdout/stderr can be a protocol (iptables is one example).
            // Write only to an explicitly selected host diagnostic directory.
            let Ok(mut output) = std::fs::OpenOptions::new().create(true).append(true).open(
                self.directory
                    .join(format!("overlay-{}.log", std::process::id())),
            ) else {
                return;
            };
            let _ = writeln!(
                output,
                "kinakaze: overlay resolution pid={} tid={} call={} elapsed_us={} caller={}:{} path={:?}",
                std::process::id(),
                crate::interrupt::current_thread_id(),
                self.count,
                elapsed,
                self.caller.file(),
                self.caller.line(),
                self.path
            );
        }
    }
}
