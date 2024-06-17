//! Utilities for sending and receiving messages across the network.

use super::{
    connection::{ConnectionPermit, ConnectionPermitHalf, PermitId},
    keep_alive::{KeepAliveSink, KeepAliveStream},
    message::{Message, MessageChannelId, Type},
    message_io::{MessageSink, MessageStream, SendError},
    raw,
    traffic_tracker::TrackingWrapper,
};
use crate::collections::{hash_map, HashMap};
use async_trait::async_trait;
use deadlock::BlockingMutex;
use futures_util::{ready, stream::SelectAll, Sink, SinkExt, Stream, StreamExt};
use scoped_task::ScopedJoinHandle;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    runtime, select,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    sync::{Mutex as AsyncMutex, Notify, Semaphore},
};

// Time after which if no message is received, the connection is dropped.
const KEEP_ALIVE_RECV_INTERVAL: Duration = Duration::from_secs(60);
// How often to send keep-alive messages if no regular messages have been sent.
const KEEP_ALIVE_SEND_INTERVAL: Duration = Duration::from_secs(30);

/// Reads/writes messages from/to the underlying TCP or QUIC streams and dispatches them to
/// individual streams/sinks based on their channel ids (in the MessageDispatcher's and
/// MessageBroker's contexts, there is a one-to-one relationship between the channel id and a
/// repository id).
#[derive(Clone)]
pub(super) struct MessageDispatcher {
    recv: Arc<RecvState>,
    send: Arc<MultiSink>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        Self {
            recv: Arc::new(RecvState::new()),
            send: Arc::new(MultiSink::new()),
        }
    }

    /// Bind this dispatcher to the given TCP of QUIC socket. Can be bound to multiple sockets and
    /// the failed ones are automatically removed.
    pub fn bind(&self, stream: raw::Stream, permit: ConnectionPermit) {
        let (reader, writer) = stream.into_split();
        let (reader_permit, writer_permit) = permit.split();

        self.recv.add(PermittedStream::new(reader, reader_permit));
        self.send.add(PermittedSink::new(writer, writer_permit));
    }

    /// Opens a stream for receiving messages with the given id.
    pub fn open_recv(&self, channel: MessageChannelId) -> ContentStream {
        ContentStream::new(channel, self.recv.clone())
    }

    /// Opens a sink for sending messages with the given id.
    pub fn open_send(&self, channel: MessageChannelId) -> ContentSink {
        ContentSink {
            channel,
            state: self.send.clone(),
        }
    }

    pub async fn close(&self) {
        self.recv.multi_stream.close();
        self.send.close().await;
    }

    pub fn is_closed(&self) -> bool {
        self.recv.multi_stream.is_empty() || self.send.is_empty()
    }
}

impl Drop for MessageDispatcher {
    fn drop(&mut self) {
        if self.is_closed() {
            return;
        }

        self.recv.multi_stream.close();

        let send = self.send.clone();

        if let Ok(handle) = runtime::Handle::try_current() {
            handle.spawn(async move { send.close().await });
        }
    }
}

pub(super) struct ContentStream {
    channel: MessageChannelId,
    state: Arc<RecvState>,
    last_transport_id: Option<PermitId>,
}

impl ContentStream {
    fn new(channel: MessageChannelId, state: Arc<RecvState>) -> Self {
        state.add_channel(channel);

        Self {
            channel,
            state,
            last_transport_id: None,
        }
    }

    /// Receive the next message content.
    pub async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError> {
        let arc = match self
            .state
            .queues
            .lock()
            .unwrap()
            .get_mut(&self.channel)
            .map(|queue| queue.rx.clone())
        {
            Some(rx) => rx,
            None => return Err(ContentStreamError::ChannelClosed),
        };

        let mut lock = arc.lock().await;

        if let Some((transport, data)) = lock.parked_message.take() {
            self.last_transport_id = Some(transport);
            return Ok(data);
        }

        let (transport, data) = match self.state.recv_on_queue(&mut lock.receiver).await {
            Some((transport, data)) => (transport, data),
            None => return Err(ContentStreamError::ChannelClosed),
        };

        if let Some(last_transport_id) = self.last_transport_id {
            if last_transport_id == transport {
                Ok(data)
            } else {
                self.last_transport_id = Some(transport);
                lock.parked_message = Some((transport, data));
                Err(ContentStreamError::TransportChanged)
            }
        } else {
            self.last_transport_id = Some(transport);
            Ok(data)
        }
    }

