//! Complete BLAKE3 hashing with bounded, short-lived parallelism for large ELFs.
use blake3::hazmat::{
    ChainingValue, HasherExt, Mode, left_subtree_len, merge_subtrees_non_root, merge_subtrees_root,
};

const PARALLEL_THRESHOLD: usize = 8 * 1024 * 1024;
const SUBTREE_THRESHOLD: usize = 4 * 1024 * 1024;

pub(super) fn hash(data: &[u8]) -> blake3::Hash {
    if data.len() < PARALLEL_THRESHOLD {
        return blake3::hash(data);
    }
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    if threads < 2 {
        return blake3::hash(data);
    }
    // A concurrent fork must not snapshot Rust thread bookkeeping while these
    // temporary host workers exist. All workers join before this guard drops;
    // there is no process-global pool for a fork child to inherit.
    let Some(_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        return blake3::hash(data);
    };
    parallel_hash(data, if threads >= 4 { 2 } else { 1 })
}

fn leaf(data: &[u8], offset: u64) -> ChainingValue {
    blake3::Hasher::new()
        .set_input_offset(offset)
        .update(data)
        .finalize_non_root()
}

fn children(data: &[u8], offset: u64, depth: u32) -> (ChainingValue, ChainingValue) {
    // BLAKE3's left subtree is the largest power-of-two number of chunks
    // strictly smaller than the input. Arbitrary equal byte halves are invalid.
    let split = left_subtree_len(data.len() as u64) as usize;
    let (left, right) = data.split_at(split);
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("image-hash".into())
            .spawn_scoped(scope, || subtree(left, offset, depth - 1));
        let rhs = subtree(right, offset + split as u64, depth - 1);
        let lhs = match worker {
            Ok(worker) => worker.join().unwrap_or_else(|_| leaf(left, offset)),
            Err(_) => leaf(left, offset),
        };
        (lhs, rhs)
    })
}

fn subtree(data: &[u8], offset: u64, depth: u32) -> ChainingValue {
    if depth == 0 || data.len() < SUBTREE_THRESHOLD {
        return leaf(data, offset);
    }
    let (left, right) = children(data, offset, depth);
    merge_subtrees_non_root(&left, &right, Mode::Hash)
}

fn parallel_hash(data: &[u8], depth: u32) -> blake3::Hash {
    let (left, right) = children(data, 0, depth);
    merge_subtrees_root(&left, &right, Mode::Hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_tree_matches_standard_hash_at_chunk_and_subtree_boundaries() {
        let bytes: Vec<_> = (0..33 * 1024 * 1024)
            .map(|i| ((i * 31 + (i >> 8)) % 251) as u8)
            .collect();
        for length in [0, 1, 63, 64, 65, 1023, 1024] {
            assert_eq!(hash(&bytes[..length]), blake3::hash(&bytes[..length]));
        }
        for boundary in [1024, 2048, 16384, 4 << 20, 8 << 20, 16 << 20, 32 << 20] {
            for length in [boundary - 1, boundary, boundary + 1] {
                if length <= blake3::CHUNK_LEN {
                    continue;
                }
                let expected = blake3::hash(&bytes[..length]);
                for depth in [1, 2] {
                    assert_eq!(
                        parallel_hash(&bytes[..length], depth),
                        expected,
                        "length={length}, depth={depth}"
                    );
                }
            }
        }
        // Uneven right subtree, matching the measured libjvm.so size.
        assert_eq!(
            hash(&bytes[..29_876_336]),
            blake3::hash(&bytes[..29_876_336])
        );
    }
}
