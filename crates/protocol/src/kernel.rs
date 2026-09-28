//! Shared-object identities and the bounded control plane for their owner.
use serde::{Deserialize, Serialize};

pub const STORE_MAGIC: u64 = u64::from_le_bytes(*b"CYMNT004");
pub const HEADER_SIZE: usize = 65536;
pub const BANK_SIZE: usize = 32 * 1024 * 1024;
pub const SECTION_SIZE: usize = HEADER_SIZE + BANK_SIZE * 2;
pub const TOPOLOGY_WORD: usize = 7;
pub const TIME_MAGIC: u64 = u64::from_le_bytes(*b"CYTIME01");
pub const BPF_MAGIC: u64 = 0x4352_5942_5046_3032;
pub const SHM_MAGIC: u64 = 0x4352_5953_5348_4d02;
pub const SEM_MAGIC: u64 = 0x4352_5953_5345_4d02;
pub const POLICY_MAGIC: u64 = u64::from_le_bytes(*b"CYMPOL02");
pub const PROCESS_TABLE_MAGIC: u64 = 0x4352_5950_4944_3137;
pub const CGROUP_CATALOG: u64 = u64::MAX - 26;
pub const CGROUP_MAGIC: u64 = u64::from_le_bytes(*b"CRYCG003");
pub const RESOURCE_MAGIC: u64 = u64::from_le_bytes(*b"CYRES001");
pub const MAX_NATIVE_RESOURCES: usize = 2048;
pub const MAX_INLINE_RESOURCES: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum ResourceInput {
    Inline(Vec<u8>),
    Section { source: u64, length: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IpcKind {
    Memory,
    Semaphore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ObjectKey {
    Mount(u64),
    Shared(u64),
    Time(u64),
    Bpf,
    ProcessTable,
    CgroupJob(u64),
    MountPolicy {
        namespace: u64,
        id: u64,
    },
    Sysv {
        kind: IpcKind,
        namespace: u64,
        id: i32,
    },
}
impl ObjectKey {
    pub fn id(self) -> u64 {
        match self {
            Self::Mount(id) | Self::Shared(id) | Self::Time(id) => id,
            Self::Bpf | Self::ProcessTable => 1,
            Self::CgroupJob(id) => id.saturating_add(1),
            Self::Sysv { id, .. } => id as u32 as u64,
            Self::MountPolicy { id, .. } => id,
        }
    }
    pub fn name(self, domain: u64) -> String {
        match self {
            Self::Mount(1) => format!(r"Local\kinakaze.mount.v2.{domain}.state"),
            Self::Mount(id) => format!(r"Local\kinakaze.mount.v2.{domain}.ns-{id}.state"),
            Self::Shared(id) => format!(r"Local\kinakaze.mount-object.v1.{domain}.{id}.state"),
            Self::Time(id) => format!(r"Local\kinakaze.time.v1.{domain}.{id}.state"),
            Self::Bpf => format!(r"Local\kinakaze.bpf.v3.{domain}"),
            Self::ProcessTable => format!(r"Local\kinakaze.v2.pidns.{domain:016x}.v15"),
            Self::CgroupJob(id) => format!(r"Local\kinakaze.cgroup.v2.{domain}.{id}"),
            Self::MountPolicy { namespace, id } => {
                format!(r"Local\kinakaze.mount-policy.v2.{domain}.{namespace}.{id}.state")
            }
            Self::Sysv {
                kind,
                namespace,
                id,
            } => {
                let kind = match kind {
                    IpcKind::Memory => "shm",
                    IpcKind::Semaphore => "sem",
                };
                format!(r"Local\kinakaze.{kind}.v2.ns{domain}.{namespace}.{id}")
            }
        }
    }
    pub fn is_root(self) -> bool {
        matches!(
            self,
            Self::Mount(1)
                | Self::Shared(1)
                | Self::Time(1)
                | Self::Bpf
                | Self::ProcessTable
                | Self::CgroupJob(_)
        ) || matches!(self, Self::Shared(id) if id >= u64::MAX - 64)
            || matches!(
                self,
                Self::MountPolicy {
                    namespace: u64::MAX,
                    ..
                }
            )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MountPublication {
    pub namespace: u64,
    pub expected: u64,
    pub dependencies: Vec<ObjectKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum KernelCommand {
    /// The native owner identity, used only for PID-bound Winsock recipes.
    ResourceOwner,
    /// A bounded binary transfer section contains native handles and socket
    /// recipes. References follow an independent, named shared lifetime token.
    RetainResources {
        owner: u64,
        id: u64,
        input: ResourceInput,
    },
    /// Native handle values stay owned by init while the caller pins owner.
    /// Socket entries are exported separately because recipes are PID-bound.
    Resources {
        owner: u64,
        id: u64,
    },
    ResourceSocket {
        owner: u64,
        id: u64,
        index: usize,
    },
    ReleaseResources {
        owner: u64,
        id: u64,
    },
    WatchDirectory {
        owner: u64,
        watch: u64,
        path: String,
    },
    RemoveDirectoryWatch {
        owner: u64,
        watch: u64,
    },
    Unlease {
        object: ObjectKey,
        owner: ObjectKey,
    },
    MountNamespaces,
    RemoveCgroup {
        id: u64,
    },
    /// IPC_RMID withdraws init's reference; existing attachments keep their handles.
    RemoveIpc {
        object: ObjectKey,
    },
    /// An unpinned native lifetime token (an open description or detached tree)
    /// keeps a resource alive. Init must not hold the token's own last handle.
    Lease {
        object: ObjectKey,
        owner: ObjectKey,
    },
    /// Retain an already initialized section before publishing its identity.
    /// Only tmpfs volumes have an additional sparse page section.
    Retain {
        object: ObjectKey,
        tmpfs: bool,
        dependencies: Vec<ObjectKey>,
    },
    /// Immutable length-prefixed tables in a worker-owned transfer section.
    /// Init pins/copies the input, validates and stages the whole batch before
    /// publishing any namespace. Worker death cannot leave a partial commit.
    PublishMounts {
        topology: u64,
        source: u64,
        length: u64,
        updates: Vec<MountPublication>,
    },
}