    pub fn channel(&self) -> &MessageChannelId {
        &self.channel
    }
}

impl Drop for ContentStream {
    fn drop(&mut self) {
        self.state.remove_channel(&self.channel);
    }
}

#[derive(Debug)]
pub(super) enum ContentStreamError {
    ChannelClosed,
    TransportChanged,
}

#[derive(Clone)]
pub(super) struct ContentSink {
    channel: MessageChannelId,
    state: Arc<MultiSink>,
}

impl ContentSink {
    pub fn channel(&self) -> &MessageChannelId {
        &self.channel
    }

    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        self.state
            .send(Message {
                tag: Type::Content,
                channel: self.channel,
                content,
            })
            .await
    }
}

//------------------------------------------------------------------------
// These traits are useful for testing.

#[async_trait]
pub(super) trait ContentSinkTrait {
    async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed>;
}

#[async_trait]
pub(super) trait ContentStreamTrait {
    async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError>;
}

#[async_trait]
impl ContentSinkTrait for ContentSink {
    async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        self.send(content).await
    }
}

#[async_trait]
impl ContentStreamTrait for ContentStream {
    async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError> {
        self.recv().await
    }
}

//------------------------------------------------------------------------

#[derive(Debug)]
pub(super) struct ChannelClosed;

struct ChannelQueue {
    reference_count: usize,
    rx: Arc<AsyncMutex<ChannelQueueReceiver>>,
    // TODO: This probably shouldn't be unbounded.
    tx: ChannelQueueSender,
}

struct ChannelQueueReceiver {
    parked_message: Option<(PermitId, Vec<u8>)>,
    receiver: UnboundedReceiver<(PermitId, Vec<u8>)>,
}

type ChannelQueueSender = UnboundedSender<(PermitId, Vec<u8>)>;

struct RecvState {
    multi_stream: Arc<MultiStream>,
    queues: BlockingMutex<HashMap<MessageChannelId, ChannelQueue>>,
    single_sorter: Semaphore,
}

impl RecvState {
    fn new() -> Self {
        Self {
            multi_stream: Arc::new(MultiStream::new()),
            queues: BlockingMutex::new(HashMap::default()),
            single_sorter: Semaphore::new(1),
        }
    }

    fn add(&self, stream: PermittedStream) {
        self.multi_stream.add(stream);
    }

    fn add_channel(&self, channel_id: MessageChannelId) {
        match self.queues.lock().unwrap().entry(channel_id) {
            hash_map::Entry::Occupied(mut entry) => entry.get_mut().reference_count += 1,
            hash_map::Entry::Vacant(entry) => {
                let (tx, rx) = mpsc::unbounded_channel();
                entry.insert(ChannelQueue {
                    reference_count: 1,
                    rx: Arc::new(AsyncMutex::new(ChannelQueueReceiver {
                        parked_message: None,
                        receiver: rx,
                    })),
                    tx,
                });
            }
        }
    }

    async fn recv_on_queue(
        &self,
        queue_rx: &mut UnboundedReceiver<(PermitId, Vec<u8>)>,
    ) -> Option<(PermitId, Vec<u8>)> {
        select! {
            maybe_message = queue_rx.recv() => maybe_message,
            _ = self.sort_incoming_messages_into_queues() => None,
        }
    }

    async fn sort_incoming_messages_into_queues(&self) {
        // The `recv_on_queue` function may be called multiple times (once per channel), but this
        // function must be called at most once, otherwise messages could get reordered.
        let _permit = self.single_sorter.acquire().await;

        while let Some((transport, message)) = self.multi_stream.recv().await {
            if let Some(queue) = self.queues.lock().unwrap().get_mut(&message.channel) {
                queue.tx.send((transport, message.content)).unwrap_or(());
            }
        }
    }

