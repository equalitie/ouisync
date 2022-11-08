use super::{
    channel_id::ChannelId,
    message::Message,
    message_io::{MessageSink, MessageStream, SendError},
};
use futures_util::{ready, FutureExt, Sink, SinkExt, Stream, StreamExt};
use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    select,
    sync::{mpsc, oneshot},
    task, time,
};
use tokio_stream::Timeout;
use tokio_util::sync::{PollSendError, PollSender};

/// Adapter for `MessageStream` which yields error when no message is received within the specified
/// timeout.
pub(super) struct KeepAliveStream<R> {
    // Need to Pin<Box> this because we need this struct to be `Unpin` but `Timeout` is not.
    inner: Pin<Box<Timeout<MessageStream<R>>>>,
}

impl<R> KeepAliveStream<R>
where
    R: AsyncRead + Unpin,
{
    pub fn new(inner: MessageStream<R>, timeout: Duration) -> Self {
        use tokio_stream::StreamExt as _;

        Self {
            inner: Box::pin(inner.timeout(timeout)),
        }
    }
}

impl<R> Stream for KeepAliveStream<R>
where
    R: AsyncRead + Unpin,
{
    type Item = io::Result<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let item = match ready!(self.inner.poll_next_unpin(cx)) {
                Some(Ok(Ok(message))) => {
                    if is_keep_alive(&message) {
                        continue;
                    } else {
                        Some(Ok(message))
                    }
                }
                Some(Ok(Err(error))) => Some(Err(error)),
                Some(Err(_)) => Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "message stream timed out",
                ))),
                None => None,
            };

            return Poll::Ready(item);
        }
    }
}

fn is_keep_alive(message: &Message) -> bool {
    message.channel == ChannelId::default() && message.content.is_empty()
}

/// Adapter for `MessageSink` which periodically sends keep-alive messages if no regular messages
/// are sent in a while.
///
/// Note: to obtain the result of a send, `flush` needs to be called afterwards. If `flush` is not
/// called and another item is sent, the result of the previous send is lost.
pub(super) struct KeepAliveSink<W> {
    command_tx: PollSender<SinkCommand>,
    result_rx: Option<oneshot::Receiver<Result<(), SendError>>>,
    _type: PhantomData<W>,
}

impl<W> KeepAliveSink<W>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(inner: MessageSink<W>, interval: Duration) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);

        task::spawn(sink_worker(inner, interval, command_rx));

        Self {
            command_tx: PollSender::new(command_tx),
            result_rx: None,
            _type: PhantomData,
        }
    }
}

impl<W> Sink<Message> for KeepAliveSink<W>
where
    W: AsyncWrite + Unpin,
{
    type Error = SendError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match ready!(self.command_tx.poll_ready_unpin(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(make_send_error(error, sink_closed_error()))),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        let (result_tx, result_rx) = oneshot::channel();

        match self.command_tx.start_send_unpin(SinkCommand {
            message: item,
            result_tx,
        }) {
            Ok(()) => {
                self.result_rx = Some(result_rx);
                Ok(())
            }
            Err(error) => Err(make_send_error(error, sink_closed_error())),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match ready!(self.command_tx.poll_flush_unpin(cx)) {
            Ok(()) => (),
            Err(error) => {
                return Poll::Ready(Err(make_send_error(error, sink_closed_error())));
            }
        }

        if let Some(result_rx) = &mut self.result_rx {
            let result = ready!(result_rx.poll_unpin(cx)).unwrap_or(Ok(()));
            self.result_rx = None;
            Poll::Ready(result)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match ready!(self.command_tx.poll_close_unpin(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(make_send_error(
                error,
                io::Error::new(io::ErrorKind::Other, "sink close failed"),
            ))),
        }
    }
}

struct SinkCommand {
    message: Message,
    result_tx: oneshot::Sender<Result<(), SendError>>,
}

async fn sink_worker<W>(
    mut inner: MessageSink<W>,
    interval: Duration,
    mut command_rx: mpsc::Receiver<SinkCommand>,
) where
    W: AsyncWrite + Unpin,
{
    loop {
        select! {
            command = command_rx.recv() => {
                if let Some(command) = command {
                    let result = inner.send(command.message).await;
                    command.result_tx.send(result).unwrap_or(());
                } else {
                    inner.close().await.unwrap_or(());
                    break;
                }
            }
            _ = time::sleep(interval) => {
                // Send keep-alive message (empty message on the default channel)
                inner
                    .send(Message::default())
                    .await
                    .unwrap_or(());
            }
        }
    }
}

fn make_send_error(command_tx_error: PollSendError<SinkCommand>, source: io::Error) -> SendError {
    SendError {
        source,
        message: command_tx_error
            .into_inner()
            .map(|command| command.message)
            .unwrap_or_else(|| Message::default()),
    }
}

fn sink_closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::Other, "sink closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn sink_keep_alive_if_no_send() {
        let (client, server) = create_connected_sockets().await;

        let mut sink = KeepAliveSink::new(MessageSink::new(client), Duration::from_millis(100));
        let mut stream = MessageStream::new(server);

        time::sleep(Duration::from_millis(150)).await;

        sink.close().await.unwrap();

        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            Message {
                channel: ChannelId::default(),
                content: Vec::new()
            }
        );
    }

    #[tokio::test]
    async fn sink_no_keep_alive_if_send() {
        let (client, server) = create_connected_sockets().await;

        let mut sink = KeepAliveSink::new(MessageSink::new(client), Duration::from_millis(100));
        let mut stream = MessageStream::new(server);

        time::sleep(Duration::from_millis(80)).await;

        let message = Message {
            channel: ChannelId::random(),
            content: b"hello".to_vec(),
        };

        sink.send(message.clone()).await.unwrap();

        time::sleep(Duration::from_millis(40)).await;

        sink.close().await.unwrap();

        assert_eq!(stream.next().await.unwrap().unwrap(), message);

        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn stream_timeout_if_no_recv() {
        let (_client, server) = create_connected_sockets().await;

        let mut stream =
            KeepAliveStream::new(MessageStream::new(server), Duration::from_millis(100));

        time::sleep(Duration::from_millis(150)).await;

        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn stream_no_timeout_if_recv() {
        let (client, server) = create_connected_sockets().await;

        let mut sink = MessageSink::new(client);
        let mut stream =
            KeepAliveStream::new(MessageStream::new(server), Duration::from_millis(100));

        time::sleep(Duration::from_millis(80)).await;

        let message = Message {
            channel: ChannelId::random(),
            content: b"hello".to_vec(),
        };

        sink.send(message.clone()).await.unwrap();

        time::sleep(Duration::from_millis(40)).await;

        sink.close().await.unwrap();

        assert_eq!(stream.next().await.unwrap().unwrap(), message);

        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn stream_ignores_keep_alive_messages() {
        let (client, server) = create_connected_sockets().await;

        let mut sink = KeepAliveSink::new(MessageSink::new(client), Duration::from_millis(100));
        let mut stream =
            KeepAliveStream::new(MessageStream::new(server), Duration::from_millis(250));

        time::sleep(Duration::from_millis(150)).await;

        sink.close().await.unwrap();

        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
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
