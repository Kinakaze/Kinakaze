//! GNU symbol versioning: `DT_VERSYM`, `DT_VERDEF` and `DT_VERNEED`.
//!
//! Versioning is what lets one library export several incompatible definitions of
//! the same name. Ignoring it does not merely lose information: picking the wrong
//! definition of a versioned symbol binds a caller to the wrong implementation,
//! which is a silent miscompile rather than a load failure.

use crate::dynamic::{DynamicInfo, TableLocation};
use crate::{ElfError, ElfFile, u16_at, u32_at};

/// Reserved version indices.
pub const VER_NDX_LOCAL: u16 = 0;
pub const VER_NDX_GLOBAL: u16 = 1;
/// Set in `DT_VERSYM` entries whose definition is not the default one.
pub const VERSYM_HIDDEN: u16 = 0x8000;
pub const VERSYM_VERSION: u16 = 0x7fff;

/// `vd_flags`: this node is the object's own base version, not a real one.
const VER_FLG_BASE: u16 = 0x1;
/// `vna_flags`: the dependency need not actually provide this version.
const VER_FLG_WEAK: u16 = 0x2;

/// The only revision of the versioning structures that exists.
const VER_REVISION: u16 = 1;

const VERDEF_SIZE: usize = 20;
const VERDAUX_SIZE: usize = 8;
const VERNEED_SIZE: usize = 16;
const VERNAUX_SIZE: usize = 16;

/// Hard ceiling on nodes walked in one list.
///
/// `DT_VERDEFNUM` and `DT_VERNEEDNUM` come straight out of the file, so a corrupt
/// object can state any count at all. Version indices are 15 bits wide, so a list
/// longer than this could not be referenced by `DT_VERSYM` anyway, which makes the
/// cap free of false positives while keeping the walk bounded.
const MAX_VERSION_NODES: u64 = 0x8000;

/// One version definition this object provides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionDefinition {
    pub index: u16,
    pub name: String,
    /// True for the definition that an unversioned reference should bind to.
    pub is_base: bool,
    pub hash: u32,
}

/// One version this object requires from a dependency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionRequirement {
    /// The dependency's soname.
    pub library: String,
    pub version: String,
    pub index: u16,
    pub hash: u32,
    /// True when the dependency need not actually provide the version.
    pub is_weak: bool,
}

/// The versioning tables of one object.
#[derive(Clone, Debug, Default)]
pub struct VersionTable {
    pub definitions: Vec<VersionDefinition>,
    pub requirements: Vec<VersionRequirement>,
    /// `DT_VERSYM`, one entry per dynamic symbol.
    version_symbols: Option<TableLocation>,
}

impl VersionTable {
    pub fn parse(elf: ElfFile<'_>, info: &DynamicInfo) -> Result<Self, ElfError> {
        // Names are copied out of the string table because the accessors hand out
        // `&str` borrowed from `self`, not from the file.
        let resolve = |offset: u32| -> Result<String, ElfError> {
            elf.dynamic_string(info, u64::from(offset))
                .map(str::to_owned)
        };

        let definitions = match info.version_definitions {
            Some(table) => parse_definitions(
                elf.bytes(),
                table.file_offset,
                info.version_definition_count,
                &resolve,
            )?,
            None => Vec::new(),
        };
        let requirements = match info.version_needs {
            Some(table) => parse_requirements(
                elf.bytes(),
                table.file_offset,
                info.version_need_count,
                &resolve,
            )?,
            None => Vec::new(),
        };

        Ok(Self {
            definitions,
            requirements,
            version_symbols: info.version_symbols,
        })
    }

    /// The raw `DT_VERSYM` entry for a symbol index.
    pub fn raw_version(&self, elf: ElfFile<'_>, symbol_index: u32) -> Option<u16> {
        // Read on demand rather than at parse time: the table is sized by the
        // symbol count, which only a hash table knows, and a caller that never
        // asks about a symbol should not pay for the whole array.
        let table = self.version_symbols?;
        let offset = table
            .file_offset
            .checked_add((symbol_index as usize).checked_mul(2)?)?;
        let entry = elf.slice(offset, 2).ok()?;
        u16_at(entry, 0).ok()
    }