    fn remove_channel(&self, channel_id: &MessageChannelId) {
        let mut queues = self.queues.lock().unwrap();

        match queues.entry(*channel_id) {
            hash_map::Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                assert_ne!(value.reference_count, 0);
                value.reference_count -= 1;
                if value.reference_count == 0 {
                    entry.remove();
                }
            }
            hash_map::Entry::Vacant(_) => unreachable!(),
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Internal

// Stream of `Message` backed by a `raw::Stream`. Closes on first error. Contains a connection
// permit which gets released on drop.
struct PermittedStream {
    inner: KeepAliveStream<TrackingWrapper<raw::OwnedReadHalf>>,
    permit: ConnectionPermitHalf,
}

impl PermittedStream {
    fn new(stream: raw::OwnedReadHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: KeepAliveStream::new(
                MessageStream::new(TrackingWrapper::new(stream, permit.tracker())),
                KEEP_ALIVE_RECV_INTERVAL,
            ),
            permit,
        }
    }
}

impl Stream for PermittedStream {
    type Item = (PermitId, Message);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match ready!(self.inner.poll_next_unpin(cx)) {
            Some(Ok(message)) => Poll::Ready(Some((self.permit.id(), message))),
            Some(Err(_)) | None => Poll::Ready(None),
        }
    }
}

// Sink for `Message` backed by a `raw::Stream`.
// Contains a connection permit which gets released on drop.
struct PermittedSink {
    inner: KeepAliveSink<TrackingWrapper<raw::OwnedWriteHalf>>,
    _permit: ConnectionPermitHalf,
}

impl PermittedSink {
    fn new(stream: raw::OwnedWriteHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: KeepAliveSink::new(
                MessageSink::new(TrackingWrapper::new(stream, permit.tracker())),
                KEEP_ALIVE_SEND_INTERVAL,
            ),
            _permit: permit,
        }
    }
}

// `Sink` impl just trivially delegates to the underlying sink.
impl Sink<Message> for PermittedSink {
    type Error = SendError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.inner.start_send_unpin(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx)
    }
}

// Stream that reads `Message`s from multiple underlying raw (byte) streams concurrently.
struct MultiStream {
    explicitly_closed: AtomicBool,
    rx: AsyncMutex<mpsc::Receiver<(PermitId, Message)>>,
    inner: Arc<BlockingMutex<MultiStreamInner>>,
    stream_added: Arc<Notify>,
    _runner: ScopedJoinHandle<()>,
}

impl MultiStream {
    fn new() -> Self {
        let inner = Arc::new(BlockingMutex::new(MultiStreamInner {
            streams: SelectAll::new(),
            waker: None,
        }));

        let stream_added = Arc::new(Notify::new());
        let (tx, rx) = mpsc::channel(1);

        let _runner =
            scoped_task::spawn(multi_stream_runner(inner.clone(), tx, stream_added.clone()));

        Self {
            explicitly_closed: AtomicBool::new(false),
            rx: AsyncMutex::new(rx),
            inner,
            stream_added,
            _runner,
        }
    }

    fn add(&self, stream: PermittedStream) {
        let mut inner = self.inner.lock().unwrap();
        inner.streams.push(stream);
        inner.wake();
        self.stream_added.notify_one();
    }

    // Receive next message from this stream.
    async fn recv(&self) -> Option<(PermitId, Message)> {
        if self.explicitly_closed.load(Ordering::Relaxed) {
            return None;
        }
        self.rx.lock().await.recv().await
    }

    // Closes this stream. Any subsequent `recv` will immediately return `None` unless new
    // streams are added first.
    fn close(&self) {
        self.explicitly_closed.store(true, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        inner.streams.clear();
        inner.wake();
    }

    fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().streams.is_empty()
    }
}

struct MultiStreamInner {
    streams: SelectAll<PermittedStream>,
    waker: Option<Waker>,
}

impl MultiStreamInner {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake()
        }
    }
}

