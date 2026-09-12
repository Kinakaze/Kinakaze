//! Cgroup v2 control interfaces backed by Windows Job Objects.
//!
//! The fixed `/sys/fs/cgroup` view and mountable cgroup2 backend share their
//! hierarchy and configuration. Individual interfaces have different levels of
//! native support; filesystem presence does not imply full controller parity.
//!
//! Windows Job Objects provide native kernel support for:
//! - CPU rate limiting / weight allocation (`cpu.max`, `cpu.weight`, `cpu.stat`)
//! - Native commit bounds and tracking (`memory.max`, `memory.current`, `memory.peak`)
//! - Process limits and tracking (`pids.max`, `pids.current`, `cgroup.procs`)
//! - I/O transfer and operation accounting (`io.stat`)
//! Native nested Job support alone does not supply complete cgroup hierarchy
//! enforcement or process migration semantics.
//! Job commit excludes section-backed guest mappings, so memory usage and limits
//! are not complete Linux memcg accounting. `memory.stat` still has legacy fixed
//! fields and must not be treated as measured memory categories.
//!
//! `cpuset.*` and `memory.swap.*` are explicitly non-enforcing, fixed-default
//! interfaces requested by the user. Writes succeed without changing either
//! their readback or host policy. Their presence satisfies feature discovery,
//! but MUST NOT be interpreted as CPU/NUMA isolation or a swap quota.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectBasicAndIoAccountingInformation,
    JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

use crate::procfs::{ProcKind, ProcMetadata};
use crate::{EEXIST, EINVAL, ENOENT, ENOTDIR, ESRCH};

pub(crate) mod fixed_defaults;
#[cfg(windows)]
mod memory;
#[cfg(all(test, windows))]
mod native_tests;
pub(crate) mod shared;

const CONTROLLERS: &str = "cpu cpuset memory pids io";

/// Internal state of one cgroup node.
#[derive(Debug)]
struct CgroupNode {
    id: u64,
    #[cfg(windows)]
    job: Option<HANDLE>,
    cpu_max_quota: Option<u64>,
    cpu_max_period: u64,
    cpu_weight: u32,
    memory_max: Option<u64>,
    pids_max: Option<u32>,
    subtree_control: String,
}

#[cfg(windows)]
unsafe impl Send for CgroupNode {}
#[cfg(windows)]
unsafe impl Sync for CgroupNode {}

impl CgroupNode {
    fn new(path: &str) -> Result<Self, i32> {
        let id = if path.is_empty() {
            0
        } else {
            shared::allocate()?
        };
        let job = Some(shared::job(id)?.0);
        Ok(Self {
            id,
            job,
            cpu_max_quota: None,
            cpu_max_period: 100_000,
            cpu_weight: 100,
            memory_max: None,
            pids_max: None,
            subtree_control: CONTROLLERS.to_string(),
        })
    }
}

/// Global registry of all active cgroups.
struct CgroupRegistry {
    groups: HashMap<String, CgroupNode>,
}

impl CgroupRegistry {
    fn new() -> Result<Self, i32> {
        let mut groups = HashMap::new();
        // Root cgroup at /sys/fs/cgroup
        groups.insert("".to_string(), CgroupNode::new("")?);
        Ok(Self { groups })
    }
}

/// Returns whether `path` falls inside the synthetic cgroup or sysfs tree.
pub fn owns(path: &str) -> bool {
    let clean = path.trim_end_matches('/');
    clean == "/sys" || clean.starts_with("/sys/")
}

fn is_sysfs_leaf(path: &str) -> bool {
    matches!(
        path,
        "/sys/devices/system/cpu/online"
            | "/sys/devices/system/cpu/possible"
            | "/sys/devices/system/cpu/present"
            | "/sys/devices/system/node/online"
            | "/sys/devices/system/node/possible"
            | "/sys/kernel/mm/transparent_hugepage/enabled"
            | "/sys/kernel/mm/transparent_hugepage/defrag"
    )
}

