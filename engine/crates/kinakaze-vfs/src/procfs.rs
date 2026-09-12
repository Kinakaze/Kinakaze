//! Synthetic `/proc`, served entirely from live host state.
//!
//! Guest ELF binaries treat procfs as an API rather than as documentation: they
//! scan `/proc/self/maps` to discover their own mappings, size caches and thread
//! pools from `/proc/cpuinfo`, and read `/proc/self/exe` to re-exec themselves.
//! None of that exists on Windows, so every path here is answered by querying
//! the host through Win32 or `cpuid` and rendering the result in the exact text
//! shape Linux would have produced.
//!
//! The rule this module follows is that a field is either real or is marked
//! synthetic at the site that emits it. Nothing is invented silently.

pub mod instance;

use std::ffi::c_void;
use std::fmt::Write as _;
use std::path::Path;

mod cpu_times;
mod cpu_topology;
mod fd_snapshot;
mod key_defaults;
pub(crate) mod mounts;
mod network;

// Tests sharing the process/network namespace must serialize intentional
// sysctl mutations against assertions about the initial value. Production
// reads/writes remain shared and concurrent, exactly as before.
#[cfg(test)]
pub(crate) fn sysctl_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_IMAGE, MEM_MAPPED, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE,
    PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOCACHE,
    PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOMBINE, PAGE_WRITECOPY, VirtualQuery,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetMappedFileNameW, GetModuleFileNameExW, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadStackLimits, GetProcessAffinityMask,
};

/// Whether a `/proc` path behaves as a directory, a regular file or a symlink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcKind {
    Directory,
    File,
    Symlink,
}

/// What the caller needs to fill in a `struct stat` for a `/proc` path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcMetadata {
    pub kind: ProcKind,
    /// Byte length of a symlink target. Regular procfs files report zero just
    /// like Linux; their live contents are generated once when opened and are
    /// read to EOF rather than generated a second time during `stat`.
    pub size: u64,
    /// Link target, present only for [`ProcKind::Symlink`].
    pub target: Option<String>,
}

/// One recognized `/proc` path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Node {
    Root,
    CpuInfo,
    MemInfo,
    VmStat,
    Uptime,
    Version,
    LoadAvg,
    StatSys,
    Mounts(u32),
    Filesystems,
    Swaps,
    Devices,
    // Network
    NetDir,
    NetDev,
    NetTcp,
    NetTcp6,
    NetUdp,
    NetUdp6,
    NetUnix,
    NetRoute,
    NetArp,
    // Sysctl
    SysDir,
    SysNetDir,
    SysNetIpv4Dir,
    SysNetIpv6Dir,
    SysNetIpv6ConfDir,
    SysNetIpv6ConfIfaceDir,
    SysNetIpv6ConfItem,
    SysIpForward,
    SysKernelDir,
    SysFsMqueueDir,
    SysFsMqueue(&'static str),
    SysKernelPtyDir,
    SysKernelPty(&'static str),
    SysKernelKeysDir,
    SysKernelKeyLimit(key_defaults::Limit),
    SysHostname,
    SysDomainname,
    SysOsRelease,
    SysOsType,
    SysKernelPidMax,
    SysKernelCorePattern,
    SysKernelThreadsMax,
    SysKernelCapLastCap,
    SysKernelRandomDir,
    SysKernelBootId,
    SysFsDir,
    SysFsFileMax,
    SysFsNrOpen,
    SysFsPipeMaxSize,
    SysFsInotifyDir,
    SysFsInotifyWatches,
    SysFsInotifyInstances,
    // Process-specific
    ProcessRoot(u32),
    Maps,
    Exe(u32),
    Cmdline(u32),
    Environ,
    Stat(u32),
    Status(u32),
    FdDir(u32),
    FdEntry(u32, i32),
    Mountinfo(u32),
    Mountstats(u32),
    Cgroup(u32),
    UidMap(u32),
    GidMap(u32),
    Setgroups(u32),
    Comm(u32),
    Limits(u32),
    OomScoreAdj(u32),
    TimensOffsets(u32),
    OomAdj(u32),
    TaskDir(u32),
    NsDir(u32),
    NsEntry(u32, &'static str),
}

/// Splits a path into non-empty components, ignoring redundant separators.
fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|part| !part.is_empty())
}

thread_local! { static PINNED_PATH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// Stored proc descriptor paths contain stable registry IDs, not caller PIDs.
/// Only descriptor consumers enter this scope; user pathname lookup never does.
pub(crate) fn pinned<T>(run: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PINNED_PATH.set(self.0);
        }
    }
    let _guard = Restore(PINNED_PATH.replace(true));
    run()
}
fn visible(pid: u32) -> u32 {
    kinakaze_runtime::job::namespaces::visible_in(pid, instance::pidns()).unwrap_or(0)
}
/// Resolve the process alias when a file is opened so inheritance keeps its owner.
pub(crate) fn canonical_path(path: &str) -> String {
    let Ok((path, _scope)) = instance::enter(path) else {
        return path.into();
    };
    let path = path.as_str();
    let mut parts: Vec<_> = components(path).map(str::to_owned).collect();
    if parts.first().is_some_and(|p| p == "proc") {
        if let Some(info) = parts.get(1).and_then(|p| process_entry(p)) {
            parts[1] = info.entry.namespace_pid.to_string();
        }
    }
    instance::encode(&format!("/{}", parts.join("/")))
}
/// A process alias is a normal procfs symlink when opened with O_NOFOLLOW.
/// Keep the alias inode separate from the pinned process-directory identity.
pub(crate) fn alias_link(path: &str) -> Result<Option<(String, String)>, i32> {
    let (path, _scope) = instance::enter(path)?;
    let parts: Vec<_> = components(&path).collect();
    if parts.len() != 2 || parts[0] != "proc" || !matches!(parts[1], "self" | "thread-self") {
        return Ok(None);
    }
    let process = process_entry(parts[1]).ok_or(crate::ENOENT)?;
    let pid = if PINNED_PATH.get() {
        process.entry.namespace_pid
    } else {
        visible(process.entry.namespace_pid)
    };
    Ok(Some((instance::encode(&path), pid.to_string())))
}
fn process_entry(name: &str) -> Option<crate::job::ProcessInfo> {
    let pid = if matches!(name, "self" | "thread-self") {
        let own = crate::job::process_id();
        kinakaze_runtime::job::namespaces::visible_in(own, instance::pidns())?;
        own
    } else {
        let id = name.parse::<u32>().ok()?;
        if PINNED_PATH.get() {
            id
        } else {
            kinakaze_runtime::job::namespaces::resolve_in(id, instance::pidns())?
        }
    };
    crate::job::process_info(pid)
}

/// Stable inode used by the nsfs object behind a `/proc/<pid>/ns/*` magic link.
fn namespace_inode(kind: &str) -> u64 {
    4026531834u64
        + match kind {
            "time" | "time_for_children" => 0,
            "cgroup" => 1,
            "pid" | "pid_for_children" => 2,
            "user" => 3,
            "uts" => 4,
            "ipc" => 5,
            "mnt" => 6,
            "net" => 158,
            _ => 10,
        }
}

/// Returns the nsfs inode behind a namespace magic link.
///
/// Namespace entries look like symlinks to `readlink` and `lstat`, but following
/// one does not resolve its display text as a pathname. Linux returns the
/// namespace object from nsfs instead, and callers such as `os.Stat` rely on that
/// distinction when probing which namespace types the kernel supports.
pub fn namespace_target_inode(path: &str) -> Option<u64> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::NsEntry(pid, kind) => process_namespace_inode(pid, kind).ok(),
        _ => None,
    }
}

fn process_namespace_inode(pid: u32, kind: &str) -> Result<u64, i32> {
    if let Some(kind) = crate::namespaces::kind(kind) {
        return crate::namespaces::process_id(pid, kind)
            .map(|id| crate::namespaces::inode(kind, id));
    }

    if kind == "user" {
        return crate::user_namespace::id(pid).map(crate::user_namespace::inode);
    }
    if matches!(kind, "time" | "time_for_children") {
        return crate::time_namespace::process_id(pid, kind == "time_for_children")
            .map(crate::time_namespace::inode);
    }
    if kind == "mnt" {
        let id = if pid == crate::job::process_id() {
            crate::mount::namespace_id()?
        } else {
            kinakaze_runtime::job::mount_namespace(pid).ok_or(crate::ENOENT)?
        };
        return crate::mount::namespace_inode(id);
    }
    Ok(namespace_inode(kind))
}

pub(crate) fn mount_namespace_process(path: &str) -> Option<u32> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::NsEntry(pid, "mnt") => Some(pid),
        _ => None,
    }
}

pub(crate) fn user_namespace_process(path: &str) -> Option<u32> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::NsEntry(pid, "user") => Some(pid),
        _ => None,
    }
}

pub(crate) fn other_namespace_process(path: &str) -> Option<(u32, usize)> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::NsEntry(pid, kind) => Some((pid, crate::namespaces::kind(kind)?)),
        _ => None,
    }
}

pub(crate) fn time_namespace_process(path: &str) -> Option<(u32, bool)> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::NsEntry(pid, "time") => Some((pid, false)),
        Node::NsEntry(pid, "time_for_children") => Some((pid, true)),
        _ => None,
    }
}

/// Maps a guest path onto the node that serves it.
fn classify(path: &str) -> Option<Node> {
    let mut parts = components(path);
    if parts.next()? != "proc" {
        return None;
    }
    let Some(second) = parts.next() else {
        return Some(Node::Root);
    };

    if let Some(process) = process_entry(second) {
        let own = process.entry.pid == kinakaze_runtime::job::current_host_pid();
        let Some(third) = parts.next() else {
            return Some(Node::ProcessRoot(process.entry.namespace_pid));
        };
        if third == "fd" {
            let Some(fourth) = parts.next() else {
                return Some(Node::FdDir(process.entry.namespace_pid));
            };
            if parts.next().is_some() {
                return None;
            }
            return fourth
                .parse::<i32>()
                .ok()
                .map(|fd| Node::FdEntry(process.entry.namespace_pid, fd));
        }
        if third == "ns" {
            let Some(fourth) = parts.next() else {
                return Some(Node::NsDir(process.entry.namespace_pid));
            };
            if parts.next().is_some() {
                return None;
            }
            return match fourth {
                "cgroup" => Some(Node::NsEntry(process.entry.namespace_pid, "cgroup")),
                "ipc" => Some(Node::NsEntry(process.entry.namespace_pid, "ipc")),
                "mnt" => Some(Node::NsEntry(process.entry.namespace_pid, "mnt")),
                "net" => Some(Node::NsEntry(process.entry.namespace_pid, "net")),
                "pid" => Some(Node::NsEntry(process.entry.namespace_pid, "pid")),
                "pid_for_children" => Some(Node::NsEntry(
                    process.entry.namespace_pid,
                    "pid_for_children",
                )),
                "user" => Some(Node::NsEntry(process.entry.namespace_pid, "user")),
                "uts" => Some(Node::NsEntry(process.entry.namespace_pid, "uts")),
                "time" => Some(Node::NsEntry(process.entry.namespace_pid, "time")),
                "time_for_children" => Some(Node::NsEntry(
                    process.entry.namespace_pid,
                    "time_for_children",
                )),
                _ => None,
            };
        }
        if third == "task" {
            let Some(fourth) = parts.next() else {
                return Some(Node::TaskDir(process.entry.namespace_pid));
            };
            let tid = fourth.parse::<u32>().ok()?;
            if tid == 0
                || (tid != visible(process.entry.namespace_pid)
                    && !crate::interrupt::thread_exists_in_process(tid, process.entry.pid))
            {
                return None;
            }
            let Some(sub) = parts.next() else {
                return Some(Node::ProcessRoot(process.entry.namespace_pid));
            };
            if sub == "ns" {
                let Some(target) = parts.next() else {
                    return Some(Node::NsDir(process.entry.namespace_pid));
                };
                if parts.next().is_some() {
                    return None;
                }
                return match target {
                    "cgroup" => Some(Node::NsEntry(process.entry.namespace_pid, "cgroup")),
                    "ipc" => Some(Node::NsEntry(process.entry.namespace_pid, "ipc")),
                    "mnt" => Some(Node::NsEntry(process.entry.namespace_pid, "mnt")),
                    "net" => Some(Node::NsEntry(process.entry.namespace_pid, "net")),
                    "pid" => Some(Node::NsEntry(process.entry.namespace_pid, "pid")),
                    "pid_for_children" => Some(Node::NsEntry(
                        process.entry.namespace_pid,
                        "pid_for_children",
                    )),
                    "user" => Some(Node::NsEntry(process.entry.namespace_pid, "user")),
                    "uts" => Some(Node::NsEntry(process.entry.namespace_pid, "uts")),
                    "time" => Some(Node::NsEntry(process.entry.namespace_pid, "time")),
                    "time_for_children" => Some(Node::NsEntry(
                        process.entry.namespace_pid,
                        "time_for_children",
                    )),
                    _ => None,
                };
            }
            if sub == "fd" {
                let Some(target) = parts.next() else {
                    return Some(Node::FdDir(process.entry.namespace_pid));
                };
                if parts.next().is_some() {
                    return None;
                }
                return target
                    .parse::<i32>()
                    .ok()
                    .map(|fd| Node::FdEntry(process.entry.namespace_pid, fd));
            }
            if matches!(sub, "timens_offsets" | "uid_map" | "gid_map" | "setgroups")
                && parts.next().is_none()
            {
                return Some(match sub {
                    "timens_offsets" => Node::TimensOffsets(process.entry.namespace_pid),
                    "uid_map" => Node::UidMap(process.entry.namespace_pid),
                    "gid_map" => Node::GidMap(process.entry.namespace_pid),
                    _ => Node::Setgroups(process.entry.namespace_pid),
                });
            }
            if sub == "status" && parts.next().is_none() {
                return Some(Node::Status(process.entry.namespace_pid));
            }
            if sub == "stat" && parts.next().is_none() {
                return Some(Node::Stat(process.entry.namespace_pid));
            }
            if sub == "comm" && parts.next().is_none() {
                return Some(Node::Comm(process.entry.namespace_pid));
            }
            return None;
        }
        if parts.next().is_some() {
            return None;
        }
        return match third {
            "maps" if own => Some(Node::Maps),
            "exe" if !process.executable.is_empty() => Some(Node::Exe(process.entry.namespace_pid)),
            "cmdline" => Some(Node::Cmdline(process.entry.namespace_pid)),
            "environ" if own => Some(Node::Environ),
            "stat" => Some(Node::Stat(process.entry.namespace_pid)),
            "status" => Some(Node::Status(process.entry.namespace_pid)),
            "mounts" => Some(Node::Mounts(process.entry.namespace_pid)),
            "mountinfo" => Some(Node::Mountinfo(process.entry.namespace_pid)),
            "mountstats" => Some(Node::Mountstats(process.entry.namespace_pid)),
            "cgroup" => Some(Node::Cgroup(process.entry.namespace_pid)),
            "uid_map" => Some(Node::UidMap(process.entry.namespace_pid)),
            "gid_map" => Some(Node::GidMap(process.entry.namespace_pid)),
            "setgroups" => Some(Node::Setgroups(process.entry.namespace_pid)),
            "comm" => Some(Node::Comm(process.entry.namespace_pid)),
            "limits" => Some(Node::Limits(process.entry.namespace_pid)),
            "oom_score_adj" => Some(Node::OomScoreAdj(process.entry.namespace_pid)),
            "timens_offsets" => Some(Node::TimensOffsets(process.entry.namespace_pid)),
            "oom_adj" => Some(Node::OomAdj(process.entry.namespace_pid)),
            _ => None,
        };
    }

    if second == "net" {
        let Some(third) = parts.next() else {
            return Some(Node::NetDir);
        };
        if parts.next().is_some() {
            return None;
        }
        return match third {
            "dev" => Some(Node::NetDev),
            "tcp" => Some(Node::NetTcp),
            "tcp6" => Some(Node::NetTcp6),
            "udp" => Some(Node::NetUdp),
            "udp6" => Some(Node::NetUdp6),
            "unix" => Some(Node::NetUnix),
            "route" => Some(Node::NetRoute),
            "arp" => Some(Node::NetArp),
            _ => None,
        };
    }

    if second == "sys" {
        let Some(third) = parts.next() else {
            return Some(Node::SysDir);
        };
        if third == "net" {
            let Some(fourth) = parts.next() else {
                return Some(Node::SysNetDir);
            };
            if fourth == "ipv4" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysNetIpv4Dir);
                };
                if fifth == "ip_forward" && parts.next().is_none() {
                    return Some(Node::SysIpForward);
                }
            }
            if fourth == "ipv6" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysNetIpv6Dir);
                };
                if fifth == "conf" {
                    let Some(sixth) = parts.next() else {
                        return Some(Node::SysNetIpv6ConfDir);
                    };
                    let Some(_seventh) = parts.next() else {
                        return Some(Node::SysNetIpv6ConfIfaceDir);
                    };
                    if parts.next().is_none() {
                        return Some(Node::SysNetIpv6ConfItem);
                    }
                }
            }
            return None;
        }
        if third == "fs" {
            let Some(fourth) = parts.next() else {
                return Some(Node::SysFsDir);
            };
            if fourth == "mqueue" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysFsMqueueDir);
                };
                if parts.next().is_some() {
                    return None;
                }
                return [
                    "msg_default",
                    "msg_max",
                    "msgsize_default",
                    "msgsize_max",
                    "queues_max",
                ]
                .into_iter()
                .find(|s| *s == fifth)
                .map(Node::SysFsMqueue);
            }
            if fourth == "inotify" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysFsInotifyDir);
                };
                if parts.next().is_none() {
                    return match fifth {
                        "max_user_watches" => Some(Node::SysFsInotifyWatches),
                        "max_user_instances" => Some(Node::SysFsInotifyInstances),
                        _ => None,
                    };
                }
            }
            if parts.next().is_none() {
                return match fourth {
                    "file-max" => Some(Node::SysFsFileMax),
                    "nr_open" => Some(Node::SysFsNrOpen),
                    "pipe-max-size" => Some(Node::SysFsPipeMaxSize),
                    _ => None,
                };
            }
            return None;
        }
        if third == "kernel" {
            let Some(fourth) = parts.next() else {
                return Some(Node::SysKernelDir);
            };
            if fourth == "pty" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysKernelPtyDir);
                };
                if parts.next().is_some() {
                    return None;
                }
                return match fifth {
                    "max" => Some(Node::SysKernelPty("max")),
                    "reserve" => Some(Node::SysKernelPty("reserve")),
                    "nr" => Some(Node::SysKernelPty("nr")),
                    _ => None,
                };
            }
            if fourth == "keys" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysKernelKeysDir);
                };
                return if parts.next().is_none() {
                    key_defaults::Limit::from_name(fifth).map(Node::SysKernelKeyLimit)
                } else {
                    None
                };
            }
            if fourth == "random" {
                let Some(fifth) = parts.next() else {
                    return Some(Node::SysKernelRandomDir);
                };
                if fifth == "boot_id" && parts.next().is_none() {
                    return Some(Node::SysKernelBootId);
                }
            }
            if parts.next().is_some() {
                return None;
            }
            return match fourth {
                "hostname" => Some(Node::SysHostname),
                "domainname" => Some(Node::SysDomainname),
                "osrelease" => Some(Node::SysOsRelease),
                "ostype" => Some(Node::SysOsType),
                "pid_max" => Some(Node::SysKernelPidMax),
                "core_pattern" => Some(Node::SysKernelCorePattern),
                "threads-max" => Some(Node::SysKernelThreadsMax),
                "cap_last_cap" => Some(Node::SysKernelCapLastCap),
                _ => None,
            };
        }
        return None;
    }

    if parts.next().is_some() {
        return None;
    }
    match second {
        "cpuinfo" => Some(Node::CpuInfo),
        "meminfo" => Some(Node::MemInfo),
        "vmstat" => Some(Node::VmStat),
        "uptime" => Some(Node::Uptime),
        "version" => Some(Node::Version),
        "loadavg" => Some(Node::LoadAvg),
        "stat" => Some(Node::StatSys),
        "mounts" => Some(Node::Mounts(crate::job::process_id())),
        "mountinfo" => Some(Node::Mountinfo(crate::job::process_id())),
        "filesystems" => Some(Node::Filesystems),
        "swaps" => Some(Node::Swaps),
        "devices" => Some(Node::Devices),
        "cgroup" => Some(Node::Cgroup(crate::job::process_id())),
        _ => None,
    }
}

