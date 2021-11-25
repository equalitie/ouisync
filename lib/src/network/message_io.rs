//! Utilities for sending and receiving messages across the network.

use super::{
    connection::ConnectionPermitHalf,
    message::{Content, Message},
    object_stream::{ObjectRead, ObjectWrite},
};
use crate::repository::PublicRepositoryId;
use futures_util::{stream::SelectAll, SinkExt, Stream, StreamExt};
use std::{
    collections::{HashMap, VecDeque},
    io,
    pin::Pin,
    sync::{Arc, Mutex as BlockingMutex},
    task::{Context, Poll},
};
use tokio::{
    net::tcp,
    select,
    sync::{watch, Mutex as AsyncMutex, Notify},
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
/// NOTE: Doesn't actually implement the `Sink` trait currently because we don't need it, only
/// provides a simple async `send` method.
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

/// Reads messages from the underlying TCP streams and dispatches them to individual streams based
/// on their ids.
pub(super) struct MessageDispatcher {
    shared: Arc<Shared>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        let (queues_changed_tx, _) = watch::channel(());

        Self {
            shared: Arc::new(Shared {
                reader: MultiReader::new(),
                queues: BlockingMutex::new(Queues::default()),
                queues_changed_tx,
            }),
        }
    }

    pub fn add_message_stream(&self, stream: MessageStream) {
        self.shared.reader.add(stream);
    }

    /// Creates a stream that will yield messages with the given id.
    pub fn subscribe(&self, id: PublicRepositoryId) -> ContentStream {
        self.shared.claim(id);
        ContentStream::new(id, self.shared.clone())
    }

    /// Creates a stream that will yield `ContentStream`s that haven't been subscribed to yet.
    pub fn incoming(&self) -> IncomingContentStreams {
        IncomingContentStreams {
            shared: self.shared.clone(),
            queues_changed_rx: self.shared.queues_changed_tx.subscribe(),
        }
    }
}

pub(super) struct ContentStream {
    id: PublicRepositoryId,
    shared: Arc<Shared>,
    queues_changed_rx: watch::Receiver<()>,
}

impl ContentStream {
    fn new(id: PublicRepositoryId, shared: Arc<Shared>) -> Self {
        let queues_changed_rx = shared.queues_changed_tx.subscribe();

        Self {
            id,
            shared,
            queues_changed_rx,
        }
    }

    /// Receive the next message content.
    pub async fn recv(&mut self) -> Option<Content> {
        let mut closed = false;

        loop {
            if let Some(content) = self.shared.pop(&self.id) {
                return Some(content);
            }

            if closed {
                return None;
            }

            select! {
                Some(message) = self.shared.reader.recv() => {
                    if message.id == self.id {
                        return Some(message.content);
                    } else {
                        self.shared.push(message)
                    }
                }
                _ = self.queues_changed_rx.changed() => (),
                else => {
                    // If the reader closed we still want to check the queues in case there are
                    // messages for us before giving up.
                    closed = true;
                }
            }
        }
    }
}

impl Drop for ContentStream {
    fn drop(&mut self) {
        self.shared.abandon(&self.id)
    }
}

pub(super) struct IncomingContentStreams {
    shared: Arc<Shared>,
    queues_changed_rx: watch::Receiver<()>,
}

impl IncomingContentStreams {
    pub async fn recv(&mut self) -> Option<ContentStream> {
        let mut closed = false;

        loop {
            if let Some(id) = self.shared.claim_any() {
                return Some(ContentStream::new(id, self.shared.clone()));
            }

            if closed {
                return None;
            }

            select! {
                Some(message) = self.shared.reader.recv() => {
                    self.shared.push(message)
                }
                _ = self.queues_changed_rx.changed() => (),
                else => {
                    closed = true;
                }
            }
        }
    }
}

struct Shared {
    reader: MultiReader,
    queues: BlockingMutex<Queues>,
    queues_changed_tx: watch::Sender<()>,
}

impl Shared {
    // Pops a message content from the given claimed queue.
    fn pop(&self, id: &PublicRepositoryId) -> Option<Content> {
        self.queues.lock().unwrap().claimed.get_mut(id)?.pop_back()
    }

    // Pushes the message into the corresponding claimed queue if it exsits, otherwise puts it into
    // backlog. Emits notification in either case.
    fn push(&self, message: Message) {
        let mut queues = self.queues.lock().unwrap();
        let queue = if let Some(queue) = queues.claimed.get_mut(&message.id) {
            queue
        } else {
            queues.backlog.entry(message.id).or_default()
        };

        queue.push_front(message.content);
        self.queues_changed_tx.send(()).unwrap_or(());
    }

    // Transfers the given queue from backlog into claimed. If it didn't exist in backlog, creates
    // an empty one in claimed.
    fn claim(&self, id: PublicRepositoryId) {
        let mut queues = self.queues.lock().unwrap();

        if let Some(queue) = queues.backlog.remove(&id) {
            queues.claimed.insert(id, queue);
        } else {
            queues.claimed.insert(id, VecDeque::new());
        }
    }

