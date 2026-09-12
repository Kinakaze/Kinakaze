//! Namespace membership and hierarchical PID numbers in the shared registry.
//! Registry IDs remain stable for native bookkeeping; guest-facing APIs translate
//! them at their boundary. A process has one number in each ancestor PID namespace.
use super::*;

pub const UTS: usize = 0;
pub const IPC: usize = 1;
pub const NET: usize = 2;
pub const CGROUP: usize = 3;
pub const PID: usize = 4;
pub const PID_CHILDREN: usize = 5;
const MEMBERSHIP: usize = 1216;
const MAP_LENGTH: usize = 1264;
const MAPS: usize = 1280;
const MAX_LEVELS: usize = 33;
const RESTORED: usize = 1816;
const RECORD_SIZE: usize = 40;
const RECORD_COUNT: usize = 1024;
pub(super) const TABLE_SIZE: usize = RECORD_SIZE * RECORD_COUNT;
// Namespace record: id, parent, next PID, init registry PID, dead, depth.

unsafe fn record(base: *mut u8, id: u64) -> Option<*mut u8> {
    (0..RECORD_COUNT)
        .map(|n| unsafe { base.add(PID_NAMESPACES_OFFSET + n * RECORD_SIZE) })
        .find(|p| unsafe { load64(*p, 0) == id })
}
unsafe fn membership(entry: *mut u8, kind: usize) -> u64 {
    unsafe { load64(entry, MEMBERSHIP + kind * 8).max(1) }
}
unsafe fn number(entry: *mut u8, ns: u64) -> Option<u32> {
    if ns == 1 {
        return Some(unsafe { load32(entry, SLOT_NAMESPACE_PID) });
    }
    for n in 0..unsafe { load32(entry, MAP_LENGTH) }.min(MAX_LEVELS as u32) as usize {
        if unsafe { load64(entry, MAPS + n * 16) } == ns {
            return Some(unsafe { load32(entry, MAPS + n * 16 + 8) });
        }
    }
    None
}
pub fn memberships(pid: u32) -> Option<[u64; 6]> {
    with_table(|base| unsafe {
        let entry = find(base, pid)?;
        Some(std::array::from_fn(|kind| membership(entry, kind)))
    })
    .flatten()
}
pub fn set_memberships(pid: u32, values: [u64; 6]) -> bool {
    if values.contains(&0) {
        return false;
    }
    with_table(|base| unsafe {
        let Some(entry) = find(base, pid) else {
            return false;
        };
        for (kind, value) in values.into_iter().enumerate() {
            store64(entry, MEMBERSHIP + kind * 8, value);
        }
        true
    })
    .unwrap_or(false)
}
pub fn create_pid_namespace(id: u64, parent: u64) -> Result<(), i32> {
    if id <= 1 {
        return Err(22);
    }
    with_table(|base| unsafe {
        if record(base, id).is_some() {
            return Err(17);
        }
        let depth = if parent == 1 {
            1
        } else {
            load32(record(base, parent).ok_or(22)?, 32) + 1
        };
        if depth >= MAX_LEVELS as u32 {
            return Err(28);
        }
        let entry = record(base, 0).ok_or(28)?;
        store64(entry, 8, parent);
        store32(entry, 16, 1);
        store32(entry, 24, 0);
        store32(entry, 28, 0);
        store32(entry, 32, depth);
        store64(entry, 0, id);
        Ok(())
    })
    .unwrap_or(Err(5))
}
pub fn pid_parent(id: u64) -> Option<u64> {
    if id == 1 {
        return None;
    }
    with_table(|base| unsafe { record(base, id).map(|p| load64(p, 8)) }).flatten()
}
pub fn pid_descendant(mut target: u64, ancestor: u64) -> bool {
    for _ in 0..MAX_LEVELS {
        if target == ancestor {
            return true;
        }
        let Some(parent) = pid_parent(target) else {
            return false;
        };
        target = parent;
    }
    false
}
/// Called under the registry mutex before a fresh child row is published.
pub(super) unsafe fn initialize_child(
    base: *mut u8,
    entry: *mut u8,
    parent: u32,
    pid: u32,
) -> bool {
    unsafe {
        store32(entry, RESTORED, 0);
        store32(entry, RESTORED + 4, 0);
    };
    let values = unsafe { find(base, parent) }
        .map(|p| std::array::from_fn(|k| unsafe { membership(p, k) }))
        .unwrap_or([1; 6]);
    let mut chain = [std::ptr::null_mut(); MAX_LEVELS];
    let mut length = 0;
    let mut namespace = values[PID_CHILDREN];
    while namespace != 1 {
        if length >= MAX_LEVELS - 1 {
            return false;
        }
        let Some(ns) = (unsafe { record(base, namespace) }) else {
            return false;
        };
        if unsafe { load32(ns, 28) != 0 || load32(ns, 16) >= i32::MAX as u32 } {
            return false;
        }
        chain[length] = ns;
        length += 1;
        namespace = unsafe { load64(ns, 8) };
    }
    for (kind, value) in values.into_iter().enumerate() {
        unsafe {
            store64(
                entry,
                MEMBERSHIP + kind * 8,
                if kind == PID {
                    values[PID_CHILDREN]
                } else {
                    value
                },
            )
        };
    }
    unsafe { store32(entry, MAP_LENGTH, length as u32) };
    for (n, ns) in chain[..length].iter().copied().enumerate() {
        unsafe {
            let local = load32(ns, 16);
            store32(ns, 16, local + 1);
            if local == 1 {
                store32(ns, 24, pid);
            }
            store64(entry, MAPS + n * 16, load64(ns, 0));
            store32(entry, MAPS + n * 16 + 8, local);
        }
    }
    true
}
pub fn visible_from(pid: u32, viewer: u32) -> Option<u32> {
    with_table(|base| unsafe {
        let ns = membership(find(base, viewer)?, PID);
        number(find(base, pid)?, ns)
    })
    .flatten()
}
pub fn resolve_from(visible: u32, viewer: u32) -> Option<u32> {
    if visible == 0 {
        return None;
    }
    with_table(|base| unsafe {
        let ns = membership(find(base, viewer)?, PID);
        if ns == 1 {
            return find(base, visible).map(|_| visible);
        }
        for n in 0..CAPACITY {
            let entry = slot(base, n);
            if load32(entry, SLOT_PID) != 0 && number(entry, ns) == Some(visible) {
                return Some(load32(entry, SLOT_NAMESPACE_PID));
            }
        }
        None
    })
    .flatten()
}
/// Proc superblocks retain their mounting PID namespace across setns/fork.
pub fn visible_in(pid: u32, namespace: u64) -> Option<u32> {
    with_table(|base| unsafe { number(find(base, pid)?, namespace) }).flatten()
}
pub fn resolve_in(pid: u32, namespace: u64) -> Option<u32> {
    if pid == 0 {
        return None;
    }
    with_table(|base| unsafe {
        for n in 0..CAPACITY {
            let entry = slot(base, n);
            if load32(entry, SLOT_PID) != 0 && number(entry, namespace) == Some(pid) {
                return Some(load32(entry, SLOT_NAMESPACE_PID));
            }
        }
        None
    })
    .flatten()
}
pub fn visible(pid: u32) -> Option<u32> {
    visible_from(pid, current_pid())
}
pub fn resolve(pid: u32) -> Option<u32> {
    resolve_from(pid, current_pid())
}
/// Reparent orphans to their namespace init and retire descendants of a dying
/// init. The signal pump delivers SIGKILL even for a guest outside libc.
pub(super) unsafe fn exiting(base: *mut u8, entry: *mut u8) {
    let pid = unsafe { load32(entry, SLOT_NAMESPACE_PID) };
    let ns = unsafe { membership(entry, PID) };
    let init = if ns == 1 {
        1
    } else {
        unsafe { record(base, ns).map(|r| load32(r, 24)).unwrap_or(1) }
    };
    // Reparent in the exiting parent's namespace. A child may itself be the
    // init of a nested namespace; choosing the child's init makes it its own
    // parent and prevents the outer shim from ever collecting its exit.
    let mut reaper = if init == pid {
        unsafe { load32(entry, SLOT_PPID) }
    } else {
        init
    };
    let mut ancestor = unsafe { load32(entry, SLOT_PPID) };
    for _ in 0..CAPACITY {
        let Some(parent) = (unsafe { find(base, ancestor) }) else {
            break;
        };
        if parent == entry || unsafe { membership(parent, PID) } != ns {
            break;
        }
        let flags = unsafe { load32(parent, SLOT_FLAGS) };
        if flags & (FLAG_SUBREAPER | FLAG_ZOMBIE) == FLAG_SUBREAPER {
            let host = unsafe { load32(parent, SLOT_PID) };
            if alive(host) && start_token(host) == unsafe { load64(parent, SLOT_TOKEN) } {
                reaper = ancestor;
                break;
            }
        }
        let next = unsafe { load32(parent, SLOT_PPID) };
        if next == ancestor {
            break;
        }
        ancestor = next;
    }
    let dies_as_init = ns != 1 && init == pid;
    if dies_as_init {
        if let Some(record) = unsafe { record(base, ns) } {
            unsafe { store32(record, 28, 1) };
        }
    }
    for n in 0..CAPACITY {
        let child = unsafe { slot(base, n) };
        if child == entry || unsafe { load32(child, SLOT_PID) } == 0 {
            continue;
        }
        if dies_as_init && unsafe { number(child, ns) }.is_some() {
            if !unsafe { notifications::signal(child) } {
                std::process::abort();
            }
            unsafe { or64(child, SLOT_PENDING, 1u64 << 8) };
        }
        if unsafe { load32(child, SLOT_PPID) } == pid {
            unsafe {
                notifications::topology(base, load32(child, SLOT_NAMESPACE_PID));
                notifications::topology(base, reaper);
                let signal = load32(child, SLOT_PDEATHSIG);
                if signal != 0 && load32(child, SLOT_PDEATH_DELIVERED) == 0 {
                    if !notifications::signal(child) {
                        std::process::abort();
                    }
                    or64(child, SLOT_PENDING, 1u64 << (signal - 1));
                }
                store32(
                    child,
                    SLOT_PPID,
                    if reaper == load32(child, SLOT_NAMESPACE_PID) {
                        0
                    } else {
                        reaper
                    },
                );
                store32(child, SLOT_PDEATH_DELIVERED, 0);
                // A zombie adopted after its former parent was notified must
                // notify its new parent as well once host teardown completes.
                store32(
                    child,
                    SLOT_FLAGS,
                    load32(child, SLOT_FLAGS) & !FLAG_EXIT_NOTIFIED,
                );
            }
        }
    }
}
pub fn pid_numbers(pid: u32) -> Vec<u32> {
    with_table(|base| unsafe {
        let Some(entry) = find(base, pid) else {
            return Vec::new();
        };
        let mut result = vec![pid];
        for n in (0..load32(entry, MAP_LENGTH).min(MAX_LEVELS as u32) as usize).rev() {
            result.push(load32(entry, MAPS + n * 16 + 8));
        }
        result
    })
    .unwrap_or_default()
}

