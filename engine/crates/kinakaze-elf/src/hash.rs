//! Symbol hash tables: `DT_GNU_HASH` and the older `DT_HASH`.
//!
//! A dynamic symbol table has no size tag, so the only way to know how many
//! symbols exist — and the only fast way to find one by name — is through a hash
//! table. Modern toolchains emit `DT_GNU_HASH` by default and often *only* that,
//! which is why supporting it is not optional.

use crate::dynamic::TableLocation;
use crate::{ElfError, ElfFile, u32_at, u64_at};

/// `STN_UNDEF`, which both table formats reuse as their end-of-chain marker.
const CHAIN_END: u32 = 0;

/// Offset of `base + count * size`, refusing to wrap.
fn region_end(base: usize, count: u32, size: usize) -> Result<usize, ElfError> {
    (count as usize)
        .checked_mul(size)
        .and_then(|span| base.checked_add(span))
        .ok_or(ElfError::IntegerOverflow)
}

/// The GNU hash function: a djb2 variant over the raw symbol name bytes.
pub fn gnu_hash_name(name: &str) -> u32 {
    let mut hash: u32 = 5381;
    for byte in name.as_bytes() {
        // hash * 33 + byte, with wrapping as the ABI specifies.
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(*byte));
    }
    hash
}

/// The System V hash function from the original ELF specification.
pub fn sysv_hash_name(name: &str) -> u32 {
    let mut hash: u32 = 0;
    for byte in name.as_bytes() {
        hash = (hash << 4).wrapping_add(u32::from(*byte));
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    hash
}

/// A parsed `DT_GNU_HASH` table.
#[derive(Clone, Debug)]
pub struct GnuHash<'a> {
    bytes: &'a [u8],
    bucket_count: u32,
    /// Symbol index below which no symbol is in the hash table at all.
    symbol_base: u32,
    bloom_word_count: u32,
    bloom_shift: u32,
    bloom_offset: usize,
    bucket_offset: usize,
    chain_offset: usize,
}

impl<'a> GnuHash<'a> {
    pub fn parse(elf: ElfFile<'a>, location: TableLocation) -> Result<Self, ElfError> {
        Self::decode(elf.bytes(), location.file_offset)
    }

    /// Decodes the header at `start` in `bytes`, which is the whole file.
    ///
    /// The chain array is deliberately not bounds-checked here: it has no stated
    /// length, so its extent is only knowable by walking it. The walks below
    /// range-check every entry instead.
    fn decode(bytes: &'a [u8], start: usize) -> Result<Self, ElfError> {
        let header = bytes
            .get(start..)
            .filter(|header| header.len() >= 16)
            .ok_or(ElfError::Truncated)?;
        let bucket_count = u32_at(header, 0)?;
        let symbol_base = u32_at(header, 4)?;
        let bloom_word_count = u32_at(header, 8)?;
        let bloom_shift = u32_at(header, 12)?;

        // A bucket-less table would make the modulo below a division by zero, and
        // the Bloom index is masked rather than divided, so a non-power-of-two
        // word count would silently read the wrong word.
        if bucket_count == 0 || !bloom_word_count.is_power_of_two() {
            return Err(ElfError::BadHashTable);
        }
        // The second Bloom bit shifts the hash by this much. Anything past the
        // width of the value being shifted cannot be what the linker meant.
        if bloom_shift >= 64 {
            return Err(ElfError::BadHashTable);
        }

        let bloom_offset = start.checked_add(16).ok_or(ElfError::IntegerOverflow)?;
        let bucket_offset = region_end(bloom_offset, bloom_word_count, 8)?;
        let chain_offset = region_end(bucket_offset, bucket_count, 4)?;
        if chain_offset > bytes.len() {
            return Err(ElfError::Truncated);
        }

        Ok(Self {
            bytes,
            bucket_count,
            symbol_base,
            bloom_word_count,
            bloom_shift,
            bloom_offset,
            bucket_offset,
            chain_offset,
        })
    }

