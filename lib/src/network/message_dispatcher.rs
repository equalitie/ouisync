//! Utilities for sending and receiving messages across the network.

use super::{
    connection::{ConnectionInfo, ConnectionPermit, ConnectionPermitHalf, PermitId},
    keep_alive::{KeepAliveSink, KeepAliveStream},
    message::{Message, MessageChannel, Type},
    message_io::{MessageSink, MessageStream, SendError},
    raw,
};
use crate::{
    collections::{hash_map, HashMap, HashSet},
    deadlock::BlockingMutex,
    iterator::IntoIntersection,
};
use async_trait::async_trait;
use futures_util::{ready, stream::SelectAll, Sink, SinkExt, Stream, StreamExt};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tokio::{
    runtime, select,
    sync::{watch, Mutex as AsyncMutex},
    time::Duration,
};

// Time after which if no message is received, the connection is dropped.
const KEEP_ALIVE_RECV_INTERVAL: Duration = Duration::from_secs(60);
// How often to send keep-alive messages if no regular messages have been sent.
const KEEP_ALIVE_SEND_INTERVAL: Duration = Duration::from_secs(30);

/// Reads/writes messages from/to the underlying TCP or QUIC streams and dispatches them to
/// individual streams/sinks based on their ids.
#[derive(Clone)]
pub(super) struct MessageDispatcher {
    recv: Arc<RecvState>,
    send: Arc<MultiSink>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        let (queues_changed_tx, _) = watch::channel(());

        Self {
            recv: Arc::new(RecvState {
                reader: MultiStream::new(),
                queues: BlockingMutex::new(HashMap::default()),
                queues_changed_tx,
            }),
            send: Arc::new(MultiSink::new()),
        }
    }

    /// Bind this dispatcher to the given TCP of QUIC socket. Can be bound to multiple sockets and
    /// the failed ones are automatically removed.
    pub fn bind(&self, stream: raw::Stream, permit: ConnectionPermit) {
        let (reader, writer) = stream.into_split();
        let (reader_permit, writer_permit) = permit.split();

        self.recv
            .reader
            .add(PermittedStream::new(reader, reader_permit));
        self.send.add(PermittedSink::new(writer, writer_permit));
    }

    /// Opens a stream for receiving messages with the given id.
    pub fn open_recv(&self, channel: MessageChannel) -> ContentStream {
        ContentStream::new(channel, self.recv.clone())
    }

    /// Opens a sink for sending messages with the given id.
    pub fn open_send(&self, channel: MessageChannel) -> ContentSink {
        ContentSink {
            channel,
            state: self.send.clone(),
        }
    }

    /// Returns the active connections of this dispatcher.
    pub fn connection_infos(&self) -> LiveConnectionInfoSet {
        LiveConnectionInfoSet {
            recv: self.recv.clone(),
            send: self.send.clone(),
        }
    }

    pub async fn close(&self) {
        self.recv.reader.close();
        self.send.close().await;
    }

    pub fn is_closed(&self) -> bool {
        self.recv.reader.is_empty() || self.send.is_empty()
    }
}

impl Drop for MessageDispatcher {
    fn drop(&mut self) {
        if self.is_closed() {
            return;
        }

        self.recv.reader.close();

        let send = self.send.clone();

        if let Ok(handle) = runtime::Handle::try_current() {
            handle.spawn(async move { send.close().await });
        }
    }
}

pub(super) struct ContentStream {
    channel: MessageChannel,
    state: Arc<RecvState>,
    queues_changed_rx: watch::Receiver<()>,
    last_transport_id: Option<PermitId>,
}

impl ContentStream {
    fn new(channel: MessageChannel, state: Arc<RecvState>) -> Self {
        let queues_changed_rx = state.queues_changed_tx.subscribe();

        state.add_channel(channel);

        Self {
            channel,
            state,
            queues_changed_rx,
            last_transport_id: None,
        }
    }

    /// Receive the next message content.
    pub async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError> {
        let mut closed = false;

        loop {
            {
                let mut queues = self.state.queues.lock().unwrap();

                if let Some(queue) = queues.get_mut(&self.channel).map(|q| &mut q.queue) {
                    if let Some((transport, _)) = queue.back() {
                        match self.last_transport_id {
                            Some(last_transport_id) => {
                                if transport == &last_transport_id {
                                    return Ok(queue.pop_back().unwrap().1);
                                } else {
                                    self.last_transport_id = Some(*transport);
                                    return Err(ContentStreamError::TransportChanged);
                                }
                            }
                            None => {
                                self.last_transport_id = Some(*transport);
                                return Ok(queue.pop_back().unwrap().1);
                            }
                        }
                    }
                }
            }

            if closed {
                return Err(ContentStreamError::ChannelClosed);
            }

            select! {
                message = self.state.reader.recv() => {
                    if let Some((transport, message)) = message {
                        if message.channel == self.channel {
                            if let Some(last_transport) = self.last_transport_id {
                                if transport == last_transport {
                                    return Ok(message.content);
                                } else {
                                    self.last_transport_id = Some(transport);
                                    self.state.push(transport, message);
                                    return Err(ContentStreamError::TransportChanged);
                                }
                            } else {
                                self.last_transport_id = Some(transport);
                                return Ok(message.content);
                            }
                        } else {
                            self.state.push(transport, message)
                        }
                    } else {
                        // If the reader closed we still want to check the queues one more time
                        // because other streams might have pushed a message in the meantime.
                        closed = true;
                    }
                }
                _ = self.queues_changed_rx.changed() => ()
            }
        }
    }

