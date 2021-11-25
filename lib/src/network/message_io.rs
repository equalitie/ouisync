//! Utilities for sending and receiving messages across the network.

use super::{
    connection::{ConnectionPermit, ConnectionPermitHalf},
    message::{Content, Message},
    object_stream::{ObjectRead, ObjectWrite},
};
use crate::repository::PublicRepositoryId;
use futures_util::{stream::SelectAll, SinkExt, Stream, StreamExt};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex as BlockingMutex},
    task::{Context, Poll, Waker},
};
use tokio::{
    net::{tcp, TcpStream},
    select,
    sync::{watch, Mutex as AsyncMutex},
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

/// Stream that reads `Message`s from multiple underlying TCP streams concurrently.
pub(super) struct MessageMultiStream {
    inner: BlockingMutex<Inner>,
}

impl MessageMultiStream {
    pub fn new() -> Self {
        Self {
            inner: BlockingMutex::new(Inner {
                streams: SelectAll::new(),
                waker: None,
            }),
        }
    }

    pub fn add(&self, stream: MessageStream) {
        let mut inner = self.inner.lock().unwrap();
        inner.streams.push(stream);
        inner.wake();
    }

    /// Receive next message from this stream. Equivalent to
    ///
    /// ```ignore
    /// async fn recv(&self) -> Option<Message>;
    /// ```
    pub fn recv(&self) -> Recv {
        Recv { inner: &self.inner }
    }

    /// Closes this stream. Any subsequent `recv` will immediately return `None` unless new
    /// streams are added first.
    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.streams.clear();
        inner.wake();
    }
}

struct Inner {
    streams: SelectAll<MessageStream>,
    waker: Option<Waker>,
}

impl Inner {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake()
        }
    }
}

/// Future returned from [`MessageMultiStream::recv`].
pub(super) struct Recv<'a> {
    inner: &'a BlockingMutex<Inner>,
}

impl Future for Recv<'_> {
    type Output = Option<Message>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();

        match inner.streams.poll_next_unpin(cx) {
            Poll::Ready(message) => Poll::Ready(message),
            Poll::Pending => {
                if inner.waker.is_none() {
                    inner.waker = Some(cx.waker().clone());
                }

                Poll::Pending
            }
        }
    }
}

/// Sink that writes to the first available of multiple underlying TCP streams.
///
/// NOTE: Doesn't actually implement the `Sink` trait currently because we don't need it, only
/// provides a simple async `send` method.
pub(super) struct MessageMultiSink {
    // The sink currently used to write messages.
    active: AsyncMutex<Option<MessageSink>>,
    // Other sinks to replace the active sinks in case it fails.
    backup: BlockingMutex<Vec<MessageSink>>,
}

impl MessageMultiSink {
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
    recv: Arc<RecvState>,
    send: Arc<MessageMultiSink>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        let (queues_changed_tx, _) = watch::channel(());

        Self {
            recv: Arc::new(RecvState {
                reader: MessageMultiStream::new(),
                queues: BlockingMutex::new(Queues::default()),
                queues_changed_tx,
            }),
            send: Arc::new(MessageMultiSink::new()),
        }
    }

    /// Bind this dispatcher to the given TCP socket. Can be bound to multiple sockets and the
    /// failed ones are automatically removed.
    pub fn bind(&self, stream: TcpStream, permit: ConnectionPermit) {
        let (reader, writer) = stream.into_split();
        let (reader_permit, writer_permit) = permit.split();

        self.recv
            .reader
            .add(MessageStream::new(reader, reader_permit));
        self.send.add(MessageSink::new(writer, writer_permit));
    }

    /// Opens a stream for receiving messages with the given id.
    pub fn open_recv(&self, id: PublicRepositoryId) -> ContentStream {
        self.recv.claim(id);
        ContentStream::new(id, self.recv.clone())
    }

    /// Opens a sink for sending messages with the given id.
    pub fn open_send(&self, id: PublicRepositoryId) -> ContentSink {
        ContentSink {
            id,
            state: self.send.clone(),
        }
    }

    /// Creates a stream that will yield `ContentStream`s that haven't been opened yet.
    pub fn incoming(&self) -> IncomingContentStreams {
        IncomingContentStreams {
            state: self.recv.clone(),
            queues_changed_rx: self.recv.queues_changed_tx.subscribe(),
        }
    }
}

