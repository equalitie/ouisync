use std::iter::Take;

pub struct Accumulate<Iter, GetKey> {
    iter: Iter,
    get_key: GetKey,
}

impl<Iter, GetKey> Accumulate<Iter, GetKey> {
    pub fn new(iter: Iter, get_key: GetKey) -> Self {
        Self { iter, get_key }
    }
}

impl<Iter: Iterator + Clone, Key, GetKey> Iterator for Accumulate<Iter, GetKey>
where
    GetKey: FnMut(Iter::Item) -> Key,
    Key: PartialEq + Copy,
{
    type Item = (Key, Take<Iter>);

    fn next(&mut self) -> Option<Self::Item> {
        let mut i = self.iter.clone();
        let mut n: usize = 0;

        let first_key = i.next().map(|v| (self.get_key)(v));

        if let Some(first_key) = first_key {
            let mut j = i.clone();
            loop {
                n += 1;
                if i.next().map(|v| (self.get_key)(v)) != Some(first_key) {
                    break;
                }
                j = i.clone();
            }
            let r = self.iter.clone().take(n);
            self.iter = j;
            Some((first_key, r))
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
            a.map(|(k, t)| (k, t.collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
            [
                (&1, vec![&1]),
                (&2, vec![&2, &2]),
                (&3, vec![&3]),
                (&4, vec![&4, &4, &4])
            ]
        );
    }
}