    /// The version name a symbol definition carries, if any.
    pub fn definition_name(&self, elf: ElfFile<'_>, symbol_index: u32) -> Option<&str> {
        let index = self.version_index(elf, symbol_index)?;
        self.definitions
            .iter()
            .find(|definition| definition.index == index)
            .map(|definition| definition.name.as_str())
    }

    /// True when the definition at this index is hidden, meaning a plain
    /// unversioned reference must not bind to it.
    pub fn is_hidden(&self, elf: ElfFile<'_>, symbol_index: u32) -> bool {
        self.raw_version(elf, symbol_index)
            .is_some_and(|raw| raw & VERSYM_HIDDEN != 0)
    }

    /// The version a reference at this symbol index demands, if any.
    pub fn requirement_name(&self, elf: ElfFile<'_>, symbol_index: u32) -> Option<&str> {
        let index = self.version_index(elf, symbol_index)?;
        self.requirements
            .iter()
            .find(|requirement| requirement.index == index)
            .map(|requirement| requirement.version.as_str())
    }

    /// The named version index a symbol carries, if it carries one at all.
    ///
    /// `VER_NDX_LOCAL` and `VER_NDX_GLOBAL` are not versions: they mean "local to
    /// this object" and "unversioned global", so neither has a name to look up.
    fn version_index(&self, elf: ElfFile<'_>, symbol_index: u32) -> Option<u16> {
        let index = self.raw_version(elf, symbol_index)? & VERSYM_VERSION;
        if index == VER_NDX_LOCAL || index == VER_NDX_GLOBAL {
            return None;
        }
        Some(index)
    }
}

/// Walks the `DT_VERDEF` list.
///
/// Takes raw bytes and a resolver so the list walk can be exercised without
/// building a whole loadable object.
fn parse_definitions<F>(
    bytes: &[u8],
    start: usize,
    stated_count: u64,
    resolve: &F,
) -> Result<Vec<VersionDefinition>, ElfError>
where
    F: Fn(u32) -> Result<String, ElfError>,
{
    let limit = stated_count.min(MAX_VERSION_NODES);
    let mut definitions = Vec::new();
    let mut cursor = start;

    // A stated count of zero describes an empty list, not a broken one, so the
    // first node is never read.
    if limit == 0 {
        return Ok(definitions);
    }

    while (definitions.len() as u64) < limit {
        let node = node_at(bytes, cursor, VERDEF_SIZE)?;
        if u16_at(node, 0).map_err(malformed)? != VER_REVISION {
            return Err(ElfError::BadVersionTable);
        }
        let flags = u16_at(node, 2).map_err(malformed)?;
        let index = u16_at(node, 4).map_err(malformed)?;
        let hash = u32_at(node, 8).map_err(malformed)?;
        let aux = u32_at(node, 12).map_err(malformed)?;
        let next = u32_at(node, 16).map_err(malformed)?;

        // The first Verdaux holds this version's own name; the rest name parent
        // versions, which nothing in the loader needs.
        let aux_offset = advance(cursor, aux)?;
        let aux_node = node_at(bytes, aux_offset, VERDAUX_SIZE)?;
        let name = resolve(u32_at(aux_node, 0).map_err(malformed)?)?;

        definitions.push(VersionDefinition {
            // The high bit of `vd_ndx` is not part of the index.
            index: index & VERSYM_VERSION,
            name,
            is_base: flags & VER_FLG_BASE != 0,
            hash,
        });

        if next == 0 {
            return Ok(definitions);
        }
        cursor = advance(cursor, next)?;
    }

    // Falling out of the loop means the list ran past its stated length without a
    // terminator. Offsets are unsigned and applied forward, so this is the shape a
    // runaway chain takes; either way the table cannot be trusted.
    Err(ElfError::BadVersionTable)
}