    /// Cheap negative test through the Bloom filter.
    pub fn may_contain(&self, hash: u32) -> bool {
        let index = (hash / 64) % self.bloom_word_count;
        let Ok(offset) = region_end(self.bloom_offset, index, 8) else {
            return true;
        };
        let Ok(word) = u64_at(self.bytes, offset) else {
            // An unreadable filter proves nothing, so it must not exclude.
            return true;
        };
        // Widened before shifting: `bloom_shift` may exceed the width of a u32,
        // and the ABI expects the shift to apply to the 64-bit-masked value.
        let mask = (1u64 << (hash % 64)) | (1u64 << ((u64::from(hash) >> self.bloom_shift) % 64));
        word & mask == mask
    }

    /// Iterates over symbol indices whose hash matches without allocating.
    pub fn for_each_candidate<E: From<ElfError>, F: FnMut(u32) -> Result<bool, E>>(
        &self,
        hash: u32,
        mut f: F,
    ) -> Result<(), E> {
        let Some(mut index) = self.bucket_start(hash % self.bucket_count)? else {
            return Ok(());
        };
        loop {
            let entry = self.chain_entry(index)?;
            // The low bit flags the end of the chain, so it is not part of the
            // hash and has to be masked out of both sides of the comparison.
            if entry | 1 == hash | 1 {
                let stop = f(index)?;
                if stop {
                    return Ok(());
                }
            }
            if entry & 1 == 1 {
                return Ok(());
            }
            index = index.checked_add(1).ok_or(ElfError::IntegerOverflow)?;
        }
    }

    /// Symbol indices whose hash matches, for the caller to compare names against.
    pub fn candidates(&self, hash: u32) -> Result<Vec<u32>, ElfError> {
        let mut found = Vec::new();
        self.for_each_candidate(hash, |index| {
            found.push(index);
            Ok(false)
        })?;
        Ok(found)
    }

    /// One past the highest symbol index the table describes.
    ///
    /// `DT_GNU_HASH` never states the symbol count, so it is recovered the way
    /// every other ELF tool recovers it: the last symbol in the table is the last
    /// one on some bucket's chain.
    pub fn symbol_count(&self) -> Result<u32, ElfError> {
        let mut highest = None;
        for bucket in 0..self.bucket_count {
            let Some(mut index) = self.bucket_start(bucket)? else {
                continue;
            };
            loop {
                let entry = self.chain_entry(index)?;
                if entry & 1 == 1 {
                    break;
                }
                index = index.checked_add(1).ok_or(ElfError::IntegerOverflow)?;
            }
            highest = Some(highest.unwrap_or(index).max(index));
        }
        match highest {
            Some(highest) => highest.checked_add(1).ok_or(ElfError::BadHashTable),
            // No chains at all: every symbol sits below the hashed range.
            None => Ok(self.symbol_base),
        }
    }

    /// The first symbol index on a bucket's chain, or `None` when it is empty.
    fn bucket_start(&self, bucket: u32) -> Result<Option<u32>, ElfError> {
        let offset = region_end(self.bucket_offset, bucket, 4)?;
        let index = u32_at(self.bytes, offset)?;
        if index == CHAIN_END {
            return Ok(None);
        }
        // A chain slot exists only for symbols at or above the base, so a bucket
        // pointing below it would index the array negatively.
        if index < self.symbol_base {
            return Err(ElfError::BadHashTable);
        }
        Ok(Some(index))
    }

    /// The chain word for a symbol index.
    ///
    /// Reading past the chain is what terminates a runaway walk: the array runs
    /// to the end of the table, so a chain with no end flag runs out of file.
    fn chain_entry(&self, index: u32) -> Result<u32, ElfError> {
        let slot = index
            .checked_sub(self.symbol_base)
            .ok_or(ElfError::BadHashTable)?;
        let offset = region_end(self.chain_offset, slot, 4)?;
        u32_at(self.bytes, offset).map_err(|_| ElfError::BadHashTable)
    }
}

