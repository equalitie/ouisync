//! Utilities for working with versioned entries.

use crate::{
    crypto::sign::PublicKey,
    directory::{EntryRef, FileRef},
    version_vector::VersionVector,
};
use std::cmp::Ordering;

pub trait Versioned {
    fn compare_versions(&self, other: &Self) -> Option<Ordering>;
    fn branch_id(&self) -> &PublicKey;
}

impl Versioned for EntryRef<'_> {
    fn compare_versions(&self, other: &Self) -> Option<Ordering> {
        compare_entry_versions(
            self.version_vector(),
            self.is_open(),
            other.version_vector(),
            other.is_open(),
        )
    }

    fn branch_id(&self) -> &PublicKey {
        EntryRef::branch_id(self)
    }
}

impl Versioned for FileRef<'_> {
    fn compare_versions(&self, other: &Self) -> Option<Ordering> {
        compare_entry_versions(
            self.version_vector(),
            self.is_open(),
            other.version_vector(),
            other.is_open(),
        )
    }

    fn branch_id(&self) -> &PublicKey {
        self.branch().id()
    }
}

// Compare the entries by their version vectors but override the comparison in case any of the
// entries is open (in use) such that an open entry is always considered up-to-date. For example,
// let's have two entries A and B, where A'a vv is {x:1, y:2} and B's vv is {x:1, y:3}, but A is
// open and B is not. Even though B's vv is happens-after A's, because A is open the comparison
// will returns `None` which results in both entries being included.
fn compare_entry_versions(
    lhs_vv: &VersionVector,
    lhs_is_open: bool,
    rhs_vv: &VersionVector,
    rhs_is_open: bool,
) -> Option<Ordering> {
    match (lhs_vv.partial_cmp(rhs_vv), lhs_is_open, rhs_is_open) {
        (Some(Ordering::Greater), _, false) => Some(Ordering::Greater),
        (Some(Ordering::Equal), false, false) => Some(Ordering::Equal),
        (Some(Ordering::Equal), true, false) => Some(Ordering::Greater),
        (Some(Ordering::Equal), false, true) => Some(Ordering::Less),
        (Some(Ordering::Less), false, _) => Some(Ordering::Less),
        (Some(Ordering::Greater), _, true)
        | (Some(Ordering::Equal), true, true)
        | (Some(Ordering::Less), true, _) => None,
        (None, _, _) => None,
    }
}

pub trait Container<E>: Default {
    fn insert(&mut self, item: E);
}

impl<E> Container<E> for Vec<E> {
    fn insert(&mut self, item: E) {
        self.push(item)
    }
}

#[derive(Default)]
pub struct Discard;

impl<E> Container<E> for Discard {
    fn insert(&mut self, _: E) {}
}

// Partition the entries into those with the maximal versions and the rest.
pub(crate) fn partition<I, M>(entries: I, local_branch_id: Option<&PublicKey>) -> (Vec<I::Item>, M)
where
    I: IntoIterator,
    I::Item: Versioned,
    M: Container<I::Item>,
{
    let mut max: Vec<I::Item> = Vec::new();
    let mut min = M::default();

    for new in entries {
        let new_is_local = local_branch_id == Some(new.branch_id());
        let mut index = 0;
        let mut push = true;

        while index < max.len() {
            match (max[index].compare_versions(&new), new_is_local) {
                // If both have identical versions, prefer the local one
                (Some(Ordering::Less), _) | (Some(Ordering::Equal), true) => {
                    // Note: using `Vec::remove` to maintain the original order. Is there a more
                    // efficient way?
                    min.insert(max.remove(index));
                }
                (Some(Ordering::Greater), _) | (Some(Ordering::Equal), false) => {
                    push = false;
                    break;
                }
                (None, _) => {
                    index += 1;
                }
            }
        }

        if push {
            max.push(new);
        } else {
            min.insert(new);
        }
    }

    (max, min)
}