/// Undo an unpublished namespace reservation if preparation fails.
pub fn discard_empty_pid_namespace(id: u64) {
    with_table(|base| unsafe {
        if let Some(entry) = record(base, id) {
            if load32(entry, 24) == 0 {
                std::ptr::write_bytes(entry, 0, RECORD_SIZE);
            }
        }
    });
}
/// NSpid columns start at the reader's namespace and continue into descendants.
pub fn visible_pid_numbers(pid: u32) -> Vec<u32> {
    visible_pid_numbers_in(pid, memberships(current_pid()).map(|v| v[PID]).unwrap_or(1))
}
pub fn visible_pid_numbers_in(pid: u32, ns: u64) -> Vec<u32> {
    let Some(local) = visible_in(pid, ns) else {
        return Vec::new();
    };
    let all = pid_numbers(pid);
    if ns == 1 {
        return all;
    }
    let mut depth = 0;
    let mut at = ns;
    while let Some(parent) = pid_parent(at) {
        depth += 1;
        at = parent;
    }
    all.get(depth..)
        .map(|v| v.to_vec())
        .unwrap_or_else(|| vec![local])
}

pub fn child_namespace_dead() -> bool {
    let own = current_pid();
    with_table(|base| unsafe {
        let Some(entry) = find(base, own) else {
            return false;
        };
        let ns = membership(entry, PID_CHILDREN);
        ns != 1 && record(base, ns).is_some_and(|r| load32(r, 28) != 0)
    })
    .unwrap_or(false)
}

