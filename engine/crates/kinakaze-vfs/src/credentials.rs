//! Live guest credentials supplied by the libc which owns the calling thread.
//! Keeping a callback, rather than another mutable UID copy, makes setresuid
//! and fork/exec restoration visible to filesystem operations immediately.

use std::sync::OnceLock;

mod process;
pub(crate) use process::process_ids;
pub use process::publish;

#[derive(Clone, Copy, Debug, Default)]
pub struct Credentials {
    pub uid: u32,
    pub gid: u32,
    pub capabilities: u64,
}

static PROVIDER: OnceLock<fn() -> Credentials> = OnceLock::new();
static FILESYSTEM: OnceLock<fn() -> Credentials> = OnceLock::new();
static GROUPS: OnceLock<fn(u32) -> bool> = OnceLock::new();
type ExecHooks = (fn() -> Vec<u8>, fn(&[u8]) -> bool);
static EXEC: OnceLock<ExecHooks> = OnceLock::new();

pub fn register_exec(snapshot: fn() -> Vec<u8>, restore: fn(&[u8]) -> bool) {
    let _ = EXEC.set((snapshot, restore));
}

pub(crate) fn serialize_exec() -> Vec<u8> {
    EXEC.get().map_or_else(Vec::new, |hooks| hooks.0())
}

pub(crate) fn restore_exec(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    EXEC.get().is_some_and(|hooks| hooks.1(bytes))
}

/// Called during provider initialization, before guest code can execute.
pub fn register(provider: fn() -> Credentials, groups: fn(u32) -> bool) {
    let _ = PROVIDER.set(provider);
    let _ = GROUPS.set(groups);
}

pub fn group_member(gid: u32) -> bool {
    gid == filesystem().gid || GROUPS.get().is_some_and(|groups| groups(gid))
}

pub fn register_filesystem(provider: fn() -> Credentials) {
    let _ = FILESYSTEM.set(provider);
}

/// File ownership and permission checks use fsuid/fsgid, independently of euid/egid.
pub fn filesystem() -> Credentials {
    FILESYSTEM.get().map_or_else(current, |provider| provider())
}

pub fn current() -> Credentials {
    PROVIDER
        .get()
        .map_or_else(Credentials::default, |provider| provider())
}

static REAL_UID: OnceLock<fn() -> u32> = OnceLock::new();
/// Real, effective and saved IDs, in the initial user namespace.
#[derive(Clone, Copy)]
pub struct Identity {
    pub uids: [u32; 3],
    pub gids: [u32; 3],
}
static IDENTITY: OnceLock<fn() -> Identity> = OnceLock::new();
pub fn register_identity(provider: fn() -> Identity) {
    let _ = IDENTITY.set(provider);
}
pub fn identity() -> Identity {
    IDENTITY.get().map_or_else(
        || {
            let ids = current();
            Identity {
                uids: [ids.uid; 3],
                gids: [ids.gid; 3],
            }
        },
        |provider| provider(),
    )
}
pub fn register_real_uid(provider: fn() -> u32) {
    let _ = REAL_UID.set(provider);
}
pub fn real_uid() -> u32 {
    REAL_UID
        .get()
        .map_or_else(|| current().uid, |provider| provider())
}