// Returns the entries with the maximal version vectors.
pub(crate) fn keep_maximal<I>(entries: I, local_branch_id: Option<&PublicKey>) -> Vec<I::Item>
where
    I: IntoIterator,
    I::Item: Versioned,
{
    let (max, Discard) = partition(entries, local_branch_id);
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iterator::{self, PairCombinations};
    use assert_matches::assert_matches;
    use proptest::{arbitrary::any, collection::vec, sample::Index, strategy::Strategy};
    use std::ops::Range;
    use test_strategy::proptest;

    #[proptest]
    fn partition_test(#[strategy(entry_vec_strategy(0..10, 1..20, 30))] entries: Vec<TestEntry>) {
        partition_test_case(entries)
    }

    fn partition_test_case(entries: Vec<TestEntry>) {
        let (max, min): (_, Vec<_>) = partition(entries.iter().cloned(), None);

        // Every input entry must end up either in `max` or `min`.
        assert_eq!(entries.len(), max.len() + min.len());

        for entry in entries {
            assert!(max.contains(&entry) || min.contains(&entry));
        }

        // For every entry in `min`, there must be at least one entry in `max` which is
        // happens-after or equal.
        for a in &min {
            let found = max.iter().any(|b| {
                matches!(
                    a.version_vector.partial_cmp(&b.version_vector),
                    Some(Ordering::Less | Ordering::Equal)
                )
            });

            assert!(found);
        }

        // Any two entries in `max` must be concurrent and have different branch ids.
        for (a, b) in PairCombinations::new(&max) {
            assert_matches!(
                a.version_vector.partial_cmp(&b.version_vector),
                None,
                "{:?}, {:?} must be concurrent",
                a,
                b
            );
            assert_ne!(a.branch_id, b.branch_id);
        }

        // `max` must preserve original order.
        assert!(iterator::is_sorted_by_key(&max, |entry| entry.index));
    }

    #[derive(Clone, Eq, PartialEq, Debug)]
    struct TestEntry {
        version_vector: VersionVector,
        branch_id: PublicKey,
        index: usize,
    }

    impl Versioned for TestEntry {
        fn compare_versions(&self, other: &Self) -> Option<Ordering> {
            self.version_vector.partial_cmp(&other.version_vector)
        }

        fn branch_id(&self) -> &PublicKey {
            &self.branch_id
        }
    }

    fn entry_vec_strategy(
        num_entries: Range<usize>,
        num_branches: Range<usize>,
        max_version: u64,
    ) -> impl Strategy<Value = Vec<TestEntry>> {
        vec(any::<PublicKey>(), num_branches)
            .prop_flat_map(move |public_keys| {
                vec(
                    entry_with_public_keys_strategy(public_keys, max_version),
                    num_entries.clone(),
                )
            })
            .prop_filter(
                "broken invariant: entries from the same branch can't be concurrent",
                |entries| {
                    for (a, b) in PairCombinations::new(entries) {
                        if a.branch_id == b.branch_id
                            && a.version_vector.partial_cmp(&b.version_vector).is_none()
                        {
                            return false;
                        }
                    }

                    true
                },
            )
            .prop_map(|mut entries| {
                for (index, entry) in entries.iter_mut().enumerate() {
                    entry.index = index;
                }

                entries
            })
    }

    fn entry_with_public_keys_strategy(
        public_keys: Vec<PublicKey>,
        max_version: u64,
    ) -> impl Strategy<Value = TestEntry> {
        let branch_id = 0..public_keys.len();
        let versions = vec((any::<Index>(), 0..max_version), 1..=public_keys.len());

        (branch_id, versions).prop_map(move |(branch_id, versions)| {
            let version_vector =
                versions
                    .into_iter()
                    .fold(VersionVector::new(), |mut vv, (index, version)| {
                        vv.insert(*index.get(&public_keys), version);
                        vv
                    });
            let branch_id = public_keys[branch_id];

            TestEntry {
                version_vector,
                branch_id,
                index: 0,
            }
        })
    }
}
