use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, ready},
};

use pin_project_lite::pin_project;
use tokio::sync::{Notify, futures::Notified};

/// Atomic counter that allows asynchronous waiting until it becomes non-zero.
#[derive(Default)]
pub(crate) struct AsyncCounter {
    value: AtomicUsize,
    notify: Notify,
}

impl AsyncCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments the counter
    pub fn increment(&self) {
        self.value.fetch_add(1, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Decrements the counter
    pub fn decrement(&self) {
        self.value.fetch_sub(1, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Increments the counter and returns a RAII guard that decrements it back on drop.
    pub fn scoped_increment(&self) -> AsyncCounterGuard<'_> {
        self.increment();
        AsyncCounterGuard { counter: self }
    }

    /// Returns a future that resolves when the counter becomes non-zero.
    pub fn wait_nonzero(&self) -> AsyncCounterFuture<'_> {
        let notified = self.notify.notified();

        AsyncCounterFuture {
            counter: self,
            notified,
        }
    }
}

pub(crate) struct AsyncCounterGuard<'a> {
    counter: &'a AsyncCounter,
}

impl Drop for AsyncCounterGuard<'_> {
    fn drop(&mut self) {
        self.counter.decrement();
    }
}

pin_project! {
    /// Future for [`AsyncCounter::wait_nonzero`]
    pub(crate) struct AsyncCounterFuture<'a> {
        counter: &'a AsyncCounter,
        #[pin]
        notified: Notified<'a>,
    }
}

impl Future for AsyncCounterFuture<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            if this.counter.value.load(Ordering::Acquire) > 0 {
                break Poll::Ready(());
            } else {
                ready!(this.notified.as_mut().poll(cx));
                this.notified.set(this.counter.notify.notified());
            }
        }
    }
}

// TODO: Consider using https://github.com/tokio-rs/loom to test this