    pub fn channel(&self) -> &MessageChannel {
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
    channel: MessageChannel,
    state: Arc<MultiSink>,
}

impl ContentSink {
    pub fn channel(&self) -> &MessageChannel {
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

/// Live* collection of active connections of a `MessageDispatcher`.
///
/// *) It means it gets automatically updated as connections are added/removed to/from the
/// dispatcher.
#[derive(Clone)]
pub(super) struct LiveConnectionInfoSet {
    recv: Arc<RecvState>,
    send: Arc<MultiSink>,
}

impl LiveConnectionInfoSet {
    /// Returns the current infos.
    pub fn iter(&self) -> impl Iterator<Item = ConnectionInfo> {
        let recv = self.recv.reader.connection_infos();
        let send = self.send.connection_infos();

        IntoIntersection::new(recv, send)
    }
}

struct ChannelQueue {
    reference_count: usize,
    queue: VecDeque<(PermitId, Vec<u8>)>,
}

struct RecvState {
    reader: MultiStream,
    queues: BlockingMutex<HashMap<MessageChannel, ChannelQueue>>,
    queues_changed_tx: watch::Sender<()>,
}

impl RecvState {
    fn add_channel(&self, channel_id: MessageChannel) {
        match self.queues.lock().unwrap().entry(channel_id) {
            hash_map::Entry::Occupied(mut entry) => entry.get_mut().reference_count += 1,
            hash_map::Entry::Vacant(entry) => {
                entry.insert(ChannelQueue {
                    reference_count: 1,
                    queue: Default::default(),
                });
            }
        }
    }

    fn remove_channel(&self, channel_id: &MessageChannel) {
        // Unwrap because we shouldn't remove more than we add.
        match self.queues.lock().unwrap().entry(*channel_id) {
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

    //// Pops a message from the corresponding queue.
    //fn pop(&self, channel: &MessageChannel) -> Option<Vec<u8>> {
    //    self.queues
    //        .lock()
    //        .unwrap()
    //        .get_mut(channel)?
    //        .queue
    //        .pop_back()
    //}

    // Pushes the message into the corresponding queue. Wakes up any waiting streams so they can
    // grab the message if it is for them. If there is currently no one waiting on the channel then
    // the message shall be ignored. Note that it should be OK to miss some of those packets
    // because the `Barrier` algorithm should take care of syncing the communication exchanges.
    fn push(&self, permit_id: PermitId, message: Message) {
        if let Some(value) = self.queues.lock().unwrap().get_mut(&message.channel) {
            value.queue.push_front((permit_id, message.content));
            self.queues_changed_tx.send(()).unwrap_or(());
        }
    }
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Internal

// Stream of `Message` backed by a `raw::Stream`. Closes on first error. Contains a connection
// permit which gets released on drop.
struct PermittedStream {
    inner: KeepAliveStream<raw::OwnedReadHalf>,
    permit: ConnectionPermitHalf,
}

impl PermittedStream {
    fn new(stream: raw::OwnedReadHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: KeepAliveStream::new(MessageStream::new(stream), KEEP_ALIVE_RECV_INTERVAL),
            permit,
        }
    }

    fn connection_info(&self) -> ConnectionInfo {
        self.permit.info()
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
    inner: KeepAliveSink<raw::OwnedWriteHalf>,
    permit: ConnectionPermitHalf,
}

impl PermittedSink {
    fn new(stream: raw::OwnedWriteHalf, permit: ConnectionPermitHalf) -> Self {
        Self {
            inner: KeepAliveSink::new(MessageSink::new(stream), KEEP_ALIVE_SEND_INTERVAL),
            permit,
        }
    }

    fn connection_info(&self) -> ConnectionInfo {
        self.permit.info()
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
    inner: BlockingMutex<MultiStreamInner>,
}

impl MultiStream {
    fn new() -> Self {
        Self {
            inner: BlockingMutex::new(MultiStreamInner {
                streams: SelectAll::new(),
                waker: None,
            }),
        }
    }

    fn add(&self, stream: PermittedStream) {
        let mut inner = self.inner.lock().unwrap();
        inner.streams.push(stream);
        inner.wake();
    }

    // Receive next message from this stream. Equivalent to
    //
    // ```ignore
    // async fn recv(&self) -> Option<Message>;
    // ```
    fn recv(&self) -> Recv {
        Recv { inner: &self.inner }
    }

    // Closes this stream. Any subsequent `recv` will immediately return `None` unless new
    // streams are added first.
    fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.streams.clear();
        inner.wake();
    }

    fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().streams.is_empty()
    }

    fn connection_infos(&self) -> HashSet<ConnectionInfo> {
        self.inner
            .lock()
            .unwrap()
            .streams
            .iter()
            .map(|stream| stream.connection_info())
            .collect()
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

    fn connection_infos(&self) -> HashSet<ConnectionInfo> {
        self.sinks
            .lock()
            .unwrap()
            .iter()
            .map(|sink| sink.connection_info())
            .collect()
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
    use std::net::Ipv4Addr;

    #[tokio::test(flavor = "multi_thread")]
    async fn recv_on_stream() {
        let (mut client, server) = setup().await;

        let channel = MessageChannel::random();
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

        let channel0 = MessageChannel::random();
        let channel1 = MessageChannel::random();

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

        let channel0 = MessageChannel::random();
        let channel1 = MessageChannel::random();

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
                assert_eq!(recv_content, build_message(server_stream.channel, i));
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_stream() {
        let (mut client, server) = setup().await;

        let channel = MessageChannel::random();

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
        assert_eq!(recv_content, send_content0);

        drop(server_stream0);

        let recv_content = server_stream1.recv().await.unwrap();
        assert_eq!(recv_content, send_content1)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_dispatcher() {
        let (_client, server) = setup().await;

        let channel = MessageChannel::random();

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
                channel: MessageChannel::random(),
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
