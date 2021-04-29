/// Generator for unique handles (e.g. inodes, file handles/descriptors, ...)
#[derive(Default)]
pub struct HandleGenerator {
    next: u64,
}

impl HandleGenerator {
    pub fn next<F>(&mut self, filter: F) -> u64
    where
        F: Fn(u64) -> bool,
    {
        let handle = (self.next..=u64::MAX)
            .chain(0..self.next)
            .find(|candidate| filter(*candidate))
            .expect("all handles taken");
        self.next = handle.wrapping_add(1);
        handle
    }
}
