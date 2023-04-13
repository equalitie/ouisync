//! Utilities for working with versioned entries.

use crate::{
    crypto::sign::PublicKey,
    directory::{EntryRef, FileRef},
    version_vector::VersionVector,
};
use std::cmp::Ordering;

pub(crate) trait Versioned {
    type Tiebreaker<'a>: Copy;

    fn compare_versions(&self, other: &Self, tiebreaker: Self::Tiebreaker<'_>) -> Option<Ordering>;
}

impl Versioned for EntryRef<'_> {
    type Tiebreaker<'a> = Option<&'a PublicKey>;

    fn compare_versions(&self, other: &Self, tiebreaker: Self::Tiebreaker<'_>) -> Option<Ordering> {
        compare_entry_versions(
            self.version_vector(),
            self.branch_id(),
            other.version_vector(),
            other.branch_id(),
            tiebreaker,
        )
    }
}

impl Versioned for FileRef<'_> {
    type Tiebreaker<'a> = Option<&'a PublicKey>;

    fn compare_versions(&self, other: &Self, tiebreaker: Self::Tiebreaker<'_>) -> Option<Ordering> {
        compare_entry_versions(
            self.version_vector(),
            self.branch().id(),
            other.version_vector(),
            other.branch().id(),
            tiebreaker,
        )
    }
}

fn compare_entry_versions(
    lhs_vv: &VersionVector,
    lhs_branch_id: &PublicKey,
    rhs_vv: &VersionVector,
    rhs_branch_id: &PublicKey,
    tiebreaker: Option<&PublicKey>,
) -> Option<Ordering> {
    lhs_vv.partial_cmp(rhs_vv).map(|ord| {
        ord.then_with(|| {
            if let Some(tiebreaker) = tiebreaker {
                match (lhs_branch_id == tiebreaker, rhs_branch_id == tiebreaker) {
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (true, true) | (false, false) => Ordering::Equal,
                }
            } else {
                lhs_branch_id.cmp(rhs_branch_id)
            }
        })
    })
}

pub(crate) trait Container<E>: Default {
    fn insert(&mut self, item: E);
}

impl<E> Container<E> for Vec<E> {
    fn insert(&mut self, item: E) {
        self.push(item)
    }
}

#[derive(Default)]
pub(crate) struct Discard;

impl<E> Container<E> for Discard {
    fn insert(&mut self, _: E) {}
}

// Partition the entries into those with the maximal versions and the rest.
pub(crate) fn partition<I, M>(
    entries: I,
    tiebreaker: <I::Item as Versioned>::Tiebreaker<'_>,
) -> (Vec<I::Item>, M)
where
    I: IntoIterator,
    I::Item: Versioned,
    M: Container<I::Item>,
{
    let mut max: Vec<I::Item> = Vec::new();
    let mut min = M::default();

    for new in entries {
        let mut index = 0;
        let mut push = true;

        while index < max.len() {
            let old = &max[index];

            match old.compare_versions(&new, tiebreaker) {
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
pub(crate) fn keep_maximal<I>(
    entries: I,
    tiebreaker: <I::Item as Versioned>::Tiebreaker<'_>,
) -> Vec<I::Item>
where
    I: IntoIterator,
    I::Item: Versioned,
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
    use proptest::{arbitrary::any, collection::vec, sample::Index, strategy::Strategy};
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
        assert_eq!(max.len(), 1);
        assert_eq!(min.len(), 1);

        if id0 > id1 {
            assert_eq!(max[0], entries[0]);
            assert_eq!(min[0], entries[1]);
        } else {
            assert_eq!(max[0], entries[1]);
            assert_eq!(min[0], entries[0]);
        }
    }

    #[derive(Clone, Eq, PartialEq, Debug)]
    struct TestEntry {
        version_vector: VersionVector,
        branch_id: PublicKey,
        index: usize,
    }

    impl Versioned for TestEntry {
        type Tiebreaker<'a> = ();

        fn compare_versions(&self, other: &Self, _: ()) -> Option<Ordering> {
            self.version_vector
                .partial_cmp(&other.version_vector)
                .map(|ord| ord.then_with(|| self.branch_id.cmp(&other.branch_id)))
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
