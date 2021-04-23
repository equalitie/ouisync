use futures::future::Future;
use std::{
    ops::Deref,
    sync::{Arc, RwLock},
};
use tokio::task::{spawn, JoinHandle};

pub struct AsyncObject<T: AsyncObjectTrait> {
    state: Arc<T>,
}

impl<T: AsyncObjectTrait> AsyncObject<T> {
    pub fn new(state: Arc<T>) -> AsyncObject<T> {
        AsyncObject { state }
    }

    pub fn arc(&self) -> Arc<T> {
        self.state.clone()
    }
}

pub trait AsyncObjectTrait {
    fn abort_handles(&self) -> &AbortHandles;

    fn abortable_spawn<Task>(&self, task: Task)
    where
        Task: Future<Output = ()> + Send + 'static,
        Task::Output: Send + 'static,
    {
        let handle = spawn(task);

        self.abort_handles()
            .abort_handles
            .write()
            .unwrap()
            .push(handle);
    }

    fn abort(&self) {
        let handles = self.abort_handles().abort_handles.write().unwrap();

        for h in &*handles {
            h.abort();
        }
    }
}

impl<T: AsyncObjectTrait> Deref for AsyncObject<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl<T: AsyncObjectTrait> Drop for AsyncObject<T> {
    fn drop(&mut self) {
        self.state.abort();
    }
}

pub struct AbortHandles {
    abort_handles: RwLock<Vec<JoinHandle<()>>>,
}

impl AbortHandles {
    pub fn new() -> AbortHandles {
        AbortHandles {
            abort_handles: RwLock::new(Vec::new()),
        }
    }
}
