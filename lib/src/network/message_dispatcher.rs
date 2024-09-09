//! Utilities for sending and receiving messages across the network.

use super::{
    connection::{ConnectionDirection, ConnectionId, ConnectionPermit, ConnectionPermitHalf},
    message::{Message, MessageChannelId},
    message_io::{MessageSink, MessageStream, MESSAGE_OVERHEAD},
    stats::{ByteCounters, Instrumented},
};
use crate::{collections::HashMap, sync::AwaitDrop};
use async_trait::async_trait;
use futures_util::{
    future, ready,
    stream::{FuturesUnordered, SelectAll},
    FutureExt, Sink, SinkExt, Stream, StreamExt,
};
use net::unified::{Connection, ConnectionError, RecvStream, SendStream};
use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::{
    select,
    sync::{mpsc, oneshot},
    task,
};

const CONTENT_STREAM_BUFFER_SIZE: usize = 1024;

/// Reads/writes messages from/to the underlying TCP or QUIC streams and dispatches them to
/// individual streams/sinks based on their channel ids (in the MessageDispatcher's and
/// MessageBroker's contexts, there is a one-to-one relationship between the channel id and a
/// repository id).
#[derive(Clone)]
pub(super) struct MessageDispatcher {
    command_tx: mpsc::UnboundedSender<Command>,
    sink_tx: mpsc::Sender<Message>,
    connection_count: Arc<AtomicUsize>,
}

impl MessageDispatcher {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (sink_tx, sink_rx) = mpsc::channel(1);
        let connection_count = Arc::new(AtomicUsize::new(0));

        let worker = Worker::new(command_rx, sink_rx, connection_count.clone());
        task::spawn(worker.run());

        Self {
            command_tx,
            sink_tx,
            connection_count,
        }
    }

    /// Bind this dispatcher to the given TCP of QUIC socket. Can be bound to multiple sockets and
    /// the failed ones are automatically removed.
    pub fn bind(
        &self,
        connection: Connection,
        permit: ConnectionPermit,
        byte_counters: Arc<ByteCounters>,
    ) {
        self.command_tx
            .send(Command::Bind {
                connection,
                permit,
                byte_counters,
            })
            .ok();
    }

    /// Is this dispatcher bound to at least one connection?
    pub fn is_bound(&self) -> bool {
        self.connection_count.load(Ordering::Acquire) > 0
    }

    /// Opens a stream for receiving messages on the given channel. Any messages received on
    /// `channel` before the stream's been opened are discarded. When a stream is opened, all
    /// previously opened streams on the same channel (if any) get automatically closed.
    pub fn open_recv(&self, channel: MessageChannelId) -> ContentStream {
        let (stream_tx, stream_rx) = mpsc::channel(CONTENT_STREAM_BUFFER_SIZE);

        self.command_tx
            .send(Command::Open { channel, stream_tx })
            .ok();

        ContentStream {
            channel,
            command_tx: self.command_tx.clone(),
            stream_rx,
            last_transport_id: None,
            parked_message: None,
        }
    }

    /// Opens a sink for sending messages on the given channel.
    pub fn open_send(&self, channel: MessageChannelId) -> ContentSink {
        ContentSink {
            channel,
            sink_tx: self.sink_tx.clone(),
        }
    }

    /// Gracefully shuts down this dispatcher. This closes all bound connections and all open
    /// message streams and sinks.
    ///
    /// Note: the dispatcher also shutdowns automatically when it and all its message streams and
    /// sinks have been dropped. Calling this function is still useful when one wants to force the
    /// existing streams/sinks to close and/or to wait until the shutdown has been completed.
    pub async fn shutdown(self) {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::Shutdown { tx }).ok();
        rx.await.ok();
    }
}

pub(super) struct ContentStream {
    channel: MessageChannelId,
    command_tx: mpsc::UnboundedSender<Command>,
    stream_rx: mpsc::Receiver<(ConnectionId, Vec<u8>)>,
    last_transport_id: Option<ConnectionId>,
    parked_message: Option<Vec<u8>>,
}

impl ContentStream {
    /// Receive the next message content.
    pub async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError> {
        if let Some(content) = self.parked_message.take() {
            return Ok(content);
        }

        let (connection_id, content) = self
            .stream_rx
            .recv()
            .await
            .ok_or(ContentStreamError::ChannelClosed)?;

        if let Some(last_transport_id) = self.last_transport_id {
            if last_transport_id == connection_id {
                Ok(content)
            } else {
                self.last_transport_id = Some(connection_id);
                self.parked_message = Some(content);
                Err(ContentStreamError::TransportChanged)
            }
        } else {
            self.last_transport_id = Some(connection_id);
            Ok(content)
        }
    }

