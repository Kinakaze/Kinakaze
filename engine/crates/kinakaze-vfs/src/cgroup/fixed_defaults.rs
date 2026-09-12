//! Explicitly non-enforcing cpuset/swap interfaces. No mutable state, native
//! quota calls, privilege changes or handles need to survive fork/exec here.
//! CPU/node defaults describe the host topology, not a requested restriction.

use std::collections::BTreeSet;
use std::mem::{offset_of, size_of};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows_sys::Win32::System::SystemInformation::{
    GROUP_RELATIONSHIP, GetLogicalProcessorInformationEx, NUMA_NODE_RELATIONSHIP,
    PROCESSOR_GROUP_INFO, RelationAll, RelationGroup, RelationNumaNode, RelationNumaNodeEx,
};

pub(super) const FILES: &[&str] = &[
    "cpuset.cpus",
    "cpuset.cpus.effective",
    "cpuset.mems",
    "cpuset.mems.effective",
    "memory.swap.max",
    "memory.swap.high",
    "memory.swap.current",
    "memory.swap.peak",
    "memory.swap.events",
];

pub(super) fn owns(name: &str) -> bool {
    FILES.contains(&name)
}

pub(super) fn read(name: &str) -> Result<Vec<u8>, i32> {
    match name {
        "cpuset.cpus" | "cpuset.cpus.effective" => cpu_list(),
        "cpuset.mems" | "cpuset.mems.effective" => node_list(),
        "memory.swap.max" | "memory.swap.high" => Ok(b"max\n".to_vec()),
        // These are default placeholders, not measured pagefile usage.
        "memory.swap.current" | "memory.swap.peak" => Ok(b"0\n".to_vec()),
        "memory.swap.events" => Ok(b"high 0\nmax 0\nfail 0\n".to_vec()),
        _ => Err(crate::ENOENT),
    }
}

pub(super) fn cpu_list() -> Result<Vec<u8>, i32> {
    Ok(format_list(&topology()?.0).into_bytes())
}

pub(super) fn node_list() -> Result<Vec<u8>, i32> {
    Ok(format_list(&topology()?.1).into_bytes())
}

pub(crate) fn topology() -> Result<(BTreeSet<u32>, BTreeSet<u32>), i32> {
    let mut storage = vec![0u64; 512];
    loop {
        let mut length = u32::try_from(storage.len() * 8).map_err(|_| crate::EOVERFLOW)?;
        let ok = unsafe {
            GetLogicalProcessorInformationEx(RelationAll, storage.as_mut_ptr().cast(), &mut length)
        };
        if ok != 0 {
            if length as usize > storage.len() * 8 {
                return Err(crate::EIO);
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), length as usize)
            };
            return decode_topology(bytes);
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_INSUFFICIENT_BUFFER {
            return Err(crate::errno_from_win32(error));
        }
        let needed = (length as usize).div_ceil(8);
        if needed <= storage.len() {
            return Err(crate::EIO);
        }
        storage
            .try_reserve_exact(needed - storage.len())
            .map_err(|_| crate::ENOMEM)?;
        storage.resize(needed, 0);
    }
}

fn decode_topology(mut bytes: &[u8]) -> Result<(BTreeSet<u32>, BTreeSet<u32>), i32> {
    let mut cpus = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return Err(crate::EIO);
        }
        let kind = i32::from_le_bytes(bytes[..4].try_into().unwrap());
        let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if size < 8 || size > bytes.len() {
            return Err(crate::EIO);
        }
        let body = &bytes[8..size];
        if kind == RelationGroup {
            let first = offset_of!(GROUP_RELATIONSHIP, GroupInfo);
            if body.len() < first {
                return Err(crate::EIO);
            }
            let count = u16::from_le_bytes(body[2..4].try_into().unwrap()) as usize;
            let stride = size_of::<PROCESSOR_GROUP_INFO>();
            if count == 0 || count > (body.len() - first) / stride || !cpus.is_empty() {
                return Err(crate::EIO);
            }
            for group in 0..count {
                let start =
                    first + group * stride + offset_of!(PROCESSOR_GROUP_INFO, ActiveProcessorMask);
                let mask = usize::from_le_bytes(
                    body[start..start + size_of::<usize>()].try_into().unwrap(),
                );
                for cpu in 0..usize::BITS {
                    if mask & (1usize << cpu) != 0 {
                        cpus.insert(group as u32 * usize::BITS + cpu);
                    }
                }
            }
        } else if kind == RelationNumaNode || kind == RelationNumaNodeEx {
            if body.len() < size_of::<NUMA_NODE_RELATIONSHIP>() {
                return Err(crate::EIO);
            }
            nodes.insert(u32::from_le_bytes(body[..4].try_into().unwrap()));
        }
        bytes = &bytes[size..];
    }
    if cpus.is_empty() || nodes.is_empty() {
        return Err(crate::EIO);
    }
    Ok((cpus, nodes))
}

