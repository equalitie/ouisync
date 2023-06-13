//! Utilities for working with versioned entries.

use crate::{crypto::sign::PublicKey, version_vector::VersionVector};
use std::cmp::Ordering;

/// Trait for things that have associated version vector.
pub(crate) trait Versioned {
    fn version_vector(&self) -> &VersionVector;
}

/// Trait for algorithms used to break ties when two distinct `Versioned`s have the same version
/// vector (NOT when they are concurrent though - concurrency is always preserved).
pub(crate) trait Tiebreaker<T: Versioned> {
    fn break_tie(&self, lhs: &T, rhs: &T) -> Ordering;
}

/// Default tiebreaker that doesn't break any ties.
impl<T: Versioned> Tiebreaker<T> for () {
    fn break_tie(&self, _lhs: &T, _rhs: &T) -> Ordering {
        Ordering::Equal
    }
}

/// Trait for things that belong to a branch.
pub(crate) trait BranchItem {
    fn branch_id(&self) -> &PublicKey;
}

/// Tiebreaker that prefers the item that belongs to the given branch. If no item belongs to it or
/// if no branch specified, arbitrarily prefers the one whose branch id is greater.
pub(crate) struct PreferBranch<'a>(pub Option<&'a PublicKey>);

impl<T: Versioned + BranchItem> Tiebreaker<T> for PreferBranch<'_> {
    fn break_tie(&self, lhs: &T, rhs: &T) -> Ordering {
        if let Some(preferred) = self.0 {
            match (lhs.branch_id() == preferred, rhs.branch_id() == preferred) {
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (true, true) | (false, false) => lhs.branch_id().cmp(rhs.branch_id()),
            }
        } else {
            lhs.branch_id().cmp(rhs.branch_id())
        }
    }
}

/// Trait for containers for collecting outdated items.
pub(crate) trait Container<E>: Default {
    fn insert(&mut self, item: E);
}

impl<E> Container<E> for Vec<E> {
    fn insert(&mut self, item: E) {
        self.push(item)
    }
}

/// Container for outdated items that discards them.
#[derive(Default)]
pub(crate) struct Discard;

impl<E> Container<E> for Discard {
    fn insert(&mut self, _: E) {}
}

// Partition the entries into those with the maximal versions and the rest.
pub(crate) fn partition<I, T, M>(entries: I, tiebreaker: T) -> (Vec<I::Item>, M)
where
    I: IntoIterator,
    I::Item: Versioned,
    T: Tiebreaker<I::Item>,
    M: Container<I::Item>,
{
    let mut max: Vec<I::Item> = Vec::new();
    let mut min = M::default();

    for new in entries {
        let mut index = 0;
        let mut push = true;

        while index < max.len() {
            let old = &max[index];

            match old
                .version_vector()
                .partial_cmp(new.version_vector())
                .map(|cmp| cmp.then_with(|| tiebreaker.break_tie(old, &new)))
            {
                Some(Ordering::Less) => {
                    // Note: using `Vec::remove` to maintain the original order. Is there a more
                    // efficient way?
                    min.insert(max.remove(index));
                }
                Some(Ordering::Greater) => {
                    push = false;
                    break;
                }
                Some(Ordering::Equal) | None => {
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
pub(crate) fn keep_maximal<I, T>(entries: I, tiebreaker: T) -> Vec<I::Item>
where
    I: IntoIterator,
    I::Item: Versioned,
    T: Tiebreaker<I::Item>,
{
    let (max, Discard) = partition(entries, tiebreaker);
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        iterator::{self, PairCombinations},
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use proptest::{
        arbitrary::any,
        collection::vec,
        sample::select,
        strategy::{Just, Strategy},
    };
    use std::ops::Range;
    use test_strategy::proptest;

    #[proptest]
    fn partition_test(#[strategy(entry_vec_strategy(0..10, 1..20, 30))] entries: Vec<TestEntry>) {
        partition_test_case(entries)
    }

    fn partition_test_case(entries: Vec<TestEntry>) {
        let (max, min): (_, Vec<_>) = partition(entries.iter().cloned(), ());

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

        // Any two entries in `max` must either be concurrent and have different branch ids or be
        // equal and have the same branch ids.
        for (a, b) in PairCombinations::new(&max) {
            if a.branch_id == b.branch_id {
                assert_eq!(a.version_vector, b.version_vector)
            } else {
                assert_matches!(
                    a.version_vector.partial_cmp(&b.version_vector),
                    None,
                    "{:?}, {:?} must be concurrent",
                    a,
                    b
                )
            }
        }

        // `max` must preserve original order.
        assert!(iterator::is_sorted_by_key(&max, |entry| entry.index));
    }

    #[test]
    fn tiebreak() {
        let id0 = PublicKey::random();
        let id1 = PublicKey::random();
        let vv = VersionVector::new().incremented(id0).incremented(id1);

        let entries = [
            TestEntry {
                version_vector: vv.clone(),
                branch_id: id0,
                index: 0,
            },
            TestEntry {
                version_vector: vv,
                branch_id: id1,
                index: 1,
            },
        ];

        let (max, min): (_, Vec<_>) = partition(entries.clone(), ());
        assert_eq!(max, entries);
        assert!(min.is_empty());

        let (max, min): (_, Vec<_>) = partition(entries.clone(), PreferBranch(None));
        assert_eq!(max.len(), 1);
        assert_eq!(min.len(), 1);

        if id0 > id1 {
            assert_eq!(max[0], entries[0]);
            assert_eq!(min[0], entries[1]);
        } else {
            assert_eq!(max[0], entries[1]);
            assert_eq!(min[0], entries[0]);
        }

        let (max, min): (_, Vec<_>) = partition(entries.clone(), PreferBranch(Some(&id0)));
        assert_eq!(max.len(), 1);
        assert_eq!(min.len(), 1);
        assert_eq!(max[0], entries[0]);
        assert_eq!(min[0], entries[1]);

        let (max, min): (_, Vec<_>) = partition(entries.clone(), PreferBranch(Some(&id1)));
        assert_eq!(max.len(), 1);
        assert_eq!(min.len(), 1);
        assert_eq!(max[0], entries[1]);
        assert_eq!(min[0], entries[0]);
    }

    #[derive(Clone, Eq, PartialEq, Debug)]
    struct TestEntry {
        version_vector: VersionVector,
        branch_id: PublicKey,
        index: usize,
    }

    impl Versioned for TestEntry {
        fn version_vector(&self) -> &VersionVector {
            &self.version_vector
        }
    }

    impl BranchItem for TestEntry {
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
        let max_len = public_keys.len();
        vec((select(public_keys), 0..max_version), 1..=max_len)
            .prop_flat_map(|entries| {
                let branch_id_index = 0..entries.len();
                (Just(entries), branch_id_index)
            })
            .prop_map(|(entries, branch_id_index)| {
                let branch_id = entries[branch_id_index].0;
                let version_vector = entries.into_iter().collect();

                TestEntry {
                    version_vector,
                    branch_id,
                    index: 0,
                }
            })
    }
}
