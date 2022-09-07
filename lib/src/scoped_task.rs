use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::task::{self, JoinError, JoinHandle};

/// Wrapper for a `JoinHandle` that auto-aborts on drop.
pub struct ScopedJoinHandle<T>(pub JoinHandle<T>);

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
