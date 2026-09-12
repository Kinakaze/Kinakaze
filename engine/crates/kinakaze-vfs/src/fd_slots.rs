//! Sparse descriptor storage. A high dup2 target allocates its own page, not
//! every intervening slot. Bitmaps include unpublished syscall reservations.
use crate::{FdEntry, MAX_FDS};
use std::ops::Index;

const WIDTH: usize = u64::BITS as usize;
const BRANCH_SIZE: usize = WIDTH * WIDTH;
const EMPTY: Option<FdEntry> = None;

struct Page {
    entries: [Option<FdEntry>; WIDTH],
    used: u64,
    reserved: u64,
}

struct Branch {
    pages: [Option<Box<Page>>; WIDTH],
}

#[derive(Default)]
pub(super) struct Slots {
    branches: Vec<Option<Box<Branch>>>,
    first_free: usize,
}

impl Slots {
    fn page(&self, fd: usize) -> Option<&Page> {
        self.branches.get(fd / BRANCH_SIZE)?.as_ref()?.pages[fd / WIDTH % WIDTH].as_deref()
    }

    fn page_mut(&mut self, fd: usize) -> Option<&mut Page> {
        self.branches.get_mut(fd / BRANCH_SIZE)?.as_mut()?.pages[fd / WIDTH % WIDTH].as_deref_mut()
    }

    fn ensure_page(&mut self, fd: usize) -> &mut Page {
        assert!(fd < MAX_FDS);
        let index = fd / BRANCH_SIZE;
        if self.branches.len() <= index {
            self.branches.resize_with(index + 1, || None);
        }
        let branch = self.branches[index].get_or_insert_with(|| {
            Box::new(Branch {
                pages: std::array::from_fn(|_| None),
            })
        });
        branch.pages[fd / WIDTH % WIDTH].get_or_insert_with(|| {
            Box::new(Page {
                entries: [None; WIDTH],
                used: 0,
                reserved: 0,
            })
        })
    }

    pub(super) fn get(&self, fd: usize) -> Option<&Option<FdEntry>> {
        if fd >= MAX_FDS {
            return None;
        }
        Some(
            self.page(fd)
                .map_or(&EMPTY, |page| &page.entries[fd % WIDTH]),
        )
    }

    /// Only modify an existing entry's fields here. Occupancy changes go
    /// through insert/remove, which also maintain allocation and reservations.
    pub(super) fn get_mut(&mut self, fd: usize) -> Option<&mut Option<FdEntry>> {
        self.page_mut(fd).map(|page| &mut page.entries[fd % WIDTH])
    }

    pub(super) fn insert(&mut self, fd: usize, entry: FdEntry) {
        let page = self.ensure_page(fd);
        page.entries[fd % WIDTH] = Some(entry);
        page.used |= 1 << (fd % WIDTH);
        self.advance(fd);
    }

    pub(super) fn remove(&mut self, fd: usize) -> Option<FdEntry> {
        let page = self.page_mut(fd)?;
        let entry = page.entries[fd % WIDTH].take()?;
        page.used &= !(1 << (fd % WIDTH));
        if page.reserved & (1 << (fd % WIDTH)) == 0 {
            self.first_free = self.first_free.min(fd);
        }
        self.reclaim(fd);
        Some(entry)
    }

    fn reclaim(&mut self, fd: usize) {
        let Some(branch) = self
            .branches
            .get_mut(fd / BRANCH_SIZE)
            .and_then(Option::as_mut)
        else {
            return;
        };
        let page = &mut branch.pages[fd / WIDTH % WIDTH];
        if page.as_ref().is_some_and(|p| p.used | p.reserved == 0) {
            *page = None;
        }
        if branch.pages.iter().all(Option::is_none) {
            self.branches[fd / BRANCH_SIZE] = None;
            while self.branches.last().is_some_and(Option::is_none) {
                self.branches.pop();
            }
        }
    }

    fn advance(&mut self, fd: usize) {
        if self.first_free == fd {
            self.first_free = self.find_free(fd + 1, MAX_FDS).unwrap_or(MAX_FDS);
        }
    }

    fn find_free(&self, mut floor: usize, ceiling: usize) -> Option<usize> {
        let ceiling = ceiling.min(MAX_FDS);
        while floor < ceiling {
            let Some(page) = self.page(floor) else {
                return Some(floor);
            };
            let available = !(page.used | page.reserved) & (u64::MAX << (floor % WIDTH));
            if available != 0 {
                let fd = floor / WIDTH * WIDTH + available.trailing_zeros() as usize;
                return (fd < ceiling).then_some(fd);
            }
            floor = (floor / WIDTH + 1) * WIDTH;
        }
        None
    }

    pub(super) fn first_free_between(&self, floor: usize, ceiling: usize) -> Option<usize> {
        self.find_free(floor.max(self.first_free), ceiling)
    }

    pub(super) fn reserve(&mut self, fd: usize) {
        let page = self.ensure_page(fd);
        assert!((page.used | page.reserved) & (1 << (fd % WIDTH)) == 0);
        page.reserved |= 1 << (fd % WIDTH);
        self.advance(fd);
    }

    pub(super) fn is_reserved(&self, fd: usize) -> bool {
        self.page(fd)
            .is_some_and(|p| p.reserved & (1 << (fd % WIDTH)) != 0)
    }

    pub(super) fn unreserve(&mut self, fd: usize) {
        if let Some(page) = self.page_mut(fd) {
            page.reserved &= !(1 << (fd % WIDTH));
            if page.used & (1 << (fd % WIDTH)) == 0 {
                self.first_free = self.first_free.min(fd);
            }
            self.reclaim(fd);
        }
    }

