//! Current host processor counters, without sampling workers or cached values.
//! Query each Windows processor group explicitly; querying just the calling
//! thread's group loses CPUs on machines with more than 64 logical processors.

use std::ffi::c_void;
use std::fmt::Write as _;
use windows_sys::Win32::Foundation::{GetLastError, RtlNtStatusToDosError};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorCount, GetActiveProcessorGroupCount,
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformationEx(
        class: u32,
        input: *const c_void,
        input_length: u32,
        output: *mut c_void,
        output_length: u32,
        returned: *mut u32,
    ) -> i32;
}

// SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION, in 100 ns units. The reserved
// fields are not interpreted as Linux IRQ/DPC counters.
// https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntquerysysteminformation
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Performance {
    idle: i64,
    kernel: i64,
    user: i64,
    reserved: [i64; 2],
    reserved_count: u32,
}
const _: () = assert!(size_of::<Performance>() == 48);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cpu {
    user: u64,
    system: u64,
    idle: u64,
}

impl TryFrom<Performance> for Cpu {
    type Error = i32;
    fn try_from(raw: Performance) -> Result<Self, i32> {
        let user = u64::try_from(raw.user).map_err(|_| crate::EIO)?;
        let idle = u64::try_from(raw.idle).map_err(|_| crate::EIO)?;
        let kernel = u64::try_from(raw.kernel).map_err(|_| crate::EIO)?;
        // Windows kernel time includes idle; count those ticks exactly once.
        let system = kernel.checked_sub(idle).ok_or(crate::EIO)?;
        Ok(Self { user, system, idle })
    }
}

impl Cpu {
    fn ticks(self) -> Self {
        Self {
            user: self.user / 100_000,
            system: self.system / 100_000,
            idle: self.idle / 100_000,
        }
    }
    fn add(self, other: Self) -> Result<Self, i32> {
        Ok(Self {
            user: self.user.checked_add(other.user).ok_or(crate::EOVERFLOW)?,
            system: self
                .system
                .checked_add(other.system)
                .ok_or(crate::EOVERFLOW)?,
            idle: self.idle.checked_add(other.idle).ok_or(crate::EOVERFLOW)?,
        })
    }
    fn write(self, out: &mut String, name: &str) {
        // Nice, iowait, irq, softirq, steal and guest have no separate source
        // here. Kernel interrupt/DPC work remains included in system time.
        let _ = writeln!(
            out,
            "{name} {} 0 {} {} 0 0 0 0 0 0",
            self.user, self.system, self.idle
        );
    }
}

fn host_error() -> i32 {
    let error = unsafe { GetLastError() };
    if error == 0 {
        crate::EIO
    } else {
        crate::errno_from_win32(error)
    }
}

fn query() -> Result<Vec<Cpu>, i32> {
    // SAFETY: no pointer parameters. These calls only read current topology.
    let groups = unsafe { GetActiveProcessorGroupCount() };
    if groups == 0 {
        return Err(host_error());
    }
    let mut cpus = Vec::new();
    for group in 0..groups {
        let count = unsafe { GetActiveProcessorCount(group) };
        if count == 0 {
            return Err(host_error());
        }
        let bytes = count
            .checked_mul(size_of::<Performance>() as u32)
            .ok_or(crate::EOVERFLOW)?;
        let mut records = vec![Performance::default(); count as usize];
        let mut returned = 0;
        // SystemProcessorPerformanceInformation (8) takes a USHORT group ID
        // in the Ex input buffer. No thread affinity is changed by the query.
        // SAFETY: both buffers are aligned and valid for the declared lengths.
        let status = unsafe {
            NtQuerySystemInformationEx(
                8,
                (&raw const group).cast(),
                size_of::<u16>() as u32,
                records.as_mut_ptr().cast(),
                bytes,
                &raw mut returned,
            )
        };
        if status < 0 {
            return Err(crate::errno_from_win32(unsafe {
                RtlNtStatusToDosError(status)
            }));
        }
        if returned != bytes {
            // Do not turn a partial/hotplugged topology into zero CPU samples.
            return Err(crate::EIO);
        }
        for raw in records {
            cpus.push(Cpu::try_from(raw)?);
        }
    }
    Ok(cpus)
}

fn render(cpus: &[Cpu]) -> Result<String, i32> {
    if cpus.is_empty() {
        return Err(crate::EIO);
    }
    let total = cpus
        .iter()
        .try_fold(Cpu::default(), |sum, cpu| sum.add(cpu.ticks()))?;
    let mut out = String::with_capacity((cpus.len() + 1) * 96);
    total.write(&mut out, "cpu");
    for (index, cpu) in cpus.iter().enumerate() {
        cpu.ticks().write(&mut out, &format!("cpu{index}"));
    }
    Ok(out)
}

pub(super) fn stat() -> Result<String, i32> {
    render(&query()?)
}

pub(super) fn idle_seconds() -> Result<f64, i32> {
    let total = query()?.into_iter().try_fold(0u64, |sum, cpu| {
        sum.checked_add(cpu.idle).ok_or(crate::EOVERFLOW)
    })?;
    Ok(total as f64 / 10_000_000.0)
}

pub(super) fn boot_time() -> Result<i64, i32> {
    let (boot_s, boot_ns) = crate::time_namespace::clock(7)?;
    // Realtime is not virtualized by the monotonic/boottime namespace API.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| crate::EIO)?;
    let nanos = (i128::from(now.as_secs()) - i128::from(boot_s)) * 1_000_000_000
        + i128::from(now.subsec_nanos())
        - i128::from(boot_ns);
    i64::try_from(nanos.div_euclid(1_000_000_000)).map_err(|_| crate::EOVERFLOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asymmetric_processors_keep_their_own_time_and_aggregate_once() {
        let cpus = [
            Cpu::try_from(Performance {
                user: 900_001,
                kernel: 300_002,
                idle: 200_001,
                ..Default::default()
            })
            .unwrap(),
            Cpu::try_from(Performance {
                user: 100_001,
                kernel: 900_002,
                idle: 800_001,
                ..Default::default()
            })
            .unwrap(),
        ];
        assert_eq!(
            render(&cpus).unwrap(),
            concat!(
                "cpu 10 0 2 10 0 0 0 0 0 0\n",
                "cpu0 9 0 1 2 0 0 0 0 0 0\n",
                "cpu1 1 0 1 8 0 0 0 0 0 0\n"
            )
        );
    }

    #[test]
    fn invalid_native_counters_are_errors_not_fabricated_idle_samples() {
        for raw in [
            Performance {
                user: -1,
                ..Default::default()
            },
            Performance {
                idle: -1,
                ..Default::default()
            },
            Performance {
                idle: 2,
                kernel: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(Cpu::try_from(raw), Err(crate::EIO));
        }
        assert_eq!(render(&[]), Err(crate::EIO));
        assert_eq!(
            Cpu {
                user: u64::MAX,
                ..Default::default()
            }
            .add(Cpu {
                user: 1,
                ..Default::default()
            }),
            Err(crate::EOVERFLOW)
        );
    }

    #[test]
    fn live_topology_has_real_counters_and_current_boot_time() {
        let cpus = query().unwrap();
        let count = unsafe { GetActiveProcessorCount(u16::MAX) };
        assert_eq!(cpus.len(), count as usize);
        assert!(cpus.iter().any(|cpu| cpu.user != 0 && cpu.idle != 0));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let elapsed = crate::time_namespace::clock(7).unwrap().0;
        assert!((now - boot_time().unwrap() - elapsed).abs() <= 1);
    }
}
