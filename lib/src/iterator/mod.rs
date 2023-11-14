mod accumulate;
mod combinations;
mod sorted_union;

use crate::collections::{hash_set, HashSet};
use std::hash::Hash;

#[cfg(test)]
pub(crate) use self::combinations::PairCombinations;
pub(crate) use self::{accumulate::Accumulate, sorted_union::SortedUnion};

/// Shim for [Iterator::is_sorted_by_key](https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.is_sorted_by_key).
#[cfg(test)]
pub(crate) fn is_sorted_by_key<I, K, F>(iter: I, mut f: F) -> bool
where
    I: IntoIterator,
    F: FnMut(I::Item) -> K,
    K: Ord,
{
    let mut iter = iter.into_iter();
    let mut prev = if let Some(item) = iter.next() {
        f(item)
    } else {
        return true;
    };

    for item in iter {
        let item = f(item);

        if item >= prev {
            prev = item;
        } else {
            return false;
        }
    }

    true
}

/// Owned version of `HashSet::intersection`
pub(crate) struct IntoIntersection<T> {
    iter: hash_set::IntoIter<T>,
    other: HashSet<T>,
}

impl<T> IntoIntersection<T> {
    pub fn new(a: HashSet<T>, b: HashSet<T>) -> Self {
        Self {
            iter: a.into_iter(),
            other: b,
        }
    }
}

impl<T> Iterator for IntoIntersection<T>
where
    T: Eq + Hash,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let e = self.iter.next()?;
            if self.other.contains(&e) {
                return Some(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sorted_by_key_positive() {
        let input = [1, 2, 3, 4];
        assert!(is_sorted_by_key(input, |n| n));
    }

    #[test]
    fn is_sorted_by_key_negative() {
        let input = [1, 3, 2, 4];
        assert!(!is_sorted_by_key(input, |n| n));
    }
}
