//! Utilities for working with orderable and partially orderable types

use std::{cmp::Ordering, iter};

/// Partition the entries according to the partial comparison function. Returns two collections:
/// `max` and `rest`. `max` contains all items that compare `Some(Greater)` than all the items from
/// `rest`. Each item from `max` compares either `Some(Equal)` or `None` to all the other items from
/// `max`.
pub(crate) fn partial_partition_by<I, F, M>(entries: I, mut compare: F) -> (Vec<I::Item>, M)
where
    I: IntoIterator,
    F: FnMut(&I::Item, &I::Item) -> Option<Ordering>,
    M: Default + Extend<I::Item>,
{
    let mut max: Vec<I::Item> = Vec::new();
    let mut rest = M::default();

    for new in entries {
        let mut index = 0;
        let mut push = true;

        while index < max.len() {
            let old = &max[index];

            match compare(old, &new) {
                Some(Ordering::Less) => {
                    // Note: using `Vec::remove` to maintain the original order. Is there a more
                    // efficient way?
                    rest.extend(iter::once(max.remove(index)));
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
            rest.extend(iter::once(new));
        }
    }

    (max, rest)
}

// Returns the maximal entries according to the partial comparison function.
pub(crate) fn partial_max_by<I, F>(entries: I, compare: F) -> Vec<I::Item>
where
    I: IntoIterator,
    F: FnMut(&I::Item, &I::Item) -> Option<Ordering>,
{
    #[derive(Default)]
    struct Discard;

    impl<A> Extend<A> for Discard {
        fn extend<T>(&mut self, iter: T)
        where
            T: IntoIterator<Item = A>,
        {
            for _ in iter {}
        }
    }

    let (max, Discard) = partial_partition_by(entries, compare);
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::sign::PublicKey,
        iterator::{self, PairCombinations},
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use proptest::{arbitrary::any, collection::vec, sample::select, strategy::Strategy};
    use std::ops::Range;
    use test_strategy::proptest;

    #[proptest]
    fn partition_by_test(
        #[strategy(entry_vec_strategy(0..10, 1..20, 30))] entries: Vec<TestEntry>,
    ) {
        partition_by_test_case(entries)
    }

    fn partition_by_test_case(entries: Vec<TestEntry>) {
        let (max, rest): (_, Vec<_>) = partial_partition_by(entries.iter().cloned(), |a, b| {
            a.version_vector.partial_cmp(&b.version_vector)
        });

        // Every input entry must end up either in `max` or `rest`.
        assert_eq!(entries.len(), max.len() + rest.len());

        for entry in entries {
            assert!(max.contains(&entry) || rest.contains(&entry));
        }

        // For every entry in `rest`, there must be at least one entry in `max` which is
        // greater or equal.
        for a in &rest {
            let found = max.iter().any(|b| {
                matches!(
                    a.version_vector.partial_cmp(&b.version_vector),
                    Some(Ordering::Less | Ordering::Equal)
                )
            });

            assert!(found);
        }

        // Any two entries in `max` must either be equal or concurrent.
        for (a, b) in PairCombinations::new(&max) {
            assert_matches!(
                a.version_vector.partial_cmp(&b.version_vector),
                Some(Ordering::Equal) | None,
                "{:?}, {:?} must be equal or concurrent",
                a,
                b
            )
        }

        // `max` must preserve original order.
        assert!(iterator::is_sorted_by_key(&max, |entry| entry.index));
    }

    #[derive(Clone, Eq, PartialEq, Debug)]
    struct TestEntry {
        version_vector: VersionVector,
        index: usize,
    }

    fn entry_vec_strategy(
        num_entries: Range<usize>,
        num_branches: Range<usize>,
        max_version: u64,
    ) -> impl Strategy<Value = Vec<TestEntry>> {
        vec(any::<PublicKey>(), num_branches)
            .prop_flat_map(move |public_keys| {
                vec(
                    entry_strategy(public_keys, max_version),
                    num_entries.clone(),
                )
            })
            .prop_map(|mut entries| {
                for (index, entry) in entries.iter_mut().enumerate() {
                    entry.index = index;
                }

                entries
            })
    }

    fn entry_strategy(
        public_keys: Vec<PublicKey>,
        max_version: u64,
    ) -> impl Strategy<Value = TestEntry> {
        let max_len = public_keys.len();
        vec((select(public_keys), 0..max_version), 1..=max_len).prop_map(|entries| {
            let version_vector = entries.into_iter().collect();

            TestEntry {
                version_vector,
                index: 0,
            }
        })
    }
}
