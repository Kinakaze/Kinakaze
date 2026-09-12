//! The passthrough groups, and the search across them.
//!
//! The host loader's 265 exports are split by kind rather than all declared in
//! one file. The split is mechanical — see `scripts/check-vulkan-groups.py`, which
//! is the authority on which group owns which name and fails if any name is
//! claimed twice or left out.

pub mod command;
pub mod instance;
pub mod memory;
pub mod physical_device;
pub mod pipeline;
pub mod sync;
pub mod wsi;

/// Every group's lookup, in the order they are searched.
///
/// Order does not affect the result — the groups partition the export table, so at
/// most one can match — but searching a fixed sequence makes the cost of a miss
/// predictable and keeps the behaviour independent of how the groups are ordered
/// in the file.
type Lookup = fn(&str) -> Option<usize>;

const LOOKUPS: &[Lookup] = &[
    command::lookup,
    instance::lookup,
    memory::lookup,
    physical_device::lookup,
    pipeline::lookup,
    sync::lookup,
    wsi::lookup,
];

/// Finds `name`'s thunk in whichever group declares it.
///
/// `None` means either that no group wraps the name or that the host loader does
/// not export it. Both are answered as null by [`crate::dispatch`], which is the
/// correct report in either case: the guest cannot use an entry point that is
/// absent for either reason.
pub fn lookup(name: &str) -> Option<usize> {
    LOOKUPS.iter().find_map(|lookup| lookup(name))
}

/// Every name declared across every group.
pub fn names() -> Vec<&'static str> {
    let mut names = Vec::new();
    for group in GROUP_NAMES {
        names.extend_from_slice(group);
    }
    names
}

/// Each group's declared names, for the partition tests.
const GROUP_NAMES: &[&[&str]] = &[
    command::NAMES,
    instance::NAMES,
    memory::NAMES,
    physical_device::NAMES,
    pipeline::NAMES,
    sync::NAMES,
    wsi::NAMES,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// No name may be declared by two groups.
    ///
    /// A duplicate is normally a link error, but it is worth testing directly: the
    /// two declarations could differ in signature, and then the one that wins is
    /// decided by link order rather than by anything meaningful.
    #[test]
    fn the_groups_do_not_overlap() {
        let mut seen = BTreeSet::new();
        for name in names() {
            assert!(seen.insert(name), "{name} is declared by two groups");
        }
    }

    /// Every Vulkan command in the registry has exactly one guest-callable ABI
    /// thunk: a loader export, a generated proc-address command, or a semantic
    /// translation such as Linux WSI/debug callbacks.  This prevents extension
    /// enumeration from getting ahead of the actual callable surface.
    #[test]
    fn the_complete_registry_has_exactly_one_thunk_per_command() {
        let mut seen = BTreeSet::new();
        for name in names()
            .into_iter()
            .chain(crate::generated_extensions::NAMES.iter().copied())
            .chain(crate::dispatch::INTERCEPTED_ENTRY_POINTS.iter().copied())
        {
            assert!(seen.insert(name), "{name} has two ABI thunks");
        }
        assert_eq!(
            seen.len(),
            crate::generated_extensions::REGISTRY_COMMAND_COUNT,
            "the generated Khronos registry and callable thunk surface diverged",
        );
    }

    /// Every entry point the host loader exports must be wrapped by some group.
    ///
    /// This is the test that keeps the passthrough complete as the loader changes.
    /// An unwrapped name resolves to null, which a guest reads as an unsupported
    /// feature — a wrong answer that looks like a legitimate one.
    #[test]
    fn every_host_export_is_wrapped() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        let declared: BTreeSet<&str> = names().into_iter().collect();
        // The dispatch roots and wsi_compat intercepted functions are answered by `crate::dispatch`, not by a group.
        let answered_elsewhere = [
            "vkGetInstanceProcAddr",
            "vkGetDeviceProcAddr",
            "vkCreateInstance",
            "vkCreateDevice",
            "vkEnumerateInstanceExtensionProperties",
        ];

        let mut unwrapped = Vec::new();
        for name in host_export_names() {
            if !declared.contains(name.as_str()) && !answered_elsewhere.contains(&name.as_str()) {
                unwrapped.push(name);
            }
        }
        assert!(
            unwrapped.is_empty(),
            "the host loader exports {} entry points no group wraps: {unwrapped:?}",
            unwrapped.len()
        );
    }

    /// Every name a group declares must resolve through the aggregate lookup, or
    /// the group is unreachable from `vkGetInstanceProcAddr`.
    #[test]
    fn every_declared_name_resolves() {
        if !crate::host::loader_present() {
            eprintln!("skipped: no host Vulkan loader on this machine");
            return;
        }
        for name in names() {
            // A name the host does not export legitimately yields `None`; only a
            // name the host *does* have must resolve.
            if crate::host::symbol(name).is_some() {
                assert!(
                    lookup(name).is_some(),
                    "{name} is declared but does not resolve"
                );
            }
        }
    }

    /// Reads the host loader's export table.
    ///
    /// Parsing the PE rather than trusting a checked-in list means this test keeps
    /// telling the truth when the machine's loader is upgraded.
    fn host_export_names() -> Vec<String> {
        let path = std::env::var(crate::host::LOADER_OVERRIDE)
            .unwrap_or_else(|_| "C:/Windows/System32/vulkan-1.dll".to_owned());
        let Ok(image) = std::fs::read(&path) else {
            return Vec::new();
        };
        export_names(&image)
    }

    /// Minimal PE export-table reader, enough to list names.
    fn export_names(image: &[u8]) -> Vec<String> {
        let word = |offset: usize| -> u32 {
            u32::from_le_bytes(image[offset..offset + 4].try_into().unwrap())
        };
        let half = |offset: usize| -> u16 {
            u16::from_le_bytes(image[offset..offset + 2].try_into().unwrap())
        };

        let pe = word(0x3c) as usize;
        let optional_size = half(pe + 20) as usize;
        let optional = pe + 24;
        // PE32+ puts the data directories 16 bytes further along than PE32.
        let directory = optional + if half(optional) == 0x20b { 112 } else { 96 };
        let export_rva = word(directory) as usize;
        if export_rva == 0 {
            return Vec::new();
        }

        let mut sections = Vec::new();
        let section_base = optional + optional_size;
        for index in 0..half(pe + 6) as usize {
            let entry = section_base + index * 40;
            let virtual_size = word(entry + 8) as usize;
            let virtual_address = word(entry + 12) as usize;
            let raw_size = word(entry + 16) as usize;
            let raw_pointer = word(entry + 20) as usize;
            sections.push((virtual_address, virtual_size.max(raw_size), raw_pointer));
        }
        let to_offset = |rva: usize| -> Option<usize> {
            sections
                .iter()
                .find(|(address, size, _)| rva >= *address && rva < address + size)
                .map(|(address, _, raw)| raw + (rva - address))
        };

        let Some(table) = to_offset(export_rva) else {
            return Vec::new();
        };
        let count = word(table + 24) as usize;
        let Some(names) = to_offset(word(table + 32) as usize) else {
            return Vec::new();
        };

        let mut found = Vec::with_capacity(count);
        for index in 0..count {
            let Some(offset) = to_offset(word(names + index * 4) as usize) else {
                continue;
            };
            let end = image[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .map_or(offset, |length| offset + length);
            if let Ok(name) = std::str::from_utf8(&image[offset..end]) {
                found.push(name.to_owned());
            }
        }
        found
    }
}
