//! Package/core identities from Windows, including asymmetric SMT widths.
use std::collections::{BTreeMap, BTreeSet};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows_sys::Win32::System::SystemInformation::{
    GROUP_AFFINITY, GetLogicalProcessorInformationEx, PROCESSOR_RELATIONSHIP, RelationAll,
    RelationProcessorCore, RelationProcessorPackage,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessAffinityMask, GetProcessGroupAffinity,
};

type Cpu = (u16, u32);

pub(super) struct Processor {
    pub package: usize,
    pub core: usize,
    pub cores: usize,
    pub siblings: usize,
}

pub(super) fn query() -> Option<Vec<Processor>> {
    let mut groups = vec![0u16; 64];
    let mut count = groups.len() as u16;
    // SAFETY: both output buffers are valid and their capacities are supplied.
    if unsafe { GetProcessGroupAffinity(GetCurrentProcess(), &mut count, groups.as_mut_ptr()) } == 0
    {
        return None;
    }
    groups.truncate(count as usize);
    let mut affinity = 0;
    let mut system = 0;
    let restricted = groups.len() == 1
        && unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut affinity, &mut system) } != 0
        && affinity != 0;
    let allowed = |(group, bit): Cpu| {
        groups.contains(&group) && (!restricted || affinity & (1usize << bit) != 0)
    };
    let mut storage = vec![0u64; 512];
    let length = loop {
        let mut length = u32::try_from(storage.len() * 8).ok()?;
        // SAFETY: aligned storage covers the declared writable length.
        if unsafe {
            GetLogicalProcessorInformationEx(RelationAll, storage.as_mut_ptr().cast(), &mut length)
        } != 0
        {
            break length as usize;
        }
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        let needed = (length as usize).div_ceil(8);
        if needed <= storage.len() {
            return None;
        }
        storage.resize(needed, 0);
    };
    if length > storage.len() * 8 {
        return None;
    }
    // SAFETY: Windows initialized exactly `length` bytes on success.
    let mut bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), length) };
    let mut cores = Vec::new();
    let mut packages = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return None;
        }
        let kind = i32::from_le_bytes(bytes[..4].try_into().ok()?);
        let size = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        if size < 8 || size > bytes.len() {
            return None;
        }
        if kind == RelationProcessorCore || kind == RelationProcessorPackage {
            let body = &bytes[8..size];
            let first = std::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupMask);
            let count_offset = std::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupCount);
            if body.len() < first {
                return None;
            }
            let count =
                u16::from_le_bytes(body[count_offset..count_offset + 2].try_into().ok()?) as usize;
            let stride = size_of::<GROUP_AFFINITY>();
            if count == 0 || count > (body.len() - first) / stride {
                return None;
            }
            let mut cpus = BTreeSet::new();
            for index in 0..count {
                let offset = first + index * stride;
                // SAFETY: the complete possibly unaligned record is in bounds.
                let mask = unsafe {
                    std::ptr::read_unaligned(body.as_ptr().add(offset).cast::<GROUP_AFFINITY>())
                };
                for bit in 0..usize::BITS {
                    if mask.Mask & (1usize << bit) != 0 && allowed((mask.Group, bit)) {
                        cpus.insert((mask.Group, bit));
                    }
                }
            }
            // Keep empty entries so package/core IDs do not change with affinity.
            if kind == RelationProcessorCore {
                cores.push(cpus);
            } else {
                packages.push(cpus);
            }
        }
        bytes = &bytes[size..];
    }
    assemble(&cores, &packages)
}

fn assemble(cores: &[BTreeSet<Cpu>], packages: &[BTreeSet<Cpu>]) -> Option<Vec<Processor>> {
    let mut processors = BTreeMap::new();
    for (package, cpus) in packages.iter().enumerate() {
        let members: Vec<_> = cores
            .iter()
            .enumerate()
            .filter(|(_, core)| !core.is_disjoint(cpus))
            .collect();
        for (core_id, core_cpus) in &members {
            if !core_cpus.is_subset(cpus) {
                return None;
            }
            for cpu in *core_cpus {
                if processors
                    .insert(
                        *cpu,
                        Processor {
                            package,
                            core: *core_id,
                            cores: members.len(),
                            siblings: cpus.len(),
                        },
                    )
                    .is_some()
                {
                    return None;
                }
            }
        }
        if members.iter().map(|(_, core)| core.len()).sum::<usize>() != cpus.len() {
            return None;
        }
    }
    (!processors.is_empty()).then(|| processors.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hybrid_cores_and_multiple_packages_keep_physical_counts() {
        let cpus = |ids: &[(u16, u32)]| ids.iter().copied().collect::<BTreeSet<_>>();
        let cores = [
            cpus(&[(0, 0), (0, 1)]),
            cpus(&[(0, 2)]),
            cpus(&[(1, 4), (1, 5)]),
        ];
        let packages = [cpus(&[(0, 0), (0, 1), (0, 2)]), cpus(&[(1, 4), (1, 5)])];
        let result = assemble(&cores, &packages).unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(
            (
                result[0].package,
                result[0].core,
                result[0].cores,
                result[0].siblings
            ),
            (0, 0, 2, 3)
        );
        assert_eq!(result[1].core, result[0].core);
        assert_ne!(result[2].core, result[0].core);
        assert_eq!(
            (result[3].package, result[3].cores, result[3].siblings),
            (1, 1, 2)
        );
    }
}