    pub fn channel(&self) -> &MessageChannelId {
        &self.channel
    }
}

impl Instrumented<ContentStream> {
    pub async fn recv(&mut self) -> Result<Vec<u8>, ContentStreamError> {
        let content = self.as_mut().recv().await?;
        self.counters()
            .increment_rx(content.len() as u64 + MESSAGE_OVERHEAD as u64);
        Ok(content)
    }

    pub fn channel(&self) -> &MessageChannelId {
        self.as_ref().channel()
    }
}

impl Drop for ContentStream {
    fn drop(&mut self) {
        self.command_tx
            .send(Command::Close {
                channel: self.channel,
            })
            .ok();
    }
}

#[derive(Eq, PartialEq, Debug)]
pub(super) enum ContentStreamError {
    ChannelClosed,
    TransportChanged,
}

#[derive(Clone)]
pub(super) struct ContentSink {
    channel: MessageChannelId,
    sink_tx: mpsc::Sender<Message>,
}

impl ContentSink {
    pub fn channel(&self) -> &MessageChannelId {
        &self.channel
    }

    /// Returns whether the send succeeded.
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        self.sink_tx
            .send(Message {
                channel: self.channel,
                content,
            })
            .await
            .map_err(|_| ChannelClosed)
    }
}

impl Instrumented<ContentSink> {
    pub async fn send(&self, content: Vec<u8>) -> Result<(), ChannelClosed> {
        let len = content.len();
        self.as_ref().send(content).await?;
        self.counters()
            .increment_tx(len as u64 + MESSAGE_OVERHEAD as u64);
        Ok(())
    }

    pub fn channel(&self) -> &MessageChannelId {
        self.as_ref().channel()
    }
}

//------------------------------------------------------------------------
// These traits are useful for testing.
// TODO: Move these traits and impls to barrier.rs as they are not used anywhere else.

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

#[derive(Debug)]
pub(super) struct ChannelClosed;

///////////////////////////////////////////////////////////////////////////////////////////////////
// Internal

// Stream for receiving messages from a single connection. Contains a connection permit half which
// gets released on drop. Automatically closes when the corresponding `ConnectionSink` closes.
struct ConnectionStream {
    // The reader is doubly instrumented - first time to track per connection stats and second time
    // to track cumulative stats across all connections.
    reader: MessageStream<Instrumented<Instrumented<RecvStream>>>,
    permit: ConnectionPermitHalf,
    permit_released: AwaitDrop,
    connection_count: Arc<AtomicUsize>,
}

impl ConnectionStream {
    fn new(
        reader: Instrumented<RecvStream>,
        permit: ConnectionPermitHalf,
        connection_count: Arc<AtomicUsize>,
    ) -> Self {
        connection_count.fetch_add(1, Ordering::Release);

        let permit_released = permit.released();

        Self {
            reader: MessageStream::new(Instrumented::new(reader, permit.byte_counters())),
            permit,
            permit_released,
            connection_count,
        }
    }
}

impl Stream for ConnectionStream {
    type Item = (ConnectionId, Message);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check if our sink was closed.
        match self.permit_released.poll_unpin(cx) {
            Poll::Pending => (),
            Poll::Ready(()) => {
                return Poll::Ready(None);
            }
        }

        match ready!(self.reader.poll_next_unpin(cx)) {
            Some(Ok(message)) => Poll::Ready(Some((self.permit.id(), message))),
            Some(Err(_)) | None => Poll::Ready(None),
        }
    }
}

impl Drop for ConnectionStream {
    fn drop(&mut self) {
        self.connection_count.fetch_sub(1, Ordering::Release);
    }
}

// Sink for sending messages on a single connection. Contains a connection permit half which gets
// released on drop. Automatically closes when the corresponding `ConnectionStream` is closed.
struct ConnectionSink {
    // The writer is doubly instrumented - first time to track per connection stats and second time
    // to track cumulative stats across all connections.
    writer: MessageSink<Instrumented<Instrumented<SendStream>>>,
    _permit: ConnectionPermitHalf,
    permit_released: AwaitDrop,
}

impl ConnectionSink {
    fn new(writer: Instrumented<SendStream>, permit: ConnectionPermitHalf) -> Self {
        let permit_released = permit.released();

        Self {
            writer: MessageSink::new(Instrumented::new(writer, permit.byte_counters())),
            _permit: permit,
            permit_released,
        }
    }
}

