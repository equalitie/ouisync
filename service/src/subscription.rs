use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{Stream, StreamExt};
use ouisync::Event;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::protocol::Response;

pub(crate) enum SubscriptionStream {
    Repository(BroadcastStream<Event>),
}

impl SubscriptionStream {
    pub fn repository(rx: broadcast::Receiver<Event>) -> Self {
        Self::Repository(BroadcastStream::new(rx))
    }
}

impl Stream for SubscriptionStream {
    type Item = Response;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Repository(stream) => {
                Poll::Ready(ready!(stream.poll_next_unpin(cx)).map(|_| Response::RepositoryEvent))
            }
        }
    }
}