/// Normalizes a path under `/sys/fs/cgroup` into `(cgroup_rel_dir, filename)`.
fn split_cgroup_path(path: &str) -> Result<(String, Option<String>), i32> {
    let clean = path.trim_end_matches('/');
    let sub = if clean == "/sys/fs/cgroup" {
        ""
    } else if let Some(stripped) = clean.strip_prefix("/sys/fs/cgroup/") {
        stripped
    } else {
        return Err(ENOENT);
    };

    if sub.is_empty() {
        return Ok(("".to_string(), None));
    }

    Ok(if let Some((dir, file)) = sub.rsplit_once('/') {
        if is_cgroup_file(file) {
            (dir.to_string(), Some(file.to_string()))
        } else {
            (sub.to_string(), None)
        }
    } else if is_cgroup_file(sub) {
        ("".to_string(), Some(sub.to_string()))
    } else {
        (sub.to_string(), None)
    })
}

fn is_cgroup_file(name: &str) -> bool {
    fixed_defaults::owns(name)
        || matches!(
            name,
            "cgroup.controllers"
                | "cgroup.subtree_control"
                | "cgroup.procs"
                | "cgroup.threads"
                | "cgroup.stat"
                | "cgroup.type"
                | "cgroup.events"
                | "cgroup.kill"
                | "cgroup.freeze"
                | "cgroup.max.descendants"
                | "cgroup.max.depth"
                | "cpu.max"
                | "cpu.weight"
                | "cpu.weight.nice"
                | "cpu.stat"
                | "memory.max"
                | "memory.high"
                | "memory.low"
                | "memory.min"
                | "memory.current"
                | "memory.peak"
                | "memory.stat"
                | "memory.events"
                | "pids.max"
                | "pids.current"
                | "pids.events"
                | "io.stat"
                | "io.max"
                | "io.weight"
        )
}

/// Collects metadata for a cgroup file or directory.
pub fn metadata(path: &str) -> Result<ProcMetadata, i32> {
    let clean = path.trim_end_matches('/');
    if matches!(
        clean,
        "/sys"
            | "/sys/fs"
            | "/sys/devices"
            | "/sys/devices/system"
            | "/sys/devices/system/cpu"
            | "/sys/devices/system/node"
            | "/sys/class"
            | "/sys/class/net"
            | "/sys/kernel"
            | "/sys/kernel/mm"
            | "/sys/kernel/mm/transparent_hugepage"
    ) {
        return Ok(ProcMetadata {
            kind: ProcKind::Directory,
            size: 0,
            target: None,
        });
    }

    if is_sysfs_leaf(clean) {
        let contents = read_file(path)?;
        return Ok(ProcMetadata {
            kind: ProcKind::File,
            size: contents.len() as u64,
            target: None,
        });
    }

    let (dir, file) = split_cgroup_path(path)?;
    let reg = shared::snapshot()?;

    if !reg.groups.contains_key(&dir) && !dir.is_empty() {
        return Err(ENOENT);
    }

    match file {
        None => Ok(ProcMetadata {
            kind: ProcKind::Directory,
            size: 0,
            target: None,
        }),
        Some(_) => {
            drop(reg);
            let contents = read_file(path)?;
            Ok(ProcMetadata {
                kind: ProcKind::File,
                size: contents.len() as u64,
                target: None,
            })
        }
    }
}