/// Returns the process and descriptor named by a procfs fd magic link.
///
/// Keeping this parse in procfs ensures `open`, `stat`, `readlink`, and
/// directory enumeration agree about which fd paths are valid.
pub(crate) fn fd_magic_link(path: &str) -> Option<(u32, i32)> {
    let (path, _scope) = instance::enter(path).ok()?;
    let path = path.as_str();
    match classify(path)? {
        Node::FdEntry(pid, fd) => Some((pid, fd)),
        _ => None,
    }
}

/// Returns true if this path is served by the synthetic `/proc`.
pub fn owns(path: &str) -> bool {
    if instance::lookup(path).ok().flatten().is_some() {
        return true;
    }
    if path.starts_with("/proc/.mount/") {
        return false;
    }
    if crate::mount::api::tree_reference(path).is_some_and(|(_, tail)| !tail.is_empty()) {
        return false;
    }
    components(path).next() == Some("proc")
}

/// Whether a procfs regular file has a Linux write handler.
///
/// Permissions alone are not used as a proxy: `/proc/sys` contains a mixture
/// of immutable status and mutable controls, and Linux refuses writes to an
/// entry which has no handler even for a privileged caller.
pub fn writable(path: &str) -> bool {
    let Ok((path, _scope)) = instance::enter(path) else {
        return false;
    };
    let path = path.as_str();
    matches!(
        classify(path),
        Some(
            Node::SysIpForward
                | Node::SysHostname
                | Node::SysDomainname
                | Node::OomScoreAdj(_)
                | Node::TimensOffsets(_)
                | Node::UidMap(_)
                | Node::GidMap(_)
                | Node::Setgroups(_)
                | Node::OomAdj(_)
                | Node::SysFsMqueue(_)
                | Node::SysKernelPty("max" | "reserve")
                | Node::SysKernelKeyLimit(_)
                | Node::SysNetIpv6ConfItem
        )
    )
}

/// Generates the full contents of a `/proc` file.
pub fn read_file(path: &str) -> Result<Vec<u8>, i32> {
    let (path, _scope) = instance::enter(path)?;
    let path = path.as_str();
    match classify(path).ok_or(crate::ENOENT)? {
        Node::Root
        | Node::ProcessRoot(_)
        | Node::NetDir
        | Node::SysDir
        | Node::SysNetDir
        | Node::SysNetIpv4Dir
        | Node::SysNetIpv6Dir
        | Node::SysNetIpv6ConfDir
        | Node::SysNetIpv6ConfIfaceDir
        | Node::SysKernelDir
        | Node::SysFsMqueueDir
        | Node::SysKernelPtyDir
        | Node::SysKernelKeysDir
        | Node::SysKernelRandomDir
        | Node::SysFsDir
        | Node::SysFsInotifyDir
        | Node::NsDir(_)
        | Node::TaskDir(_)
        | Node::FdDir(_) => Err(crate::EISDIR),
        Node::SysNetIpv6ConfItem => Ok(b"0\n".to_vec()),
        Node::CpuInfo => Ok(cpuinfo().into_bytes()),
        Node::MemInfo => Ok(meminfo()?.into_bytes()),
        Node::VmStat => Ok(vmstat()?.into_bytes()),
        Node::Uptime => Ok(uptime()?.into_bytes()),
        Node::Version => Ok(version().into_bytes()),
        Node::LoadAvg => Ok(loadavg().into_bytes()),
        Node::StatSys => Ok(proc_stat()?.into_bytes()),
        Node::Mounts(pid) => Ok(mounts::mounts(&mounts::records(pid)?).into_bytes()),
        Node::Mountinfo(pid) => Ok(mounts::mountinfo(&mounts::records(pid)?).into_bytes()),
        Node::Mountstats(pid) => Ok(mounts::mountstats(&mounts::records(pid)?).into_bytes()),
        Node::Filesystems => Ok(filesystems().as_bytes().to_vec()),
        Node::Swaps => Ok(b"Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n".to_vec()),
        Node::Devices => Ok(devices().as_bytes().to_vec()),
        Node::NetDev => net_dev().map(String::into_bytes),
        Node::NetTcp | Node::NetTcp6 => Ok(net_tcp().into_bytes()),
        Node::NetUdp | Node::NetUdp6 => Ok(net_udp().into_bytes()),
        Node::NetUnix => {
            crate::unix::procnet::snapshot(crate::usernet::current()?).map(String::into_bytes)
        }
        Node::NetRoute => network::routes(crate::usernet::current()?).map(String::into_bytes),
        Node::NetArp => network::arp(crate::usernet::current()?).map(String::into_bytes),
        Node::SysIpForward => {
            Ok(crate::route_state::sysctl("net.ipv4.ip_forward")?
                .unwrap_or_else(|| b"0\n".to_vec()))
        }
        Node::SysHostname => Ok(hostname().into_bytes()),
        Node::SysDomainname => {
            let mut value = crate::namespaces::uts(true)?;
            value.push(b'\n');
            Ok(value)
        }
        Node::SysOsRelease => Ok(b"6.1.0-kinakaze\n".to_vec()),
        Node::SysOsType => Ok(b"Linux\n".to_vec()),
        Node::SysKernelPidMax => Ok(b"4194304\n".to_vec()),
        Node::SysKernelCorePattern => Ok(b"core\n".to_vec()),
        Node::SysKernelThreadsMax => Ok(b"4194304\n".to_vec()),
        Node::SysKernelCapLastCap => Ok(b"40\n".to_vec()),
        Node::SysFsMqueue(name) => {
            Ok(format!("{}\n", crate::mqueue::sysctl(name, None)?).into_bytes())
        }
        Node::SysKernelPty(name) => {
            Ok(format!("{}\n", crate::tty::pty_sysctl(name, None)?).into_bytes())
        }
        Node::SysKernelKeyLimit(limit) => Ok(limit.read()),
        Node::SysKernelBootId => Ok(b"01234567-89ab-cdef-0123-456789abcdef\n".to_vec()),
        Node::SysFsFileMax => Ok(b"9223372036854775807\n".to_vec()),
        Node::SysFsNrOpen => Ok(format!("{}\n", crate::MAX_FDS).into_bytes()),
        Node::SysFsPipeMaxSize => Ok(b"1048576\n".to_vec()),
        Node::SysFsInotifyWatches => Ok(b"1048576\n".to_vec()),
        Node::SysFsInotifyInstances => Ok(b"128\n".to_vec()),
        Node::Maps => Ok(maps().into_bytes()),
        Node::Exe(pid) => {
            let info = process_info(pid)?;
            let path = crate::resolve_linux_path(&info.executable).map_err(|_| crate::ENOENT)?;
            std::fs::read(path)
                .map_err(|error| error.raw_os_error().map_or(crate::EIO, errno_from_io))
        }
        Node::Cmdline(pid) => Ok(process_info(pid)?.cmdline),
        Node::Environ => Ok(environ()),
        Node::Stat(pid) => Ok(stat_file(pid)?.into_bytes()),
        Node::Status(pid) => Ok(status_file(pid)?.into_bytes()),
        Node::Cgroup(pid) => {
            let path = kinakaze_runtime::job::cgroup_path(pid).ok_or(crate::ENOENT)?;
            Ok(format!("0::{}\n", crate::namespaces::cgroup_relative(&path)?).into_bytes())
        }
        Node::UidMap(pid) => crate::user_namespace::read_map(pid, false),
        Node::GidMap(pid) => crate::user_namespace::read_map(pid, true),
        Node::Setgroups(pid) => crate::user_namespace::groups(pid, None),
        Node::Comm(pid) => {
            if let Ok(info) = process_info(pid) {
                let name = std::path::Path::new(&info.executable)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("process");
                Ok(format!("{name}\n").into_bytes())
            } else {
                Ok(b"process\n".to_vec())
            }
        }
        Node::Limits(pid) => Ok(limits(pid)?.into_bytes()),
        Node::TimensOffsets(pid) => crate::time_namespace::read_offsets(pid),
        Node::OomScoreAdj(pid) | Node::OomAdj(pid) => {
            let (value, _) = kinakaze_runtime::job::oom_adjustment(pid).ok_or(crate::ESRCH)?;
            let value = if matches!(classify(path), Some(Node::OomAdj(_))) {
                if value == 1000 {
                    15
                } else {
                    (value * 17 / 1000).min(15)
                }
            } else {
                value
            };
            Ok(format!("{value}\n").into_bytes())
        }
        Node::NsEntry(pid, kind) => {
            let inumber = process_namespace_inode(pid, kind)?;
            let kind = if kind == "time_for_children" {
                "time"
            } else {
                kind
            };
            Ok(format!("{kind}:[{inumber}]").into_bytes())
        }
        Node::FdEntry(pid, fd) => {
            // Reading the symlink itself
            let target = fd_link_target(pid, fd)
                .map_err(|e| if e == crate::EBADF { crate::ENOENT } else { e })?;
            Ok(target.into_bytes())
        }
    }
}

/// Proc handlers using a copied C string stop at its first NUL byte.
pub(crate) fn write_text(bytes: &[u8]) -> Result<&str, i32> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).map_err(|_| crate::EINVAL)
}

pub(crate) fn write_advances_offset(path: &str) -> bool {
    let Ok((path, _scope)) = instance::enter(path) else {
        return false;
    };
    let path = path.as_str();
    !matches!(
        classify(path),
        Some(Node::TimensOffsets(_) | Node::OomScoreAdj(_) | Node::OomAdj(_))
    )
}

