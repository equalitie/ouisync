//! Future and Stream combinators// TODO: move this to some generic utils module.

use futures_util::TryStream;
use pin_project_lite::pin_project;
use std::{
    future::Future,
    iter,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// Additional combinators for `TryStream`.
pub(crate) trait TryStreamExt: TryStream {
    /// Attempts to collect all successful results of this stream into the specified collection.
    /// Similar to `try_collect` but uses existing collection.
    fn try_collect_into<D>(self, dst: &mut D) -> TryCollectInto<Self, D>
    where
        Self: Sized,
    {
        TryCollectInto { stream: self, dst }
    }

    /// Consumes the whole stream, discarding the items.
    #[cfg(test)] // currently used only in tests
    fn try_consume(self) -> TryConsume<Self>
    where
        Self: Sized,
    {
        TryConsume { stream: self }
    }
}

impl<S> TryStreamExt for S where S: TryStream + ?Sized {}

pin_project! {
    /// Future for the `try_collect_into` combinator.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub(crate) struct TryCollectInto<'a, S, D> {
        #[pin]
        stream: S,
        dst: &'a mut D
    }
}

impl<'a, S, D> Future for TryCollectInto<'a, S, D>
where
    S: TryStream,
    D: Extend<S::Ok>,
{
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        while let Some(item) = ready!(this.stream.as_mut().try_poll_next(cx)?) {
            this.dst.extend(iter::once(item));
        }

        Poll::Ready(Ok(()))
    }
}

pin_project! {
    /// Future for the `try_consume` combinator.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub(crate) struct TryConsume<S> {
        #[pin]
        stream: S,
    }
}

impl<S> Future for TryConsume<S>
where
    S: TryStream,
{
    type Output = Result<(), S::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        while ready!(this.stream.as_mut().try_poll_next(cx)?).is_some() {}

        Poll::Ready(Ok(()))
    }
}
