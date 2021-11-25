//! Utilities for sending and receiving messages across the network.

use super::{
    connection::ConnectionPermitHalf,
    message::Message,
    object_stream::{ObjectRead, ObjectWrite},
};
use futures_util::{stream::SelectAll, SinkExt, Stream, StreamExt};
use std::{
    io,
    pin::Pin,
    sync::Mutex as BlockingMutex,
    task::{Context, Poll},
};
use tokio::{
    net::tcp,
    select,
    sync::{Mutex as AsyncMutex, Notify},
};

/// Stream of `Message` backed by a `TcpStream`. Closes on first error.
pub(super) struct MessageStream {
    inner: ObjectRead<Message, tcp::OwnedReadHalf>,
    _permit: ConnectionPermitHalf,
}

impl MessageStream {
    pub fn new(stream: tcp::OwnedReadHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: ObjectRead::new(stream),
            _permit: permit,
        }
    }
}

impl Stream for MessageStream {
    type Item = Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(message))) => Poll::Ready(Some(message)),
            Poll::Ready(Some(Err(_)) | None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Sink for `Message` backend by a `TcpStream`.
///
/// NOTE: We don't actually implement the `Sink` trait here because it's quite boilerplate-y and we
/// don't need it. There is just a simple async `send` method instead.
pub(super) struct MessageSink {
    inner: ObjectWrite<Message, tcp::OwnedWriteHalf>,
    _permit: ConnectionPermitHalf,
}

impl MessageSink {
    pub fn new(stream: tcp::OwnedWriteHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: ObjectWrite::new(stream),
            _permit: permit,
        }
    }

    pub async fn send(&mut self, message: &Message) -> io::Result<()> {
        self.inner.send(message).await
    }
}

/// Stream that reads `Message`s from multiple underlying TCP streams.
pub(super) struct MultiReader {
    // Streams that are being read from. Protected by an async mutex so we don't require &mut
    // access to read them.
    active: AsyncMutex<SelectAll<MessageStream>>,
    // Streams recently added but not yet being read from. Protected by a regular blocking mutex
    // because we only ever lock it for very short time and never across await points.
    new: BlockingMutex<Vec<MessageStream>>,
    // This notify gets signaled when we add new stream(s) to the `new` collection. It interrupts
    // the currently ongoing `recv` (if any) which will then transfer the streams from `new` to
    // `active` and resume the read.
    new_notify: Notify,
}

impl MultiReader {
    pub fn new() -> Self {
        Self {
            active: AsyncMutex::new(SelectAll::new()),
            new: BlockingMutex::new(Vec::new()),
            new_notify: Notify::new(),
        }
    }

    pub fn add(&self, stream: MessageStream) {
        self.new.lock().unwrap().push(stream);
        self.new_notify.notify_one();
    }

    pub async fn recv(&self) -> Option<Message> {
        let mut active = self.active.lock().await;

        loop {
            active.extend(self.new.lock().unwrap().drain(..));

            select! {
                item = active.next() => return item,
                _ = self.new_notify.notified() => {}
            }
        }
    }
}

/// Sink that writes to the first available underlying TCP stream.
///
/// NOTE: Doesn't actually implement the `Sink` trait currently because we don't need it.
pub(super) struct MultiWriter {
    // The sink currently used to write messages.
    active: AsyncMutex<Option<MessageSink>>,
    // Other sinks to replace the active sinks in case it fails.
    backup: BlockingMutex<Vec<MessageSink>>,
}

impl MultiWriter {
    pub fn new() -> Self {
        Self {
            active: AsyncMutex::new(None),
            backup: BlockingMutex::new(Vec::new()),
        }
    }

    pub fn add(&self, sink: MessageSink) {
        self.backup.lock().unwrap().push(sink)
    }

    /// Returns whether the send succeeded.
    pub async fn send(&self, message: &Message) -> bool {
        let mut active = self.active.lock().await;

        loop {
            let sink = if let Some(sink) = &mut *active {
                sink
            } else if let Some(sink) = self.backup.lock().unwrap().pop() {
                active.insert(sink)
            } else {
                return false;
            };

            if sink.send(message).await.is_ok() {
                return true;
            }

            *active = None;
        }
    }
}