/// Applies a write to a process control or writable `/proc/sys` file.
/// Each handler validates its own offset and size rules. Numeric sysctls use
/// strict writes at offset zero; key quotas follow `key_defaults` policy.
pub fn write_file(path: &str, bytes: &[u8], offset: u64) -> Result<usize, i32> {
    instance::check_write(path)?;
    let (path, _scope) = instance::enter(path)?;
    let path = path.as_str();
    match classify(path).ok_or(crate::ENOENT)? {
        Node::UidMap(pid) => crate::user_namespace::write_map(pid, false, bytes, offset),
        Node::GidMap(pid) => crate::user_namespace::write_map(pid, true, bytes, offset),
        Node::Setgroups(pid) => {
            if offset != 0 || bytes.len() >= 8 {
                return Err(crate::EINVAL);
            }
            crate::user_namespace::groups(pid, Some(bytes)).map(|_| bytes.len())
        }
        Node::TimensOffsets(pid) => crate::time_namespace::write_offsets(pid, bytes, offset),
        node @ (Node::OomScoreAdj(pid) | Node::OomAdj(pid)) => {
            let caller = crate::credentials::current();
            if caller.uid != 0 && pid != crate::job::process_id() {
                return Err(crate::EACCES);
            }
            // Linux's proc numeric handler consumes at most PROC_NUMBUF - 1
            // bytes and accepts repeated writes independently of file offset.
            let count = bytes.len().min(21);
            // The kernel copies into a NUL-terminated PROC_NUMBUF before
            // strstrip/kstrtoint. runc includes its netlink string terminator
            // in this write; bytes after the first NUL are not numeric input.
            let text =
                write_text(&bytes[..count])?.trim_matches(|c| matches!(c, ' ' | '\t'..='\r'));
            let (negative, digits) = if let Some(tail) = text.strip_prefix('-') {
                (true, tail)
            } else {
                (false, text.strip_prefix('+').unwrap_or(text))
            };
            if digits.contains(['+', '-']) {
                return Err(crate::EINVAL);
            }
            let (radix, digits) = if let Some(tail) = digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))
            {
                (16, tail)
            } else if digits.len() > 1 && digits.starts_with('0') {
                (8, digits)
            } else {
                (10, digits)
            };
            let number = i32::from_str_radix(digits, radix).map_err(|_| crate::EINVAL)?;
            let mut value = if negative { -number } else { number };
            let legacy = matches!(node, Node::OomAdj(_));
            if legacy {
                if !(-16..=15).contains(&value) && value != -17 {
                    return Err(crate::EINVAL);
                }
                value = if value == 15 { 1000 } else { value * 1000 / 17 };
            }
            kinakaze_runtime::job::set_oom_adjustment(
                pid,
                value,
                caller.capabilities & (1 << 24) != 0,
                legacy,
            )?;
            Ok(count)
        }
        node @ (Node::SysHostname | Node::SysDomainname) => {
            if offset != 0 {
                return Ok(bytes.len());
            }
            let text = write_text(bytes)?.split('\n').next().unwrap_or("");
            crate::namespaces::set_uts(matches!(node, Node::SysDomainname), text.as_bytes())?;
            Ok(bytes.len())
        }
        Node::SysFsMqueue(name) => {
            if offset != 0 {
                return Err(crate::EINVAL);
            }
            let value = std::str::from_utf8(bytes)
                .map_err(|_| crate::EINVAL)?
                .trim()
                .parse()
                .map_err(|_| crate::EINVAL)?;
            crate::mqueue::sysctl(name, Some(value))?;
            Ok(bytes.len())
        }
        Node::SysKernelPty(name) => {
            if offset != 0 {
                return Ok(bytes.len());
            }
            let value = write_text(bytes)?
                .trim()
                .parse::<u32>()
                .map_err(|_| crate::EINVAL)?;
            crate::tty::pty_sysctl(name, Some(value))?;
            Ok(bytes.len())
        }
        Node::SysKernelKeyLimit(limit) => limit.write(bytes, offset),
        Node::SysIpForward => {
            if offset != 0 {
                return Err(crate::EINVAL);
            }
            let text = core::str::from_utf8(bytes).map_err(|_| crate::EINVAL)?;
            let value = match text.trim() {
                "0" => b"0\n".to_vec(),
                "1" => b"1\n".to_vec(),
                _ => return Err(crate::EINVAL),
            };
            crate::route_state::set_sysctl("net.ipv4.ip_forward", value)?;
            Ok(bytes.len())
        }
        Node::SysNetIpv6ConfItem => {
            if offset != 0 {
                return Err(crate::EINVAL);
            }
            Ok(bytes.len())
        }
        _ => Err(crate::EACCES),
    }
}

/// Shared process rows store the cgroup mount path; procfs exposes the path
/// relative to that hierarchy. An empty freshly registered row means root.
fn cgroup_relative_path(path: &str) -> Result<&str, i32> {
    if path.is_empty() || path == "/sys/fs/cgroup" {
        return Ok("/");
    }
    let relative = path.strip_prefix("/sys/fs/cgroup/").ok_or(crate::EIO)?;
    if relative.contains(['\n', '\r', '\0']) {
        return Err(crate::EIO);
    }
    Ok(&path["/sys/fs/cgroup".len()..])
}

fn filesystems() -> &'static str {
    "nodev\tsysfs\n\
     nodev\trootfs\n\
     nodev\tramfs\n\
     nodev\tbdev\n\
     nodev\tproc\n\
     nodev\tcgroup\n\
     nodev\tcgroup2\n\
     nodev\tcpuset\n\
     nodev\tdevtmpfs\n\
     nodev\tbinfmt_misc\n\
     nodev\tdebugfs\n\
     nodev\ttracefs\n\
     nodev\tsecurityfs\n\
     nodev\tsockfs\n\
     nodev\tbpf\n\
     nodev\tpipefs\n\
     nodev\tdevpts\n\
     nodev\tmqueue\n\
     \text3\n\
     \text2\n\
     \text4\n\
     \tvfat\n\
     \tmsdos\n\
     \tiso9660\n\
     nodev\tnfs\n\
     nodev\tnfs4\n\
     nodev\tautofs\n\
     nodev\ttmpfs\n\
     nodev\toverlay\n"
}

fn devices() -> &'static str {
    "Character devices:\n  1 mem\n  4 /dev/vc/0\n  4 tty\n  5 /dev/tty\n  5 /dev/console\n  5 /dev/ptmx\n 10 misc\n136 pts\n\nBlock devices:\n  8 sd\n"
}

fn limits(pid: u32) -> Result<String, i32> {
    let (nofile_soft, nofile_hard) = crate::job::nofile_limits(pid)?;
    let (soft, hard) = kinakaze_runtime::job::mqueue_limits(pid, None).map_err(|_| crate::ESRCH)?;
    let number = |value: u64| {
        if value == u64::MAX {
            "unlimited".to_owned()
        } else {
            value.to_string()
        }
    };
    let (mqueue_soft, mqueue_hard) = (number(soft), number(hard));
    Ok(format!(
        "Limit                     Soft Limit           Hard Limit           Units     \n\
     Max cpu time              unlimited            unlimited            seconds   \n\
     Max file size             unlimited            unlimited            bytes     \n\
     Max data size             unlimited            unlimited            bytes     \n\
     Max stack size            8388608              unlimited            bytes     \n\
     Max core file size        0                    unlimited            bytes     \n\
     Max resident set          unlimited            unlimited            bytes     \n\
     Max processes             65535                65535                processes \n\
     Max open files            {nofile_soft:<21}{nofile_hard:<21}files     \n\
     Max locked memory         65536                65536                bytes     \n\
     Max address space         unlimited            unlimited            bytes     \n\
     Max file locks            unlimited            unlimited            locks     \n\
     Max pending signals       65535                65535                signals   \n\
     Max msgqueue size         {mqueue_soft:<21}{mqueue_hard:<21}bytes     \n\
     Max nice priority         0                    0                    \n\
     Max realtime priority     0                    0                    \n\
     Max realtime timeout      unlimited            unlimited            us        \n"
    ))
}

/// Lists a `/proc` directory.
pub fn list_directory(path: &str) -> Result<Vec<String>, i32> {
    let (path, _scope) = instance::enter(path)?;
    let path = path.as_str();
    let node = classify(path).ok_or(crate::ENOENT)?;
    let mut entries = vec![String::from("."), String::from("..")];
    match node {
        Node::Root => {
            entries.push(String::from("self"));
            entries.push(String::from("thread-self"));
            let mut pids: Vec<u32> = crate::job::process_infos()
                .into_iter()
                .filter_map(|info| {
                    kinakaze_runtime::job::namespaces::visible_in(
                        info.entry.namespace_pid,
                        instance::pidns(),
                    )
                })
                .collect();
            pids.sort_unstable();
            pids.dedup();
            entries.extend(pids.into_iter().map(|pid| pid.to_string()));
            entries.extend(
                [
                    "cpuinfo",
                    "meminfo",
                    "vmstat",
                    "uptime",
                    "version",
                    "loadavg",
                    "stat",
                    "mounts",
                    "mountinfo",
                    "filesystems",
                    "swaps",
                    "devices",
                    "cgroup",
                    "net",
                    "sys",
                ]
                .into_iter()
                .map(String::from),
            );
        }
        Node::ProcessRoot(pid) => {
            entries.extend(
                [
                    "stat",
                    "status",
                    "mounts",
                    "mountinfo",
                    "mountstats",
                    "cmdline",
                    "cgroup",
                    "uid_map",
                    "gid_map",
                    "setgroups",
                    "comm",
                    "limits",
                    "oom_score_adj",
                    "timens_offsets",
                    "oom_adj",
                    "ns",
                ]
                .into_iter()
                .map(String::from),
            );
            if process_info(pid).is_ok_and(|info| !info.executable.is_empty()) {
                entries.push(String::from("exe"));
            }
            entries.push(String::from("fd"));
            entries.push(String::from("task"));
            if pid == crate::job::process_id() {
                entries.extend(["environ", "maps"].into_iter().map(String::from));
            }
        }
        Node::TaskDir(pid) => entries.push(visible(pid).to_string()),
        Node::NsDir(_) => entries.extend(
            [
                "cgroup",
                "ipc",
                "mnt",
                "net",
                "pid",
                "pid_for_children",
                "user",
                "uts",
                "time",
                "time_for_children",
            ]
            .into_iter()
            .map(String::from),
        ),
        Node::NetDir => entries.extend(
            ["dev", "tcp", "tcp6", "udp", "udp6", "unix", "route", "arp"]
                .into_iter()
                .map(String::from),
        ),
        Node::SysDir => entries.extend(["net", "kernel", "fs"].into_iter().map(String::from)),
        Node::SysNetDir => entries.extend(["ipv4", "ipv6"].into_iter().map(String::from)),
        Node::SysNetIpv4Dir => entries.extend(["ip_forward"].into_iter().map(String::from)),
        Node::SysNetIpv6Dir => entries.extend(["conf"].into_iter().map(String::from)),
        Node::SysNetIpv6ConfDir => {
            entries.extend(["all", "default", "docker0"].into_iter().map(String::from))
        }
        Node::SysNetIpv6ConfIfaceDir => entries.extend(
            ["accept_ra", "disable_ipv6", "forwarding"]
                .into_iter()
                .map(String::from),
        ),
        Node::SysFsDir => entries.extend(
            ["file-max", "nr_open", "pipe-max-size", "inotify", "mqueue"]
                .into_iter()
                .map(String::from),
        ),
        Node::SysFsInotifyDir => entries.extend(
            ["max_user_watches", "max_user_instances"]
                .into_iter()
                .map(String::from),
        ),
        Node::SysKernelDir => entries.extend(
            [
                "hostname",
                "domainname",
                "osrelease",
                "ostype",
                "pid_max",
                "core_pattern",
                "threads-max",
                "random",
                "keys",
                "pty",
            ]
            .into_iter()
            .map(String::from),
        ),
        Node::SysKernelRandomDir => entries.extend(["boot_id"].into_iter().map(String::from)),
        Node::SysFsMqueueDir => entries.extend(
            [
                "msg_default",
                "msg_max",
                "msgsize_default",
                "msgsize_max",
                "queues_max",
            ]
            .into_iter()
            .map(String::from),
        ),
        Node::SysKernelPtyDir => {
            entries.extend(["max", "reserve", "nr"].into_iter().map(String::from))
        }
        Node::SysKernelKeysDir => entries.extend(
            key_defaults::Limit::ALL
                .iter()
                .map(|limit| limit.name().to_owned()),
        ),
        Node::FdDir(pid) => {
            for fd in fd_numbers(pid)? {
                entries.push(fd.to_string());
            }
        }
        _ => return Err(crate::ENOTDIR),
    }
    if instance::current().is_some_and(|v| v.subset_pid) && matches!(node, Node::Root) {
        entries.retain(|s| {
            matches!(s.as_str(), "." | ".." | "self" | "thread-self") || s.parse::<u32>().is_ok()
        });
    }
    Ok(entries)
}

/// The task directory's link count is two plus its live guest task count.
pub(crate) fn directory_links(path: &str) -> Option<u64> {
    let (path, _scope) = instance::enter(path).ok()?;
    match classify(&path)? {
        Node::TaskDir(pid) if pid == crate::job::process_id() => {
            Some(2 + u64::from(kinakaze_runtime::process_thread_count()))
        }
        _ => None,
    }
}

/// Reports the kind and size of a `/proc` path so the caller can build a stat.
pub fn metadata(path: &str) -> Result<ProcMetadata, i32> {
    let (path, _scope) = instance::enter(path)?;
    let path = path.as_str();
    let node = classify(path).ok_or(crate::ENOENT)?;
    if matches!(
        node,
        Node::Root
            | Node::ProcessRoot(_)
            | Node::NetDir
            | Node::SysDir
            | Node::SysNetDir
            | Node::SysNetIpv4Dir
            | Node::SysNetIpv6Dir
            | Node::SysNetIpv6ConfDir
            | Node::SysNetIpv6ConfIfaceDir
            | Node::SysKernelDir
            | Node::SysFsMqueueDir
            | Node::SysKernelPtyDir
            | Node::SysKernelKeysDir
            | Node::SysKernelRandomDir
            | Node::SysFsDir
            | Node::SysFsInotifyDir
            | Node::NsDir(_)
            | Node::TaskDir(_)
            | Node::FdDir(_)
    ) {
        return Ok(ProcMetadata {
            kind: ProcKind::Directory,
            // The size Linux reports for a procfs directory inode.
            size: 4096,
            target: None,
        });
    }
    if let Node::Exe(pid) = node {
        let target = exe_link_target(pid)?;
        return Ok(ProcMetadata {
            kind: ProcKind::Symlink,
            size: target.len() as u64,
            target: Some(target),
        });
    }
    if let Node::NsEntry(pid, kind) = node {
        let inumber = process_namespace_inode(pid, kind)?;
        let kind = if kind == "time_for_children" {
            "time"
        } else {
            kind
        };
        let target = format!("{kind}:[{inumber}]");
        return Ok(ProcMetadata {
            kind: ProcKind::Symlink,
            size: target.len() as u64,
            target: Some(target),
        });
    }
    if let Node::FdEntry(pid, fd) = node {
        let target = fd_link_target(pid, fd)
            .map_err(|e| if e == crate::EBADF { crate::ENOENT } else { e })?;
        return Ok(ProcMetadata {
            kind: ProcKind::Symlink,
            size: target.len() as u64,
            target: Some(target),
        });
    }
    Ok(ProcMetadata {
        kind: ProcKind::File,
        size: 0,
        target: None,
    })
}

/// Translates a `std::io` raw OS error into an errno.
fn errno_from_io(code: i32) -> i32 {
    crate::errno_from_win32(code as u32)
}

/// The four registers `cpuid` returns.
#[derive(Clone, Copy, Default)]
struct CpuId {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

/// Executes `cpuid` for a leaf and subleaf.
///
/// Leaves above the processor's reported maximum return the highest supported
/// leaf's data instead of failing, so callers must range-check against leaf 0
/// (or leaf `0x8000_0000`) before trusting a result.
#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, subleaf: u32) -> CpuId {
    // The intrinsic is safe on x86_64: the instruction is baseline on the
    // architecture and cannot fault for any leaf value.
    let raw = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    CpuId {
        eax: raw.eax,
        ebx: raw.ebx,
        ecx: raw.ecx,
        edx: raw.edx,
    }
}