pub fn mark_fork_restored() -> bool {
    use std::os::windows::io::AsRawHandle;
    let Some(own) = lookup_host(current_host_pid()) else {
        return false;
    };
    with_table(|base| unsafe {
        let Some(entry) = find(base, own.namespace_pid) else {
            return false;
        };
        if load32(entry, SLOT_PID) != own.pid || load64(entry, SLOT_TOKEN) != own.token {
            return false;
        }
        let Ok(event) = super::fork_handoff::event(&read_slot(entry)) else {
            return false;
        };
        // Commit under the table lock before signalling. If the child dies
        // before SetEvent, the parent's exact process handle still wakes it.
        store32(entry, RESTORED, 1);
        windows_sys::Win32::System::Threading::SetEvent(event.as_raw_handle()) != 0
    })
    .unwrap_or(false)
}
pub fn fork_result(pid: u32) -> Option<u32> {
    with_table(|base| unsafe {
        let entry = find(base, pid)?;
        (load32(entry, RESTORED) != 0).then(|| load32(entry, RESTORED + 4))
    })
    .flatten()
}

/// A clone initializer runs before the parent receives a successful result.
pub fn set_fork_error(error: u32) {
    let pid = super::lookup_host(std::process::id())
        .map(|e| e.namespace_pid)
        .unwrap_or(0);
    let _ = with_table(|base| unsafe {
        if let Some(entry) = find(base, pid) {
            store32(entry, RESTORED + 4, error);
        }
    });
}
/// Install PID 1 for a new process clone without changing its ancestor PIDs.
pub fn enter_new_pid_namespace() -> Result<(), i32> {
    let pid = super::lookup_host(std::process::id())
        .map(|e| e.namespace_pid)
        .unwrap_or(0);
    with_table(|base| unsafe {
        let entry = find(base, pid).ok_or(3)?;
        let target = membership(entry, PID_CHILDREN);
        let ns = record(base, target).ok_or(22)?;
        let length = load32(entry, MAP_LENGTH) as usize;
        if load64(ns, 8) != membership(entry, PID)
            || load32(ns, 24) != 0
            || load32(ns, 16) != 1
            || length >= MAX_LEVELS
        {
            return Err(22);
        }
        store64(entry, MAPS + length * 16, target);
        store32(entry, MAPS + length * 16 + 8, 1);
        store32(entry, MAP_LENGTH, (length + 1) as u32);
        store32(ns, 24, pid);
        store32(ns, 16, 2);
        store64(entry, MEMBERSHIP + PID * 8, target);
        Ok(())
    })
    .unwrap_or(Err(5))
}