/// A parsed `DT_HASH` table.
#[derive(Clone, Debug)]
pub struct SysvHash<'a> {
    bytes: &'a [u8],
    bucket_count: u32,
    chain_count: u32,
    bucket_offset: usize,
    chain_offset: usize,
}

impl<'a> SysvHash<'a> {
    pub fn parse(elf: ElfFile<'a>, location: TableLocation) -> Result<Self, ElfError> {
        Self::decode(elf.bytes(), location.file_offset)
    }

    /// Decodes the header at `start` in `bytes`, which is the whole file.
    fn decode(bytes: &'a [u8], start: usize) -> Result<Self, ElfError> {
        let header = bytes
            .get(start..)
            .filter(|header| header.len() >= 8)
            .ok_or(ElfError::Truncated)?;
        let bucket_count = u32_at(header, 0)?;
        let chain_count = u32_at(header, 4)?;
        // Every lookup takes the hash modulo the bucket count.
        if bucket_count == 0 {
            return Err(ElfError::BadHashTable);
        }

        let bucket_offset = start.checked_add(8).ok_or(ElfError::IntegerOverflow)?;
        let chain_offset = region_end(bucket_offset, bucket_count, 4)?;
        // Unlike GNU hash, both arrays have stated lengths, so a table that does
        // not fit is malformed up front rather than at the first walk.
        if region_end(chain_offset, chain_count, 4)? > bytes.len() {
            return Err(ElfError::Truncated);
        }

        Ok(Self {
            bytes,
            bucket_count,
            chain_count,
            bucket_offset,
            chain_offset,
        })
    }

    /// Iterates over symbol indices in the bucket chain without allocating.
    pub fn for_each_candidate<E: From<ElfError>, F: FnMut(u32) -> Result<bool, E>>(
        &self,
        hash: u32,
        mut f: F,
    ) -> Result<(), E> {
        let bucket = hash % self.bucket_count;
        let offset = region_end(self.bucket_offset, bucket, 4)?;
        let mut index = u32_at(self.bytes, offset)?;

        for _ in 0..self.chain_count.saturating_add(1) {
            if index == CHAIN_END {
                return Ok(());
            }
            if index >= self.chain_count {
                return Err(ElfError::BadHashTable.into());
            }
            let stop = f(index)?;
            if stop {
                return Ok(());
            }
            let offset = region_end(self.chain_offset, index, 4)?;
            index = u32_at(self.bytes, offset)?;
        }
        Err(ElfError::BadHashTable.into())
    }

    /// Symbol indices in the bucket chain for this hash.
    pub fn candidates(&self, hash: u32) -> Result<Vec<u32>, ElfError> {
        let mut found = Vec::new();
        self.for_each_candidate(hash, |index| {
            found.push(index);
            Ok(false)
        })?;
        Ok(found)
    }

