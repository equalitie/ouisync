/// Iterator adapter that yields all unordered pairs of elements from the input iterator without
/// repetition.
///
/// Example: [1, 2, 3] -> [(1, 2), (1, 3), (2, 3)]
pub(crate) struct PairCombinations<I>
where
    I: Iterator,
{
    a_item: Option<I::Item>,
    a_iter: I,
    b_iter: I,
}

impl<I> PairCombinations<I>
where
    I: Iterator + Clone,
{
    #[cfg(test)]
    pub fn new<J>(iter: J) -> Self
    where
        J: IntoIterator<IntoIter = I>,
    {
        let mut a_iter = iter.into_iter();
        let a_item = a_iter.next();
        let b_iter = a_iter.clone();

        Self {
            a_item,
            a_iter,
            b_iter,
        }
    }
}

impl<I> Iterator for PairCombinations<I>
where
    I: Iterator + Clone,
    I::Item: Clone,
{
    type Item = (I::Item, I::Item);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(b) = self.b_iter.next() {
            let a = self.a_item.clone()?;
            Some((a, b))
        } else {
            self.a_item = self.a_iter.next();
            self.b_iter = self.a_iter.clone();
            let a = self.a_item.clone()?;
            let b = self.b_iter.next()?;
            Some((a, b))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_combinations() {
        let mut i = PairCombinations::new([1, 2, 3, 4]);
        assert_eq!(i.next(), Some((1, 2)));
        assert_eq!(i.next(), Some((1, 3)));
        assert_eq!(i.next(), Some((1, 4)));
        assert_eq!(i.next(), Some((2, 3)));
        assert_eq!(i.next(), Some((2, 4)));
        assert_eq!(i.next(), Some((3, 4)));
        assert_eq!(i.next(), None);
    }
}