/// Reads the synthesized contents of a cgroup control file.
pub fn read_file(path: &str) -> Result<Vec<u8>, i32> {
    read_from(path, &shared::snapshot()?)
}
/// Read one mounted attribute only if its retained inode still names this group.
/// Checking the ID and reading from the same snapshot prevents a removed/rebuilt
/// path from redirecting an already-open descriptor to the replacement group.
pub(crate) fn read_file_generation(path: &str, generation: u64) -> Result<Vec<u8>, i32> {
    let reg = shared::snapshot()?;
    let (dir, _) = split_cgroup_path(path)?;
    if reg
        .groups
        .get(&dir)
        .is_none_or(|node| node.id != generation)
    {
        return Err(crate::ENODEV);
    }
    read_from(path, &reg)
}
fn read_from(path: &str, reg: &CgroupRegistry) -> Result<Vec<u8>, i32> {
    let clean = path.trim_end_matches('/');
    if is_sysfs_leaf(clean) {
        return match clean {
            "/sys/devices/system/cpu/online"
            | "/sys/devices/system/cpu/possible"
            | "/sys/devices/system/cpu/present" => fixed_defaults::cpu_list(),
            "/sys/devices/system/node/online" | "/sys/devices/system/node/possible" => {
                fixed_defaults::node_list()
            }
            "/sys/kernel/mm/transparent_hugepage/enabled" => {
                Ok(b"[always] madvise never\n".to_vec())
            }
            "/sys/kernel/mm/transparent_hugepage/defrag" => {
                Ok(b"always [madvise] never\n".to_vec())
            }
            _ => Err(ENOENT),
        };
    }

    let (dir, file) = split_cgroup_path(path)?;
    let Some(file_name) = file else {
        return Err(crate::EISDIR);
    };

    let node = reg.groups.get(&dir).ok_or(ENOENT)?;

    if fixed_defaults::owns(&file_name) {
        return fixed_defaults::read(&file_name);
    }

    #[cfg(windows)]
    let job_handle = node.job;
    #[cfg(not(windows))]
    let job_handle: Option<()> = None;

    let text = match file_name.as_str() {
        "cgroup.controllers" => format!("{CONTROLLERS}\n"),
        "cgroup.subtree_control" => format!("{}\n", node.subtree_control),
        "cgroup.type" => "domain\n".to_string(),
        "cgroup.events" => {
            let root = if dir.is_empty() { "/sys/fs/cgroup".into() } else { format!("/sys/fs/cgroup/{dir}") };
            let populated = kinakaze_runtime::job::everyone().iter().any(|p| kinakaze_runtime::job::cgroup_path(p.namespace_pid).is_some_and(|path| path == root || path.starts_with(&format!("{root}/"))));
            format!("populated {}\nfrozen 0\n",u8::from(populated))
        },
        "cgroup.kill" => "0\n".to_string(),
        "cgroup.freeze" => "0\n".to_string(),
        "cgroup.stat" => {
            let count = reg.groups.keys().filter(|p| !p.is_empty() && (dir.is_empty() || p.starts_with(&format!("{dir}/")))).count();
            format!("nr_descendants {count}\nnr_dying_descendants 0\n")
        },
        "cgroup.max.descendants" => "max\n".to_string(),
        "cgroup.max.depth" => "max\n".to_string(),
        "cgroup.procs" | "cgroup.threads" => {
            let group_path = if dir.is_empty() { "/sys/fs/cgroup".to_string() } else { format!("/sys/fs/cgroup/{dir}") };
            // Shared guest membership excludes unrelated native job members and
            // projects PID numbers through the reader's active PID namespace.
            let mut procs: Vec<_> = kinakaze_runtime::job::everyone().into_iter().filter_map(|entry| {
                let path = kinakaze_runtime::job::cgroup_path(entry.namespace_pid)?;
                let path = if path.is_empty() { "/sys/fs/cgroup" } else { &path };
                (path == group_path).then(|| kinakaze_runtime::job::namespaces::visible(entry.namespace_pid)).flatten()
            }).collect();
            procs.sort_unstable();
            procs.dedup();
            let mut out = String::new();
            for pid in procs {
                out.push_str(&format!("{pid}\n"));
            }
            out
        }
        "cpu.max" => match node.cpu_max_quota {
            Some(quota) => format!("{quota} {}\n", node.cpu_max_period),
            None => format!("max {}\n", node.cpu_max_period),
        },
        "cpu.weight" => format!("{}\n", node.cpu_weight),
        "cpu.weight.nice" => "0\n".to_string(),
        "cpu.stat" => {
            let mut usage_usec = 0u64;
            let mut user_usec = 0u64;
            let mut system_usec = 0u64;
            #[cfg(windows)]
            if let Some(job) = job_handle {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { core::mem::zeroed() };
                let mut return_len = 0u32;
                let ok = unsafe {
                    QueryInformationJobObject(
                        job,
                        JobObjectBasicAccountingInformation,
                        (&raw mut info).cast(),
                        core::mem::size_of_val(&info) as u32,
                        &raw mut return_len,
                    )
                };
                if ok != 0 {
                    let total_user = info.TotalUserTime.max(0) as u64;
                    let total_kernel = info.TotalKernelTime.max(0) as u64;
                    user_usec = total_user / 10;
                    system_usec = total_kernel / 10;
                    usage_usec = user_usec + system_usec;
                }
            }
            format!(
                "usage_usec {usage_usec}\nuser_usec {user_usec}\nsystem_usec {system_usec}\nnr_periods 0\nnr_throttled 0\nthrottled_usec 0\n"
            )
        }
        "memory.max" => match node.memory_max {
            Some(max) => format!("{max}\n"),
            None => "max\n".to_string(),
        },
        "memory.high" | "memory.low" | "memory.min" => "0\n".to_string(),
        "memory.current" | "memory.peak" => {
            // Current native commit and its high-water mark are distinct. Job
            // accounting excludes section-backed guest memory; do not call this
            // complete Linux memcg/RSS accounting or synthesize nonzero defaults.
            #[cfg(windows)]
            {
                let usage = memory::query(job_handle.ok_or(crate::EIO)?)?;
                format!("{}\n", if file_name == "memory.current" { usage.current } else { usage.peak })
            }
            #[cfg(not(windows))]
            return Err(crate::EOPNOTSUPP);
        }
        "memory.stat" => {
            "anon 10485760\nfile 4194304\nkernel_stack 1048576\npagetable 524288\npercpu 65536\nsock 32768\nshmem 0\nfile_mapped 2097152\nfile_dirty 0\nfile_writeback 0\ninactive_anon 0\nactive_anon 10485760\ninactive_file 0\nactive_file 4194304\nunevictable 0\nslab_reclaimable 524288\nslab_unreclaimable 524288\npgfault 1024\npgmajfault 0\nworkingset_refault 0\nworkingset_activate 0\nworkingset_nodereclaim 0\npgrefill 0\npgscan 0\npgsteal 0\npgactivate 0\npgdeactivate 0\npglazyfree 0\npglazyfreed 0\nthp_fault_alloc 0\nthp_collapse_alloc 0\n".to_string()
        }
        "memory.events" => "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\n".to_string(),
        "pids.max" => match node.pids_max {
            Some(max) => format!("{max}\n"),
            None => "max\n".to_string(),
        },
        "pids.current" => {
            let mut current = 1u32;
            #[cfg(windows)]
            if let Some(job) = job_handle {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { core::mem::zeroed() };
                let mut return_len = 0u32;
                let ok = unsafe {
                    QueryInformationJobObject(
                        job,
                        JobObjectBasicAccountingInformation,
                        (&raw mut info).cast(),
                        core::mem::size_of_val(&info) as u32,
                        &raw mut return_len,
                    )
                };
                if ok != 0 && info.ActiveProcesses > 0 {
                    current = info.ActiveProcesses;
                }
            }
            format!("{current}\n")
        }
        "pids.events" => "max 0\n".to_string(),
        "io.stat" => {
            let mut rbytes = 0u64;
            let mut wbytes = 0u64;
            let mut rios = 0u64;
            let mut wios = 0u64;
            #[cfg(windows)]
            if let Some(job) = job_handle {
                let mut info: JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION =
                    unsafe { core::mem::zeroed() };
                let mut return_len = 0u32;
                let ok = unsafe {
                    QueryInformationJobObject(
                        job,
                        JobObjectBasicAndIoAccountingInformation,
                        (&raw mut info).cast(),
                        core::mem::size_of_val(&info) as u32,
                        &raw mut return_len,
                    )
                };
                if ok != 0 {
                    rbytes = info.IoInfo.ReadTransferCount;
                    wbytes = info.IoInfo.WriteTransferCount;
                    rios = info.IoInfo.ReadOperationCount;
                    wios = info.IoInfo.WriteOperationCount;
                }
            }
            format!("8:0 rbytes={rbytes} wbytes={wbytes} rios={rios} wios={wios} dbytes=0 dios=0\n")
        }
        "io.max" | "io.weight" => "8:0 rbps=max wbps=max riops=max wiops=max\n".to_string(),
        _ => "\n".to_string(),
    };

    Ok(text.into_bytes())
}

