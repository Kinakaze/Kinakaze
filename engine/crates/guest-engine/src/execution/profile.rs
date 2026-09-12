//! Opt-in execution preparation timings, sharing the loader's output directory.
use std::{io::Write, path::PathBuf, sync::OnceLock, time::Instant};

pub(super) struct Span {
    start: Instant,
    phase: &'static str,
    length: usize,
    directory: &'static PathBuf,
}

pub(super) fn begin(phase: &'static str, length: usize) -> Option<Span> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
    let directory = DIRECTORY
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os("KINAKAZE_LOADER_PROFILE")?);
            (path.is_absolute() && path.is_dir()).then_some(path)
        })
        .as_ref()?;
    Some(Span {
        start: Instant::now(),
        phase,
        length,
        directory,
    })
}

impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_micros();
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
            self.directory
                .join(format!("execution-{}.log", std::process::id())),
        ) {
            let _ = writeln!(
                file,
                "{} elapsed_us={} bytes={}",
                self.phase, elapsed, self.length
            );
        }
    }
}
