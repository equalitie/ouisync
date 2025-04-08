use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::task::{self, AbortHandle, JoinError, JoinHandle};

/// Wrapper for a `JoinHandle` that auto-aborts on drop.
pub struct ScopedJoinHandle<T>(pub JoinHandle<T>);

impl<T> ScopedJoinHandle<T> {
    /// Explicitly abort the task before dropping this handle.
    pub fn abort(&self) {
        self.0.abort()
    }
}

impl<T> Drop for ScopedJoinHandle<T> {
    fn drop(&mut self) {
        self.0.abort()
    }
}

impl<T> Future for ScopedJoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

pub fn spawn<T>(task: T) -> ScopedJoinHandle<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    ScopedJoinHandle(task::spawn(task))
}

/// Wrapper for `AbortHandle` that auto-aborts on drop.
pub struct ScopedAbortHandle(pub AbortHandle);

impl From<AbortHandle> for ScopedAbortHandle {
    fn from(handle: AbortHandle) -> Self {
        Self(handle)
    }
}

impl Drop for ScopedAbortHandle {
    fn drop(&mut self) {
        self.0.abort()
    }
}
