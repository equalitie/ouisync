use std::iter::Peekable;

//--------------------------------------------------------------------
// Union
//--------------------------------------------------------------------
pub struct Union<L, R, GetKey>
where
    L: Iterator,
    R: Iterator,
{
    lhs: Peekable<L>,
    rhs: Peekable<R>,
    get_key: GetKey,
}

impl<L, R, GetKey> Union<L, R, GetKey>
where
    L: Iterator,
    R: Iterator,
{
    pub fn new(lhs: L, rhs: R, get_key: GetKey) -> Self {
        Self {
            lhs: lhs.peekable(),
            rhs: rhs.peekable(),
            get_key,
        }
    }
}

impl<L, R, T, Key, GetKey> Iterator for Union<L, R, GetKey>
where
    T: Copy,
    L: Iterator<Item = T>,
    R: Iterator<Item = T>,
    GetKey: FnMut(T) -> Key,
    Key: Ord + Copy,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        use core::cmp::Ordering;

        match (self.lhs.peek(), self.rhs.peek()) {
            (Some(ref l), Some(ref r)) => match (self.get_key)(**l).cmp(&(self.get_key)(**r)) {
                Ordering::Less => self.lhs.next(),
                Ordering::Equal => self.lhs.next(),
                Ordering::Greater => self.rhs.next(),
            },
            (Some(_), None) => self.lhs.next(),
            (None, Some(_)) => self.rhs.next(),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_union() {
        let l = &[(1, "l0"), (3, "l1"), (4, "l2"), (4, "l3")];
        let r = &[(1, "r0"), (2, "r1"), (2, "r2"), (3, "r3")];
        let u = Union::new(l.iter(), r.iter(), |p: &(i32, &str)| p.0);
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
}