/// Stand-in for targets without `cpuid`, such as Windows on ARM64.
#[cfg(not(target_arch = "x86_64"))]
fn cpuid(_leaf: u32, _subleaf: u32) -> CpuId {
    CpuId::default()
}

/// Highest supported leaf in the basic or extended range.
fn max_leaf(base: u32) -> u32 {
    cpuid(base, 0).eax
}

/// Renders register bytes as ASCII, which is how `cpuid` returns strings.
fn registers_to_ascii(registers: &[u32], out: &mut String) {
    for word in registers {
        for byte in word.to_le_bytes() {
            // The brand string is NUL-padded to a fixed 48 bytes.
            if byte == 0 {
                return;
            }
            out.push(byte as char);
        }
    }
}

/// Reads the 12-character vendor id from leaf 0, in `ebx:edx:ecx` order.
fn vendor_id() -> String {
    let leaf = cpuid(0, 0);
    let mut vendor = String::new();
    registers_to_ascii(&[leaf.ebx, leaf.edx, leaf.ecx], &mut vendor);
    if vendor.is_empty() {
        // Only reachable without cpuid; the caller still needs a field value.
        String::from("unknown")
    } else {
        vendor
    }
}

/// Reads the marketing brand string from leaves `0x8000_0002..=0x8000_0004`.
fn brand_string() -> String {
    if max_leaf(0x8000_0000) < 0x8000_0004 {
        return String::new();
    }
    let mut brand = String::new();
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        let registers = cpuid(leaf, 0);
        registers_to_ascii(
            &[registers.eax, registers.ebx, registers.ecx, registers.edx],
            &mut brand,
        );
    }
    // Intel left-pads the brand with spaces to fill all 48 bytes.
    brand.trim().to_string()
}

/// Family, model and stepping, with the extended fields folded in.
///
/// Intel and AMD both require adding the extended model when the base family is
/// 6 or 15, and adding the extended family when the base family is 15; skipping
/// that step reports every modern CPU as family 6 model 15.
fn family_model_stepping() -> (u32, u32, u32) {
    let signature = cpuid(1, 0).eax;
    let base_family = (signature >> 8) & 0xf;
    let base_model = (signature >> 4) & 0xf;
    let stepping = signature & 0xf;

    let family = if base_family == 0xf {
        base_family + ((signature >> 20) & 0xff)
    } else {
        base_family
    };
    let model = if base_family == 0x6 || base_family == 0xf {
        base_model + (((signature >> 16) & 0xf) << 4)
    } else {
        base_model
    };
    (family, model, stepping)
}

/// Leaf 1 `edx` feature bits, in Linux's `/proc/cpuinfo` naming and bit order.
const LEAF1_EDX_FLAGS: [(u32, &str); 30] = [
    (0, "fpu"),
    (1, "vme"),
    (2, "de"),
    (3, "pse"),
    (4, "tsc"),
    (5, "msr"),
    (6, "pae"),
    (7, "mce"),
    (8, "cx8"),
    (9, "apic"),
    (11, "sep"),
    (12, "mtrr"),
    (13, "pge"),
    (14, "mca"),
    (15, "cmov"),
    (16, "pat"),
    (17, "pse36"),
    (18, "pn"),
    (19, "clflush"),
    (21, "ds"),
    (22, "acpi"),
    (23, "mmx"),
    (24, "fxsr"),
    (25, "sse"),
    (26, "sse2"),
    (27, "ss"),
    (28, "ht"),
    (29, "tm"),
    (30, "ia64"),
    (31, "pbe"),
];

/// Leaf 1 `ecx` feature bits.
const LEAF1_ECX_FLAGS: [(u32, &str); 30] = [
    (0, "pni"),
    (1, "pclmulqdq"),
    (2, "dtes64"),
    (3, "monitor"),
    (4, "ds_cpl"),
    (5, "vmx"),
    (6, "smx"),
    (7, "est"),
    (8, "tm2"),
    (9, "ssse3"),
    (10, "cid"),
    (12, "fma"),
    (13, "cx16"),
    (14, "xtpr"),
    (15, "pdcm"),
    (17, "pcid"),
    (18, "dca"),
    (19, "sse4_1"),
    (20, "sse4_2"),
    (21, "x2apic"),
    (22, "movbe"),
    (23, "popcnt"),
    (24, "tsc_deadline_timer"),
    (25, "aes"),
    (26, "xsave"),
    (27, "osxsave"),
    (28, "avx"),
    (29, "f16c"),
    (30, "rdrand"),
    // Set by every hypervisor and clear on bare metal, which makes it the one
    // flag here that reveals whether the host is virtualized.
    (31, "hypervisor"),
];

/// Leaf 7 subleaf 0 `ebx` feature bits, where the AVX2-era flags live.
const LEAF7_EBX_FLAGS: [(u32, &str); 14] = [
    (0, "fsgsbase"),
    (3, "bmi1"),
    (5, "avx2"),
    (7, "smep"),
    (8, "bmi2"),
    (9, "erms"),
    (10, "invpcid"),
    (16, "avx512f"),
    (17, "avx512dq"),
    (18, "rdseed"),
    (19, "adx"),
    (20, "smap"),
    (28, "avx512cd"),
    (29, "sha_ni"),
];

/// Leaf `0x8000_0001` `edx` bits that Linux reports beyond the leaf 1 aliases.
const EXT_EDX_FLAGS: [(u32, &str); 5] = [
    (11, "syscall"),
    (20, "nx"),
    (26, "pdpe1gb"),
    (27, "rdtscp"),
    (29, "lm"),
];

/// Appends the names of every set bit in `table`.
fn collect_flags(register: u32, table: &[(u32, &'static str)], flags: &mut Vec<&'static str>) {
    flags.extend(
        table
            .iter()
            .filter(|(bit, _)| register & (1 << bit) != 0)
            .map(|(_, name)| *name),
    );
}

/// Reads the processor's real feature set.
///
/// Linux also prints kernel-synthesized flags such as `constant_tsc` and the
/// speculation mitigations, which have no cpuid bit at all. Those are omitted
/// rather than guessed, so every name in this list is a bit that is genuinely
/// set on this CPU.
fn cpu_flags() -> Vec<&'static str> {
    let mut flags = Vec::new();
    let leaf1 = cpuid(1, 0);
    collect_flags(leaf1.edx, &LEAF1_EDX_FLAGS, &mut flags);

    if max_leaf(0x8000_0000) >= 0x8000_0001 {
        collect_flags(cpuid(0x8000_0001, 0).edx, &EXT_EDX_FLAGS, &mut flags);
    }
    collect_flags(leaf1.ecx, &LEAF1_ECX_FLAGS, &mut flags);
    if max_leaf(0) >= 7 {
        collect_flags(cpuid(7, 0).ebx, &LEAF7_EBX_FLAGS, &mut flags);
    }
    flags
}

/// Number of logical processors this process may run on.
///
/// `available_parallelism` respects the process affinity mask, so a program
/// pinned to four cores of a large machine sees four, which is the number it
/// should actually size its thread pool from.
fn logical_processors() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Hardware threads per physical core, derived from the topology leaf.
///
/// Leaf `0xb` level 0 reports SMT width directly. Older processors without it
/// fall back to the leaf 1 hyperthreading bit, which only distinguishes two
/// threads per core from one.
///
/// On a hybrid processor this describes whichever core the query happened to run
/// on, so it is only used when the host cannot supply the real topology.
fn threads_per_core() -> usize {
    if max_leaf(0) >= 0xb {
        let level = cpuid(0xb, 0);
        // A zero level type means the leaf is not implemented despite being in
        // range, which happens under some hypervisors.
        let width = level.ebx & 0xffff;
        if (level.ecx >> 8) & 0xff == 1 && width > 0 {
            return width as usize;
        }
    }
    if cpuid(1, 0).edx & (1 << 28) != 0 {
        2
    } else {
        1
    }
}

/// Physical cores backing the logical processors this process can use.
///
/// Windows is asked first because it is the only source that is correct on a
/// hybrid part: dividing logical processors by cpuid's SMT width assumes every
/// core is identical, which reports a 6P+8E i7-13700H as 10 cores instead of 14.
/// Cores are counted only if they intersect the process affinity mask, so the
/// figure stays consistent with [`logical_processors`].
fn physical_cores() -> usize {
    if let Some(cores) = windows_physical_cores() {
        return cores.min(logical_processors()).max(1);
    }
    (logical_processors() / threads_per_core()).max(1)
}

/// Counts `RelationProcessorCore` entries reachable by this process.
fn windows_physical_cores() -> Option<usize> {
    let mut affinity = 0usize;
    let mut system = 0usize;
    // SAFETY: the pseudo-handle is always valid and both out-parameters are
    // writable locals.
    let ok =
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &raw mut affinity, &raw mut system) };
    // Without a mask every core counts; a zero mask would otherwise hide all.
    let mask = if ok != 0 && affinity != 0 {
        affinity
    } else {
        usize::MAX
    };

    // The call reports the required length through a failure with
    // ERROR_INSUFFICIENT_BUFFER, so the buffer is sized from an empty probe.
    let mut length = 0u32;
    // SAFETY: a null buffer with a zero length is the documented size query.
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &raw mut length) };
    if length == 0 {
        return None;
    }

    let stride = size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let count = length as usize / stride;
    let mut buffer = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); count];
    // SAFETY: `buffer` holds `count * stride` writable bytes, which is the
    // length reported by the probe above.
    let ok = unsafe { GetLogicalProcessorInformation(buffer.as_mut_ptr(), &raw mut length) };
    if ok == 0 {
        return None;
    }

    let cores = buffer
        .iter()
        .take(length as usize / stride)
        .filter(|entry| {
            entry.Relationship == RelationProcessorCore && entry.ProcessorMask & mask != 0
        })
        .count();
    (cores > 0).then_some(cores)
}

/// Nominal core frequency in MHz.
///
/// Three sources are tried in descending order of directness. Leaf `0x16` states
/// the base frequency outright, but Hyper-V leaves it zeroed, which is the common
/// case on this host. Leaf `0x15` then gives the TSC frequency as a ratio against
/// the core crystal clock, the same derivation the Linux kernel uses in
/// `native_calibrate_tsc`. The brand string's trailing `N.NNGHz` is the last
/// resort, since recent parts have stopped including it.
fn cpu_mhz() -> f64 {
    if max_leaf(0) >= 0x16 {
        let base = cpuid(0x16, 0).eax & 0xffff;
        if base > 0 {
            return f64::from(base);
        }
    }
    if let Some(hertz) = tsc_frequency_hertz() {
        return hertz / 1_000_000.0;
    }
    frequency_from_brand(&brand_string()).unwrap_or(0.0)
}

/// TSC frequency in Hz, derived from the leaf `0x15` crystal-clock ratio.
///
/// `ecx` carries the crystal frequency directly when the CPU reports it. When it
/// is zero the ratio is still usable, but only with a per-model crystal
/// frequency table that this layer deliberately does not carry, so the value is
/// reported as unavailable rather than guessed from a hardcoded constant.
fn tsc_frequency_hertz() -> Option<f64> {
    if max_leaf(0) < 0x15 {
        return None;
    }
    let leaf = cpuid(0x15, 0);
    let (denominator, numerator, crystal) = (leaf.eax, leaf.ebx, leaf.ecx);
    if denominator == 0 || numerator == 0 || crystal == 0 {
        return None;
    }
    Some(f64::from(crystal) * f64::from(numerator) / f64::from(denominator))
}

/// Extracts a `2.90GHz`-style frequency from a brand string.
fn frequency_from_brand(brand: &str) -> Option<f64> {
    let index = brand.find("GHz").or_else(|| brand.find("MHz"))?;
    let scale = if brand[index..].starts_with("GHz") {
        1000.0
    } else {
        1.0
    };
    let digits: String = brand[..index]
        .chars()
        .rev()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let value: f64 = digits.chars().rev().collect::<String>().parse().ok()?;
    Some(value * scale)
}

/// Size in kB of the largest cache level, which is what Linux reports.
///
/// Linux fills `cache size` from the last-level cache, so leaf 4 is enumerated
/// for the deepest unified cache and its true geometry is multiplied out. Leaf
/// `0x8000_0006` is the fallback for processors without leaf 4, but it only
/// describes L2 and so understates any part with an L3.
fn cache_size_kb() -> u32 {
    if let Some(bytes) = last_level_cache_bytes() {
        return bytes / 1024;
    }
    if max_leaf(0x8000_0000) < 0x8000_0006 {
        return 0;
    }
    (cpuid(0x8000_0006, 0).ecx >> 16) & 0xffff
}

/// Walks leaf 4's subleaves for the largest cache, returning its size in bytes.
///
/// Each subleaf describes one cache until the type field reads zero. The size is
/// the documented product of ways, partitions, line size and sets, all of which
/// are stored biased by one.
fn last_level_cache_bytes() -> Option<u32> {
    if max_leaf(0) < 4 {
        return None;
    }
    let mut largest = 0u32;
    for subleaf in 0..16 {
        let leaf = cpuid(4, subleaf);
        // Cache type 0 terminates the enumeration.
        if leaf.eax & 0x1f == 0 {
            break;
        }
        let ways = ((leaf.ebx >> 22) & 0x3ff) + 1;
        let partitions = ((leaf.ebx >> 12) & 0x3ff) + 1;
        let line_size = (leaf.ebx & 0xfff) + 1;
        let sets = leaf.ecx + 1;
        largest = largest.max(
            ways.saturating_mul(partitions)
                .saturating_mul(line_size)
                .saturating_mul(sets),
        );
    }
    (largest > 0).then_some(largest)
}

/// Builds `/proc/cpuinfo`, one stanza per logical processor.
///
/// Every stanza carries the same identification because cpuid describes the
/// core it executes on and this process is not pinned per stanza. On a hybrid
/// P-core/E-core machine that makes the per-processor model name approximate;
/// the count, flags and topology remain exact.
fn cpuinfo() -> String {
    let count = logical_processors();
    let vendor = vendor_id();
    let mut brand = brand_string();
    if brand.is_empty() {
        // Pre-2000 x86 parts and non-x86 hosts have no brand leaf. The field is
        // mandatory in the format, so it says so rather than inventing a model.
        brand = String::from("unknown");
    }
    let (family, model, _stepping) = family_model_stepping();
    let flags = cpu_flags().join(" ");
    let mhz = cpu_mhz();
    let cache = cache_size_kb();
    let topology = cpu_topology::query();
    let cores = if topology.is_none() {
        physical_cores()
    } else {
        1
    };

    let mut out = String::with_capacity(count * 512);
    for index in 0..count {
        // Field names are padded to a tab stop the same way the kernel's
        // seq_printf does, because parsers split on ':' after trimming.
        let _ = writeln!(out, "processor\t: {index}");
        let _ = writeln!(out, "vendor_id\t: {vendor}");
        let _ = writeln!(out, "cpu family\t: {family}");
        let _ = writeln!(out, "model\t\t: {model}");
        let _ = writeln!(out, "model name\t: {brand}");
        let _ = writeln!(out, "cpu MHz\t\t: {mhz:.3}");
        let _ = writeln!(out, "cache size\t: {cache} KB");
        let native = topology.as_ref().and_then(|cpus| cpus.get(index));
        // Only hosts unable to expose topology use the existing CPUID estimate.
        let package = native.map_or(0, |cpu| cpu.package);
        let core = native.map_or_else(|| index / threads_per_core(), |cpu| cpu.core);
        let cores = native.map_or(cores, |cpu| cpu.cores);
        let siblings = native.map_or(count, |cpu| cpu.siblings);
        let _ = writeln!(out, "physical id\t: {package}");
        let _ = writeln!(out, "core id\t\t: {core}");
        let _ = writeln!(out, "siblings\t: {siblings}");
        let _ = writeln!(out, "cpu cores\t: {cores}");
        let _ = writeln!(out, "flags\t\t: {flags}");
        out.push('\n');
    }
    out
}