/// Writes configuration to a cgroup control file, applying limits to the Windows Job Object.
pub fn write_file(path: &str, content: &[u8]) -> Result<(), i32> {
    let (dir, file) = split_cgroup_path(path)?;
    let file_name = file.ok_or(crate::EISDIR)?;
    let input = std::str::from_utf8(content).map_err(|_| EINVAL)?.trim();
    shared::update(|reg| {
        let node = reg.groups.get_mut(&dir).ok_or(ENOENT)?;
        if fixed_defaults::owns(&file_name) {
            return Ok(());
        }
        match file_name.as_str() {
            "cgroup.subtree_control" => {
                let mut enabled: std::collections::BTreeSet<_> = node
                    .subtree_control
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                for item in input.split_whitespace() {
                    let (op, name) = item.split_at_checked(1).ok_or(EINVAL)?;
                    if !CONTROLLERS.split_whitespace().any(|v| v == name)
                        || !matches!(op, "+" | "-")
                    {
                        return Err(EINVAL);
                    }
                    if op == "+" {
                        enabled.insert(name.into());
                    } else {
                        enabled.remove(name);
                    }
                }
                node.subtree_control = CONTROLLERS
                    .split_whitespace()
                    .filter(|n| enabled.contains(*n))
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            "cgroup.procs" | "cgroup.threads" => {
                let pid: u32 = input.parse().map_err(|_| EINVAL)?;
                let namespace_pid = if pid == 0 {
                    kinakaze_runtime::job::current_pid()
                } else {
                    kinakaze_runtime::job::namespaces::resolve(pid).ok_or(ESRCH)?
                };
                let process_entry = kinakaze_runtime::job::lookup(namespace_pid).ok_or(ESRCH)?;
                let host_pid = process_entry.pid;
                let desired = if dir.is_empty() {
                    "/sys/fs/cgroup".to_string()
                } else {
                    format!("/sys/fs/cgroup/{dir}")
                };
                if kinakaze_runtime::job::cgroup_path(namespace_pid).as_deref() == Some(&desired) {
                    return Ok(());
                }
                #[cfg(windows)]
                {
                    let job = node.job.ok_or(crate::EIO)?;
                    let process =
                        unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, host_pid) };
                    if process.is_null() || process == INVALID_HANDLE_VALUE {
                        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
                    }
                    let assigned = unsafe { AssignProcessToJobObject(job, process) };
                    let error = if assigned == 0 {
                        Some(crate::errno_from_win32(unsafe { GetLastError() }))
                    } else {
                        None
                    };
                    unsafe { CloseHandle(process) };
                    if let Some(error) = error {
                        return Err(error);
                    }
                }
                let group_path = if dir.is_empty() {
                    "/sys/fs/cgroup".to_string()
                } else {
                    format!("/sys/fs/cgroup/{dir}")
                };
                if !kinakaze_runtime::job::set_cgroup_path(namespace_pid, &group_path) {
                    return Err(crate::EIO);
                }
            }
            "cpu.max" => {
                let parts: Vec<_> = input.split_whitespace().collect();
                if parts.is_empty() || parts.len() > 2 {
                    return Err(EINVAL);
                }
                let period = if parts.len() == 2 {
                    parts[1].parse::<u64>().map_err(|_| EINVAL)?
                } else {
                    node.cpu_max_period
                };
                if !(1000..=1_000_000).contains(&period) {
                    return Err(EINVAL);
                }
                let quota = if parts[0] == "max" {
                    None
                } else {
                    let value = parts[0].parse::<u64>().map_err(|_| EINVAL)?;
                    if value < 1000 {
                        return Err(EINVAL);
                    }
                    Some(value)
                };
                node.cpu_max_quota = quota;
                node.cpu_max_period = period;
                shared::apply(node)?;
            }
            "cpu.weight" => {
                let weight = input.parse::<u32>().map_err(|_| EINVAL)?;
                if !(1..=10_000).contains(&weight) {
                    return Err(EINVAL);
                }
                node.cpu_weight = weight;
                shared::apply(node)?;
            }
            "memory.max" => {
                node.memory_max = if input == "max" {
                    None
                } else {
                    Some(input.parse::<u64>().map_err(|_| EINVAL)?)
                };
                if node.memory_max == Some(0) {
                    return Err(crate::EOPNOTSUPP);
                }
                shared::apply(node)?;
            }
            "pids.max" => {
                node.pids_max = if input == "max" {
                    None
                } else {
                    Some(input.parse::<u32>().map_err(|_| EINVAL)?)
                };
                shared::apply(node)?;
            }
            _ => return Err(crate::EOPNOTSUPP),
        }
        Ok(())
    })
}

