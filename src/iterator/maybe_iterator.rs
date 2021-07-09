pub enum MaybeIterator<I>
where
    I: Iterator,
{
    NoIterator,
    SomeIterator(I),
}

impl<I> Iterator for MaybeIterator<I>
where
    I: Iterator,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::NoIterator => None,
            Self::SomeIterator(i) => i.next(),
        }
    }
}
