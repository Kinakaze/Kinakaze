//! Optional startup phase timings; never write diagnostics to guest stdio.
use std::{fmt, io::Write, path::PathBuf, sync::OnceLock, time::Instant};

pub(crate) struct Span {
    start: Instant,
    phase: &'static str,
    name: String,
    directory: &'static PathBuf,
    cpu: Cpu,
}
#[derive(Default)]
struct Cpu {
    user: u64,
    kernel: u64,
    cycles: u64,
}

#[cfg(windows)]
fn cpu() -> Cpu {
    use windows_sys::Win32::{
        Foundation::{FILETIME, HANDLE},
        System::Threading::{GetCurrentThread, GetThreadTimes},
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
    }
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let mut value = Cpu::default();
    unsafe {
        if GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) != 0
        {
            value.user = (u64::from(user.dwHighDateTime) << 32) | u64::from(user.dwLowDateTime);
            value.kernel =
                (u64::from(kernel.dwHighDateTime) << 32) | u64::from(kernel.dwLowDateTime);
        }
        QueryThreadCycleTime(GetCurrentThread(), &mut value.cycles);
    }
    value
}

#[cfg(not(windows))]
fn cpu() -> Cpu {
    Cpu::default()
}

pub(crate) fn begin(phase: &'static str, name: impl fmt::Display) -> Option<Span> {
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
        name: name.to_string(),
        directory,
        cpu: cpu(),
    })
}
impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_micros();
        let cpu = cpu();
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
            self.directory
                .join(format!("loader-{}.log", std::process::id())),
        ) {
            let _ = writeln!(
                file,
                "{} elapsed_us={} thread_user_us={} thread_kernel_us={} thread_cycles={} object={}",
                self.phase,
                elapsed,
                cpu.user.saturating_sub(self.cpu.user) / 10,
                cpu.kernel.saturating_sub(self.cpu.kernel) / 10,
                cpu.cycles.saturating_sub(self.cpu.cycles),
                self.name
            );
        }
    }
}