/// Creates a new cgroup subdirectory at `path`.
pub fn create_directory(path: &str) -> Result<(), i32> {
    let clean = path.trim_end_matches('/');
    let sub = clean.strip_prefix("/sys/fs/cgroup/").ok_or(EEXIST)?;
    if sub.is_empty() {
        return Err(EEXIST);
    }
    shared::update(|reg| {
        if reg.groups.contains_key(sub) {
            return Err(EEXIST);
        }
        let (parent, name) = sub.rsplit_once('/').unwrap_or(("", sub));
        if !reg.groups.contains_key(parent) {
            return Err(ENOENT);
        }
        if matches!(name, "." | "..") || name.is_empty() || is_cgroup_file(name) {
            return Err(EINVAL);
        }
        let node = CgroupNode::new(sub)?;
        reg.groups.insert(sub.into(), node);
        Ok(())
    })
}
pub fn remove_directory(path: &str) -> Result<(), i32> {
    let clean = path.trim_end_matches('/');
    let sub = clean.strip_prefix("/sys/fs/cgroup/").ok_or(crate::EBUSY)?;
    if sub.is_empty() {
        return Err(crate::EBUSY);
    }
    shared::update(|reg| {
        if !reg.groups.contains_key(sub) {
            return Err(ENOENT);
        }
        if reg.groups.keys().any(|k| k.starts_with(&format!("{sub}/"))) {
            return Err(crate::EBUSY);
        }
        if kinakaze_runtime::job::everyone()
            .iter()
            .any(|p| kinakaze_runtime::job::cgroup_path(p.namespace_pid).as_deref() == Some(clean))
        {
            return Err(crate::EBUSY);
        }
        crate::bpf::remove_cgroup(clean)?;
        reg.groups.remove(sub);
        Ok(())
    })
}
// Directory lookup/stat must not sample every group's counters or depend on an
// unrelated group's native query succeeding. Content is read by the open inode.
pub(crate) fn filesystem_tree() -> Result<Vec<(String, u32, u64)>, i32> {
    filesystem_tree_from(&shared::snapshot()?)
}
fn filesystem_tree_from(reg: &CgroupRegistry) -> Result<Vec<(String, u32, u64)>, i32> {
    let mut tree = Vec::new();
    let files: Vec<_> = list_from("/sys/fs/cgroup", reg)?
        .into_iter()
        .filter(|name| is_cgroup_file(name))
        .collect();
    for (relative, node) in &reg.groups {
        if !relative.is_empty() {
            tree.push((relative.clone(), crate::fs::S_IFDIR | 0o755, node.id));
        }
        for name in &files {
            let key = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            tree.push((
                key,
                crate::fs::S_IFREG | if writable(name) { 0o644 } else { 0o444 },
                node.id,
            ));
        }
    }
    Ok(tree)
}
pub(crate) fn writable(name: &str) -> bool {
    fixed_defaults::owns(name)
        || matches!(
            name,
            "cgroup.subtree_control"
                | "cgroup.procs"
                | "cgroup.threads"
                | "cpu.max"
                | "cpu.weight"
                | "memory.max"
                | "pids.max"
        )
}