pub(crate) fn format_list(values: &BTreeSet<u32>) -> String {
    let mut ranges = Vec::new();
    let mut values = values.iter().copied().peekable();
    while let Some(first) = values.next() {
        let mut last = first;
        while values
            .peek()
            .copied()
            .is_some_and(|next| last.checked_add(1) == Some(next))
        {
            last = values.next().unwrap();
        }
        ranges.push(if first == last {
            first.to_string()
        } else {
            format!("{first}-{last}")
        });
    }
    format!("{}\n", ranges.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_lists_do_not_invent_processors_or_nodes() {
        assert_eq!(
            format_list(&BTreeSet::from([0, 1, 2, 5, 64, 65, 130])),
            "0-2,5,64-65,130\n"
        );
        assert_eq!(format_list(&BTreeSet::from([0])), "0\n");
        assert_eq!(format_list(&BTreeSet::from([u32::MAX])), "4294967295\n");
    }

    #[test]
    fn malformed_topology_is_not_replaced_with_a_one_cpu_guess() {
        assert!(decode_topology(&[]).is_err());
        assert!(decode_topology(&[0; 8]).is_err());
        assert!(decode_topology(&[0; 7]).is_err());
    }

    #[test]
    fn topology_decodes_multiple_groups_and_sparse_numa_nodes() {
        let first = offset_of!(GROUP_RELATIONSHIP, GroupInfo);
        let stride = size_of::<PROCESSOR_GROUP_INFO>();
        let size = 8 + first + 3 * stride;
        let mut bytes = vec![0u8; size];
        bytes[..4].copy_from_slice(&RelationGroup.to_le_bytes());
        bytes[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        bytes[10..12].copy_from_slice(&3u16.to_le_bytes());
        for (group, mask) in [3usize, 5, 4].into_iter().enumerate() {
            let start =
                8 + first + group * stride + offset_of!(PROCESSOR_GROUP_INFO, ActiveProcessorMask);
            bytes[start..start + size_of::<usize>()].copy_from_slice(&mask.to_le_bytes());
        }
        for node in [0u32, 3] {
            let start = bytes.len();
            let size = 8 + size_of::<NUMA_NODE_RELATIONSHIP>();
            bytes.resize(start + size, 0);
            bytes[start..start + 4].copy_from_slice(&RelationNumaNodeEx.to_le_bytes());
            bytes[start + 4..start + 8].copy_from_slice(&(size as u32).to_le_bytes());
            bytes[start + 8..start + 12].copy_from_slice(&node.to_le_bytes());
        }
        let (cpus, nodes) = decode_topology(&bytes).unwrap();
        assert_eq!(
            cpus,
            BTreeSet::from([0, 1, usize::BITS, usize::BITS + 2, 2 * usize::BITS + 2])
        );
        assert_eq!(nodes, BTreeSet::from([0, 3]));
        // An incomplete trailing native record is an error, not partial data.
        bytes.pop();
        assert_eq!(decode_topology(&bytes), Err(crate::EIO));
    }

    #[test]
    fn live_host_defaults_are_nonempty() {
        let cpus = cpu_list().unwrap();
        let nodes = node_list().unwrap();
        assert!(cpus.len() >= 2 && cpus.ends_with(b"\n"));
        assert!(nodes.len() >= 2 && nodes.ends_with(b"\n"));
        eprintln!(
            "fixed defaults: cpus={} nodes={}",
            String::from_utf8_lossy(&cpus).trim(),
            String::from_utf8_lossy(&nodes).trim()
        );
    }
}
