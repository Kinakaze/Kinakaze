//! Linux fs-verity descriptor and bounded-memory Merkle construction.
//! Tree blocks are stored top level first, independent of backing storage.

use crate::{EINVAL, EIO, EOVERFLOW};
use sha2::{Digest, Sha256, Sha512};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor([u8; 256]);

#[derive(Debug)]
struct Layout {
    // Lowest hash level first; offsets are bytes relative to the tree start.
    starts: Vec<u64>,
    size: u64,
}

impl Descriptor {
    pub fn new(algorithm: u32, block_size: u32, salt: &[u8], data_size: u64) -> Result<Self, i32> {
        if !matches!(algorithm, 1 | 2)
            || !matches!(block_size, 1024 | 2048 | 4096)
            || salt.len() > 32
            || data_size > i64::MAX as u64
        {
            return Err(EINVAL);
        }
        let mut bytes = [0u8; 256];
        bytes[0] = 1;
        bytes[1] = algorithm as u8;
        bytes[2] = block_size.trailing_zeros() as u8;
        bytes[3] = salt.len() as u8;
        bytes[8..16].copy_from_slice(&data_size.to_le_bytes());
        bytes[80..80 + salt.len()].copy_from_slice(salt);
        Ok(Self(bytes))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() != 256
            || bytes[0] != 1
            || !matches!(bytes[1], 1 | 2)
            || !(10..=12).contains(&bytes[2])
            || bytes[3] > 32
            || bytes[4..8] != [0; 4]
            || bytes[112..].iter().any(|&v| v != 0)
            || bytes[80 + bytes[3] as usize..112].iter().any(|&v| v != 0)
            || (bytes[1] == 1 && bytes[48..80].iter().any(|&v| v != 0))
        {
            return Err(EIO);
        }
        let value = Self(bytes.try_into().unwrap());
        if value.data_size() > i64::MAX as u64
            || (value.data_size() == 0 && bytes[16..80].iter().any(|&v| v != 0))
        {
            return Err(EIO);
        }
        value.layout()?;
        Ok(value)
    }

    pub fn bytes(&self) -> &[u8; 256] {
        &self.0
    }
    pub fn data_size(&self) -> u64 {
        u64::from_le_bytes(self.0[8..16].try_into().unwrap())
    }
    pub fn block_size(&self) -> usize {
        1usize << self.0[2]
    }
    pub fn algorithm(&self) -> u32 {
        self.0[1] as u32
    }
    fn digest_size(&self) -> usize {
        if self.algorithm() == 1 { 32 } else { 64 }
    }
    fn arity(&self) -> u64 {
        (self.block_size() / self.digest_size()) as u64
    }
    pub fn digest(&self) -> Vec<u8> {
        match self.algorithm() {
            1 => Sha256::digest(&self.0).to_vec(),
            _ => Sha512::digest(&self.0).to_vec(),
        }
    }
    pub fn tree_size(&self) -> Result<u64, i32> {
        Ok(self.layout()?.size)
    }

    fn hash(&self, block: &[u8]) -> Vec<u8> {
        let salt = &self.0[80..80 + self.0[3] as usize];
        // Salt is padded to the hash compression-block size, NOT tree block size.
        let mut padded = [0u8; 128];
        padded[..salt.len()].copy_from_slice(salt);
        if self.algorithm() == 1 {
            let mut hash = Sha256::new();
            if !salt.is_empty() {
                hash.update(&padded[..64]);
            }
            hash.update(block);
            hash.finalize().to_vec()
        } else {
            let mut hash = Sha512::new();
            if !salt.is_empty() {
                hash.update(padded);
            }
            hash.update(block);
            hash.finalize().to_vec()
        }
    }

    fn layout(&self) -> Result<Layout, i32> {
        let mut blocks = self.data_size().div_ceil(self.block_size() as u64);
        let mut counts = Vec::new();
        while blocks > 1 {
            blocks = blocks.div_ceil(self.arity());
            counts.push(blocks);
        }
        let mut starts = vec![0; counts.len()];
        let mut size = 0u64;
        for level in (0..counts.len()).rev() {
            starts[level] = size;
            size = size
                .checked_add(
                    counts[level]
                        .checked_mul(self.block_size() as u64)
                        .ok_or(EOVERFLOW)?,
                )
                .ok_or(EOVERFLOW)?;
        }
        Ok(Layout { starts, size })
    }