/// Walks the `DT_VERNEED` list, flattening each Verneed's Vernaux chain.
///
/// One requirement is produced per Vernaux, because that is the granularity a
/// reference is resolved at: the Verneed only supplies the library name.
fn parse_requirements<F>(
    bytes: &[u8],
    start: usize,
    stated_count: u64,
    resolve: &F,
) -> Result<Vec<VersionRequirement>, ElfError>
where
    F: Fn(u32) -> Result<String, ElfError>,
{
    let limit = stated_count.min(MAX_VERSION_NODES);
    let mut requirements = Vec::new();
    let mut cursor = start;
    let mut walked = 0u64;

    if limit == 0 {
        return Ok(requirements);
    }

    while walked < limit {
        let node = node_at(bytes, cursor, VERNEED_SIZE)?;
        if u16_at(node, 0).map_err(malformed)? != VER_REVISION {
            return Err(ElfError::BadVersionTable);
        }
        let count = u16_at(node, 2).map_err(malformed)?;
        let file = u32_at(node, 4).map_err(malformed)?;
        let aux = u32_at(node, 8).map_err(malformed)?;
        let next = u32_at(node, 12).map_err(malformed)?;
        walked += 1;

        let library = resolve(file)?;
        let mut aux_cursor = advance(cursor, aux)?;
        // `vn_cnt` bounds the chain, so no extra cap is needed here; a missing
        // terminator on the last Vernaux is harmless once the count is exhausted.
        for _ in 0..count {
            let aux_node = node_at(bytes, aux_cursor, VERNAUX_SIZE)?;
            let hash = u32_at(aux_node, 0).map_err(malformed)?;
            let flags = u16_at(aux_node, 4).map_err(malformed)?;
            let index = u16_at(aux_node, 6).map_err(malformed)?;
            let name = u32_at(aux_node, 8).map_err(malformed)?;
            let aux_next = u32_at(aux_node, 12).map_err(malformed)?;

            requirements.push(VersionRequirement {
                library: library.clone(),
                version: resolve(name)?,
                index: index & VERSYM_VERSION,
                hash,
                is_weak: flags & VER_FLG_WEAK != 0,
            });

            if aux_next == 0 {
                break;
            }
            aux_cursor = advance(aux_cursor, aux_next)?;
        }

        if next == 0 {
            return Ok(requirements);
        }
        cursor = advance(cursor, next)?;
    }

    Err(ElfError::BadVersionTable)
}

/// Applies a node-relative byte offset.
fn advance(base: usize, delta: u32) -> Result<usize, ElfError> {
    base.checked_add(delta as usize)
        .ok_or(ElfError::BadVersionTable)
}

/// Borrows a fixed-size node, treating a short read as a corrupt table.
fn node_at(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], ElfError> {
    offset
        .checked_add(len)
        .and_then(|end| bytes.get(offset..end))
        .ok_or(ElfError::BadVersionTable)
}