impl Sink<Message> for ConnectionSink {
    type Error = io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Check if our stream was closed.
        match self.permit_released.poll_unpin(cx) {
            Poll::Pending => (),
            Poll::Ready(()) => {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "message channel closed",
                )));
            }
        }

        self.writer.poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.writer.start_send_unpin(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.writer.poll_flush_unpin(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.writer.poll_close_unpin(cx)
    }
}

struct Worker {
    command_rx: mpsc::UnboundedReceiver<Command>,
    connection_count: Arc<AtomicUsize>,
    send: SendState,
    recv: RecvState,
}

impl Worker {
    fn new(
        command_rx: mpsc::UnboundedReceiver<Command>,
        sink_rx: mpsc::Receiver<Message>,
        connection_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            command_rx,
            connection_count,
            send: SendState {
                sink_rx,
                sinks: Vec::new(),
            },
            recv: RecvState {
                streams: SelectAll::default(),
                channels: HashMap::default(),
                message: None,
            },
        }
    }

    async fn run(mut self) {
        let mut streams = FuturesUnordered::new();

        loop {
            let command = select! {
                command = self.command_rx.recv() => command,
                Some(result) = streams.next() => {
                    match result {
                        Ok((connection, tx, rx)) => {
                            self.send.sinks.push((connection, tx));
                            self.recv.streams.push(rx);
                        }
                        Err(error) => {
                            tracing::debug!(?error, "Failed to establish a new connection stream");
                        }
                    }

                    continue;
                }
                _ = self.send.run() => unreachable!(),
                _ = self.recv.run() => unreachable!(),
            };

            let Some(command) = command else {
                break;
            };

            match command {
                Command::Open { channel, stream_tx } => {
                    self.recv.channels.insert(channel, stream_tx);
                }
                Command::Close { channel } => {
                    self.recv.channels.remove(&channel);
                }
                Command::Bind {
                    connection,
                    permit,
                    byte_counters,
                } => {
                    let connection_count = self.connection_count.clone();

                    streams.push(async move {
                        let (tx, rx) = match ConnectionDirection::from_source(permit.source()) {
                            ConnectionDirection::Incoming => connection.incoming().await?,
                            ConnectionDirection::Outgoing => connection.outgoing().await?,
                        };

                        let (tx_permit, rx_permit) = permit.into_split();

                        let tx = Instrumented::new(tx, byte_counters.clone());
                        let tx = ConnectionSink::new(tx, tx_permit);

                        let rx = Instrumented::new(rx, byte_counters.clone());
                        let rx = ConnectionStream::new(rx, rx_permit, connection_count);

                        Ok::<_, ConnectionError>((connection, tx, rx))
                    });
                }
                Command::Shutdown { tx } => {
                    self.shutdown().await;
                    tx.send(()).ok();
                }
            }
        }

        self.shutdown().await;
    }

    async fn shutdown(&mut self) {
        future::join_all(self.send.sinks.drain(..).map(|(connection, _)| async move {
            connection.close().await;
        }))
        .await;

        self.send.sink_rx.close();

        self.recv.streams.clear();
        self.recv.channels.clear();
    }
}

enum Command {
    Open {
        channel: MessageChannelId,
        stream_tx: mpsc::Sender<(ConnectionId, Vec<u8>)>,
    },
    Close {
        channel: MessageChannelId,
    },
    Bind {
        connection: Connection,
        permit: ConnectionPermit,
        byte_counters: Arc<ByteCounters>,
    },
    Shutdown {
        tx: oneshot::Sender<()>,
    },
}

struct SendState {
    sink_rx: mpsc::Receiver<Message>,
    // We need to keep the `Connection` around so the sink/stream stay opened. We can store it here
    // or in the `RecvState` but storing it here is slightly simpler.
    sinks: Vec<(Connection, ConnectionSink)>,
}

impl SendState {
    // Keep sending outgoing messages. This function never returns, but it's safe to cancel.
    async fn run(&mut self) {
        while let Some((_, sink)) = self.sinks.first_mut() {
            // The order of operations here is important for cancel-safety: first wait for the sink
            // to become ready for sending, then receive the message to be sent and finally send
            // the message on the sink. This order ensures that if this function is cancelled at
            // any point, the message to be sent is never lost.
            match future::poll_fn(|cx| sink.poll_ready_unpin(cx)).await {
                Ok(()) => (),
                Err(_) => {
                    self.sinks.swap_remove(0);
                    continue;
                }
            }

            let Some(message) = self.sink_rx.recv().await else {
                break;
            };

            match sink.start_send_unpin(message) {
                Ok(()) => (),
                Err(_) => {
                    self.sinks.swap_remove(0);
                    continue;
                }
            }
        }

        future::pending().await
    }
}

