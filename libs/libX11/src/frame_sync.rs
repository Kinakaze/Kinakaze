//! Commit client pixels before publishing the extended EWMH frame counter.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn trace_directory() -> Option<&'static std::path::Path> {
    static DIRECTORY: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| std::env::var_os("KINAKAZE_FRAME_TRACE_DIR").map(std::path::PathBuf::from))
        .as_deref()
}
pub fn trace_enabled() -> bool {
    trace_directory().is_some()
}
pub fn trace(args: core::fmt::Arguments<'_>) {
    use std::io::Write;
    let Some(directory) = trace_directory() else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(format!("frame-{}.log", std::process::id())))
    {
        let _ = writeln!(file, "{} {args}", crate::focus::now());
    }
}

fn counters() -> &'static Mutex<HashMap<usize, usize>> {
    static COUNTERS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(crate) fn bind(window: usize, counter: Option<usize>) {
    trace(format_args!("bind window={window:#x} counter={counter:?}"));
    let mut records = counters().lock().unwrap_or_else(|e| e.into_inner());
    records.retain(|_, owner| *owner != window);
    if let Some(counter) = counter {
        records.insert(counter, window);
    }
    drop(records);
    kinakaze_libdisplay::ui::presentation::frame_protocol(window, counter.is_some());
}
pub fn counter_changing(counter: usize, value: i64) {
    // Odd means painting; a positive even value closes the frame. Initial
    // background allocation and partial XRender batches are not complete frames.
    if value <= 0 || value & 1 != 0 {
        return;
    }
    let window = counters()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&counter)
        .copied();
    if let Some(window) = window {
        crate::graphics::flush_all();
        kinakaze_libdisplay::ui::presentation::client_frame_ready(window);
    }
}
