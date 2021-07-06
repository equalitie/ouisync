use std::iter::Peekable;

//--------------------------------------------------------------------
// SortedUnion
//--------------------------------------------------------------------
pub struct SortedUnion<L, R, GetKey>
where
    L: Iterator,
    R: Iterator,
{
    lhs: Peekable<L>,
    rhs: Peekable<R>,
    get_key: GetKey,
}

impl<L, R, GetKey> SortedUnion<L, R, GetKey>
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

pub fn new_from_many<'a, Iter, T, Key, GetKey>(
    mut iter: Iter,
    get_key: GetKey,
) -> Box<dyn Iterator<Item = T> + 'a>
where
    Iter: Iterator,
    Iter::Item: Iterator<Item = T> + 'a,
    T: Copy + 'a,
    Key: Copy + Ord,
    GetKey: FnMut(T) -> Key + 'a + Copy,
{
    if let Some(first) = iter.next() {
        let mut u: Box<dyn Iterator<Item = T> + 'a> = Box::new(first);

        for n in iter {
            u = Box::new(SortedUnion::new(u, n, get_key));
        }

        u
    } else {
        Box::new(std::iter::empty())
    }
}

impl<L, R, T, Key, GetKey> Iterator for SortedUnion<L, R, GetKey>
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
    use core::array::IntoIter;

    #[test]
    fn test_union() {
        let l = &[(1, "l0"), (3, "l1"), (4, "l2"), (4, "l3")];
        let r = &[(1, "r0"), (2, "r1"), (2, "r2"), (3, "r3")];
        let u = SortedUnion::new(l.iter(), r.iter(), |p: &(i32, &str)| p.0);
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
    fn test_union_many() {
        let v0 = &[(1, "00"), (3, "01"), (4, "02")];
        let v1 = &[(1, "10"), (2, "11"), (2, "12")];
        let v2 = &[(2, "20"), (2, "21"), (5, "22")];
        let u = new_from_many(
            IntoIter::new([v0.iter(), v1.iter(), v2.iter()]),
            |p: &(i32, &str)| p.0,
        );
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
