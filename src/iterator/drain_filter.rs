/// Poor-map's replacement for
/// [`Vec::drain_filter`](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.drain_filter)
/// until it gets stabilized.
pub struct DrainFilter<'a, T, P> {
    vec: &'a mut Vec<T>,
    pred: P,
    index: usize,
}

impl<'a, T, P> DrainFilter<'a, T, P>
where
    P: FnMut(&mut T) -> bool,
{
    pub fn new(vec: &'a mut Vec<T>, pred: P) -> Self {
        Self {
            vec,
            pred,
            index: 0,
        }
    }
}

impl<'a, T, P> Iterator for DrainFilter<'a, T, P>
where
    P: FnMut(&mut T) -> bool,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        while self.index < self.vec.len() {
            if (self.pred)(&mut self.vec[self.index]) {
                return Some(self.vec.remove(self.index));
            } else {
                self.index += 1;
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn some() {
        let mut vec = vec![0, 1, 2, 3];
        assert_eq!(
            DrainFilter::new(&mut vec, |n| *n % 2 == 0).collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(vec, vec![1, 3]);
    }

    #[test]
    fn all() {
        let mut vec = vec![0, 1, 2, 3];
        assert_eq!(
            DrainFilter::new(&mut vec, |_| true).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(vec, vec![]);
    }

    #[test]
    fn none() {
        let mut vec = vec![0, 1, 2, 3];
        assert_eq!(
            DrainFilter::new(&mut vec, |_| false).collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(vec, vec![0, 1, 2, 3]);
    }

    #[test]
    fn partialy_consumed() {
        let mut vec = vec![0, 1, 2, 3, 4, 5];
        let mut iter = DrainFilter::new(&mut vec, |n| *n % 2 == 0);

        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next(), Some(2));

        assert_eq!(vec, vec![1, 3, 4, 5]);
    }
}
