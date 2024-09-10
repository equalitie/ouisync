mod dispatch;
mod topic;
mod worker;

pub use topic::TopicId;

use crate::unified::{Connection, RecvStream, SendStream};
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    task,
};
use worker::Command;

/// Wrapper around connection that allow creating arbitrary (up to a limit determined by the
/// underlying connection) number of independent streams, each bound to a specific topic. When the
/// two peers create streams bound to the same topic, they can communicate on them with each
/// other.
pub struct Bus {
    command_tx: mpsc::UnboundedSender<Command>,
}

impl Bus {
    pub fn new(connection: Connection) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        task::spawn(worker::run(connection, command_rx));

        Self { command_tx }
    }

    /// Creates a pair of send and receive streams bound to the given topic.
    ///
    /// It doesn't matter when or in what order the peers create the streams - as long as both
    /// eventually create them, they will connect.
    ///
    /// Note: topics don't have to be unique. If multiple streams are created with the same topic,
    /// each one is still connected to exactly one corresponding streams of the remote peer, but
    /// it's undefined which local stream gets connected to which remote stream.
    pub fn create_topic(&self, topic_id: TopicId) -> (BusSendStream, BusRecvStream) {
        let (send_stream_tx, send_stream_rx) = oneshot::channel();
        let (recv_stream_tx, recv_stream_rx) = oneshot::channel();

        let send = BusSendStream {
            inner: StreamInner::Pending(send_stream_rx),
        };

        let recv = BusRecvStream {
            inner: StreamInner::Pending(recv_stream_rx),
        };

        self.command_tx
            .send(Command::Create {
                topic_id,
                send_stream_tx,
                recv_stream_tx,
            })
            .expect("bus worker unexpectedly terminated");

        (send, recv)
    }

    /// Gracefully shuts down the underlying connection.
    pub async fn shutdown(&self) {
        let (reply_tx, reply_rx) = oneshot::channel();

        if self
            .command_tx
            .send(Command::Shutdown { reply_tx })
            .is_err()
        {
            return;
        }

        reply_rx.await.ok();
    }
}

pub struct BusSendStream {
    inner: StreamInner<SendStream>,
}

impl AsyncWrite for BusSendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        ready!(self.get_mut().inner.poll(cx))?.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        ready!(self.get_mut().inner.poll(cx))?.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        ready!(self.get_mut().inner.poll(cx))?.poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        ready!(self.get_mut().inner.poll(cx))?.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        if let StreamInner::Active(stream) = &self.inner {
            stream.is_write_vectored()
        } else {
            false
        }
    }
}

pub struct BusRecvStream {
    inner: StreamInner<RecvStream>,
}

impl AsyncRead for BusRecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        ready!(self.get_mut().inner.poll(cx))?.poll_read(cx, buf)
    }
}

enum StreamInner<T> {
    Pending(oneshot::Receiver<Result<T, io::Error>>),
    Active(T),
}

impl<T> StreamInner<T>
where
    T: Unpin,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<Pin<&mut T>, io::Error>> {
        loop {
            return match self {
                Self::Pending(rx) => match ready!(Pin::new(rx).poll(cx)) {
                    Ok(Ok(stream)) => {
                        *self = Self::Active(stream);
                        continue;
                    }
                    Ok(Err(error)) => Poll::Ready(Err(error)),
                    Err(_) => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
                },
                Self::Active(stream) => Poll::Ready(Ok(Pin::new(stream))),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        create_connected_connections, create_connected_peers, init_log, Proto,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn sanity_check_tcp() {
        sanity_check_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn sanity_check_quic() {
        sanity_check_case(Proto::Quic).await
    }

    async fn sanity_check_case(proto: Proto) {
        init_log();

        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        let client = Bus::new(client);
        let server = Bus::new(server);

        let topic_id = TopicId::random();

        let (mut client_send_stream, mut client_recv_stream) = client.create_topic(topic_id);
        let (mut server_send_stream, mut server_recv_stream) = server.create_topic(topic_id);

        let client_message = b"hello from client";
        let server_message = b"hello from server";

        client_send_stream.write_all(client_message).await.unwrap();

        let mut buffer = vec![0; client_message.len()];
        server_recv_stream.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, client_message);

        server_send_stream.write_all(server_message).await.unwrap();

        let mut buffer = vec![0; server_message.len()];
        client_recv_stream.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, server_message);
    }

    #[tokio::test]
    async fn duplicate_topic_tcp() {
        duplicate_topic_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn duplicate_topic_quic() {
        duplicate_topic_case(Proto::Quic).await
    }

    async fn duplicate_topic_case(proto: Proto) {
        init_log();

        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        let client = Bus::new(client);
        let server = Bus::new(server);

        let topic_id = TopicId::random();

        let (mut client_send_stream_0, _client_recv_stream_0) = client.create_topic(topic_id);
        let (mut client_send_stream_1, _client_recv_stream_1) = client.create_topic(topic_id);

        let (_server_send_stream_0, mut server_recv_stream_0) = server.create_topic(topic_id);
        let (_server_send_stream_1, mut server_recv_stream_1) = server.create_topic(topic_id);

        client_send_stream_0.write_all(b"ping 0").await.unwrap();
        client_send_stream_1.write_all(b"ping 1").await.unwrap();

        let mut buffer_0 = [0; 6];
        server_recv_stream_0
            .read_exact(&mut buffer_0)
            .await
            .unwrap();

        let mut buffer_1 = [0; 6];
        server_recv_stream_1
            .read_exact(&mut buffer_1)
            .await
            .unwrap();

        // The streams can be connected in any order
        match (&buffer_0, &buffer_1) {
            (b"ping 0", b"ping 1") => (),
            (b"ping 1", b"ping 0") => (),
            _ => panic!("unexpected {:?}", (&buffer_0, &buffer_1)),
        }
    }
}