struct RecvState {
    streams: SelectAll<ConnectionStream>,
    channels: HashMap<MessageChannelId, mpsc::Sender<(ConnectionId, Vec<u8>)>>,
    message: Option<(MessageChannelId, ConnectionId, Vec<u8>)>,
}

impl RecvState {
    // Keeps receiving incomming messages and dispatches them to their respective message channels.
    // This function never returns but it's safe to cancel.
    async fn run(&mut self) {
        loop {
            let (channel, connection_id, content) = match self.message.take() {
                Some(message) => message,
                None => match self.streams.next().await {
                    Some((connection_id, message)) => {
                        (message.channel, connection_id, message.content)
                    }
                    None => break,
                },
            };

            let Some(tx) = self.channels.get(&channel) else {
                continue;
            };

            // Cancel safety: Remember the message while we are awaiting the send permit, so that if
            // this function is cancelled here we can resume sending of the message on the next
            // invocation.
            self.message = Some((channel, connection_id, content));

            let Ok(send_permit) = tx.reserve().await else {
                continue;
            };

            // unwrap is ok because `self.message` is `Some` here.
            let (_, connection_id, content) = self.message.take().unwrap();

            send_permit.send((connection_id, content));
        }

        future::pending().await
    }
}

#[cfg(test)]
mod tests {
    use super::{super::stats::ByteCounters, *};
    use assert_matches::assert_matches;
    use futures_util::{future, stream};
    use net::unified::{Acceptor, Connection, Connector};
    use std::{collections::BTreeSet, net::Ipv4Addr, str::from_utf8, time::Duration};

    #[tokio::test(flavor = "multi_thread")]
    async fn recv_on_stream() {
        let channel = MessageChannelId::random();
        let send_content = b"hello world";

        let server_dispatcher = MessageDispatcher::new();
        let mut server_stream = server_dispatcher.open_recv(channel);

        let (client, server) = create_connection_pair().await;

        let (client_tx, _client_rx) = client.outgoing().await.unwrap();
        let mut client_sink = MessageSink::new(client_tx);

        server_dispatcher.bind(
            server,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::default()),
        );

        client_sink
            .send(Message {
                channel,
                content: send_content.to_vec(),
            })
            .await
            .unwrap();