/// Every failed field read inside a version node means the same thing.
fn malformed(_: ElfError) -> ElfError {
    ElfError::BadVersionTable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names keyed by the string-table offset a test node points at.
    fn resolver(pairs: &[(u32, &'static str)]) -> impl Fn(u32) -> Result<String, ElfError> {
        let pairs = pairs.to_vec();
        move |offset| {
            pairs
                .iter()
                .find(|(at, _)| *at == offset)
                .map(|(_, name)| (*name).to_owned())
                .ok_or(ElfError::InvalidString)
        }
    }

    fn verdef(flags: u16, index: u16, count: u16, hash: u32, aux: u32, next: u32) -> Vec<u8> {
        let mut node = Vec::with_capacity(VERDEF_SIZE);
        node.extend_from_slice(&VER_REVISION.to_le_bytes());
        node.extend_from_slice(&flags.to_le_bytes());
        node.extend_from_slice(&index.to_le_bytes());
        node.extend_from_slice(&count.to_le_bytes());
        node.extend_from_slice(&hash.to_le_bytes());
        node.extend_from_slice(&aux.to_le_bytes());
        node.extend_from_slice(&next.to_le_bytes());
        node
    }

    fn verdaux(name: u32, next: u32) -> Vec<u8> {
        let mut node = Vec::with_capacity(VERDAUX_SIZE);
        node.extend_from_slice(&name.to_le_bytes());
        node.extend_from_slice(&next.to_le_bytes());
        node
    }

    fn verneed(count: u16, file: u32, aux: u32, next: u32) -> Vec<u8> {
        let mut node = Vec::with_capacity(VERNEED_SIZE);
        node.extend_from_slice(&VER_REVISION.to_le_bytes());
        node.extend_from_slice(&count.to_le_bytes());
        node.extend_from_slice(&file.to_le_bytes());
        node.extend_from_slice(&aux.to_le_bytes());
        node.extend_from_slice(&next.to_le_bytes());
        node
    }

    fn vernaux(hash: u32, flags: u16, other: u16, name: u32, next: u32) -> Vec<u8> {
        let mut node = Vec::with_capacity(VERNAUX_SIZE);
        node.extend_from_slice(&hash.to_le_bytes());
        node.extend_from_slice(&flags.to_le_bytes());
        node.extend_from_slice(&other.to_le_bytes());
        node.extend_from_slice(&name.to_le_bytes());
        node.extend_from_slice(&next.to_le_bytes());
        node
    }

    #[test]
    fn reads_a_two_node_verdef_list() {
        // Node A at 0 with its aux at 20, node B at 28 with its aux at 48.
        let mut bytes = verdef(VER_FLG_BASE, 1, 1, 0x1111, 20, 28);
        bytes.extend(verdaux(1, 0));
        bytes.extend(verdef(0, 2, 1, 0x2222, 20, 0));
        bytes.extend(verdaux(10, 0));

        let resolve = resolver(&[(1, "libfoo.so.1"), (10, "FOO_2.0")]);
        let definitions = parse_definitions(&bytes, 0, 2, &resolve).unwrap();

        assert_eq!(
            definitions,
            vec![
                VersionDefinition {
                    index: 1,
                    name: "libfoo.so.1".to_owned(),
                    is_base: true,
                    hash: 0x1111,
                },
                VersionDefinition {
                    index: 2,
                    name: "FOO_2.0".to_owned(),
                    is_base: false,
                    hash: 0x2222,
                },
            ]
        );
    }

    #[test]
    fn uses_only_the_first_verdaux_as_the_version_name() {
        // Two aux entries: the second names a parent version and must be ignored.
        let mut bytes = verdef(0, 3, 2, 0x3333, 20, 0);
        bytes.extend(verdaux(1, 8));
        bytes.extend(verdaux(9, 0));

        let resolve = resolver(&[(1, "FOO_1.1"), (9, "FOO_1.0")]);
        let definitions = parse_definitions(&bytes, 0, 1, &resolve).unwrap();

        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "FOO_1.1");
    }

    #[test]
    fn flattens_one_verneed_into_two_requirements() {
        let mut bytes = verneed(2, 1, 16, 0);
        bytes.extend(vernaux(0xaaaa, 0, 2, 20, 16));
        bytes.extend(vernaux(0xbbbb, VER_FLG_WEAK, 3, 30, 0));

        let resolve = resolver(&[(1, "libc.so.6"), (20, "GLIBC_2.2.5"), (30, "GLIBC_2.34")]);
        let requirements = parse_requirements(&bytes, 0, 1, &resolve).unwrap();

        assert_eq!(
            requirements,
            vec![
                VersionRequirement {
                    library: "libc.so.6".to_owned(),
                    version: "GLIBC_2.2.5".to_owned(),
                    index: 2,
                    hash: 0xaaaa,
                    is_weak: false,
                },
                VersionRequirement {
                    library: "libc.so.6".to_owned(),
                    version: "GLIBC_2.34".to_owned(),
                    index: 3,
                    hash: 0xbbbb,
                    is_weak: true,
                },
            ]
        );
    }

    #[test]
    fn masks_the_high_bit_out_of_version_indices() {
        let mut definition = verdef(0, 0x8005, 1, 0, 20, 0);
        definition.extend(verdaux(1, 0));
        let resolve = resolver(&[(1, "FOO_1.0"), (2, "libc.so.6"), (3, "GLIBC_2.2.5")]);
        let definitions = parse_definitions(&definition, 0, 1, &resolve).unwrap();
        assert_eq!(definitions[0].index, 5);

        let mut need = verneed(1, 2, 16, 0);
        need.extend(vernaux(0, 0, 0x8007, 3, 0));
        let requirements = parse_requirements(&need, 0, 1, &resolve).unwrap();
        assert_eq!(requirements[0].index, 7);
    }

    #[test]
    fn accepts_a_terminator_before_the_stated_count() {
        // Linkers do emit a short list; only running past the count is an error.
        let mut bytes = verdef(0, 1, 1, 0, 20, 0);
        bytes.extend(verdaux(1, 0));
        let resolve = resolver(&[(1, "FOO_1.0")]);
        assert_eq!(parse_definitions(&bytes, 0, 4, &resolve).unwrap().len(), 1);
    }

    #[test]
    fn rejects_a_chain_that_never_terminates() {
        // Every node points at the next copy of itself, so the list is endless.
        // The stated count is what stops the walk; without it this would run to
        // the end of the file, and with a huge stated count, forever.
        let stride = (VERDEF_SIZE + VERDAUX_SIZE) as u32;
        let mut bytes = Vec::new();
        for _ in 0..64 {
            bytes.extend(verdef(0, 1, 1, 0, VERDEF_SIZE as u32, stride));
            bytes.extend(verdaux(1, 0));
        }
        let resolve = resolver(&[(1, "FOO_1.0")]);
        assert_eq!(
            parse_definitions(&bytes, 0, 2, &resolve),
            Err(ElfError::BadVersionTable)
        );
        // Also with a count no real object could state, to prove the hard cap and
        // the bounds checks together keep the walk finite.
        assert_eq!(
            parse_definitions(&bytes, 0, u64::MAX, &resolve),
            Err(ElfError::BadVersionTable)
        );

        let mut needs = Vec::new();
        for _ in 0..64 {
            needs.extend(verneed(1, 1, 16, VERNEED_SIZE as u32 + VERNAUX_SIZE as u32));
            needs.extend(vernaux(0, 0, 2, 1, 0));
        }
        assert_eq!(
            parse_requirements(&needs, 0, u64::MAX, &resolve),
            Err(ElfError::BadVersionTable)
        );
    }

    #[test]
    fn rejects_truncated_and_out_of_range_nodes() {
        let resolve = resolver(&[(1, "FOO_1.0")]);

        // A node header cut short.
        let short = verdef(0, 1, 1, 0, 20, 0);
        for len in 0..VERDEF_SIZE {
            assert_eq!(
                parse_definitions(&short[..len], 0, 1, &resolve),
                Err(ElfError::BadVersionTable),
                "length {len}"
            );
        }

        // A complete node whose aux offset points past the end.
        assert_eq!(
            parse_definitions(&short, 0, 1, &resolve),
            Err(ElfError::BadVersionTable)
        );

        // A next offset that leaves the buffer.
        let mut runaway = verdef(0, 1, 1, 0, 20, 4096);
        runaway.extend(verdaux(1, 0));
        assert_eq!(
            parse_definitions(&runaway, 0, 2, &resolve),
            Err(ElfError::BadVersionTable)
        );

        // An offset that would overflow the cursor rather than merely leave it.
        let mut overflow = verdef(0, 1, 1, 0, u32::MAX, 0);
        overflow.extend(verdaux(1, 0));
        assert_eq!(
            parse_definitions(&overflow, usize::MAX - 8, 1, &resolve),
            Err(ElfError::BadVersionTable)
        );

        let short_need = verneed(1, 1, 16, 0);
        for len in 0..VERNEED_SIZE {
            assert_eq!(
                parse_requirements(&short_need[..len], 0, 1, &resolve),
                Err(ElfError::BadVersionTable),
                "length {len}"
            );
        }
        // Header intact, Vernaux missing.
        assert_eq!(
            parse_requirements(&short_need, 0, 1, &resolve),
            Err(ElfError::BadVersionTable)
        );
    }

    #[test]
    fn rejects_an_unknown_node_revision() {
        let mut bytes = verdef(0, 1, 1, 0, 20, 0);
        bytes[0..2].copy_from_slice(&2u16.to_le_bytes());
        bytes.extend(verdaux(1, 0));
        let resolve = resolver(&[(1, "FOO_1.0")]);
        assert_eq!(
            parse_definitions(&bytes, 0, 1, &resolve),
            Err(ElfError::BadVersionTable)
        );

        let mut needs = verneed(1, 1, 16, 0);
        needs[0..2].copy_from_slice(&7u16.to_le_bytes());
        needs.extend(vernaux(0, 0, 2, 1, 0));
        assert_eq!(
            parse_requirements(&needs, 0, 1, &resolve),
            Err(ElfError::BadVersionTable)
        );
    }

    #[test]
    fn treats_a_stated_count_of_zero_as_an_empty_list() {
        let mut bytes = verdef(0, 1, 1, 0, 20, 0);
        bytes.extend(verdaux(1, 0));
        let resolve = resolver(&[(1, "FOO_1.0")]);
        assert!(
            parse_definitions(&bytes, 0, 0, &resolve)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_requirements(&bytes, 0, 0, &resolve)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reserved_indices_have_no_name() {
        let table = VersionTable {
            definitions: vec![VersionDefinition {
                index: 2,
                name: "FOO_1.0".to_owned(),
                is_base: false,
                hash: 0,
            }],
            requirements: vec![VersionRequirement {
                library: "libc.so.6".to_owned(),
                version: "GLIBC_2.2.5".to_owned(),
                index: 3,
                hash: 0,
                is_weak: false,
            }],
            version_symbols: None,
        };

        // Without DT_VERSYM there is nothing to look up, so every accessor is
        // empty regardless of what the lists hold.
        let bytes = minimal_elf();
        let elf = ElfFile::parse(&bytes).unwrap();
        assert_eq!(table.raw_version(elf, 0), None);
        assert_eq!(table.definition_name(elf, 0), None);
        assert_eq!(table.requirement_name(elf, 0), None);
        assert!(!table.is_hidden(elf, 0));

        // The name lookups themselves ignore VER_NDX_LOCAL and VER_NDX_GLOBAL.
        for index in [VER_NDX_LOCAL, VER_NDX_GLOBAL] {
            assert!(
                !table
                    .definitions
                    .iter()
                    .any(|definition| definition.index == index)
            );
        }
    }

    #[test]
    fn resolves_names_through_dt_versym() {
        // A header followed by a four-entry DT_VERSYM array. Only `file_offset`
        // is read, so no PT_LOAD segment is needed to exercise the accessors.
        let mut bytes = minimal_elf().to_vec();
        let versym = 64usize;
        for entry in [VER_NDX_GLOBAL, 2, 3 | VERSYM_HIDDEN, 9] {
            bytes.extend_from_slice(&entry.to_le_bytes());
        }

        let table = VersionTable {
            definitions: vec![VersionDefinition {
                index: 3,
                name: "FOO_2.0".to_owned(),
                is_base: false,
                hash: 0,
            }],
            requirements: vec![VersionRequirement {
                library: "libc.so.6".to_owned(),
                version: "GLIBC_2.2.5".to_owned(),
                index: 2,
                hash: 0,
                is_weak: false,
            }],
            version_symbols: Some(TableLocation {
                address: 0,
                file_offset: versym,
                size: None,
                entry_size: Some(2),
            }),
        };
        let elf = ElfFile::parse(&bytes).unwrap();

        // Symbol 0 is unversioned: a real index, but not a named one.
        assert_eq!(table.raw_version(elf, 0), Some(VER_NDX_GLOBAL));
        assert_eq!(table.definition_name(elf, 0), None);
        assert_eq!(table.requirement_name(elf, 0), None);

        // Symbol 1 references a version; symbol 2 defines a hidden one.
        assert_eq!(table.requirement_name(elf, 1), Some("GLIBC_2.2.5"));
        assert_eq!(table.definition_name(elf, 1), None);
        assert!(!table.is_hidden(elf, 1));
        assert_eq!(table.raw_version(elf, 2), Some(3 | VERSYM_HIDDEN));
        assert_eq!(table.definition_name(elf, 2), Some("FOO_2.0"));
        assert!(table.is_hidden(elf, 2));

        // An index no list mentions, and a read past the end of the file.
        assert_eq!(table.definition_name(elf, 3), None);
        assert_eq!(table.raw_version(elf, 4), None);
        assert_eq!(table.raw_version(elf, u32::MAX), None);
    }

    /// A header-only object, enough for `ElfFile::parse`.
    fn minimal_elf() -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&crate::ET_DYN.to_le_bytes());
        bytes[18..20].copy_from_slice(&crate::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes
    }
}
