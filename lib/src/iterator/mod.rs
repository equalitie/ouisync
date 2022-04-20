mod accumulate;
mod combinations;
mod sorted_union;

pub(crate) use self::{
    accumulate::Accumulate, combinations::PairCombinations, sorted_union::SortedUnion,
};

/// Shim for [Iterator::is_sorted_by_key](https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.is_sorted_by_key).
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