// We need this runner because we want to detect when a peer has disconnected even if the user has
// not called `MultiStream::recv`.
async fn multi_stream_runner(
    inner: Arc<BlockingMutex<MultiStreamInner>>,
    tx: mpsc::Sender<(PermitId, Message)>,
    stream_added: Arc<Notify>,
) {
    loop {
        // Wait for at least one stream to be added.
        stream_added.notified().await;

        while let Some((permit_id, message)) = (Recv { inner: &inner }).await {
            match tx.send((permit_id, message)).await {
                Ok(()) => (),
                Err(_) => break,
            }
        }

        // Even though the last stream was removed, more streams might still be added so we are not
        // done yet.
    }
}

// Future returned from [`MultiStream::recv`].
struct Recv<'a> {
    inner: &'a BlockingMutex<MultiStreamInner>,
}

impl Future for Recv<'_> {
    type Output = Option<(PermitId, Message)>;

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

// Sink that writes to multiple underlying TCP streams sequentially until one of them succeeds,
// automatically removing the failed ones.
//
// NOTE: Doesn't actually implement the `Sink` trait currently because we don't need it, only
// provides an async `send` method.
struct MultiSink {
    single_send: AsyncMutex<()>,
    sinks: BlockingMutex<Vec<PermittedSink>>,
}

impl MultiSink {
    fn new() -> Self {
        Self {
            single_send: AsyncMutex::new(()),
            sinks: BlockingMutex::new(Vec::new()),
        }
    }

    fn add(&self, sink: PermittedSink) {
        self.sinks.lock().unwrap().push(sink);
    }

    async fn close(&self) {
        // TODO: Other functions should fail if called after the call to this function.

        let mut sinks = {
            let mut sinks = self.sinks.lock().unwrap();
            std::mem::take(&mut *sinks)
        };

        let mut futures = Vec::with_capacity(sinks.len());

        for mut sink in sinks.drain(..) {
            futures.push(async move { sink.close().await.unwrap_or(()) });
        }

        futures_util::future::join_all(futures).await;
    }

    async fn send(&self, message: Message) -> Result<(), ChannelClosed> {
        let _lock = self.single_send.lock().await;
        Send {
            message: Some(message),
            sinks: &self.sinks,
        }
        .await
    }

    fn is_empty(&self) -> bool {
        self.sinks.lock().unwrap().is_empty()
    }
}

// Future returned from [`MultiSink::send`].
struct Send<'a> {
    message: Option<Message>,
    sinks: &'a BlockingMutex<Vec<PermittedSink>>,
}

