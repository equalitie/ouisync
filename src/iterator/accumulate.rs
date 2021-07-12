use std::iter::Peekable;

pub struct Accumulate<Iter, Key, GetKey>
where
    Iter: Iterator,
    GetKey: FnMut(Iter::Item) -> Key,
    Key: Copy,
{
    iter: Peekable<Iter>,
    get_key: GetKey,
}

impl<Iter, Key, GetKey> Accumulate<Iter, Key, GetKey>
where
    Iter: Iterator,
    GetKey: FnMut(Iter::Item) -> Key,
    Key: Copy,
{
    pub fn new(iter: Iter, get_key: GetKey) -> Self {
        Self {
            iter: iter.peekable(),
            get_key,
        }
    }
}

impl<Iter: Iterator, Key, GetKey> Iterator for Accumulate<Iter, Key, GetKey>
where
    GetKey: FnMut(Iter::Item) -> Key,
    Key: PartialEq + Copy,
    Iter::Item: Copy,
{
    type Item = (Key, Vec<Iter::Item>);

    fn next(&mut self) -> Option<Self::Item> {
        // Note: This could be implemented without allocating the Vec buffer, but then we would
        // need self.iter to be Copy-able, and unfortunately, what we get from
        // iterator::union::new_from_many is not Copy-able.
        let first = self.iter.next();

        if let Some(first) = first {
            let get_key = &mut self.get_key;
            let key = (get_key)(first);
            let mut buf = vec![first];

            while let Some(v) = self.iter.next_if(|&e| (get_key)(e) == key) {
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
        let a = Accumulate::new(v.iter(), |i| i);
        assert_eq!(
            a.map(|(k, t)| (k, t)).collect::<Vec<_>>(),
            [
                (&1, vec![&1]),
                (&2, vec![&2, &2]),
                (&3, vec![&3]),
                (&4, vec![&4, &4, &4])
            ]
        );
    }
}