    /// Verify the exact padded data block and its complete path to the pinned
    /// descriptor root. Untrusted hash blocks are never treated as authority.
    pub fn verify_block(
        &self,
        mut index: u64,
        block: &[u8],
        mut read_tree: impl FnMut(u64, &mut [u8]) -> Result<(), i32>,
    ) -> Result<(), i32> {
        if block.len() != self.block_size()
            || index >= self.data_size().div_ceil(self.block_size() as u64)
        {
            return Err(EINVAL);
        }
        let layout = self.layout()?;
        let mut hash = self.hash(block);
        let mut tree_block = vec![0; self.block_size()];
        for start in layout.starts {
            let offset = start + (index / self.arity()) * self.block_size() as u64;
            read_tree(offset, &mut tree_block)?;
            let slot = (index % self.arity()) as usize * self.digest_size();
            if tree_block[slot..slot + self.digest_size()] != hash {
                return Err(EIO);
            }
            hash = self.hash(&tree_block);
            index /= self.arity();
        }
        if self.0[16..16 + self.digest_size()] != hash {
            return Err(EIO);
        }
        Ok(())
    }
}

/// Streaming builder: at most one block per tree level plus a data block is
/// retained. Even very large input files never require a whole-file allocation.
pub fn build(
    mut descriptor: Descriptor,
    mut read_data: impl FnMut(u64, &mut [u8]) -> Result<(), i32>,
    mut write_tree: impl FnMut(u64, &[u8]) -> Result<(), i32>,
) -> Result<Descriptor, i32> {
    let layout = descriptor.layout()?;
    let block_size = descriptor.block_size();
    let digest_size = descriptor.digest_size();
    let mut buffers = vec![vec![0u8; block_size]; layout.starts.len()];
    let mut filled = vec![0usize; buffers.len()];
    let mut written = vec![0u64; buffers.len()];
    let mut root = vec![0u8; digest_size];
    let mut feed = |mut level: usize, mut hash: Vec<u8>| -> Result<(), i32> {
        loop {
            if level == buffers.len() {
                root.copy_from_slice(&hash);
                return Ok(());
            }
            let next = filled[level] + digest_size;
            buffers[level][filled[level]..next].copy_from_slice(&hash);
            filled[level] = next;
            if next != block_size {
                return Ok(());
            }
            write_tree(layout.starts[level] + written[level], &buffers[level])?;
            written[level] += block_size as u64;
            hash = descriptor.hash(&buffers[level]);
            buffers[level].fill(0);
            filled[level] = 0;
            level += 1;
        }
    };
    let mut block = vec![0; block_size];
    let mut offset = 0u64;
    while offset < descriptor.data_size() {
        block.fill(0);
        let amount = (descriptor.data_size() - offset).min(block_size as u64) as usize;
        read_data(offset, &mut block[..amount])?;
        feed(0, descriptor.hash(&block))?;
        offset += amount as u64;
    }
    // Release closure borrows before flushing partial blocks bottom-up.
    drop(feed);
    for level in 0..buffers.len() {
        if filled[level] == 0 {
            continue;
        }
        write_tree(layout.starts[level] + written[level], &buffers[level])?;
        let hash = descriptor.hash(&buffers[level]);
        if level + 1 == buffers.len() {
            root.copy_from_slice(&hash);
        } else {
            let slot = filled[level + 1];
            // A partial final child contributes exactly one digest. It cannot
            // overflow the next level, whose last full block was already sent.
            buffers[level + 1][slot..slot + digest_size].copy_from_slice(&hash);
            filled[level + 1] += digest_size;
        }
    }
    descriptor.0[16..16 + digest_size].copy_from_slice(&root);
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    // Independent published libfsverity vectors, not values generated by our
    // builder. Upstream test data and vectors: Google LLC, MIT license.
    // https://github.com/ebiggers/fsverity-utils/blob/36660d79c9ddbc4d560d9a496b6b67c1c25dcd13/programs/test_compute_digest.c
    #[test]
    fn published_libfsverity_digest_vectors_match() {
        let cases: &[(u32, u32, u64, &[u8], &str)] = &[
            (
                1,
                4096,
                1_000_000,
                b"",
                "48df0c462329cd879661bd05b39aa81b05cc16afd27a7196a559da83531d39d9",
            ),
            (
                1,
                4096,
                100_000,
                b"",
                "f2096a36c5cdca4fa33ee8852833150bb324992e5417a9d571f1bffff73b9efc",
            ),
            (
                1,
                4096,
                4096,
                b"",
                "6ac39979016e3ddf3d39fff6cb984f7c118acdf1852919f5c100c4b142c1818e",
            ),
            (
                1,
                4096,
                1,
                b"",
                "b803429503d95915829b29fdbc8bbad142f3abfd11b1cadf5526582e685c0551",
            ),
            (
                1,
                4096,
                0,
                b"",
                "3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95",
            ),
            (
                1,
                4096,
                1_000_000,
                b"abcd",
                "917900b0d299454aa304d5debc6f39e4af7b5abe33bdbc568d5d8f1e5c4d8652",
            ),
            (
                1,
                4096,
                1_000_000,
                b"0123456789:;<=>?@ABCDEFGHIJKLMNO",
                "bc2d70324c048c220a2cb190832140863eb268e680427939e5d467bea5ec5ad9",
            ),
            (
                1,
                1024,
                1_000_000,
                b"",
                "e9df927c14fcb961d5f51c666d8ae4c14fe4ff98a374c733e898d00c9e74a8e3",
            ),
            (
                2,
                4096,
                1_000_000,
                b"abcd",
                "8425c6d0c94f84ed904c12936845fbb7af9953753789712dcc3be142db3d4b6b47a399ad52aa609256ce29a960bf4bb0e595ec386ca58c06519d546dc5b197bb",
            ),
        ];
        for &(algorithm, block_size, length, salt, expected) in cases {
            let descriptor = build(
                Descriptor::new(algorithm, block_size, salt, length).unwrap(),
                |offset, output| {
                    for (index, byte) in output.iter_mut().enumerate() {
                        let i = offset + index as u64;
                        *byte = ((i % 11) + (i % 439) + (i % 1103)) as u8;
                    }
                    Ok(())
                },
                |_, _| Ok(()),
            )
            .unwrap();
            assert_eq!(
                hex(&descriptor.digest()),
                expected,
                "algorithm={algorithm} block={block_size} len={length}"
            );
        }
    }

    #[test]
    fn published_libfsverity_tree_blocks_and_offsets_match() {
        let expected = [
            (
                1024,
                "f789baab53859faf36d6d75d1042064294202d6e13e7716f394fba434ccc4986",
            ),
            (
                2048,
                "f789baab53859faf36d6d75d1042064294202d6e13e7716f394fba434ccc4986",
            ),
            (
                3072,
                "f789baab53859faf36d6d75d1042064294202d6e13e7716f394fba434ccc4986",
            ),
            (
                4096,
                "00fed03c5d6eab213143f3d96a5ca31c2b89f5684e6c8e07873e5e976517b48f",
            ),
            (
                0,
                "68c538e11958d65d68b6fe8e9fb8ccabecfd928b01d06344e223ed41ddc4544a",
            ),
        ];
        let mut observed = Vec::new();
        let result = build(
            Descriptor::new(1, 1024, &[], 100_000).unwrap(),
            |_, output| {
                output.fill(0);
                Ok(())
            },
            |offset, block| {
                observed.push((offset, hex(&Sha256::digest(block))));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(observed.len(), expected.len());
        for ((offset, digest), (expected_offset, expected_digest)) in observed.iter().zip(expected)
        {
            assert_eq!(*offset, expected_offset);
            assert_eq!(digest, expected_digest);
        }
        assert_eq!(result.tree_size().unwrap(), 5 * 1024);
        assert_eq!(
            hex(&result.digest()),
            "09cbbaeed2a04c2da242c10e1568d96f358a16aa1ebe8cf0286120c13c9366d1"
        );
    }

    #[test]
    fn empty_and_single_block_roots_use_linux_special_cases() {
        for algorithm in [1, 2] {
            let empty = Descriptor::new(algorithm, 4096, &[], 0).unwrap();
            let result = build(
                empty.clone(),
                |_, _| panic!("no data"),
                |_, _| panic!("no tree"),
            )
            .unwrap();
            assert_eq!(result, empty);
            assert_eq!(result.tree_size(), Ok(0));
            let data = b"abc";
            let mut block = vec![0; 4096];
            block[..3].copy_from_slice(data);
            let input = Descriptor::new(algorithm, 4096, &[1, 2, 3], 3).unwrap();
            let result = build(
                input.clone(),
                |_, out| {
                    out.copy_from_slice(data);
                    Ok(())
                },
                |_, _| panic!("no tree"),
            )
            .unwrap();
            assert_eq!(
                &result.bytes()[16..16 + input.digest_size()],
                input.hash(&block)
            );
            result
                .verify_block(0, &block, |_, _| panic!("no tree"))
                .unwrap();
            block[1] ^= 1;
            assert_eq!(
                result.verify_block(0, &block, |_, _| unreachable!()),
                Err(EIO)
            );
            assert_eq!(Descriptor::decode(result.bytes()), Ok(result));
        }
    }

    #[test]
    fn multi_level_tree_verifies_every_block_and_detects_corruption() {
        for algorithm in [1, 2] {
            for size in [1024usize, 2048, 4096] {
                let count = (size / if algorithm == 1 { 32 } else { 64 }) + 3;
                let data: Vec<u8> = (0..count * size - 37)
                    .map(|i| (i * 17 + 23) as u8)
                    .collect();
                let input =
                    Descriptor::new(algorithm, size as u32, b"salt", data.len() as u64).unwrap();
                let mut tree = BTreeMap::new();
                let result = build(
                    input,
                    |offset, out| {
                        out.copy_from_slice(&data[offset as usize..offset as usize + out.len()]);
                        Ok(())
                    },
                    |offset, block| {
                        assert!(tree.insert(offset, block.to_vec()).is_none());
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(tree.len() as u64 * size as u64, result.tree_size().unwrap());
                for index in 0..count {
                    let mut block = vec![0u8; size];
                    let n = (data.len() - index * size).min(size);
                    block[..n].copy_from_slice(&data[index * size..index * size + n]);
                    result
                        .verify_block(index as u64, &block, |off, out| {
                            out.copy_from_slice(&tree[&off]);
                            Ok(())
                        })
                        .unwrap();
                }
                tree.get_mut(&0).unwrap()[0] ^= 1;
                assert_eq!(
                    result.verify_block(0, &data[..size], |off, out| {
                        out.copy_from_slice(&tree[&off]);
                        Ok(())
                    }),
                    Err(EIO)
                );
            }
        }
    }

    #[test]
    fn streaming_carries_and_partial_flushes_cross_three_levels() {
        for algorithm in [1, 2] {
            let size = 1024usize;
            let arity = size / if algorithm == 1 { 32 } else { 64 };
            for count in [
                arity - 1,
                arity,
                arity + 1,
                arity * arity - 1,
                arity * arity,
                arity * arity + 1,
            ] {
                for padding in [0, 7] {
                    let length = count * size - padding;
                    let fill = |offset: usize, output: &mut [u8]| {
                        for (index, byte) in output.iter_mut().enumerate() {
                            *byte = ((offset + index) % 251) as u8;
                        }
                    };
                    let mut tree = BTreeMap::new();
                    let descriptor = build(
                        Descriptor::new(algorithm, size as u32, b"carry", length as u64).unwrap(),
                        |offset, output| {
                            fill(offset as usize, output);
                            Ok(())
                        },
                        |offset, block| {
                            assert!(tree.insert(offset, block.to_vec()).is_none());
                            Ok(())
                        },
                    )
                    .unwrap();
                    assert_eq!(
                        tree.len() as u64 * size as u64,
                        descriptor.tree_size().unwrap()
                    );
                    for index in 0..count {
                        let mut block = vec![0u8; size];
                        let wanted = size.min(length - index * size);
                        fill(index * size, &mut block[..wanted]);
                        descriptor
                            .verify_block(index as u64, &block, |offset, output| {
                                output.copy_from_slice(&tree[&offset]);
                                Ok(())
                            })
                            .unwrap();
                    }
                }
            }
        }
    }
}