impl Future for Send<'_> {
    type Output = Result<(), ChannelClosed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut sinks = self.sinks.lock().unwrap();

        loop {
            let sink = if let Some(sink) = sinks.first_mut() {
                sink
            } else {
                return Poll::Ready(Err(ChannelClosed));
            };

            let message = match sink.poll_ready_unpin(cx) {
                Poll::Ready(Ok(())) => self.message.take().expect("polled Send after completion"),
                Poll::Ready(Err(error)) => {
                    sinks.swap_remove(0);
                    self.message = Some(error.message);
                    continue;
                }
                Poll::Pending => return Poll::Pending,
            };

            match sink.start_send_unpin(message) {
                Ok(()) => return Poll::Ready(Ok(())),
                Err(error) => {
                    sinks.swap_remove(0);
                    self.message = Some(error.message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use net::tcp::{TcpListener, TcpStream};
    use std::{net::Ipv4Addr, str::from_utf8};

    #[tokio::test(flavor = "multi_thread")]
    async fn recv_on_stream() {
        let (mut client, server) = setup().await;

        let channel = MessageChannelId::random();
        let send_content = b"hello world";

        client
            .send(Message {
                tag: Type::Content,
                channel,
                content: send_content.to_vec(),
            })
            .await
            .unwrap();

        let mut server_stream = server.open_recv(channel);

        let recv_content = server_stream.recv().await.unwrap();
        assert_eq!(recv_content, send_content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recv_on_two_streams() {
        let (mut client, server) = setup().await;

        let channel0 = MessageChannelId::random();
        let channel1 = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        for (channel, content) in [(channel0, send_content0), (channel1, send_content1)] {
            client
                .send(Message {
                    tag: Type::Content,
                    channel,
                    content: content.to_vec(),
                })
                .await
                .unwrap();
        }

        let server_stream0 = server.open_recv(channel0);
        let server_stream1 = server.open_recv(channel1);

        for (mut server_stream, send_content) in [
            (server_stream0, send_content0),
            (server_stream1, send_content1),
        ] {
            let recv_content = server_stream.recv().await.unwrap();
            assert_eq!(recv_content, send_content);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn send_on_two_streams_parallel() {
        use tokio::{task, time::timeout};

        let (client, server) = setup_two_dispatchers().await;

        let channel0 = MessageChannelId::random();
        let channel1 = MessageChannelId::random();

        let num_messages = 20;

        let mut send_tasks = vec![];

        let client_sink0 = client.open_send(channel0);
        let client_sink1 = client.open_send(channel1);

        let build_message = |channel, i| format!("{:?}:{}", channel, i).as_bytes().to_vec();

        for sink in [client_sink0, client_sink1] {
            send_tasks.push(task::spawn(async move {
                for i in 0..num_messages {
                    sink.send(build_message(sink.channel, i)).await.unwrap();
                }
            }));
        }

        for task in send_tasks {
            timeout(Duration::from_secs(3), task)
                .await
                .expect("Timed out")
                .expect("Send failed");
        }

        let server_stream0 = server.open_recv(channel0);
        let server_stream1 = server.open_recv(channel1);

        for mut server_stream in [server_stream0, server_stream1] {
            for i in 0..num_messages {
                let recv_content = server_stream.recv().await.unwrap();
                assert_eq!(
                    from_utf8(&recv_content).unwrap(),
                    from_utf8(&build_message(server_stream.channel, i)).unwrap()
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_stream() {
        let (mut client, server) = setup().await;

        let channel = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        for content in [send_content0, send_content1] {
            client
                .send(Message {
                    tag: Type::Content,
                    channel,
                    content: content.to_vec(),
                })
                .await
                .unwrap();
        }

        let mut server_stream0 = server.open_recv(channel);
        let mut server_stream1 = server.open_recv(channel);

        let recv_content = server_stream0.recv().await.unwrap();
        assert_eq!(
            from_utf8(&recv_content).unwrap(),
            from_utf8(send_content0).unwrap()
        );

        drop(server_stream0);

        let recv_content = server_stream1.recv().await.unwrap();
        assert_eq!(recv_content, send_content1)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_dispatcher() {
        let (_client, server) = setup().await;

        let channel = MessageChannelId::random();

        let mut server_stream = server.open_recv(channel);

        drop(server);

        assert_matches!(
            server_stream.recv().await,
            Err(ContentStreamError::ChannelClosed)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_stream_close() {
        let (client, server) = create_connected_sockets().await;
        let (server_reader, _server_writer) = server.into_split();

        let stream = MultiStream::new();
        stream.add(PermittedStream::new(
            server_reader,
            ConnectionPermit::dummy().split().0,
        ));

        let mut client = MessageSink::new(client);
        client
            .send(Message {
                tag: Type::Content,
                channel: MessageChannelId::random(),
                content: b"hello world".to_vec(),
            })
            .await
            .unwrap();

        stream.close();

        assert!(stream.recv().await.is_none());
    }

    async fn setup() -> (MessageSink<raw::Stream>, MessageDispatcher) {
        let (client, server) = create_connected_sockets().await;
        let client_writer = MessageSink::new(client);

        let server_dispatcher = MessageDispatcher::new();
        server_dispatcher.bind(server, ConnectionPermit::dummy());

        (client_writer, server_dispatcher)
    }

    async fn setup_two_dispatchers() -> (MessageDispatcher, MessageDispatcher) {
        let (client, server) = create_connected_sockets().await;

        let client_dispatcher = MessageDispatcher::new();
        client_dispatcher.bind(client, ConnectionPermit::dummy());

        let server_dispatcher = MessageDispatcher::new();
        server_dispatcher.bind(server, ConnectionPermit::dummy());

        (client_dispatcher, server_dispatcher)
    }

    async fn create_connected_sockets() -> (raw::Stream, raw::Stream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16))
            .await
            .unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();

        (raw::Stream::Tcp(client), raw::Stream::Tcp(server))
    }
}