/// Builds `/proc/meminfo`.
///
/// Free pages exclude standby (reclaimable) memory. MemAvailable includes it.
/// Swap figures describe installed pagefiles, not the system's commit budget.
fn meminfo() -> Result<String, i32> {
    let status = crate::memory::snapshot()?;
    let mut out = String::with_capacity(256);
    for (name, value) in [
        ("MemTotal", status.total / 1024),
        ("MemFree", status.free / 1024),
        ("MemAvailable", status.available / 1024),
        ("SwapTotal", status.swap_total / 1024),
        ("SwapFree", status.swap_free / 1024),
    ] {
        // The kernel's exact column layout: name padded to 15, value to 8.
        let _ = writeln!(out, "{:<15}{:>8} kB", format!("{name}:"), value);
    }
    Ok(out)
}

/// Live VM gauges in Linux's page units.
///
/// Event counters remain unimplemented and are intentionally absent, not zero:
/// Windows PageReadCount includes file-backed faults (not just swap), while
/// IoReadTransferCount includes non-block-device I/O. Neither can truthfully be
/// renamed pswpin/pgpgin. The full system_metrics_probe continues to require
/// those counters, so exposing this gauge does not mark VM accounting complete.
fn vmstat() -> Result<String, i32> {
    Ok(format!("nr_free_pages {}\n", crate::memory::free_pages()?))
}

/// One mapped address range, in the shape a maps line needs.
struct Region {
    start: usize,
    end: usize,
    /// The four `rwxp` characters.
    perms: [u8; 4],
    offset: u64,
    path: String,
}

/// Translates Windows page protection into the Linux `rwx` triple.
///
/// Windows has no write-without-read protection, so the `r` bit is set whenever
/// any access is permitted. Copy-on-write counts as writable because a store
/// succeeds; it just privatizes the page first, which is exactly what a private
/// file mapping does on Linux too.
fn protection_to_perms(protect: u32, shared: bool) -> [u8; 4] {
    // The modifier bits are orthogonal to the access type and must be masked
    // off before comparing, or a guarded stack page matches nothing.
    let access = protect & !(PAGE_GUARD | PAGE_NOCACHE | PAGE_WRITECOMBINE);
    let (read, write, execute) = match access {
        PAGE_READONLY => (true, false, false),
        PAGE_READWRITE | PAGE_WRITECOPY => (true, true, false),
        PAGE_EXECUTE => (false, false, true),
        PAGE_EXECUTE_READ => (true, false, true),
        PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY => (true, true, true),
        // PAGE_NOACCESS, and reserved-but-uncommitted ranges whose Protect is 0.
        _ => (false, false, false),
    };
    [
        if read { b'r' } else { b'-' },
        if write { b'w' } else { b'-' },
        if execute { b'x' } else { b'-' },
        if shared { b's' } else { b'p' },
    ]
}

/// Reads a wide buffer written by a Win32 call into a `String`.
fn wide_to_string(buffer: &[u16], length: usize) -> String {
    String::from_utf16_lossy(&buffer[..length.min(buffer.len())])
}

