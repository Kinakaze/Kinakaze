//! Opt-in, nested VFS/native timings. Never used as an installation score.
//!
//! Records use the version-one format consumed by tools/analyze-io-trace.py.
//! Each outer call flushes its thread's buffer, including before process exit.
//! Native last-error state is preserved around all diagnostic work.
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};

static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}
struct Sink {
    writer: BufWriter<File>,
    sequence: u64,
    parent: u64,
    origin: Instant,
    unix_ns: u64,
    pid: u32,
    tid: u32,
    failed: u64,
}
pub(crate) struct Span(Option<Active>);
struct Active {
    sequence: u64,
    parent: u64,
    start: Instant,
    unix_ns: u64,
    operation: &'static str,
    path: String,
    args: [u64; 3],
    result: i64,
    truncated: bool,
}
impl Span {
    #[inline]
    pub(crate) fn enter(operation: &'static str, path: &str, args: [u64; 3]) -> Self {
        let error = unsafe { GetLastError() };
        let directory =
            DIRECTORY.get_or_init(|| std::env::var_os("KINAKAZE_IO_TRACE_DIR").map(PathBuf::from));
        let Some(directory) = directory else {
            unsafe { SetLastError(error) };
            return Self(None);
        };
        let active = SINK
            .try_with(|slot| {
                let mut slot = slot.try_borrow_mut().ok()?;
                if slot.is_none() {
                    let pid = unsafe { GetCurrentProcessId() };
                    let tid = unsafe { GetCurrentThreadId() };
                    let unix_ns = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()?
                        .as_nanos() as u64;
                    let file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(directory.join(format!("io-{pid}-{tid}-{unix_ns}.ktrace")))
                        .ok()?;
                    *slot = Some(Sink {
                        writer: BufWriter::with_capacity(65536, file),
                        sequence: 0,
                        parent: 0,
                        origin: Instant::now(),
                        unix_ns,
                        pid,
                        tid,
                        failed: 0,
                    });
                }
                let sink = slot.as_mut()?;
                sink.sequence += 1;
                let start = Instant::now();
                let active = Active {
                    sequence: sink.sequence,
                    parent: sink.parent,
                    start,
                    unix_ns: sink.unix_ns + start.duration_since(sink.origin).as_nanos() as u64,
                    operation,
                    path: path.chars().take(2048).collect(),
                    args,
                    result: i64::MIN,
                    truncated: path.chars().count() > 2048,
                };
                sink.parent = active.sequence;
                Some(active)
            })
            .ok()
            .flatten();
        unsafe { SetLastError(error) };
        Self(active)
    }
    pub(crate) fn result(&mut self, result: i64) {
        if let Some(active) = &mut self.0 {
            active.result = result;
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let Some(active) = self.0.take() else { return };
        let elapsed = active.start.elapsed().as_nanos() as u64;
        let error = unsafe { GetLastError() };
        let _ = SINK.try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return;
            };
            let Some(sink) = slot.as_mut() else { return };
            sink.parent = active.parent;
            sink.record(&active, elapsed);
            if active.parent == 0 {
                // A checkpoint proves the preceding complete outer call reached
                // the host file. An interrupted outer call has no final marker.
                sink.sequence += 1;
                let checkpoint = Active {
                    sequence: sink.sequence,
                    parent: 0,
                    start: Instant::now(),
                    unix_ns: active.unix_ns + elapsed,
                    operation: "trace.checkpoint",
                    path: String::new(),
                    args: [0, sink.failed, 0],
                    result: 0,
                    truncated: false,
                };
                sink.record(&checkpoint, 0);
                if sink.writer.flush().is_err() {
                    sink.failed += 1;
                }
            }
        });
        unsafe { SetLastError(error) };
    }
}

impl Sink {
    fn record(&mut self, active: &Active, elapsed: u64) {
        let mut header = [0u8; 88];
        let size = header.len() + active.operation.len() + active.path.len();
        header[..4].copy_from_slice(&(size as u32).to_le_bytes());
        header[4..6].copy_from_slice(&(active.operation.len() as u16).to_le_bytes());
        header[6..8].copy_from_slice(&u16::from(active.truncated).to_le_bytes());
        for (index, value) in [
            active.sequence,
            active.parent,
            active.unix_ns,
            elapsed,
            active.args[0],
            active.args[1],
            active.args[2],
            active.result as u64,
        ]
        .into_iter()
        .enumerate()
        {
            header[8 + index * 8..16 + index * 8].copy_from_slice(&value.to_le_bytes());
        }
        for (index, value) in [self.pid, self.tid, active.path.len() as u32, 1]
            .into_iter()
            .enumerate()
        {
            header[72 + index * 4..76 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        if self
            .writer
            .write_all(&header)
            .and_then(|_| self.writer.write_all(active.operation.as_bytes()))
            .and_then(|_| self.writer.write_all(active.path.as_bytes()))
            .is_err()
        {
            self.failed += 1;
        }
    }
}
