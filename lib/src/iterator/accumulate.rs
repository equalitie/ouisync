use std::iter::Peekable;

/// Iterator adaptor that groups elements from the input iterator such that all consecutive elements
/// for which the given closure returns the same key are put into the same group.
pub struct Accumulate<Iter, GetKey>
where
    Iter: Iterator,
{
    iter: Peekable<Iter>,
    get_key: GetKey,
}

impl<Iter, Key, GetKey> Accumulate<Iter, GetKey>
where
    Iter: Iterator,
    GetKey: FnMut(&Iter::Item) -> Key,
{
    pub fn new(iter: Iter, get_key: GetKey) -> Self {
        Self {
            iter: iter.peekable(),
            get_key,
        }
    }
}

impl<Iter: Iterator, Key, GetKey> Iterator for Accumulate<Iter, GetKey>
where
    GetKey: FnMut(&Iter::Item) -> Key,
    Key: PartialEq,
{
    type Item = (Key, Vec<Iter::Item>);

    fn next(&mut self) -> Option<Self::Item> {
        // Note: This could be implemented without allocating the Vec buffer, but then we would
        // need self.iter to be Copy-able, and unfortunately, what we get from
        // iterator::union::new_from_many is not Copy-able.
        let first = self.iter.next();

        if let Some(first) = first {
            let get_key = &mut self.get_key;
            let key = (get_key)(&first);
            let mut buf = vec![first];

            while let Some(v) = self.iter.next_if(|e| (get_key)(e) == key) {
                buf.push(v);
            }

            Some((key, buf))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulate() {
        let v = &[1, 2, 2, 3, 4, 4, 4];
        let a = Accumulate::new(v.iter(), |&i| i);
        assert_eq!(
            a.collect::<Vec<_>>(),
            [
                (&1, vec![&1]),
                (&2, vec![&2, &2]),
                (&3, vec![&3]),
                (&4, vec![&4, &4, &4])
            ]
        );
    }
}
