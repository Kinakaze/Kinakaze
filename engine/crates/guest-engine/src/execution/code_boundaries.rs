//! Exact instruction addresses using one bit per executable byte.
//!
//! A hash entry per instruction makes large stripped executables expensive to
//! scan and leaves a large freed allocation in the forked managed arena.

use crate::execution::ExecutionError;

struct Segment {
    start: usize,
    end: usize,
    words: Vec<u64>,
}

pub(crate) struct CodeBoundaries {
    segments: Vec<Segment>,
}

impl CodeBoundaries {
    pub(crate) fn new(ranges: &[(usize, usize)]) -> Result<Self, ExecutionError> {
        let mut ranges = ranges
            .iter()
            .map(|&(start, len)| {
                Ok((
                    start,
                    start
                        .checked_add(len)
                        .ok_or(ExecutionError::AddressOverflow)?,
                ))
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in ranges {
            if start == end {
                continue;
            }
            if let Some(previous) = merged.last_mut().filter(|previous| start <= previous.1) {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        // ELF program headers need not be ordered or disjoint. Normalize them
        // so iteration remains unique and ascending, just like the old sorted
        // address set used by the detour planner.
        let segments = merged
            .into_iter()
            .map(|(start, end)| Segment {
                start,
                end,
                words: vec![0; (end - start).div_ceil(64)],
            })
            .collect();
        Ok(Self { segments })
    }

    pub(crate) fn contains(&self, address: usize) -> bool {
        self.segments.iter().any(|segment| {
            if address < segment.start || address >= segment.end {
                return false;
            }
            let offset = address - segment.start;
            segment.words[offset / 64] & (1 << (offset % 64)) != 0
        })
    }

    /// Returns true exactly once for an address inside the executable ranges.
    pub(crate) fn insert(&mut self, address: usize) -> bool {
        let Some(segment) = self
            .segments
            .iter_mut()
            .find(|segment| address >= segment.start && address < segment.end)
        else {
            return false;
        };
        let offset = address - segment.start;
        let word = &mut segment.words[offset / 64];
        let bit = 1 << (offset % 64);
        let fresh = *word & bit == 0;
        *word |= bit;
        fresh
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.segments.iter().flat_map(|segment| {
            segment
                .words
                .iter()
                .enumerate()
                .flat_map(move |(index, &word)| {
                    let base = segment.start + index * 64;
                    let mut remaining = word;
                    std::iter::from_fn(move || {
                        if remaining == 0 {
                            return None;
                        }
                        let address = base + remaining.trailing_zeros() as usize;
                        remaining &= remaining - 1;
                        Some(address)
                    })
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn exact_addresses_across_words_gaps_and_partial_tail() {
        let mut boundaries = CodeBoundaries::new(&[(0x1003, 130), (0x3000, 65)]).unwrap();
        let mut expected = BTreeSet::new();
        for address in 0..0x3100 {
            if address % 3 != 0 {
                continue;
            }
            let in_range =
                (0x1003..0x1085).contains(&address) || (0x3000..0x3041).contains(&address);
            assert_eq!(boundaries.insert(address), in_range);
            assert!(!boundaries.insert(address));
            if in_range {
                expected.insert(address);
            }
        }
        for address in 0..0x3100 {
            assert_eq!(boundaries.contains(address), expected.contains(&address));
        }
        assert_eq!(
            boundaries.iter().collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>()
        );
        assert_eq!(
            boundaries
                .segments
                .iter()
                .map(|s| s.words.len())
                .sum::<usize>(),
            5
        );
    }

    #[test]
    fn empty_and_overflow_ranges() {
        let mut empty = CodeBoundaries::new(&[(5, 0)]).unwrap();
        assert!(!empty.insert(5));
        assert_eq!(empty.iter().next(), None);
        assert!(CodeBoundaries::new(&[(usize::MAX, 1)]).is_err());
    }

    #[test]
    fn unordered_overlapping_segments_yield_unique_sorted_addresses() {
        let mut boundaries = CodeBoundaries::new(&[(200, 100), (100, 300), (400, 2)]).unwrap();
        for address in (100..402).rev() {
            assert!(boundaries.insert(address));
            assert!(!boundaries.insert(address));
        }
        assert_eq!(
            boundaries.iter().collect::<Vec<_>>(),
            (100..402).collect::<Vec<_>>()
        );
        assert_eq!(boundaries.segments.len(), 1);
    }
}
