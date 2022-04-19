//! Utilities for working with versioned entries.

use crate::{
    crypto::sign::PublicKey,
    directory::{EntryRef, FileRef},
    version_vector::VersionVector,
};
use std::cmp::Ordering;

pub(super) trait Versioned {
    fn version_vector(&self) -> &VersionVector;
    fn branch_id(&self) -> &PublicKey;
}

impl Versioned for EntryRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        EntryRef::version_vector(self)
    }

    fn branch_id(&self) -> &PublicKey {
        EntryRef::branch_id(self)
    }
}

impl Versioned for FileRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        FileRef::version_vector(self)
    }

    fn branch_id(&self) -> &PublicKey {
        FileRef::branch_id(self)
    }
}

pub(super) trait Container<E>: Default {
    fn insert(&mut self, item: E);
}

impl<E> Container<E> for Vec<E> {
    fn insert(&mut self, item: E) {
        self.push(item)
    }
}

#[derive(Default)]
pub(super) struct Discard;

impl<E> Container<E> for Discard {
    fn insert(&mut self, _: E) {}
}

// Partition the entries into those with the maximal versions and the rest.
pub(super) fn partition<I, M>(entries: I, local_branch_id: Option<&PublicKey>) -> (Vec<I::Item>, M)
where
    I: IntoIterator,
    I::Item: Versioned + std::fmt::Debug,
    M: Container<I::Item>,
{
    let mut max: Vec<I::Item> = Vec::new();
    let mut min = M::default();

    for new in entries {
        let new_is_local = local_branch_id == Some(new.branch_id());
        let mut index = 0;
        let mut push = true;

        while index < max.len() {
            match (
                max[index]
                    .version_vector()
                    .partial_cmp(new.version_vector()),
                new_is_local,
            ) {
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
pub(super) fn keep_maximal<I>(entries: I, local_branch_id: Option<&PublicKey>) -> Vec<I::Item>
where
    I: IntoIterator,
    I::Item: Versioned + std::fmt::Debug,
{
    let (max, Discard) = partition(entries, local_branch_id);
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use proptest::{arbitrary::any, collection::vec, sample::Index, strategy::Strategy};
    use std::ops::Range;
    use test_strategy::proptest;

    #[proptest]
    fn partition_test(#[strategy(entry_vec_strategy(0..10, 1..20, 30))] entries: Vec<TestEntry>) {
        partition_test_case(entries)
    }

    fn partition_test_case(entries: Vec<TestEntry>) {
        let (max, min): (_, Vec<_>) = partition(entries, None);

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
        for i in 0..max.len() {
            for j in 0..i {
                let a = &max[i];
                let b = &max[j];

                assert_matches!(
                    a.version_vector.partial_cmp(&b.version_vector),
                    None,
                    "{:?}, {:?} must be concurrent",
                    a,
                    b
                );
                assert_ne!(a.branch_id, b.branch_id);
            }
        }
    }

    #[derive(Debug)]
    struct TestEntry {
        version_vector: VersionVector,
        branch_id: PublicKey,
    }

    impl Versioned for TestEntry {
        fn version_vector(&self) -> &VersionVector {
            &self.version_vector
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
                    for i in 0..entries.len() {
                        for j in 0..i {
                            if entries[i].branch_id == entries[j].branch_id
                                && entries[i]
                                    .version_vector
                                    .partial_cmp(&entries[j].version_vector)
                                    == None
                            {
                                return false;
                            }
                        }
                    }

                    true
                },
            )
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
            }
        })
    }
}