    /// The symbol table length, which `DT_HASH` states directly.
    pub fn symbol_count(&self) -> u32 {
        self.chain_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    /// A `DT_HASH` table: 2 buckets over 5 chain slots.
    ///
    /// Bucket 0 chains 1 -> 3, bucket 1 chains 2 -> 4, both ending at STN_UNDEF.
    fn sysv_table() -> Vec<u8> {
        words(&[
            2, 5, /* buckets */ 1, 2, /* chain */ 0, 3, 4, 0, 0,
        ])
    }

    #[test]
    fn sysv_hash_matches_the_specified_algorithm() {
        // "printf": each step is hash = (hash << 4) + byte.
        //   p: 0x0        <<4 + 0x70 = 0x70
        //   r: 0x700      + 0x72     = 0x772
        //   i: 0x7720     + 0x69     = 0x7789
        //   n: 0x77890    + 0x6e     = 0x778fe
        //   t: 0x778fe0   + 0x74     = 0x779054
        //   f: 0x7790540  + 0x66     = 0x77905a6
        assert_eq!(sysv_hash_name("printf"), 0x0779_05a6);
        assert_eq!(sysv_hash_name(""), 0);

        // "printf" is short enough that the top nibble is never populated, so the
        // fold-and-clear branch needs a name long enough to reach it: this one
        // folds 11 times, and "gettimeofday" folds 5.
        assert_eq!(sysv_hash_name("__libc_start_main"), 0x0177_ff8e);
        assert_eq!(sysv_hash_name("gettimeofday"), 0x0579_7189);
        // The result always fits in 28 bits, because the top nibble is cleared on
        // every step.
        for name in ["printf", "__libc_start_main", "gettimeofday", "memcpy"] {
            assert_eq!(sysv_hash_name(name) & 0xf000_0000, 0);
        }
    }

    #[test]
    fn gnu_hash_matches_the_specified_algorithm() {
        // djb2 from 5381: hash = hash * 33 + byte, wrapping at 32 bits.
        //   p: 5381 * 33 + 112     = 177685
        //   r: 177685 * 33 + 114   = 5863719
        //   i: 5863719 * 33 + 105  = 193502832
        //   n: 193502832 * 33 + 110 = 2090626270  (wraps: 6385593566 - 2^32)
        //   t: 2090626270 * 33 + 116 = 271190290  (wraps)
        //   f: 271190290 * 33 + 102  = 359345080  (wraps)
        assert_eq!(gnu_hash_name("printf"), 359_345_080);
        assert_eq!(gnu_hash_name("printf"), 0x156b_2bb8);
        assert_eq!(gnu_hash_name(""), 5381);
    }

    #[test]
    fn sysv_walks_both_bucket_chains() {
        let table = sysv_table();
        let hash = SysvHash::decode(&table, 0).unwrap();
        assert_eq!(hash.symbol_count(), 5);
        // Even hashes land in bucket 0, odd ones in bucket 1.
        assert_eq!(hash.candidates(4).unwrap(), vec![1, 3]);
        assert_eq!(hash.candidates(7).unwrap(), vec![2, 4]);
    }

    #[test]
    fn sysv_rejects_a_truncated_table_and_a_cyclic_chain() {
        let table = sysv_table();
        let error = |table: &[u8]| SysvHash::decode(table, 0).err();
        // The header claims 5 chain slots, so cutting the last one short must be
        // caught at decode rather than at the walk that runs off the end.
        assert_eq!(error(&table[..table.len() - 1]), Some(ElfError::Truncated));
        assert_eq!(error(&table[..6]), Some(ElfError::Truncated));
        assert_eq!(
            error(&words(&[0, 5, 0, 0, 0, 0, 0])),
            Some(ElfError::BadHashTable)
        );

        // chain[1] = 3, chain[3] = 1: a cycle that never reaches STN_UNDEF.
        let cyclic = words(&[2, 5, 1, 2, 0, 3, 4, 1, 0]);
        let hash = SysvHash::decode(&cyclic, 0).unwrap();
        assert_eq!(hash.candidates(4), Err(ElfError::BadHashTable));
    }

    /// A `DT_GNU_HASH` table holding "printf" at symbol index 1.
    ///
    /// One Bloom word, 2 buckets, `symoffset` 1, and a single chain slot whose
    /// low bit ends the chain.
    fn gnu_table() -> Vec<u8> {
        let hash = gnu_hash_name("printf");
        // hash % 64 = 56 and (hash >> 5) % 64 = 29 are the two bits the filter
        // must carry for this name.
        let bloom = (1u64 << (hash % 64)) | (1u64 << ((hash >> 5) % 64));
        let mut table = words(&[2, 1, 1, 5]);
        table.extend_from_slice(&bloom.to_le_bytes());
        // hash % 2 == 0, so bucket 0 starts the chain at symbol 1; bucket 1 empty.
        table.extend_from_slice(&words(&[1, 0]));
        table.extend_from_slice(&words(&[hash | 1]));
        table
    }

    #[test]
    fn gnu_finds_a_symbol_through_bloom_and_chain() {
        let table = gnu_table();
        let hash = GnuHash::decode(&table, 0).unwrap();
        let printf = gnu_hash_name("printf");

        assert!(hash.may_contain(printf));
        assert_eq!(hash.candidates(printf).unwrap(), vec![1]);
        // Symbol 1 is the highest index reachable, so the table describes 2.
        assert_eq!(hash.symbol_count().unwrap(), 2);
    }

    #[test]
    fn gnu_bloom_excludes_an_absent_hash() {
        let table = gnu_table();
        let hash = GnuHash::decode(&table, 0).unwrap();
        // Bits 1 and 0 were never set in the filter, so hash 1 is definitely out.
        assert!(!hash.may_contain(1));
        // An even hash reaching bucket 0 still fails the name-hash comparison.
        assert!(hash.candidates(2).unwrap().is_empty());
        // Bucket 1 is empty, so an odd hash short-circuits with no chain walk.
        assert!(hash.candidates(3).unwrap().is_empty());
    }

    #[test]
    fn gnu_rejects_malformed_headers_and_runaway_chains() {
        let error = |table: &[u8]| GnuHash::decode(table, 0).err();
        assert_eq!(error(&words(&[2, 1, 1])), Some(ElfError::Truncated));
        // 3 Bloom words: the index is masked, not divided, so this is unusable.
        assert_eq!(error(&words(&[2, 1, 3, 5])), Some(ElfError::BadHashTable));
        assert_eq!(error(&words(&[0, 1, 1, 5])), Some(ElfError::BadHashTable));
        assert_eq!(error(&words(&[2, 1, 1, 64])), Some(ElfError::BadHashTable));
        // Buckets are promised but absent.
        assert_eq!(
            error(&words(&[2, 1, 1, 5, 0, 0])),
            Some(ElfError::Truncated)
        );

        // No chain entry sets the end bit, so the walk must run out of table
        // instead of looping forever.
        let mut runaway = gnu_table();
        let last = runaway.len() - 4;
        runaway[last..].copy_from_slice(&gnu_hash_name("printf").to_le_bytes());
        let hash = GnuHash::decode(&runaway, 0).unwrap();
        assert_eq!(
            hash.candidates(gnu_hash_name("printf")),
            Err(ElfError::BadHashTable)
        );
        assert_eq!(hash.symbol_count(), Err(ElfError::BadHashTable));

        // Raising symoffset above the index bucket 0 points at would make the
        // chain slot land before the array starts.
        let mut below = gnu_table();
        below[4..8].copy_from_slice(&5u32.to_le_bytes());
        let hash = GnuHash::decode(&below, 0).unwrap();
        assert_eq!(hash.symbol_count(), Err(ElfError::BadHashTable));
    }

    /// Drives the public `parse` entry point through a real `ElfFile`.
    #[test]
    fn parses_tables_located_by_file_offset() {
        let mut bytes = vec![0u8; 64];
        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&crate::ET_DYN.to_le_bytes());
        bytes[18..20].copy_from_slice(&crate::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());

        let sysv_offset = bytes.len();
        bytes.extend_from_slice(&sysv_table());
        let gnu_offset = bytes.len();
        bytes.extend_from_slice(&gnu_table());

        let elf = ElfFile::parse(&bytes).unwrap();
        let at = |file_offset| TableLocation {
            address: 0,
            file_offset,
            size: None,
            entry_size: None,
        };

        let sysv = SysvHash::parse(elf, at(sysv_offset)).unwrap();
        assert_eq!(sysv.symbol_count(), 5);
        assert_eq!(sysv.candidates(4).unwrap(), vec![1, 3]);

        let gnu = GnuHash::parse(elf, at(gnu_offset)).unwrap();
        assert_eq!(gnu.candidates(gnu_hash_name("printf")).unwrap(), vec![1]);
        assert_eq!(gnu.symbol_count().unwrap(), 2);

        // A location past the end of the file must fail, not panic.
        assert_eq!(
            GnuHash::parse(elf, at(bytes.len())).err(),
            Some(ElfError::Truncated)
        );
        assert_eq!(
            SysvHash::parse(elf, at(usize::MAX)).err(),
            Some(ElfError::Truncated)
        );
    }
}