    pub(super) fn clear_reservations(&mut self) {
        for branch in self.branches.iter_mut().flatten() {
            for page in &mut branch.pages {
                if let Some(p) = page {
                    p.reserved = 0;
                    if p.used == 0 {
                        *page = None;
                    }
                }
            }
        }
        self.first_free = self.find_free(0, MAX_FDS).unwrap_or(MAX_FDS);
    }

    /// Enumerate allocated pages with original descriptor numbers. Gaps are
    /// skipped in fork/exec and procfs, including after a sparse high dup2.
    pub(super) fn enumerated(&self) -> impl Iterator<Item = (usize, &Option<FdEntry>)> {
        self.branches
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.as_ref().map(|b| (i, b)))
            .flat_map(|(i, b)| {
                b.pages
                    .iter()
                    .enumerate()
                    .filter_map(move |(j, p)| p.as_ref().map(|p| (i, j, p)))
            })
            .flat_map(|(i, j, p)| {
                p.entries
                    .iter()
                    .enumerate()
                    .map(move |(k, e)| (i * BRANCH_SIZE + j * WIDTH + k, e))
            })
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Option<FdEntry>> {
        self.enumerated().map(|(_, entry)| entry)
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Option<FdEntry>> {
        self.branches
            .iter_mut()
            .flatten()
            .flat_map(|b| b.pages.iter_mut().flatten())
            .flat_map(|p| p.entries.iter_mut())
    }
}

impl Index<usize> for Slots {
    type Output = Option<FdEntry>;
    fn index(&self, fd: usize) -> &Self::Output {
        self.get(fd).expect("descriptor index")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FdFlags, FdKind};
    fn entry(fd: usize) -> FdEntry {
        FdEntry {
            raw: 0,
            kind: FdKind::File,
            flags: FdFlags::NONE,
            generation: 1,
            description_id: fd as u64 + 1,
            offset: 0,
        }
    }
    fn bytes(slots: &Slots) -> usize {
        slots.branches.capacity() * size_of::<Option<Box<Branch>>>()
            + slots
                .branches
                .iter()
                .flatten()
                .map(|b| size_of::<Branch>() + b.pages.iter().flatten().count() * size_of::<Page>())
                .sum::<usize>()
    }

    #[test]
    fn sparse_high_descriptors_allocate_only_their_pages_and_reclaim_them() {
        let mut slots = Slots::default();
        assert_eq!(bytes(&slots), 0);
        for fd in 0..3 {
            slots.insert(fd, entry(fd));
        }
        let initial = bytes(&slots);
        let high = MAX_FDS - 1;
        slots.insert(high, entry(high));
        let sparse = bytes(&slots);
        assert!(initial < 4096 && sparse < 16384);
        assert_eq!(
            slots
                .enumerated()
                .filter_map(|(fd, e)| e.map(|_| fd))
                .collect::<Vec<_>>(),
            vec![0, 1, 2, high]
        );
        assert_eq!(slots.first_free_between(3, MAX_FDS), Some(3));
        assert_eq!(slots.first_free_between(high, MAX_FDS), None);
        assert!(slots.get(MAX_FDS).is_none());
        slots.remove(high).unwrap();
        assert_eq!(slots.first_free_between(high, MAX_FDS), Some(high));
        for fd in 0..3 {
            slots.remove(fd).unwrap();
        }
        assert!(slots.branches.is_empty());
        println!(
            "FD_TABLE_BYTES initial_three={initial} plus_highest={sparse} old_fixed_1024={}",
            1024 * size_of::<Option<FdEntry>>()
        );
    }

    #[test]
    fn dense_growth_reuses_holes_and_reservations_across_page_boundaries() {
        let mut slots = Slots::default();
        for fd in 0..65_536 {
            assert_eq!(slots.first_free_between(0, MAX_FDS), Some(fd));
            slots.reserve(fd);
            assert!(slots.is_reserved(fd));
            assert_eq!(slots.first_free_between(0, MAX_FDS), Some(fd + 1));
            slots.insert(fd, entry(fd));
            slots.unreserve(fd);
        }
        assert_eq!(slots.iter().flatten().count(), 65_536);
        assert_eq!(slots.first_free_between(0, 65_536), None);
        for fd in (0..65_536).step_by(2) {
            slots.remove(fd).unwrap();
        }
        for fd in (0..65_536).step_by(2) {
            assert_eq!(slots.first_free_between(0, 65_536), Some(fd));
            slots.reserve(fd);
        }
        assert_eq!(slots.first_free_between(0, 65_536), None);
        slots.clear_reservations();
        assert_eq!(slots.first_free_between(0, MAX_FDS), Some(0));
        for fd in (1..65_536).step_by(2) {
            slots.remove(fd).unwrap();
        }
        assert!(slots.branches.is_empty());
    }

    #[test]
    fn random_mutations_match_an_independent_ordered_set() {
        use std::collections::BTreeSet;
        let mut slots = Slots::default();
        let mut occupied = BTreeSet::new();
        let mut seed = 0x47c9u64;
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let fd = (seed >> 24) as usize % 10_000;
            if seed & 1 == 0 {
                slots.insert(fd, entry(fd));
                occupied.insert(fd);
            } else {
                slots.remove(fd);
                occupied.remove(&fd);
            }
            let floor = (seed >> 40) as usize % 10_000;
            assert_eq!(
                slots.first_free_between(floor, 10_000),
                (floor..10_000).find(|fd| !occupied.contains(fd))
            );
        }
        assert_eq!(
            slots
                .enumerated()
                .filter_map(|(fd, e)| e.map(|_| fd))
                .collect::<Vec<_>>(),
            occupied.into_iter().collect::<Vec<_>>()
        );
    }
}