/// Lists the contents of a cgroup directory.
pub fn list_directory(path: &str) -> Result<Vec<String>, i32> {
    list_from(path, &shared::snapshot()?)
}
fn list_from(path: &str, reg: &CgroupRegistry) -> Result<Vec<String>, i32> {
    let clean = path.trim_end_matches('/');
    match clean {
        "/sys" => {
            return Ok(vec![
                "fs".to_string(),
                "devices".to_string(),
                "class".to_string(),
                "kernel".to_string(),
            ]);
        }
        "/sys/fs" => return Ok(vec!["cgroup".to_string()]),
        "/sys/devices" => return Ok(vec!["system".to_string()]),
        "/sys/devices/system" => return Ok(vec!["cpu".to_string(), "node".to_string()]),
        "/sys/devices/system/cpu" => {
            return Ok(vec![
                "online".to_string(),
                "possible".to_string(),
                "present".to_string(),
            ]);
        }
        "/sys/devices/system/node" => {
            return Ok(vec!["online".to_string(), "possible".to_string()]);
        }
        "/sys/class" => return Ok(vec!["net".to_string()]),
        "/sys/class/net" => return Ok(vec!["lo".to_string(), "eth0".to_string()]),
        "/sys/kernel" => return Ok(vec!["mm".to_string()]),
        "/sys/kernel/mm" => return Ok(vec!["transparent_hugepage".to_string()]),
        "/sys/kernel/mm/transparent_hugepage" => {
            return Ok(vec!["enabled".to_string(), "defrag".to_string()]);
        }
        _ => {}
    }

    let (dir, file) = split_cgroup_path(path)?;
    if file.is_some() {
        return Err(ENOTDIR);
    }

    if !reg.groups.contains_key(&dir) && !dir.is_empty() {
        return Err(ENOENT);
    }

    let mut entries = vec![
        "cgroup.controllers".to_string(),
        "cgroup.subtree_control".to_string(),
        "cgroup.procs".to_string(),
        "cgroup.threads".to_string(),
        "cgroup.stat".to_string(),
        "cgroup.type".to_string(),
        "cgroup.events".to_string(),
        "cgroup.max.descendants".to_string(),
        "cgroup.max.depth".to_string(),
        "cpu.max".to_string(),
        "cpu.weight".to_string(),
        "cpu.weight.nice".to_string(),
        "cpu.stat".to_string(),
        "memory.max".to_string(),
        "memory.high".to_string(),
        "memory.low".to_string(),
        "memory.min".to_string(),
        "memory.current".to_string(),
        "memory.peak".to_string(),
        "memory.stat".to_string(),
        "memory.events".to_string(),
        "pids.max".to_string(),
        "pids.current".to_string(),
        "pids.events".to_string(),
        "io.stat".to_string(),
        "io.max".to_string(),
        "io.weight".to_string(),
    ];
    entries.extend(fixed_defaults::FILES.iter().map(|name| (*name).to_string()));

    // Find any direct sub-cgroups
    let prefix = if dir.is_empty() {
        "".to_string()
    } else {
        format!("{dir}/")
    };
    for key in reg.groups.keys() {
        if key.starts_with(&prefix) && key != &dir {
            let rest = &key[prefix.len()..];
            if !rest.is_empty() && !rest.contains('/') {
                entries.push(rest.to_string());
            }
        }
    }

    entries.sort_unstable();
    entries.dedup();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_does_not_depend_on_unrelated_native_counters() {
        let mut reg = CgroupRegistry::new().unwrap();
        let mut node = CgroupNode::new("unqueryable").unwrap();
        node.job = None; // A real content query must fail; metadata must survive.
        reg.groups.insert("unqueryable".into(), node);
        let tree = filesystem_tree_from(&reg).unwrap();
        assert!(
            tree.iter()
                .any(|(path, _, _)| path == "unqueryable/memory.current")
        );
        assert_eq!(
            read_from("/sys/fs/cgroup/unqueryable/cpu.max", &reg).unwrap(),
            b"max 100000\n"
        );
        assert_eq!(
            read_from("/sys/fs/cgroup/unqueryable/memory.current", &reg),
            Err(crate::EIO)
        );
    }

    #[test]
    fn cgroup_root_exposes_standard_controllers() {
        assert!(owns("/sys/fs/cgroup"));
        assert!(owns("/sys/fs/cgroup/cgroup.controllers"));
        assert!(owns("/sys/fs/cgroup/cpu.max"));

        let controllers = read_file("/sys/fs/cgroup/cgroup.controllers").unwrap();
        assert_eq!(
            std::str::from_utf8(&controllers).unwrap(),
            "cpu cpuset memory pids io\n"
        );
    }

    #[test]
    fn cpuset_and_swap_interfaces_keep_defaults_after_writes() {
        let dir = "/sys/fs/cgroup/fixed-defaults-test";
        create_directory(dir).unwrap();
        for root in ["/sys/fs/cgroup", dir] {
            let entries = list_directory(root).unwrap();
            for name in fixed_defaults::FILES {
                let path = format!("{root}/{name}");
                assert!(entries.iter().any(|entry| entry == name));
                assert!(matches!(metadata(&path).unwrap().kind, ProcKind::File));
                let before = read_file(&path).unwrap();
                write_file(&path, b"0").unwrap();
                assert_eq!(read_file(&path).unwrap(), before, "{path}");
                write_file(&path, b"max").unwrap();
                assert_eq!(read_file(&path).unwrap(), before, "{path}");
            }
            assert_eq!(
                read_file(&format!("{root}/memory.swap.max")).unwrap(),
                b"max\n"
            );
            assert_eq!(
                read_file(&format!("{root}/memory.swap.current")).unwrap(),
                b"0\n"
            );
            assert_eq!(
                read_file(&format!("{root}/cpuset.cpus.effective")).unwrap(),
                read_file("/sys/devices/system/cpu/online").unwrap()
            );
            assert_eq!(
                read_file(&format!("{root}/cpuset.mems.effective")).unwrap(),
                read_file("/sys/devices/system/node/online").unwrap()
            );
        }
        remove_directory(dir).unwrap();
        assert!(matches!(
            read_file(&format!("{dir}/memory.swap.max")),
            Err(ENOENT)
        ));
    }

    #[test]
    fn cgroup_cpu_and_memory_round_trip() {
        write_file("/sys/fs/cgroup/cpu.max", b"50000 100000").unwrap();
        let cpu_max = read_file("/sys/fs/cgroup/cpu.max").unwrap();
        assert_eq!(std::str::from_utf8(&cpu_max).unwrap(), "50000 100000\n");

        write_file("/sys/fs/cgroup/memory.max", b"1073741824").unwrap();
        let mem_max = read_file("/sys/fs/cgroup/memory.max").unwrap();
        assert_eq!(std::str::from_utf8(&mem_max).unwrap(), "1073741824\n");
    }

    #[test]
    fn fixed_defaults_round_trip_through_file_descriptors() {
        for name in fixed_defaults::FILES {
            let path = format!("/sys/fs/cgroup/{name}");
            let expected = read_file(&path).unwrap();
            let fd = crate::fs::open(&path, crate::fs::O_RDWR, 0).unwrap();
            assert_eq!(crate::write(fd, b"0\n"), Ok(2));
            assert_eq!(crate::fs::lseek(fd, 0, 0), Ok(0));
            let mut bytes = [0u8; 4096];
            let count = crate::read(fd, &mut bytes).unwrap();
            assert_eq!(&bytes[..count], expected, "{path}");
            crate::close(fd).unwrap();
        }
    }

    #[test]
    fn cgroup_subgroup_creation_and_removal() {
        create_directory("/sys/fs/cgroup/docker-test").unwrap();
        assert!(owns("/sys/fs/cgroup/docker-test/cgroup.procs"));

        let list = list_directory("/sys/fs/cgroup").unwrap();
        assert!(list.contains(&"docker-test".to_string()));

        remove_directory("/sys/fs/cgroup/docker-test").unwrap();
        let list_after = list_directory("/sys/fs/cgroup").unwrap();
        assert!(!list_after.contains(&"docker-test".to_string()));
    }
}
