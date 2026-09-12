//! Cross-process hierarchy and named native jobs. Handles are process-local;
//! only stable IDs and configuration are published in the shared store.
use super::*;
use crate::mount::shared::{self, Store};
use crate::state_codec::{Reader, bytes, word};
use std::sync::Arc;

pub(crate) fn store() -> Result<Arc<Store>, i32> {
    static STORE: OnceLock<Arc<Store>> = OnceLock::new();
    if let Some(s) = STORE.get() {
        return Ok(s.clone());
    }
    let s = Arc::new(Store::user_object(u64::MAX - 26, true)?);
    s.update(|old| {
        Ok((
            if old.is_empty() {
                encode(&CgroupRegistry::new()?)
            } else {
                old.to_vec()
            },
            (),
        ))
    })?;
    let _ = STORE.set(s.clone());
    Ok(s)
}
fn encode(reg: &CgroupRegistry) -> Vec<u8> {
    let mut out = b"CRYCG002".to_vec();
    word(&mut out, reg.groups.len() as u64);
    let mut rows: Vec<_> = reg.groups.iter().collect();
    rows.sort_by_key(|(p, _)| *p);
    for (path, n) in rows {
        bytes(&mut out, path.as_bytes());
        word(&mut out, n.id);
        for v in [
            n.cpu_max_quota.unwrap_or(u64::MAX),
            n.cpu_max_period,
            n.cpu_weight as u64,
            n.memory_max.unwrap_or(u64::MAX),
            n.pids_max.map(u64::from).unwrap_or(u64::MAX),
        ] {
            word(&mut out, v);
        }
        bytes(&mut out, n.subtree_control.as_bytes());
    }
    out
}
fn decode(data: &[u8]) -> Result<CgroupRegistry, i32> {
    if data.get(..8) != Some(b"CRYCG002") {
        return Err(crate::EIO);
    }
    let mut r = Reader(&data[8..]);
    let count = r.word()?;
    if count > 100_000 {
        return Err(crate::EIO);
    }
    let mut groups = HashMap::new();
    for _ in 0..count {
        let path = r.text()?;
        let id = r.word()?;
        let optional = |v| if v == u64::MAX { None } else { Some(v) };
        let mut n = CgroupNode {
            id,
            job: None,
            cpu_max_quota: optional(r.word()?),
            cpu_max_period: r.word()?,
            cpu_weight: r.word()? as u32,
            memory_max: optional(r.word()?),
            pids_max: optional(r.word()?).map(|v| v as u32),
            subtree_control: r.text()?,
        };
        let (handle, fresh) = job(id)?;
        n.job = Some(handle);
        if fresh {
            apply(&n)?;
        }
        if groups.insert(path, n).is_some() {
            return Err(crate::EIO);
        }
    }
    r.end()?;
    Ok(CgroupRegistry { groups })
}
pub(super) fn snapshot() -> Result<CgroupRegistry, i32> {
    decode(&store()?.read()?.1)
}
pub(super) fn update<T>(f: impl FnOnce(&mut CgroupRegistry) -> Result<T, i32>) -> Result<T, i32> {
    store()?.update(|old| {
        let mut reg = decode(old)?;
        let result = f(&mut reg)?;
        Ok((encode(&reg), result))
    })
}
pub(super) fn allocate() -> Result<u64, i32> {
    Ok(shared::new_object()?.id())
}

pub(super) fn job(id: u64) -> Result<(HANDLE, bool), i32> {
    static JOBS: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
    let mut jobs = JOBS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| crate::EIO)?;
    if let Some(h) = jobs.get(&id) {
        return Ok((*h as HANDLE, false));
    }
    let name: Vec<u16> = format!(
        "Local\\kinakaze.cgroup.v2.{}.{}",
        kinakaze_runtime::authority::domain_id(),
        id
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let h = unsafe { CreateJobObjectW(core::ptr::null(), name.as_ptr()) };
    if h.is_null() {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let fresh = unsafe { GetLastError() } != 183;
    jobs.insert(id, h as usize);
    Ok((h, fresh))
}
pub(super) fn apply(node: &CgroupNode) -> Result<(), i32> {
    let job = node.job.ok_or(crate::EIO)?;
    let mut previous: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { core::mem::zeroed() };
    let mut previous_cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { core::mem::zeroed() };
    if unsafe {
        QueryInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&mut previous as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            core::mem::size_of_val(&previous) as u32,
            core::ptr::null_mut(),
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    if unsafe {
        QueryInformationJobObject(
            job,
            JobObjectCpuRateControlInformation,
            (&mut previous_cpu as *mut JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
            core::mem::size_of_val(&previous_cpu) as u32,
            core::ptr::null_mut(),
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { core::mem::zeroed() };
    if let Some(bytes) = node.memory_max {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        limits.JobMemoryLimit = bytes as usize;
    }
    if let Some(count) = node.pids_max {
        if count == 0 {
            return Err(crate::EOPNOTSUPP);
        }
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = count;
    }
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            core::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { core::mem::zeroed() };
    // Windows validates the rate even when disabling rate control.
    cpu.Anonymous.CpuRate = 10_000;
    if let Some(quota) = node.cpu_max_quota {
        cpu.ControlFlags =
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        cpu.Anonymous.CpuRate =
            ((quota as u128 * 10_000 / node.cpu_max_period as u128).clamp(1, 10_000)) as u32;
    } else if node.cpu_weight != 100 {
        cpu.ControlFlags =
            JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED;
        cpu.Anonymous.Weight = ((node.cpu_weight as u64 * 9 + 9999) / 10000).clamp(1, 9) as u32;
    }
    if cpu.ControlFlags == 0 && previous_cpu.ControlFlags == 0 {
        return Ok(());
    }
    if unsafe {
        SetInformationJobObject(
            job,
            JobObjectCpuRateControlInformation,
            (&cpu as *const JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
            core::mem::size_of_val(&cpu) as u32,
        )
    } == 0
    {
        let error = crate::errno_from_win32(unsafe { GetLastError() });
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&previous as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                core::mem::size_of_val(&previous) as u32,
            );
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_limits_survive_independent_updates_and_can_be_disabled() {
        let mut node = CgroupNode::new("native-limits-regression").unwrap();
        node.memory_max = Some(64 * 1024 * 1024);
        apply(&node).unwrap(); // An unconfigured CPU limit must not fail this.
        node.pids_max = Some(7);
        apply(&node).unwrap();
        let query = || {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { core::mem::zeroed() };
            assert_ne!(
                unsafe {
                    QueryInformationJobObject(
                        node.job.unwrap(),
                        JobObjectExtendedLimitInformation,
                        (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                        core::mem::size_of_val(&info) as u32,
                        core::ptr::null_mut(),
                    )
                },
                0
            );
            info
        };
        let info = query();
        assert_eq!(info.JobMemoryLimit, 64 * 1024 * 1024);
        assert_eq!(info.BasicLimitInformation.ActiveProcessLimit, 7);
        assert_eq!(
            info.BasicLimitInformation.LimitFlags
                & (JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS),
            JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        );
        node.cpu_max_quota = Some(50_000);
        apply(&node).unwrap();
        node.cpu_max_quota = None;
        node.pids_max = None;
        apply(&node).unwrap();
        let info = query();
        assert_eq!(info.JobMemoryLimit, 64 * 1024 * 1024);
        assert_eq!(
            info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            0
        );
        node.memory_max = None;
        apply(&node).unwrap();
        assert_eq!(
            query().BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY,
            0
        );
    }
}