    // Transfers one queue from backlog into claimed and returns its id, if it exists.
    fn claim_any(&self) -> Option<PublicRepositoryId> {
        let mut queues = self.queues.lock().unwrap();

        let id = *queues.backlog.keys().next()?;
        // unwrap is ok because `id` is an existing key we just retrieved.
        let queue = queues.backlog.remove(&id).unwrap();
        queues.claimed.insert(id, queue);

        Some(id)
    }

    // Removes the given queue from claimed and if it is not empty, puts it back into backlog and
    // emits notification.
    fn abandon(&self, id: &PublicRepositoryId) {
        let mut queues = self.queues.lock().unwrap();

        if let Some((id, queue)) = queues.claimed.remove_entry(id) {
            if !queue.is_empty() {
                queues.backlog.insert(id, queue);
                self.queues_changed_tx.send(()).unwrap_or(());
            }
        }
    }
}

#[derive(Default)]
struct Queues {
    claimed: HashMap<PublicRepositoryId, VecDeque<Content>>,
    backlog: HashMap<PublicRepositoryId, VecDeque<Content>>,
}

#[cfg(test)]
mod tests {
    use super::{super::message::Request, *};
    use crate::block::BlockId;
    use assert_matches::assert_matches;
    use std::net::Ipv4Addr;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn message_dispatcher_recv_on_subscribed() {
        let (mut client, server) = setup().await;

        let repo_id = PublicRepositoryId::random();
        let send_block_id: BlockId = rand::random();

        client
            .send(&Message {
                id: repo_id,
                content: Content::Request(Request::Block(send_block_id)),
            })
            .await
            .unwrap();

        let mut server_stream = server.subscribe(repo_id);

        let content = server_stream.recv().await.unwrap();
        assert_matches!(content, Content::Request(Request::Block(recv_block_id)) => {
            assert_eq!(recv_block_id, send_block_id) }
        );
    }

    #[tokio::test]
    async fn message_dispatcher_recv_on_incoming() {
        let (mut client, server) = setup().await;

        let repo_id = PublicRepositoryId::random();
        let send_block_id: BlockId = rand::random();

        client
            .send(&Message {
                id: repo_id,
                content: Content::Request(Request::Block(send_block_id)),
            })
            .await
            .unwrap();

        let mut server_incoming = server.incoming();
        let mut server_stream = server_incoming.recv().await.unwrap();

        let content = server_stream.recv().await.unwrap();
        assert_matches!(content, Content::Request(Request::Block(recv_block_id)) => {
            assert_eq!(recv_block_id, send_block_id) }
        );
    }

    #[tokio::test]
    async fn message_dispatcher_recv_on_two_subscribed() {
        let (mut client, server) = setup().await;

        let repo_id0 = PublicRepositoryId::random();
        let repo_id1 = PublicRepositoryId::random();

        let send_block_id0: BlockId = rand::random();
        let send_block_id1: BlockId = rand::random();

        for (repo_id, block_id) in [(repo_id0, send_block_id0), (repo_id1, send_block_id1)] {
            client
                .send(&Message {
                    id: repo_id,
                    content: Content::Request(Request::Block(block_id)),
                })
                .await
                .unwrap();
        }

        let server_stream0 = server.subscribe(repo_id0);
        let server_stream1 = server.subscribe(repo_id1);

        for (mut server_stream, send_block_id) in [
            (server_stream0, send_block_id0),
            (server_stream1, send_block_id1),
        ] {
            let content = server_stream.recv().await.unwrap();
            assert_matches!(content, Content::Request(Request::Block(recv_block_id)) => {
                assert_eq!(recv_block_id, send_block_id)
            });
        }
    }

    #[tokio::test]
    async fn message_dispatcher_drop_stream() {
        let (mut client, server) = setup().await;

        let repo_id = PublicRepositoryId::random();

        let send_block_id0: BlockId = rand::random();
        let send_block_id1: BlockId = rand::random();

        for block_id in [send_block_id0, send_block_id1] {
            client
                .send(&Message {
                    id: repo_id,
                    content: Content::Request(Request::Block(block_id)),
                })
                .await
                .unwrap();
        }

        let mut server_stream = server.subscribe(repo_id);
        let mut server_incoming = server.incoming();

        let content = server_stream.recv().await.unwrap();
        assert_matches!(content, Content::Request(Request::Block(recv_block_id)) => {
            assert_eq!(recv_block_id, send_block_id0)
        });

        drop(server_stream);

        let mut server_stream = server_incoming.recv().await.unwrap();
        let content = server_stream.recv().await.unwrap();
        assert_matches!(content, Content::Request(Request::Block(recv_block_id)) => {
            assert_eq!(recv_block_id, send_block_id1)
        });
    }

    async fn setup() -> (ObjectWrite<Message, TcpStream>, MessageDispatcher) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (server_reader, _) = server.into_split();

        let client_writer = ObjectWrite::new(client);

        let server_dispatcher = MessageDispatcher::new();
        server_dispatcher.add_message_stream(MessageStream::new(
            server_reader,
            ConnectionPermitHalf::dummy(),
        ));

        (client_writer, server_dispatcher)
    }
}
