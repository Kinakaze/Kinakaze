//! Opt-in startup timings. Disabled measurements neither query CPU clocks nor write files.
use std::{
    io::Write,
    path::PathBuf,
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetProcessTimes, GetThreadTimes,
    },
};

#[derive(Clone, Copy, Default)]
struct Cpu {
    thread: u64,
    process: u64,
    cycles: u64,
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn cpu() -> Cpu {
    let (mut created, mut ended, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let mut result = Cpu::default();
    // SAFETY: pseudo-handles identify this process/thread; all output slots are writable.
    unsafe {
        if GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut ended,
            &mut kernel,
            &mut user,
        ) != 0
        {
            result.thread = ticks(kernel) + ticks(user);
        }
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut ended,
            &mut kernel,
            &mut user,
        ) != 0
        {
            result.process = ticks(kernel) + ticks(user);
        }
        QueryThreadCycleTime(GetCurrentThread(), &mut result.cycles);
    }
    result
}

pub struct StartupSpan {
    start: Instant,
    timestamp_us: u128,
    cpu: Cpu,
    phase: &'static str,
    directory: &'static PathBuf,
}

impl StartupSpan {
    pub fn begin(phase: &'static str) -> Option<Self> {
        static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
        let directory = DIRECTORY
            .get_or_init(|| {
                let path = PathBuf::from(std::env::var_os("KINAKAZE_STARTUP_PROFILE")?);
                (path.is_absolute() && path.is_dir()).then_some(path)
            })
            .as_ref()?;
        Some(Self {
            start: Instant::now(),
            timestamp_us: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_micros(),
            cpu: cpu(),
            phase,
            directory,
        })
    }
}

impl Drop for StartupSpan {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_micros();
        let cpu = cpu();
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
            self.directory
                .join(format!("startup-{}.log", std::process::id())),
        ) {
            let _ = writeln!(
                file,
                "phase={} start_us={} wall_us={} thread_cpu_us={} process_cpu_us={} thread_cycles={} tid={}",
                self.phase,
                self.timestamp_us,
                elapsed,
                cpu.thread.saturating_sub(self.cpu.thread) / 10,
                cpu.process.saturating_sub(self.cpu.process) / 10,
                cpu.cycles.saturating_sub(self.cpu.cycles),
                unsafe { GetCurrentThreadId() }
            );
        }
    }
}