impl Drop for MessageDispatcher {
    fn drop(&mut self) {
        self.recv.reader.close()
    }
}

pub(super) struct ContentStream {
    id: PublicRepositoryId,
    state: Arc<RecvState>,
    queues_changed_rx: watch::Receiver<()>,
}

impl ContentStream {
    fn new(id: PublicRepositoryId, state: Arc<RecvState>) -> Self {
        let queues_changed_rx = state.queues_changed_tx.subscribe();

        Self {
            id,
            state,
            queues_changed_rx,
        }
    }

    pub fn id(&self) -> &PublicRepositoryId {
        &self.id
    }

    /// Receive the next message content.
    pub async fn recv(&mut self) -> Option<Content> {
        let mut closed = false;

        loop {
            if let Some(content) = self.state.pop(&self.id) {
                return Some(content);
            }

            if closed {
                return None;
            }

            select! {
                message = self.state.reader.recv() => {
                    if let Some(message) = message {
                        if message.id == self.id {
                            return Some(message.content);
                        } else {
                            self.state.push(message)
                        }
                    } else {
                        // If the reader closed we still want to check the queues one more time
                        // before bailing out.
                        closed = true;
                    }
                }
                _ = self.queues_changed_rx.changed() => ()
            }
        }
    }
}

impl Drop for ContentStream {
    fn drop(&mut self) {
        self.state.abandon(&self.id)
    }
}

#[derive(Clone)]
pub(super) struct ContentSink {
    id: PublicRepositoryId,
    state: Arc<MessageMultiSink>,
}

impl ContentSink {
    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Content) -> bool {
        self.state
            .send(&Message {
                id: self.id,
                content,
            })
            .await
    }
}

pub(super) struct IncomingContentStreams {
    state: Arc<RecvState>,
    queues_changed_rx: watch::Receiver<()>,
}

impl IncomingContentStreams {
    pub async fn recv(&mut self) -> Option<ContentStream> {
        let mut closed = false;

        loop {
            if let Some(id) = self.state.claim_any() {
                return Some(ContentStream::new(id, self.state.clone()));
            }

            if closed {
                return None;
            }

            select! {
                message = self.state.reader.recv() => {
                    if let Some(message) = message {
                        self.state.push(message)
                    } else {
                        closed = true;
                    }
                }
                _ = self.queues_changed_rx.changed() => (),
            }
        }
    }
}

struct RecvState {
    reader: MessageMultiStream,
    queues: BlockingMutex<Queues>,
    queues_changed_tx: watch::Sender<()>,
}

impl RecvState {
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
    async fn message_dispatcher_recv_on_stream() {
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

        let mut server_stream = server.open_recv(repo_id);

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
    async fn message_dispatcher_recv_on_two_streams() {
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

        let server_stream0 = server.open_recv(repo_id0);
        let server_stream1 = server.open_recv(repo_id1);

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

        let mut server_stream = server.open_recv(repo_id);
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

    #[tokio::test]
    async fn message_dispatcher_drop_dispatcher() {
        let (_client, server) = setup().await;

        let repo_id = PublicRepositoryId::random();

        let mut server_stream = server.open_recv(repo_id);
        let mut server_incoming = server.incoming();

        drop(server);

        assert!(server_stream.recv().await.is_none());
        assert!(server_incoming.recv().await.is_none());
    }

    #[tokio::test]
    async fn message_multi_stream_close() {
        let (client, server) = create_connected_sockets().await;
        let (server_reader, _server_writer) = server.into_split();

        let stream = MessageMultiStream::new();
        stream.add(MessageStream::new(
            server_reader,
            ConnectionPermit::dummy().split().0,
        ));

        let mut client = ObjectWrite::new(client);
        client
            .send(&Message {
                id: PublicRepositoryId::random(),
                content: Content::CreateLink,
            })
            .await
            .unwrap();

        stream.close();

        assert!(stream.recv().await.is_none());
    }

    async fn setup() -> (ObjectWrite<Message, TcpStream>, MessageDispatcher) {
        let (client, server) = create_connected_sockets().await;
        let client_writer = ObjectWrite::new(client);

        let server_dispatcher = MessageDispatcher::new();
        server_dispatcher.bind(server, ConnectionPermit::dummy());

        (client_writer, server_dispatcher)
    }

    async fn create_connected_sockets() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();

        (client, server)
    }
}
