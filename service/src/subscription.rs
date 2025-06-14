use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{Stream, StreamExt};
use ouisync::{Event, NetworkEventReceiver, NetworkEventStream};
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::protocol::Response;

pub(crate) enum SubscriptionStream {
    Network(NetworkEventStream),
    Repository(BroadcastStream<Event>),
    StateMonitor(WatchStream<()>),
}

impl From<broadcast::Receiver<Event>> for SubscriptionStream {
    fn from(rx: broadcast::Receiver<Event>) -> Self {
        Self::Repository(BroadcastStream::new(rx))
    }
}

impl From<NetworkEventReceiver> for SubscriptionStream {
    fn from(rx: NetworkEventReceiver) -> Self {
        Self::Network(NetworkEventStream::new(rx))
    }
}

impl From<watch::Receiver<()>> for SubscriptionStream {
    fn from(rx: watch::Receiver<()>) -> Self {
        Self::StateMonitor(WatchStream::new(rx))
    }
}

impl Stream for SubscriptionStream {
    type Item = Response;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Network(stream) => {
                Poll::Ready(ready!(stream.poll_next_unpin(cx)).map(Response::NetworkEvent))
            }
            Self::Repository(stream) => {
                Poll::Ready(ready!(stream.poll_next_unpin(cx)).map(|_| Response::RepositoryEvent))
            }
            Self::StateMonitor(stream) => {
                Poll::Ready(ready!(stream.poll_next_unpin(cx)).map(|_| Response::StateMonitorEvent))
            }
        }
    }
}