        let recv_content = server_stream.recv().await.unwrap();
        assert_eq!(recv_content, send_content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recv_on_two_streams() {
        let channel0 = MessageChannelId::random();
        let channel1 = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        let server_dispatcher = MessageDispatcher::new();
        let server_stream0 = server_dispatcher.open_recv(channel0);
        let server_stream1 = server_dispatcher.open_recv(channel1);

        let (client, server) = create_connection_pair().await;

        let mut client_sink = MessageSink::new(client.outgoing().await.unwrap().0);

        server_dispatcher.bind(
            server,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::default()),
        );

        for (channel, content) in [(channel0, send_content0), (channel1, send_content1)] {
            client_sink
                .send(Message {
                    channel,
                    content: content.to_vec(),
                })
                .await
                .unwrap();
        }

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

        let channel0 = MessageChannelId::random();
        let channel1 = MessageChannelId::random();

        let client_dispatcher = MessageDispatcher::new();
        let client_sink0 = client_dispatcher.open_send(channel0);
        let client_sink1 = client_dispatcher.open_send(channel1);

        let server_dispatcher = MessageDispatcher::new();
        let server_stream0 = server_dispatcher.open_recv(channel0);
        let server_stream1 = server_dispatcher.open_recv(channel1);

        let (client, server) = create_connection_pair().await;
        client_dispatcher.bind(
            client,
            ConnectionPermit::dummy(ConnectionDirection::Outgoing),
            Arc::new(ByteCounters::new()),
        );
        server_dispatcher.bind(
            server,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );

        let num_messages = 20;
        let mut send_tasks = vec![];

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
    async fn duplicate_stream() {
        let channel = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        let server_dispatcher = MessageDispatcher::new();
        let mut server_stream0 = server_dispatcher.open_recv(channel);
        let mut server_stream1 = server_dispatcher.open_recv(channel);

        let (client, server) = create_connection_pair().await;

        let mut client_sink = MessageSink::new(client.outgoing().await.unwrap().0);

        server_dispatcher.bind(
            server,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );

        for content in [send_content0, send_content1] {
            client_sink
                .send(Message {
                    channel,
                    content: content.to_vec(),
                })
                .await
                .unwrap();
        }

        assert_matches!(
            server_stream0.recv().await,
            Err(ContentStreamError::ChannelClosed)
        );
        assert_eq!(server_stream1.recv().await.unwrap(), send_content0);
        assert_eq!(server_stream1.recv().await.unwrap(), send_content1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_connections_recv() {
        crate::test_utils::init_log();

        let channel = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        let server_dispatcher = MessageDispatcher::new();
        let mut server_stream = server_dispatcher.open_recv(channel);

        let (client0, server0) = create_connection_pair().await;
        let (client1, server1) = create_connection_pair().await;

        let client_sink0 = MessageSink::new(client0.outgoing().await.unwrap().0);
        let client_sink1 = MessageSink::new(client1.outgoing().await.unwrap().0);

        server_dispatcher.bind(
            server0,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );
        server_dispatcher.bind(
            server1,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );

        for (mut client_sink, content) in
            [(client_sink0, send_content0), (client_sink1, send_content1)]
        {
            client_sink
                .send(Message {
                    channel,
                    content: content.to_vec(),
                })
                .await
                .unwrap();
        }

        let recv_content0 = server_stream.recv().await.unwrap();

        assert_eq!(
            server_stream.recv().await,
            Err(ContentStreamError::TransportChanged)
        );

        let recv_content1 = server_stream.recv().await.unwrap();

        // The messages may be received in any order
        assert_eq!(
            BTreeSet::from([recv_content0.as_slice(), recv_content1.as_slice()]),
            BTreeSet::from([send_content0.as_slice(), send_content1.as_slice()]),
        );

        client0.close().await;
        client1.close().await;
        server_dispatcher.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_connections_send() {
        let channel = MessageChannelId::random();

        let send_content0 = b"one two three";
        let send_content1 = b"four five six";

        let server_dispatcher = MessageDispatcher::new();
        let server_sink = server_dispatcher.open_send(channel);

        let (client0, server0) = create_connection_pair().await;
        let (client1, server1) = create_connection_pair().await;

        let (client0_tx, client0_rx) = client0.outgoing().await.unwrap();
        let (client1_tx, client1_rx) = client1.outgoing().await.unwrap();

        // The incoming streams are accepted only after something is sent on the corresponding
        // outgoing streams first.
        let mut client0_sink = MessageSink::new(client0_tx);
        let mut client1_sink = MessageSink::new(client1_tx);

        for sink in [&mut client0_sink, &mut client1_sink] {
            sink.send(Message {
                channel,
                content: Vec::new(),
            })
            .await
            .unwrap();
        }

        let client0_stream = MessageStream::new(client0_rx);
        let client1_stream = MessageStream::new(client1_rx);

        server_dispatcher.bind(
            server0,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );
        server_dispatcher.bind(
            server1,
            ConnectionPermit::dummy(ConnectionDirection::Incoming),
            Arc::new(ByteCounters::new()),
        );

        for content in [send_content0, send_content1] {
            server_sink.send(content.to_vec()).await.unwrap();
        }

        // The messages may be received on any stream
        let recv_contents: BTreeSet<_> = stream::select(client0_stream, client1_stream)
            .map(|message| message.unwrap().content)
            .take(2)
            .collect()
            .await;

        assert_eq!(
            recv_contents,
            [send_content0.to_vec(), send_content1.to_vec()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown() {
        let server_dispatcher = MessageDispatcher::new();
        let mut server_stream = server_dispatcher.open_recv(MessageChannelId::random());
        let server_sink = server_dispatcher.open_send(MessageChannelId::random());

        server_dispatcher.shutdown().await;

        assert_matches!(
            server_stream.recv().await,
            Err(ContentStreamError::ChannelClosed)
        );

        assert_matches!(server_sink.send(vec![]).await, Err(ChannelClosed));
    }

    async fn create_connection_pair() -> (Connection, Connection) {
        let client = net::quic::configure((Ipv4Addr::LOCALHOST, 0).into())
            .unwrap()
            .0;
        let server = net::quic::configure((Ipv4Addr::LOCALHOST, 0).into())
            .unwrap()
            .1;

        let client = Connector::from(client);
        let server = Acceptor::from(server);

        let client = client.connect(*server.local_addr());
        let server = async { server.accept().await?.await };

        future::try_join(client, server).await.unwrap()
    }
}
