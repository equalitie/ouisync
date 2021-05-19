use std::future::Future;
use tokio::task::{self, JoinHandle};

/// Set of tasks which are all automatically aborted when the set goes out of scope.
#[derive(Default)]
pub struct ScopedTaskSet {
    handles: Vec<JoinHandle<()>>,
}

impl ScopedTaskSet {
    /// Spawns a new task on this set. The task gets aborted when this set is dropped.
    pub fn spawn<T>(&mut self, task: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        self.handles.push(task::spawn(task))
    }
}

impl Drop for ScopedTaskSet {
    fn drop(&mut self) {
        for handle in &mut self.handles {
            handle.abort();
        }
    }
}
