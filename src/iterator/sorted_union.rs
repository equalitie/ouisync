use std::iter::Peekable;

/// Iterator adaptor that flattens an iterator of iterators by merging then in ascending order of
/// the keys returned from the given closure.
/// If the input iterators are sorted, the resulting iterator is sorted as well.
pub struct SortedUnion<I, GetKey>
where
    I: Iterator,
{
    iters: Vec<Peekable<I>>,
    get_key: GetKey,
}

impl<I, Key, GetKey> SortedUnion<I, GetKey>
where
    I: Iterator,
    GetKey: FnMut(&I::Item) -> Key,
{
    pub fn new<J>(iters: J, get_key: GetKey) -> Self
    where
        J: IntoIterator,
        J::Item: IntoIterator<IntoIter = I>,
    {
        Self {
            iters: iters
                .into_iter()
                .map(|i| i.into_iter().peekable())
                .collect(),
            get_key,
        }
    }
}

impl<I, Key, GetKey> Iterator for SortedUnion<I, GetKey>
where
    I: Iterator,
    GetKey: FnMut(&I::Item) -> Key,
    Key: Ord,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        let get_key = &mut self.get_key;
        let index = self
            .iters
            .iter_mut()
            .enumerate()
            .filter_map(|(index, iter)| iter.peek().map(|e| (index, e)))
            .min_by_key(|(_, e)| (get_key)(e))
            .map(|(index, _)| index)?;
        self.iters[index].next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{array::IntoIter, iter};

    #[test]
    fn test_sorted_union_zero() {
        let u = SortedUnion::new(iter::empty::<iter::Empty<i32>>(), |x| *x);
        assert_eq!(u.collect::<Vec<_>>(), []);
    }

    #[test]
    fn test_sorted_union_one() {
        let l = &[(1, "l0"), (3, "l1"), (4, "l2"), (4, "l3")];
        let u = SortedUnion::new(iter::once(l), |(num, _)| num);
        assert_eq!(
            u.collect::<Vec<_>>(),
            [&(1, "l0"), &(3, "l1"), &(4, "l2"), &(4, "l3")]
        );
    }

    #[test]
    fn test_sorted_union_two() {
        let l = &[(1, "l0"), (3, "l1"), (4, "l2"), (4, "l3")];
        let r = &[(1, "r0"), (2, "r1"), (2, "r2"), (3, "r3")];
        let u = SortedUnion::new(IntoIter::new([l, r]), |(num, _)| num);
        assert_eq!(
            u.collect::<Vec<_>>(),
            [
                &(1, "l0"),
                &(1, "r0"),
                &(2, "r1"),
                &(2, "r2"),
                &(3, "l1"),
                &(3, "r3"),
                &(4, "l2"),
                &(4, "l3")
            ]
        );
    }

    #[test]
    fn test_sorted_union_three() {
        let v0 = &[(1, "00"), (3, "01"), (4, "02")];
        let v1 = &[(1, "10"), (2, "11"), (2, "12")];
        let v2 = &[(2, "20"), (2, "21"), (5, "22")];
        let u = SortedUnion::new(IntoIter::new([v0, v1, v2]), |(num, _)| num);
        assert_eq!(
            u.collect::<Vec<_>>(),
            [
                &(1, "00"),
                &(1, "10"),
                &(2, "11"),
                &(2, "12"),
                &(2, "20"),
                &(2, "21"),
                &(3, "01"),
                &(4, "02"),
                &(5, "22")
            ]
        );
    }
}
