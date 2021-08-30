use slab::Slab;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::task::{self, JoinHandle};

/// Set of tasks which are all automatically aborted when the set goes out of scope.
#[derive(Default)]
pub struct ScopedTaskSet(ScopedTaskHandle);

impl ScopedTaskSet {
    /// Returns a handle for spawning tasks.
    pub fn handle(&self) -> &ScopedTaskHandle {
        &self.0
    }

    /// Spawns a new task on the set. Shortcut for `self.handle().spawn()`.
    pub fn spawn<T>(&self, task: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        self.0.spawn(task)
    }
}

impl Drop for ScopedTaskSet {
    fn drop(&mut self) {
        self.0.abort_all()
    }
}

/// Handle to the `ScopedTaskSet` used to spawn tasks on the set. The handle can be cheaply cloned
/// and send across threads. Dropping a handle *does not* abort the tasks in the set, only dropping
/// the set itself.
#[derive(Default, Clone)]
pub struct ScopedTaskHandle(Arc<Mutex<Slab<JoinHandle<()>>>>);

impl ScopedTaskHandle {
    /// Spawns a new task on the set.
    pub fn spawn<T>(&self, task: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        let mut handles = self.0.lock().unwrap();

        let entry = handles.vacant_entry();
        let key = entry.key();

        let task = task::spawn({
            let handles = self.0.clone();
            async move {
                task.await;
                // remove the handle from the set when the task terminates by itself.
                handles.lock().unwrap().remove(key);
            }
        });

        entry.insert(task);
    }

    /// Abort all tasks in the set.
    pub fn abort_all(&self) {
        let mut handles = self.0.lock().unwrap();
        for handle in handles.drain() {
            handle.abort();
        }
    }
}
