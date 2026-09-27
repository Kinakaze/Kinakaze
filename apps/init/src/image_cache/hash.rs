//! Full BLAKE3 on an unpublished snapshot. The init thread owns every helper;
//! no guest address space or fork state is copied while these threads exist.
use blake3::hazmat::{
    ChainingValue, HasherExt, Mode, left_subtree_len, merge_subtrees_non_root, merge_subtrees_root,
};

fn leaf(bytes: &[u8], offset: u64) -> ChainingValue {
    blake3::Hasher::new()
        .set_input_offset(offset)
        .update(bytes)
        .finalize_non_root()
}
fn children(bytes: &[u8], offset: u64, depth: u32) -> (ChainingValue, ChainingValue) {
    let split = left_subtree_len(bytes.len() as u64) as usize;
    let (left, right) = bytes.split_at(split);
    std::thread::scope(|scope| {
        let worker =
            std::thread::Builder::new().spawn_scoped(scope, || subtree(left, offset, depth - 1));
        let rhs = subtree(right, offset + split as u64, depth - 1);
        let lhs = match worker {
            Ok(worker) => worker.join().unwrap_or_else(|_| leaf(left, offset)),
            Err(_) => leaf(left, offset),
        };
        (lhs, rhs)
    })
}
fn subtree(bytes: &[u8], offset: u64, depth: u32) -> ChainingValue {
    if depth == 0 || bytes.len() < 4 * 1024 * 1024 {
        return leaf(bytes, offset);
    }
    let (left, right) = children(bytes, offset, depth);
    merge_subtrees_non_root(&left, &right, Mode::Hash)
}
pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    let threads = std::thread::available_parallelism().map_or(1, |v| v.get());
    if threads < 2 || bytes.len() < 8 * 1024 * 1024 {
        return *blake3::hash(bytes).as_bytes();
    }
    let (left, right) = children(bytes, 0, if threads >= 4 { 2 } else { 1 });
    *merge_subtrees_root(&left, &right, Mode::Hash).as_bytes()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_complete_digest_across_subtree_boundaries() {
        let bytes: Vec<_> = (0..33 * 1024 * 1024)
            .map(|i| ((i * 31 + (i >> 8)) % 251) as u8)
            .collect();
        for length in [
            0,
            1,
            1023,
            1024,
            1025,
            (8 << 20) - 1,
            8 << 20,
            (8 << 20) + 1,
            (16 << 20) + 37,
            29_876_336,
            32 << 20,
            (32 << 20) + 1,
        ] {
            assert_eq!(
                hash(&bytes[..length]),
                *blake3::hash(&bytes[..length]).as_bytes(),
                "length={length}"
            );
        }
    }
}