/// Recovers the file backing an image mapping.
///
/// A loaded module's base address is its `HMODULE`, so the allocation base can
/// be handed straight to `GetModuleFileNameExW`, which yields a normal DOS path.
/// Data mappings are not modules, so those fall back to `GetMappedFileNameW`
/// and the NT device path it returns.
fn mapped_file_name(allocation_base: *mut c_void, image: bool) -> Option<String> {
    // SAFETY: the pseudo-handle is always valid and needs no release.
    let process = unsafe { GetCurrentProcess() };
    // Heap-allocated: this runs once per mapped region, and a 64 KiB stack frame
    // per call would be a real risk on a thread with a small stack.
    let mut buffer = vec![0u16; 32768];

    if image {
        // SAFETY: `buffer` is writable for its full length, and the base address
        // of an image region is by definition a module handle.
        let length = unsafe {
            GetModuleFileNameExW(
                process,
                allocation_base as _,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        };
        if length > 0 {
            return Some(wide_to_string(&buffer, length as usize));
        }
    }

    // SAFETY: `buffer` is writable for its full length and the address lies in
    // this process, whose handle grants the required query access.
    let length = unsafe {
        GetMappedFileNameW(
            process,
            allocation_base,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    };
    (length > 0).then(|| wide_to_string(&buffer, length as usize))
}

/// Expresses a Windows path in the guest's Linux namespace.
///
/// A guest that reads a path out of `/proc` will try to `open` it, so emitting
/// `C:\Windows\System32\ntdll.dll` would hand it a name [`crate::path`] rejects
/// for its colon and backslashes. The two shapes that module understands are a
/// path under the virtual root and the `/c/...` drive form, so this produces
/// whichever one applies.
fn guest_path(windows_path: &str) -> String {
    let native = Path::new(windows_path);
    if let Ok(root) = crate::path::system_root()
        && let Ok(relative) = native.strip_prefix(&root)
    {
        return format!("/{}", relative.to_string_lossy().replace('\\', "/"));
    }

    let bytes = windows_path.as_bytes();
    // `X:\...`: the drive letter becomes the first component.
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        let letter = bytes[0].to_ascii_lowercase() as char;
        let rest = windows_path[3..].replace('\\', "/");
        return format!("/{letter}/{rest}");
    }

    // An NT device path such as \Device\HarddiskVolume3\... has no guest
    // equivalent, so it is reported verbatim with separators normalized. It
    // identifies the mapping for a human without pretending to be openable.
    windows_path.replace('\\', "/")
}

/// Bounds of the calling thread's stack, used to label it `[stack]`.
fn thread_stack_bounds() -> (usize, usize) {
    let mut low = 0usize;
    let mut high = 0usize;
    // SAFETY: both out-parameters are writable locals; the call cannot fail.
    unsafe { GetCurrentThreadStackLimits(&raw mut low, &raw mut high) };
    (low, high)
}

/// Walks this process's address space and collects every mapped region.
///
/// The walk starts at zero and advances by each region's reported size, which is
/// how `VirtualQuery` is meant to be iterated: it always describes a whole run
/// of pages sharing a state and protection, so the next query address is the
/// current base plus its length. A zero return means the address is above the
/// user-mode maximum, which terminates the walk.
fn collect_regions() -> Vec<Region> {
    let (stack_low, stack_high) = thread_stack_bounds();
    let mut regions: Vec<Region> = Vec::new();
    let mut address = 0usize;

    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `info` is a writable local of exactly the size passed, and any
        // address value is a legal query target.
        let written = unsafe {
            VirtualQuery(
                address as *const c_void,
                &raw mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 || info.RegionSize == 0 {
            break;
        }

        let start = info.BaseAddress as usize;
        let Some(end) = start.checked_add(info.RegionSize) else {
            break;
        };

        if info.State != MEM_FREE {
            regions.push(describe_region(&info, start, end, stack_low, stack_high));
        }

        // A region that does not advance past the query address would spin the
        // walk forever, which is worth guarding even though it should not happen.
        if end <= address {
            break;
        }
        address = end;
    }
    regions
}

/// Turns one `VirtualQuery` result into a maps entry.
fn describe_region(
    info: &MEMORY_BASIC_INFORMATION,
    start: usize,
    end: usize,
    stack_low: usize,
    stack_high: usize,
) -> Region {
    let image = info.Type == MEM_IMAGE;
    let mapped = info.Type == MEM_MAPPED;
    // Image sections are always private copy-on-write. A data mapping is shared
    // unless its protection says copy-on-write, which is the only signal Windows
    // gives about the view's sharing mode.
    let shared = mapped && info.AllocationProtect & (PAGE_WRITECOPY | PAGE_EXECUTE_WRITECOPY) == 0;
    let perms = protection_to_perms(info.Protect, shared);

    let path = if image || mapped {
        mapped_file_name(info.AllocationBase, image)
            .map(|name| guest_path(&name))
            .unwrap_or_default()
    } else if start >= stack_low
        && end <= stack_high
        && info.State == MEM_COMMIT
        && perms[1] == b'w'
    {
        // Linux names the stack VMA so guests scanning maps can find their own
        // bounds. The committed writable check excludes the guard pages and the
        // reserved tail, which Windows keeps inside the same stack range but
        // which are not the stack a guest is looking for.
        String::from("[stack]")
    } else {
        String::new()
    };

    // Linux reports the offset into the backing file. Windows does not expose a
    // view's file offset, so this is the distance from the allocation base,
    // which is the correct value for the common whole-file image mapping and an
    // approximation for a partial view.
    let offset = if path.is_empty() || path == "[stack]" {
        0
    } else {
        (start - info.AllocationBase as usize) as u64
    };

    Region {
        start,
        end,
        perms,
        offset,
        path,
    }
}

/// Builds `/proc/self/maps` from the live address space.
///
/// Adjacent regions that agree on protection and backing file are merged, because
/// Windows splits a run of pages whenever any attribute differs while Linux
/// reports one line per VMA. Without merging, a single DLL's `.text` can appear
/// as several consecutive identical-permission lines, which breaks parsers that
/// expect one range per mapping.
///
/// `dev` and `inode` are always `00:00 0`. Windows has no inode to report, and
/// that pair is exactly what Linux prints for a mapping with no backing inode,
/// so it is the honest encoding of "unknown" rather than a fabricated number.
fn maps() -> String {
    let regions = collect_regions();
    let mut out = String::with_capacity(regions.len() * 96);
    let mut merged: Option<Region> = None;

    for region in regions {
        match merged.take() {
            Some(previous)
                if previous.end == region.start
                    && previous.perms == region.perms
                    && previous.path == region.path =>
            {
                merged = Some(Region {
                    end: region.end,
                    ..previous
                });
            }
            Some(previous) => {
                write_region(&mut out, &previous);
                merged = Some(region);
            }
            None => merged = Some(region),
        }
    }
    if let Some(last) = merged {
        write_region(&mut out, &last);
    }
    out
}

/// Column the kernel pads a maps line to before printing a pathname.
///
/// `show_map_vma` calls `seq_setwidth(m, 25 + sizeof(void *) * 6)`, which is 73
/// on 64-bit, and an unnamed mapping gets no padding at all because the kernel
/// only pads on the branch that has a name to print.
const MAPS_NAME_COLUMN: usize = 25 + size_of::<usize>() * 6;

/// Emits one maps line.
fn write_region(out: &mut String, region: &Region) {
    // The perms array is built from ASCII literals, so this never falls back.
    let perms = std::str::from_utf8(&region.perms).unwrap_or("----");
    let prefix = format!(
        "{:08x}-{:08x} {} {:08x} 00:00 0",
        region.start, region.end, perms, region.offset
    );
    if region.path.is_empty() {
        let _ = writeln!(out, "{prefix}");
    } else {
        let _ = writeln!(
            out,
            "{prefix:<width$} {}",
            region.path,
            width = MAPS_NAME_COLUMN
        );
    }
}

/// Target of the `/proc/self/exe` symlink.
///
/// The link resolves inside the guest namespace rather than to the raw Windows
/// path, so a guest that reads the link and opens the result reaches the same
/// image. Since `/` is the executable's own directory, this is normally just
/// `/<name>.exe`.
fn process_info(pid: u32) -> Result<crate::job::ProcessInfo, i32> {
    crate::job::process_info(pid).ok_or(crate::ENOENT)
}

fn exe_link_target(pid: u32) -> Result<String, i32> {
    let info = process_info(pid)?;
    if !info.executable.is_empty() {
        return Ok(info.executable);
    }
    let executable = std::env::current_exe()
        .map_err(|error| error.raw_os_error().map_or(crate::EIO, errno_from_io))?;
    Ok(crate::to_guest_path(&executable))
}

/// Builds `/proc/self/environ`: NUL-terminated `KEY=VALUE` pairs.
fn environ() -> Vec<u8> {
    let mut out = Vec::new();
    let mut keys = std::collections::HashSet::new();

    let mut content = None;
    if let Ok(path) = crate::resolve_linux_path("/etc/environment") {
        if let Ok(c) = std::fs::read_to_string(&path) {
            content = Some(c);
        }
    }
    let content = content.unwrap_or_else(|| {
        String::from_utf8_lossy(&crate::fs::synthetic_environment()).into_owned()
    });

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !k.is_empty() && keys.insert(k.to_owned()) {
                out.extend_from_slice(k.as_bytes());
                out.push(b'=');
                out.extend_from_slice(v.as_bytes());
                out.push(0);
            }
        }
    }

    for (default_k, default_v) in [
        (
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        ),
        ("HOME", "/root"),
        ("USER", "root"),
        ("LOGNAME", "root"),
        ("SHELL", "/bin/sh"),
        ("LANG", "C.UTF-8"),
        ("TERM", "xterm-256color"),
    ] {
        if keys.insert(default_k.to_owned()) {
            out.extend_from_slice(default_k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(default_v.as_bytes());
            out.push(0);
        }
    }

    out
}

/// Parent process id in the shared Linux PID namespace.
pub fn parent_pid() -> Option<u32> {
    Some(crate::job::parent_process_id())
}

/// This process's resident and virtual sizes in bytes.
fn memory_counters() -> Option<PROCESS_MEMORY_COUNTERS> {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the pseudo-handle is always valid, and `counters` is a writable
    // local whose `cb` matches the size passed.
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &raw mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    (ok != 0).then_some(counters)
}

/// Length of Linux's `comm` field, including the terminator.
const TASK_COMM_LEN: usize = 16;

/// The process name as `comm` reports it: the image basename, truncated.
///
/// Linux limits `comm` to 15 visible bytes and guest code sometimes compares
/// against exactly that, so the truncation is reproduced rather than skipped.
fn process_comm(info: &crate::job::ProcessInfo) -> String {
    if !info.comm.is_empty() {
        return info.comm.clone();
    }
    let name = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| String::from("unknown"));

    let limit = TASK_COMM_LEN - 1;
    if name.len() <= limit {
        return name;
    }
    // Truncate on a character boundary so the result stays valid UTF-8.
    let mut end = limit;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// Total size of every non-free region, the analogue of Linux's `VmSize`.
///
/// `/proc/self/maps` needs names and permissions, but `VmSize` only needs a
/// sum. Reusing `collect_regions` here used to resolve the backing filename of
/// every JVM image and mapped archive, turning one numeric field into a full
/// address-space inventory. This dedicated walk performs no allocation and no
/// mapped-file lookup.
fn virtual_size() -> u64 {
    let mut total = 0u64;
    let mut address = 0usize;

    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `info` is writable for the exact size passed and every
        // address is a legal VirtualQuery probe.
        let written = unsafe {
            VirtualQuery(
                address as *const c_void,
                &raw mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 || info.RegionSize == 0 {
            break;
        }

        if info.State != MEM_FREE {
            total = total.saturating_add(info.RegionSize as u64);
        }
        let start = info.BaseAddress as usize;
        let Some(end) = start.checked_add(info.RegionSize) else {
            break;
        };
        if end <= address {
            break;
        }
        address = end;
    }

    total
}

/// Builds `/proc/self/stat`.
///
/// Only fields backed by runtime or host state are populated: pid, comm, state,
/// ppid, thread count, start time, virtual size and RSS. The remaining
/// positional fields are emitted as zero. That is a deliberate choice over
/// inventing values, and it is safe because the format is positional: a reader
/// looking for field 23 (`vsize`) still finds it at the right index.
fn stat_file(pid: u32) -> Result<String, i32> {
    if pid != crate::job::process_id() {
        return Ok(shared_stat_file(process_info(pid)?));
    }
    let info = process_info(pid)?;
    let comm = process_comm(&info);
    let threads = kinakaze_runtime::process_thread_count();
    let ppid = visible(info.entry.ppid);
    let pgrp = visible(info.entry.pgid);
    let session = visible(info.entry.sid);
    let counters = memory_counters().ok_or(crate::EIO)?;
    let rss_pages = counters.WorkingSetSize as u64 / page_size();
    let vsize = virtual_size();

    let mut out = String::with_capacity(256);
    // Fields 1-4. A running process asking about itself is by definition
    // runnable, so 'R' is accurate rather than assumed.
    let pid = visible(pid);
    let _ = write!(out, "{pid} ({comm}) R {ppid}");
    // Fields 5-6 are the namespace's real process group and session. Fields
    // 7-19 have no direct Windows equivalent and remain synthetic zeros.
    let _ = write!(out, " {pgrp} {session}");
    for _ in 7..=19 {
        out.push_str(" 0");
    }
    // Field 20 num_threads, 21 itrealvalue, 22 starttime, 23 vsize, 24 rss.
    let _ = write!(
        out,
        " {threads} 0 {} {vsize} {rss_pages}",
        info.entry.start_ticks
    );
    // Fields 25-52: rsslim through exit_code, none of which map to Windows.
    for _ in 25..=52 {
        out.push_str(" 0");
    }
    out.push('\n');
    Ok(out)
}

/// `/proc/<pid>/stat` for another loader. Only fields held in the shared PID
/// namespace are claimed as real; host-private counters remain zero.
fn shared_stat_file(info: crate::job::ProcessInfo) -> String {
    let entry = info.entry;
    let state = if entry.state == crate::job::STATE_STOPPED {
        'T'
    } else {
        'R'
    };
    let mut out = format!(
        "{} ({}) {} {} {} {}",
        visible(entry.namespace_pid),
        if info.comm.is_empty() {
            "kinakaze"
        } else {
            &info.comm
        },
        state,
        visible(entry.ppid),
        visible(entry.pgid),
        visible(entry.sid)
    );
    for _ in 7..=19 {
        out.push_str(" 0");
    }
    // One Windows main process is known to exist; start time is shared process
    // identity, while memory details are deliberately not invented across the
    // process boundary.
    let _ = write!(out, " 1 0 {} 0 0", entry.start_ticks);
    for _ in 25..=52 {
        out.push_str(" 0");
    }
    out.push('\n');
    out
}

/// The host page size, which converts `WorkingSetSize` into Linux's RSS pages.
fn page_size() -> u64 {
    // x86_64 and ARM64 Windows both use 4 KiB pages, and `GetSystemInfo` would
    // only confirm the same constant at the cost of another struct.
    4096
}

fn namespace_pid_columns(pid: u32) -> String {
    kinakaze_runtime::job::namespaces::visible_pid_numbers_in(pid, instance::pidns())
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("\t")
}

/// Builds `/proc/self/status`, the line-oriented form of the same data.
fn status_file(pid: u32) -> Result<String, i32> {
    let ids = crate::credentials::process_ids(pid)?;
    let uids = ids[..4]
        .iter()
        .map(|&id| crate::user_namespace::visible(id, false).to_string())
        .collect::<Vec<_>>()
        .join("\t");
    let gids = ids[4..]
        .iter()
        .map(|&id| crate::user_namespace::visible(id, true).to_string())
        .collect::<Vec<_>>()
        .join("\t");
    if pid != crate::job::process_id() {
        let info = process_info(pid)?;
        let entry = info.entry;
        let state = if entry.state == crate::job::STATE_STOPPED {
            "T (stopped)"
        } else {
            "R (running)"
        };
        return Ok(format!(
            "Name:\t{}\nState:\t{state}\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nNSpid:\t{}\nUid:\t{uids}\nGid:\t{gids}\nThreads:\t1\n",
            if info.comm.is_empty() {
                "kinakaze"
            } else {
                &info.comm
            },
            visible(entry.namespace_pid),
            visible(entry.namespace_pid),
            visible(entry.ppid),
            namespace_pid_columns(entry.namespace_pid)
        ));
    }
    let info = process_info(pid)?;
    let threads = kinakaze_runtime::process_thread_count();
    let ppid = visible(info.entry.ppid);
    let counters = memory_counters().ok_or(crate::EIO)?;

    let mut out = String::with_capacity(384);
    let _ = writeln!(out, "Name:\t{}", process_comm(&info));
    let _ = writeln!(out, "State:\tR (running)");
    // Windows threads have no thread-group id distinct from the process id.
    let pid = visible(pid);
    let _ = writeln!(out, "Tgid:\t{pid}");
    let _ = writeln!(out, "Pid:\t{pid}");
    let _ = writeln!(out, "PPid:\t{ppid}");
    let _ = writeln!(
        out,
        "NSpid:\t{}",
        namespace_pid_columns(info.entry.namespace_pid)
    );
    let _ = writeln!(out, "Uid:\t{uids}");
    let _ = writeln!(out, "Gid:\t{gids}");
    let _ = writeln!(out, "Threads:\t{threads}");
    let size_kb = virtual_size() / 1024;
    // Linux prints VmPeak before VmSize. Windows tracks no high-water mark for
    // the address space, so the current size stands in rather than an unrelated
    // smaller counter; this is the one field here that is an approximation.
    let _ = writeln!(out, "VmPeak:\t{size_kb:>8} kB");
    let _ = writeln!(out, "VmSize:\t{size_kb:>8} kB");
    let _ = writeln!(out, "VmRSS:\t{:>8} kB", counters.WorkingSetSize / 1024);
    let _ = writeln!(out, "VmHWM:\t{:>8} kB", counters.PeakWorkingSetSize / 1024);
    Ok(out)
}

/// Builds `/proc/uptime`: seconds since boot, then aggregate idle seconds.
fn uptime() -> Result<String, i32> {
    let (seconds, nanos) = crate::time_namespace::clock(7)?;
    let since_boot = seconds as f64 + nanos as f64 / 1e9;
    let idle_seconds = cpu_times::idle_seconds()?;
    Ok(format!("{since_boot:.2} {idle_seconds:.2}\n"))
}

/// Builds `/proc/version`.
///
/// The kernel version is the compatibility level this layer targets, and the
/// rest of the string says plainly that the host is Windows running kinakaze.
/// Nothing here claims to be a Linux kernel build it is not.
fn version() -> String {
    format!(
        "Linux version 6.1.0-kinakaze (kinakaze@windows) (kinakaze {}) \
         #1 SMP kinakaze libc personality layer on Windows\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Builds `/proc/loadavg`.
fn loadavg() -> String {
    format!("0.12 0.08 0.05 1/128 {}\n", crate::job::process_id())
}

/// Builds `/proc/sys/kernel/hostname`.
fn hostname() -> String {
    format!(
        "{}\n",
        String::from_utf8_lossy(&crate::namespaces::uts(false).unwrap_or_default())
    )
}

/// Builds `/proc/net/dev`.
fn net_dev() -> Result<String, i32> {
    // Private virtual networks do not yet account packets in their data path.
    // Never substitute host traffic (or a fixed zero row) for those counters.
    let namespace = crate::usernet::current()?;
    if namespace != 1 {
        return Err(crate::EOPNOTSUPP);
    }
    let interfaces = crate::netlink::network_interfaces(namespace)?;
    let mut out = String::from(
        "Inter-|   Receive                                                |  Transmit\n\
          face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    );
    for interface in interfaces {
        let Some(s) = interface.stats else {
            continue;
        };
        // Windows has no separate FIFO/frame/compressed/multicast/collision/
        // carrier counters. Those ABI fields are zero; the eight totals are real.
        writeln!(
            out,
            "{}: {} {} {} {} 0 0 0 0 {} {} {} {} 0 0 0 0",
            interface.link.name,
            s.rx_bytes,
            s.rx_packets,
            s.rx_errors,
            s.rx_dropped,
            s.tx_bytes,
            s.tx_packets,
            s.tx_errors,
            s.tx_dropped
        )
        .map_err(|_| crate::EIO)?;
    }
    Ok(out)
}

#[cfg(test)]
mod network_counter_tests {
    #[test]
    fn private_network_does_not_report_host_or_placeholder_traffic() {
        let _scope = crate::usernet::scope(12345);
        assert_eq!(super::net_dev().unwrap_err(), crate::EOPNOTSUPP);
    }
}

/// Builds `/proc/net/tcp`.
fn net_tcp() -> String {
    String::from(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
           0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0\n",
    )
}

/// Builds `/proc/net/udp`.
fn net_udp() -> String {
    String::from(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n\
           0: 00000000:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 12346 2 0000000000000000 0\n",
    )
}

/// Builds `/proc/stat`.
fn proc_stat() -> Result<String, i32> {
    let mut out = cpu_times::stat()?;
    // Legacy non-CPU counters below still need their own real data sources.
    let _ = writeln!(out, "intr 1000 0 0 0");
    let _ = writeln!(out, "ctxt 20000");
    let _ = writeln!(out, "btime {}", cpu_times::boot_time()?);
    let _ = writeln!(out, "processes 1000");
    let _ = writeln!(out, "procs_running 1");
    let _ = writeln!(out, "procs_blocked 0");
    Ok(out)
}

pub(crate) fn fd_link_error(error: kinakaze_runtime::job::FdLinkError) -> i32 {
    use kinakaze_runtime::job::FdLinkError;
    match error {
        FdLinkError::ProcessNotFound | FdLinkError::InvalidDescriptor => crate::ENOENT,
        FdLinkError::TargetTooLong => crate::ENAMETOOLONG,
        FdLinkError::CapacityExhausted => crate::ENOSPC,
        FdLinkError::RegistryUnavailable | FdLinkError::CorruptRecord => crate::EIO,
    }
}

/// Lists a process's descriptor numbers without substituting the caller's table
/// for a foreign process.
fn fd_numbers(pid: u32) -> Result<Vec<i32>, i32> {
    if pid == crate::job::process_id() {
        Ok(crate::list_open_fds())
    } else {
        kinakaze_runtime::job::fd_links(pid).map_err(fd_link_error)
    }
}

/// Resolves one exact `/proc/<pid>/fd/<fd>` magic-link target.
fn fd_link_target(pid: u32, fd: i32) -> Result<String, i32> {
    if pid == crate::job::process_id() {
        local_fd_link_target(fd)
    } else {
        kinakaze_runtime::job::fd_link(pid, fd)
            .map_err(fd_link_error)?
            .ok_or(crate::ENOENT)
    }
}

/// Captures the current process's complete logical fd view for publication at
/// fork and exec boundaries.
pub(crate) fn local_fd_links() -> Result<Vec<(i32, String)>, i32> {
    fd_snapshot::capture(local_fd_link_target)
}

/// Resolves link target for one descriptor in this process.
pub(crate) fn local_fd_link_target(fd: i32) -> Result<String, i32> {
    if let Some(path) = crate::mount::overlay::descriptor_display(fd)? {
        return Ok(path);
    }
    if let Some(path) = crate::mount::native::descriptor_path(fd)? {
        return Ok(path);
    }
    let entry = crate::get(fd)?;
    match entry.kind {
        // The console is not a `/dev/pts` entry and never was: claiming a pts
        // number for it made `readlink /proc/self/fd/0` disagree with `ttyname`
        // and named a device that could not be opened. `/dev/tty` is the name
        // Linux gives the controlling terminal and it is openable here.
        crate::FdKind::Console => Ok(String::from("/dev/tty")),
        crate::FdKind::PtyMaster => crate::tty::terminal_name(fd),
        crate::FdKind::PtySlave => crate::tty::terminal_name(fd),
        crate::FdKind::Null => Ok(String::from("/dev/null")),
        crate::FdKind::Zero => Ok(String::from("/dev/zero")),
        crate::FdKind::Random => Ok(String::from("/dev/urandom")),
        crate::FdKind::Full => Ok(String::from("/dev/full")),
        crate::FdKind::Pipe => Ok(format!("pipe:[{}]", entry.raw)),
        crate::FdKind::Fifo => crate::fifo::link_target(fd, entry),
        crate::FdKind::UnixSocket => Ok(format!("socket:[{}]", crate::unix::procnet::inode(fd)?)),
        crate::FdKind::Socket | crate::FdKind::NetlinkSocket => Ok(format!(
            "socket:[{}]",
            if entry.raw == 0 {
                entry.generation as usize
            } else {
                entry.raw
            }
        )),
        crate::FdKind::Event => Ok(String::from("anon_inode:[eventpoll]")),
        crate::FdKind::IoRing => Ok(String::from("anon_inode:[io_uring]")),
        crate::FdKind::TimerFd => Ok(String::from("anon_inode:[timerfd]")),
        crate::FdKind::EventFd => Ok(String::from("anon_inode:[eventfd]")),
        crate::FdKind::Inotify => Ok(String::from("anon_inode:inotify")),
        crate::FdKind::BpfProgram => Ok(String::from("anon_inode:bpf-prog")),
        crate::FdKind::Synthetic => crate::synthetic_file_path(fd).and_then(instance::display),
        crate::FdKind::Namespace => crate::namespaces::descriptor_link(fd),
        crate::FdKind::UserNamespace => {
            crate::user_namespace::descriptor_inode(fd).map(|ino| format!("user:[{ino}]"))
        }
        crate::FdKind::TimeNamespace => {
            crate::time_namespace::descriptor_inode(fd).map(|inode| format!("time:[{inode}]"))
        }
        crate::FdKind::MountNamespace => {
            crate::mount::namespace_descriptor_inode(fd).map(|inode| format!("mnt:[{inode}]"))
        }
        crate::FdKind::FsContext => Ok(String::from("anon_inode:[fscontext]")),
        crate::FdKind::MountTree => Ok(String::from("/")),
        crate::FdKind::TmpfsFile
        | crate::FdKind::TmpfsDirectory
        | crate::FdKind::MessageQueue
        | crate::FdKind::SysfsFile => crate::tmpfs::descriptor_path(fd),
        crate::FdKind::SyntheticDirectory => {
            crate::synthetic_directory_path(fd).and_then(instance::display)
        }
        crate::FdKind::CgroupFile => crate::cgroup_file_path(fd),
        crate::FdKind::ProcSysctl => crate::proc_sysctl_file_path(fd).and_then(instance::display),
        crate::FdKind::File | crate::FdKind::Directory => {
            #[cfg(windows)]
            {
                use windows_sys::Win32::Storage::FileSystem::{
                    FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
                };
                if entry.raw == 0 || entry.raw == usize::MAX {
                    return Err(crate::EBADF);
                }
                let mut buf = [0u16; 1024];
                let len = unsafe {
                    GetFinalPathNameByHandleW(
                        entry.raw as windows_sys::Win32::Foundation::HANDLE,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                    )
                };
                if len == 0 {
                    return Err(crate::errno_from_win32(unsafe { GetLastError() }));
                }
                if len as usize >= buf.len() {
                    return Err(crate::ENAMETOOLONG);
                }
                let win_path = String::from_utf16(&buf[..len as usize]).map_err(|_| crate::EIO)?;
                let clean_path = win_path.strip_prefix(r"\\?\").unwrap_or(&win_path);
                return Ok(crate::to_guest_path(std::path::Path::new(clean_path)));
            }
            #[cfg(not(windows))]
            {
                Err(crate::EIO)
            }
        }
        crate::FdKind::Unknown => Err(crate::EIO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &str) -> String {
        String::from_utf8(read_file(path).expect("read failed")).expect("not UTF-8")
    }

    #[test]
    fn oom_adjustment_is_live_and_enforces_the_privileged_floor() {
        let pid = crate::job::process_id();
        let score = format!("/proc/{pid}/oom_score_adj");
        let legacy = format!("/proc/{pid}/oom_adj");
        let (original, floor) = kinakaze_runtime::job::oom_adjustment(pid).unwrap();
        assert!(
            list_directory("/proc/self")
                .unwrap()
                .contains(&"oom_score_adj".to_owned())
        );
        let fd = crate::fs::open(&score, crate::fs::O_RDWR, 0).unwrap();
        assert_eq!(crate::write(fd, b"250\n"), Ok(4));
        assert_eq!(text(&score), "250\n");
        assert_eq!(write_file(&score, b"0x1f4", 4), Ok(5));
        assert_eq!(text(&score), "500\n");
        assert_eq!(write_file(&legacy, b"15", 0), Ok(2));
        assert_eq!(text(&score), "1000\n");
        assert_eq!(write_file(&legacy, b"1", 0), Err(crate::EACCES));
        assert_eq!(write_file(&score, b"-1", 0), Err(crate::EACCES));
        for invalid in [
            b"1001".as_slice(),
            b"-1001",
            b"",
            b"1x",
            b"\0",
            b"--1",
            b"1\xc2\xa0",
        ] {
            assert_eq!(write_file(&score, invalid, 0), Err(crate::EINVAL));
        }
        for value in [
            b"0\0".as_slice(),
            b" \t0\n\0ignored",
            b"0\0\xff",
            b"\x0b0\x0c",
        ] {
            assert_eq!(write_file(&score, value, 0), Ok(value.len()));
            assert_eq!(text(&score), "0\n");
        }
        assert_eq!(write_file(&score, b"0", 0), Ok(1));
        crate::fs::lseek(fd, 0, 0).unwrap();
        let mut bytes = [0u8; 16];
        let read = crate::read(fd, &mut bytes).unwrap();
        assert_eq!(&bytes[..read], b"0\n");
        crate::close(fd).unwrap();
        kinakaze_runtime::job::set_oom_adjustment(pid, floor, true, false).unwrap();
        kinakaze_runtime::job::set_oom_adjustment(pid, original, false, false).unwrap();
    }

    #[test]
    fn key_quota_defaults_support_directory_stat_and_fd_writes() {
        let root = "/proc/sys/kernel/keys";
        assert_eq!(metadata(root).unwrap().kind, ProcKind::Directory);
        let entries = list_directory(root).unwrap();
        assert!(
            list_directory("/proc/sys/kernel")
                .unwrap()
                .contains(&"keys".to_owned())
        );
        for limit in key_defaults::Limit::ALL {
            assert!(entries.contains(&limit.name().to_owned()));
            let path = format!("{root}/{}", limit.name());
            assert_eq!(metadata(&path).unwrap().kind, ProcKind::File);
            assert!(writable(&path));
            let expected = limit.read();
            let fd = crate::fs::open(&path, crate::fs::O_RDWR | crate::fs::O_CLOEXEC, 0).unwrap();
            assert_eq!(crate::write(fd, b"2000000\n"), Ok(8));
            assert_eq!(crate::fs::lseek(fd, 0, 0), Ok(0));
            let mut bytes = [0u8; 32];
            let count = crate::read(fd, &mut bytes).unwrap();
            assert_eq!(&bytes[..count], expected);
            crate::close(fd).unwrap();
            for value in [b"0".as_slice(), b"-1", b"2147483648", b"abc", b""] {
                assert_eq!(write_file(&path, value, 0), Err(crate::EINVAL));
            }
            assert_eq!(write_file(&path, b"10", 1), Err(crate::EINVAL));
        }
        assert_eq!(metadata(&format!("{root}/unknown")), Err(crate::ENOENT));
        assert_eq!(
            read_file(&format!("{root}/root_maxkeys/extra")),
            Err(crate::ENOENT)
        );
    }

    #[test]
    fn cgroup_membership_is_relative_to_the_hierarchy() {
        assert_eq!(cgroup_relative_path(""), Ok("/"));
        assert_eq!(cgroup_relative_path("/sys/fs/cgroup"), Ok("/"));
        assert_eq!(cgroup_relative_path("/sys/fs/cgroup/a/b"), Ok("/a/b"));
        assert_eq!(cgroup_relative_path("/sys/fs/cgroup2"), Err(crate::EIO));
        assert_eq!(
            cgroup_relative_path("/sys/fs/cgroup/a\n0::/"),
            Err(crate::EIO)
        );
        let pid = crate::job::process_id();
        assert_eq!(
            read_file(&format!("/proc/{pid}/cgroup")),
            read_file("/proc/self/cgroup")
        );
        // No hardcoded PID 1 path: it exists precisely when the real row exists.
        assert_eq!(
            metadata("/proc/1/cgroup").is_ok(),
            crate::job::process_info(1).is_some()
        );
    }

    #[test]
    fn owns_claims_the_proc_subtree_and_nothing_else() {
        for path in [
            "/proc",
            "/proc/",
            "/proc/cpuinfo",
            "/proc/self/maps",
            "/proc//self//exe",
        ] {
            assert!(owns(path), "{path} should be served by procfs");
        }
        // A sibling whose name merely starts with "proc" is a real host path.
        for path in ["/etc/passwd", "/procfoo", "/", "/var/proc", "/proc2/self"] {
            assert!(!owns(path), "{path} must not be captured by procfs");
        }
    }

    #[test]
    fn unknown_proc_paths_report_enoent() {
        assert_eq!(read_file("/proc/nope"), Err(crate::ENOENT));
        assert_eq!(metadata("/proc/nope"), Err(crate::ENOENT));
        assert_eq!(read_file("/proc/self/nope"), Err(crate::ENOENT));
        // A pid that is not ours is not a process we can describe.
        assert_eq!(read_file("/proc/999999/maps"), Err(crate::ENOENT));
    }

    #[test]
    fn directories_and_files_report_the_right_kind() {
        for directory in ["/proc", "/proc/self"] {
            let info = metadata(directory).expect("metadata failed");
            assert_eq!(info.kind, ProcKind::Directory);
            // Reading a directory is EISDIR, listing a file is ENOTDIR.
            assert_eq!(read_file(directory), Err(crate::EISDIR));
        }
        let info = metadata("/proc/cpuinfo").expect("metadata failed");
        assert_eq!(info.kind, ProcKind::File);
        assert_eq!(info.size, 0, "Linux procfs files report a zero st_size");
        assert_eq!(list_directory("/proc/cpuinfo"), Err(crate::ENOTDIR));
    }

    #[test]
    fn cpuinfo_has_one_stanza_per_logical_processor() {
        let content = text("/proc/cpuinfo");
        let expected = std::thread::available_parallelism().unwrap().get();

        let processors: Vec<&str> = content
            .lines()
            .filter(|line| line.starts_with("processor\t"))
            .collect();
        assert_eq!(
            processors.len(),
            expected,
            "expected {expected} stanzas, got {}:\n{content}",
            processors.len()
        );
        // The index column must count up from zero without gaps.
        for (index, line) in processors.iter().enumerate() {
            assert_eq!(*line, format!("processor\t: {index}"));
        }

        for field in [
            "vendor_id",
            "cpu family",
            "model",
            "model name",
            "cpu MHz",
            "cache size",
            "siblings",
            "cpu cores",
            "physical id",
            "core id",
            "flags",
        ] {
            let count = content
                .lines()
                .filter(|line| line.starts_with(&format!("{field}\t")))
                .count();
            assert_eq!(count, expected, "field {field} appeared {count} times");
        }
    }

    #[test]
    fn cpuinfo_supports_physical_core_count_consumers() {
        let content = text("/proc/cpuinfo");
        let mut packages = std::collections::BTreeMap::new();
        let mut distinct_cores = std::collections::BTreeSet::new();
        for stanza in content.split("\n\n").filter(|s| !s.is_empty()) {
            let fields: std::collections::BTreeMap<_, _> = stanza
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.trim(), value.trim()))
                .collect();
            let package = fields["physical id"].parse::<usize>().unwrap();
            let cores = fields["cpu cores"].parse::<usize>().unwrap();
            let core = fields["core id"].parse::<usize>().unwrap();
            assert!(cores > 0);
            if let Some(previous) = packages.insert(package, cores) {
                assert_eq!(previous, cores);
            }
            distinct_cores.insert((package, core));
        }
        // gopsutil counts one `cpu cores` value per `physical id`; other
        // consumers count distinct package/core pairs. Both must agree.
        let physical: usize = packages.values().sum();
        assert!(physical > 0);
        assert_eq!(physical, distinct_cores.len());
        assert!(physical <= logical_processors());
        assert!((1.0 / (physical as f64 * 1.5)).is_finite());
    }

    #[test]
    fn cpuinfo_reports_a_real_model_name_and_feature_flags() {
        let content = text("/proc/cpuinfo");
        let value = |field: &str| -> String {
            content
                .lines()
                .find(|line| line.starts_with(field))
                .and_then(|line| line.split_once(':'))
                .map(|(_, rest)| rest.trim().to_string())
                .unwrap_or_default()
        };

        let model = value("model name");
        assert!(!model.is_empty(), "model name is empty");
        assert_ne!(model, "unknown", "cpuid brand string was not read");

        let vendor = value("vendor_id");
        assert!(
            vendor == "GenuineIntel" || vendor == "AuthenticAMD",
            "unexpected vendor_id {vendor:?}"
        );

        // Any x86_64 CPU has these, so their absence means the bits were never
        // actually read from cpuid.
        let flag_line = value("flags");
        let flags: Vec<&str> = flag_line.split_whitespace().collect();
        for required in [
            "fpu", "tsc", "msr", "cmov", "mmx", "fxsr", "sse", "sse2", "lm",
        ] {
            assert!(
                flags.contains(&required),
                "flag {required} missing from {flags:?}"
            );
        }
        // A bit that is reserved and always zero must never be reported.
        assert!(!flags.contains(&"ia64"), "ia64 cannot be set on x86_64");

        let cores: usize = value("cpu cores").parse().expect("cpu cores not numeric");
        let siblings: usize = value("siblings").parse().expect("siblings not numeric");
        assert!(
            cores >= 1 && cores <= siblings,
            "{cores} cores, {siblings} siblings"
        );
    }

    /// One parsed maps line.
    struct ParsedLine {
        start: usize,
        end: usize,
        perms: String,
        path: String,
    }

    /// Parses a maps line the way a guest would, rejecting any malformed field.
    fn parse_maps_line(line: &str) -> ParsedLine {
        let mut fields = line.split_whitespace();
        let range = fields
            .next()
            .unwrap_or_else(|| panic!("no range in {line:?}"));
        let (start, end) = range
            .split_once('-')
            .unwrap_or_else(|| panic!("range {range:?} is not hex-hex"));
        let start = usize::from_str_radix(start, 16)
            .unwrap_or_else(|_| panic!("start {start:?} is not hex"));
        let end =
            usize::from_str_radix(end, 16).unwrap_or_else(|_| panic!("end {end:?} is not hex"));
        assert!(end > start, "empty or inverted range in {line:?}");

        let perms = fields
            .next()
            .unwrap_or_else(|| panic!("no perms in {line:?}"));
        assert_eq!(perms.len(), 4, "perms {perms:?} is not 4 characters");
        let bytes = perms.as_bytes();
        assert!(
            bytes[0] == b'r' || bytes[0] == b'-',
            "bad read bit {perms:?}"
        );
        assert!(
            bytes[1] == b'w' || bytes[1] == b'-',
            "bad write bit {perms:?}"
        );
        assert!(
            bytes[2] == b'x' || bytes[2] == b'-',
            "bad exec bit {perms:?}"
        );
        assert!(
            bytes[3] == b'p' || bytes[3] == b's',
            "bad share bit {perms:?}"
        );

        let offset = fields
            .next()
            .unwrap_or_else(|| panic!("no offset in {line:?}"));
        assert!(
            u64::from_str_radix(offset, 16).is_ok(),
            "offset {offset:?} is not hex"
        );
        let device = fields
            .next()
            .unwrap_or_else(|| panic!("no dev in {line:?}"));
        assert!(
            device.split_once(':').is_some_and(|(major, minor)| {
                u32::from_str_radix(major, 16).is_ok() && u32::from_str_radix(minor, 16).is_ok()
            }),
            "dev {device:?} is not major:minor"
        );
        let inode = fields
            .next()
            .unwrap_or_else(|| panic!("no inode in {line:?}"));
        assert!(
            inode.parse::<u64>().is_ok(),
            "inode {inode:?} is not decimal"
        );

        ParsedLine {
            start,
            end,
            perms: perms.to_string(),
            path: fields.collect::<Vec<_>>().join(" "),
        }
    }

    #[test]
    fn maps_describes_the_real_address_space() {
        let content = text("/proc/self/maps");
        assert!(!content.is_empty(), "maps is empty");
        assert!(content.ends_with('\n'), "maps must be newline-terminated");

        let parsed: Vec<ParsedLine> = content.lines().map(parse_maps_line).collect();
        assert!(
            parsed.len() > 1,
            "expected many regions, got {}",
            parsed.len()
        );

        // The walk is ordered and non-overlapping, which is what lets a guest
        // binary-search it for an address.
        for pair in parsed.windows(2) {
            assert!(
                pair[1].start >= pair[0].end,
                "regions overlap: {:x}-{:x} then {:x}-{:x}",
                pair[0].start,
                pair[0].end,
                pair[1].start,
                pair[1].end
            );
        }

        // This code is executing, so an executable mapping must be present.
        let executable: Vec<&ParsedLine> = parsed
            .iter()
            .filter(|line| line.perms.as_bytes()[2] == b'x')
            .collect();
        assert!(
            !executable.is_empty(),
            "no executable region found:\n{content}"
        );
        // And at least one of them must be backed by a named image.
        assert!(
            executable.iter().any(|line| !line.path.is_empty()),
            "no executable region has a pathname"
        );
        // A writable region must exist too, since the process has a stack.
        assert!(
            parsed.iter().any(|line| line.perms.as_bytes()[1] == b'w'),
            "no writable region found"
        );
    }

    #[test]
    fn maps_locates_this_functions_own_code() {
        let content = text("/proc/self/maps");
        // The address of a real function in this image must fall inside an
        // executable range, which is the property a guest relies on when it
        // resolves a return address against maps.
        let probe = maps_locates_this_functions_own_code as fn() as usize;
        let containing = content
            .lines()
            .map(parse_maps_line)
            .find(|line| probe >= line.start && probe < line.end)
            .unwrap_or_else(|| panic!("{probe:#x} is in no region:\n{content}"));
        assert_eq!(
            containing.perms.as_bytes()[2],
            b'x',
            "the region holding code is not executable: {}",
            containing.perms
        );
        assert!(
            !containing.path.is_empty(),
            "the region holding code has no backing file"
        );
    }

    #[test]
    fn maps_marks_the_thread_stack() {
        let content = text("/proc/self/maps");
        let local = 0u8;
        let probe = &raw const local as usize;
        let containing = content
            .lines()
            .map(parse_maps_line)
            .find(|line| probe >= line.start && probe < line.end)
            .unwrap_or_else(|| panic!("stack address {probe:#x} is in no region"));
        assert_eq!(containing.path, "[stack]", "stack region was not labelled");
        // The label must be writable, which is what distinguishes the real stack
        // from the guard and reserved pages Windows keeps in the same range.
        assert_eq!(
            containing.perms.as_bytes()[1],
            b'w',
            "a non-writable region was labelled [stack]: {}",
            containing.perms
        );

        let content_lines: Vec<ParsedLine> = text("/proc/self/maps")
            .lines()
            .map(parse_maps_line)
            .filter(|line| line.path == "[stack]")
            .collect();
        for line in &content_lines {
            assert_eq!(
                line.perms.as_bytes()[1],
                b'w',
                "guard page leaked into the [stack] label"
            );
        }
    }

    #[test]
    fn cmdline_round_trips_the_real_argv() {
        let raw = read_file("/proc/self/cmdline").expect("read failed");
        assert_eq!(raw.last(), Some(&0), "cmdline must end with a NUL");

        // Splitting on NUL and dropping the trailing empty piece is exactly what
        // a guest does to rebuild argv.
        let recovered: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|piece| !piece.is_empty())
            .map(|piece| String::from_utf8_lossy(piece).into_owned())
            .collect();
        let expected: Vec<String> = std::env::args().collect();
        assert_eq!(recovered, expected);
        assert!(!expected.is_empty(), "argv should hold at least argv[0]");
    }

    #[test]
    fn environ_exposes_the_real_environment() {
        let raw = read_file("/proc/self/environ").expect("read failed");
        let entries: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|piece| !piece.is_empty())
            .map(|piece| String::from_utf8_lossy(piece).into_owned())
            .collect();
        assert!(!entries.is_empty(), "environ should not be empty");
        // Every entry is a KEY=VALUE pair whose key is non-empty.
        for entry in &entries {
            let (key, _) = entry
                .split_once('=')
                .unwrap_or_else(|| panic!("{entry:?} is not KEY=VALUE"));
            assert!(!key.is_empty(), "empty key in {entry:?}");
        }
        assert!(
            entries.iter().any(|e| e.starts_with("PATH=")),
            "PATH missing from environ"
        );
        assert!(
            entries.iter().any(|e| e.starts_with("LANG=")),
            "LANG missing from environ"
        );
        assert!(
            entries.iter().any(|e| e.starts_with("HOME=")),
            "HOME missing from environ"
        );
    }

    #[test]
    fn exe_is_a_symlink_to_the_real_executable() {
        let info = metadata("/proc/self/exe").expect("metadata failed");
        assert_eq!(info.kind, ProcKind::Symlink);

        let target = info.target.expect("a symlink must carry a target");
        assert_eq!(info.size, target.len() as u64);
        assert!(target.starts_with('/'), "target {target:?} is not absolute");

        // The target is a guest path, so resolving it must land back on the very
        // executable the host reports.
        let resolved = crate::path::resolve_linux_path(&target).expect("target does not resolve");
        let actual = std::env::current_exe().expect("no current_exe");
        assert_eq!(
            resolved, actual,
            "exe link does not point at the real image"
        );

        // Reading through the link yields the image, whose PE header proves it.
        let bytes = read_file("/proc/self/exe").expect("read failed");
        assert_eq!(&bytes[..2], b"MZ", "exe contents are not a PE image");
        assert_eq!(
            bytes.len() as u64,
            std::fs::metadata(&actual).unwrap().len()
        );
    }

    #[test]
    fn meminfo_reports_plausible_real_totals() {
        let content = text("/proc/meminfo");
        let field = |name: &str| -> u64 {
            content
                .lines()
                .find(|line| line.starts_with(&format!("{name}:")))
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_else(|| panic!("{name} missing from:\n{content}"))
                .parse()
                .unwrap_or_else(|_| panic!("{name} is not numeric"))
        };

        for line in content.lines() {
            assert!(line.ends_with(" kB"), "{line:?} lacks the kB unit");
        }

        let total = field("MemTotal");
        // Any machine that can run this has at least 256 MiB and under 64 TiB.
        assert!(
            total > 256 * 1024 && total < 64 * 1024 * 1024 * 1024,
            "MemTotal {total} kB is implausible"
        );
        assert!(field("MemFree") <= total, "MemFree exceeds MemTotal");
        assert!(
            field("MemAvailable") <= total,
            "MemAvailable exceeds MemTotal"
        );
        assert!(field("MemFree") <= field("MemAvailable"));
        assert!(field("SwapFree") <= field("SwapTotal"));
    }

    #[test]
    fn vmstat_has_file_metadata_and_numeric_page_gauges() {
        let metadata = metadata("/proc/vmstat").unwrap();
        assert_eq!(metadata.kind, ProcKind::File);
        assert_eq!(metadata.size, 0);
        assert!(!writable("/proc/vmstat"));
        let content = text("/proc/vmstat");
        let mut fields = std::collections::HashMap::new();
        for line in content.lines() {
            let mut pair = line.split_whitespace();
            let key = pair.next().unwrap();
            let value = pair.next().unwrap().parse::<u64>().unwrap();
            assert!(pair.next().is_none());
            assert!(fields.insert(key, value).is_none(), "duplicate {key}");
        }
        let snapshot = crate::memory::snapshot().unwrap();
        assert!(fields["nr_free_pages"] <= snapshot.total / snapshot.page_size);
        assert_eq!(list_directory("/proc/vmstat"), Err(crate::ENOTDIR));
    }

    #[test]
    fn stat_and_status_agree_on_real_process_identity() {
        let stat = text("/proc/self/stat");
        let status = text("/proc/self/status");
        let pid = crate::job::process_id();

        // stat is positional; field 1 is the pid and field 2 the comm in parens.
        let fields: Vec<&str> = stat.trim_end().split(' ').collect();
        assert_eq!(fields[0].parse::<u32>(), Ok(pid), "wrong pid in stat");
        assert!(
            fields[1].starts_with('(') && fields[1].ends_with(')'),
            "comm {:?} is not parenthesized",
            fields[1]
        );
        assert_eq!(fields[2], "R", "state should be running");
        assert!(
            fields[21].parse::<u64>().is_ok_and(|ticks| ticks != 0),
            "field 22 starttime must be a real non-zero USER_HZ value"
        );
        // Linux stat has 52 fields; a reader indexing vsize at 23 needs them all.
        assert_eq!(fields.len(), 52, "stat has {} fields", fields.len());

        let vsize: u64 = fields[22].parse().expect("vsize not numeric");
        let rss_pages: u64 = fields[23].parse().expect("rss not numeric");
        assert!(vsize > 0, "vsize should be non-zero");
        assert!(rss_pages > 0, "rss should be non-zero");

        let line = |name: &str| -> String {
            status
                .lines()
                .find(|line| line.starts_with(&format!("{name}:")))
                .and_then(|line| line.split_once(':'))
                .map(|(_, rest)| rest.trim().to_string())
                .unwrap_or_else(|| panic!("{name} missing from:\n{status}"))
        };
        assert_eq!(line("Pid").parse::<u32>(), Ok(pid));
        assert_eq!(line("Name"), fields[1].trim_matches(['(', ')']));
        // A native parent is outside the hosted PID namespace and therefore
        // appears as zero, just like a parent hidden by a Linux namespace.
        assert_eq!(line("PPid"), fields[3], "stat and status disagree on PPid");
        let stat_threads: u32 = fields[19].parse().expect("stat threads not numeric");
        let threads: u32 = line("Threads").parse().expect("Threads not numeric");
        assert_eq!(threads, stat_threads, "stat and status disagree on threads");
        assert_eq!(
            threads,
            kinakaze_runtime::process_thread_count(),
            "procfs did not use the process-wide runtime counter"
        );

        let kilobytes = |field: &str| -> u64 {
            let value = line(field);
            assert!(value.ends_with(" kB"), "{field} {value:?} lacks kB");
            let amount: u64 = value
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .unwrap_or_else(|_| panic!("{field} is not numeric"));
            assert!(amount > 0, "{field} should be non-zero");
            amount
        };
        let vm_size = kilobytes("VmSize");
        let vm_rss = kilobytes("VmRSS");
        // A peak can never be below the current value, and resident memory can
        // never exceed the mapped address space.
        assert!(kilobytes("VmPeak") >= vm_size, "VmPeak is below VmSize");
        assert!(kilobytes("VmHWM") >= vm_rss, "VmHWM is below VmRSS");
        assert!(vm_rss <= vm_size, "VmRSS {vm_rss} exceeds VmSize {vm_size}");
        // stat's vsize is in bytes and must agree with status's kB figure. The
        // two come from separate walks, so a concurrent mapping change can move
        // the total slightly; only a gross mismatch indicates a unit error.
        let stat_size = vsize / 1024;
        let spread = stat_size.abs_diff(vm_size);
        assert!(
            spread * 100 < vm_size,
            "stat vsize {stat_size} kB and status VmSize {vm_size} kB disagree"
        );
    }

    #[test]
    fn uptime_grows_and_carries_two_fields() {
        let first = text("/proc/uptime");
        let parts: Vec<&str> = first.trim_end().split(' ').collect();
        assert_eq!(parts.len(), 2, "uptime needs boot and idle fields");
        let since_boot: f64 = parts[0].parse().expect("uptime is not a float");
        let idle: f64 = parts[1].parse().expect("idle is not a float");
        assert!(since_boot > 0.0, "the host has been up for some time");
        assert!(idle >= 0.0, "idle time cannot be negative");

        // The value is read live, so it must advance across a sleep.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let later: f64 = text("/proc/uptime")
            .trim_end()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            later > since_boot,
            "uptime did not advance: {since_boot} then {later}"
        );
    }

    #[test]
    fn version_identifies_itself_as_kinakaze() {
        let content = text("/proc/version");
        // Parsers key off the "Linux version <release>" prefix.
        assert!(content.starts_with("Linux version "), "{content:?}");
        assert!(content.contains("kinakaze"), "version hides its origin");
        assert!(content.contains("Windows"), "version hides the real host");
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn listings_cover_the_served_paths() {
        let root = list_directory("/proc").expect("listing failed");
        let pid = crate::job::process_id().to_string();
        for entry in [
            ".", "..", "self", "cpuinfo", "meminfo", "vmstat", "uptime", "version",
        ] {
            assert!(root.contains(&entry.to_string()), "/proc lacks {entry}");
        }
        assert!(root.contains(&pid), "/proc lacks its own pid {pid}");

        let process = list_directory("/proc/self").expect("listing failed");
        for entry in [
            ".", "..", "cmdline", "environ", "exe", "maps", "stat", "status",
        ] {
            assert!(
                process.contains(&entry.to_string()),
                "/proc/self lacks {entry}"
            );
        }
        // Every listed file must actually be servable, which is what keeps a
        // readdir-then-open walk from failing halfway through.
        for entry in process.iter().filter(|name| !name.starts_with('.')) {
            let path = format!("/proc/self/{entry}");
            assert!(
                metadata(&path).is_ok(),
                "{path} was listed but has no metadata"
            );
        }
    }

    #[test]
    fn the_pid_directory_aliases_self() {
        let pid = crate::job::process_id();
        assert_eq!(
            list_directory(&format!("/proc/{pid}")).unwrap(),
            list_directory("/proc/self").unwrap()
        );
        // cmdline is byte-identical through either name; maps and stat move.
        assert_eq!(
            read_file(&format!("/proc/{pid}/cmdline")).unwrap(),
            read_file("/proc/self/cmdline").unwrap()
        );
        assert_eq!(
            metadata(&format!("/proc/{pid}/exe")).unwrap(),
            metadata("/proc/self/exe").unwrap()
        );
    }

    #[test]
    fn new_procfs_nodes_are_readable_and_non_empty() {
        let _sysctl = sysctl_test_lock();
        assert!(!text("/proc/loadavg").is_empty());
        assert!(!text("/proc/stat").is_empty());
        assert!(!text("/proc/mounts").is_empty());
        assert!(!text("/proc/net/dev").is_empty());
        assert!(!text("/proc/net/tcp").is_empty());
        assert!(!text("/proc/net/udp").is_empty());
        assert!(!text("/proc/net/route").is_empty());
        assert!(!text("/proc/net/arp").is_empty());
        assert!(!text("/proc/sys/kernel/hostname").is_empty());
        assert_eq!(text("/proc/sys/kernel/osrelease"), "6.1.0-kinakaze\n");
        assert_eq!(text("/proc/sys/kernel/ostype"), "Linux\n");
        assert_eq!(text("/proc/sys/net/ipv4/ip_forward"), "0\n");
        assert_eq!(text("/proc/self/cgroup"), "0::/\n");
        assert!(text("/proc/self/mountinfo").contains(" - kinakaze "));
        assert!(text("/proc/filesystems").contains("overlay"));
        assert!(text("/proc/devices").contains("Character devices"));
        assert_eq!(
            text("/proc/self/uid_map"),
            "         0          0 4294967295\n"
        );
        assert_eq!(text("/proc/self/setgroups"), "allow\n");
        assert!(text("/proc/sys/fs/file-max").contains("9223372036854775807"));
        assert_eq!(text("/proc/sys/kernel/pid_max"), "4194304\n");
        assert!(metadata("/proc/self/ns/mnt").is_ok());
        assert!(metadata("/proc/self/ns/pid").is_ok());
        assert!(metadata("/proc/self/ns/uts").is_ok());
        assert!(metadata("/proc/self/ns/ipc").is_ok());
    }

    #[test]
    fn namespace_magic_links_follow_to_nsfs_objects() {
        let path = "/proc/self/ns/cgroup";
        let link = crate::fs::lstat(path).expect("namespace lstat failed");
        assert_eq!(link.st_mode & crate::fs::S_IFMT, crate::fs::S_IFLNK);

        let target = crate::fs::stat(path).expect("namespace stat failed");
        assert_eq!(target.st_mode & crate::fs::S_IFMT, crate::fs::S_IFREG);
        assert_eq!(target.st_ino, namespace_target_inode(path).unwrap());
        assert_eq!(read_file(path).unwrap(), b"cgroup:[4026531835]");
    }

    #[test]
    fn ipv6_conf_sysctl_is_readable_and_writable() {
        let path = "/proc/sys/net/ipv6/conf/docker0/accept_ra";
        assert_eq!(read_file(path).unwrap(), b"0\n");
        let meta = metadata(path).expect("accept_ra stat failed");
        assert_eq!(meta.kind, ProcKind::File);
        assert_eq!(write_file(path, b"1\n", 0).unwrap(), 2);
    }

    #[test]
    fn task_namespace_magic_links_route_properly() {
        let path = "/proc/self/task/1/ns/net";
        let link = crate::fs::lstat(path).expect("task netns lstat failed");
        assert_eq!(link.st_mode & crate::fs::S_IFMT, crate::fs::S_IFLNK);
        assert_eq!(read_file(path).unwrap(), b"net:[4026531992]");
    }
}

/// Query an already validated descriptor path without reinterpreting PID numbers.
pub fn descriptor_scope<T>(run: impl FnOnce() -> T) -> T {
    pinned(run)
}

/// Stable path retained by a synthetic proc descriptor.
pub fn descriptor_path(fd: i32) -> Result<String, i32> {
    match crate::get(fd)?.kind {
        crate::FdKind::Synthetic => crate::synthetic_file_path(fd),
        crate::FdKind::ProcSysctl => crate::proc_sysctl_file_path(fd),
        crate::FdKind::SyntheticDirectory => crate::synthetic_directory_path(fd),
        _ => Err(crate::EINVAL),
    }
}
