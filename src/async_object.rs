use futures::future::{abortable, Future, AbortHandle};
use std::sync::{Arc, RwLock};
use tokio::task::spawn;

pub struct AsyncObject<T: AsyncObjectTrait> {
    state: Arc<T>
}

impl<T: AsyncObjectTrait> AsyncObject<T> {
    pub fn new(state: Arc<T>) -> AsyncObject<T> {
        AsyncObject { state }
    }
}

pub trait AsyncObjectTrait {
    fn abort_handles(&self) -> &AbortHandles;

    fn foo(&self)
    {}

    fn abortable_spawn<Task>(&self, task: Task)
        where
            Task: Future + Send + 'static,
            Task::Output: Send + 'static,
    {
        let (future, abort_handle) = abortable(task);
        self.abort_handles().abort_handles.write().unwrap().push(abort_handle);
        spawn(future);
    }
}

impl<T: AsyncObjectTrait> Drop for AsyncObject<T> {
    fn drop(&mut self) {
        let handles = self.state.abort_handles().abort_handles.write().unwrap();

        for h in &*handles {
            h.abort();
        }
    }
}

pub struct AbortHandles {
    abort_handles: RwLock<Vec<AbortHandle>>
}

impl AbortHandles {
    pub fn new() -> AbortHandles {
        AbortHandles { abort_handles: RwLock::new(Vec::new()) }
    }
}
