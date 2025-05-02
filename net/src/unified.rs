//! Unified interface over different network protocols (currently TCP and QUIC).

#[cfg(test)]
use crate::mock;
use crate::{quic, tcp};
use futures_util::future::Either;
use std::{
    future::{self, Future, IntoFuture, Ready},
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Unified connector
pub enum Connector {
    Tcp(tcp::Connector),
    Quic(quic::Connector),
    #[cfg(test)]
    Mock(mock::Connector),
}

impl Connector {
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, ConnectionError> {
        match self {
            Self::Tcp(inner) => inner
                .connect(addr)
                .await
                .map(Connection::Tcp)
                .map_err(Into::into),
            Self::Quic(inner) => inner
                .connect(addr)
                .await
                .map(Connection::Quic)
                .map_err(Into::into),
            #[cfg(test)]
            Self::Mock(inner) => inner
                .connect(addr)
                .await
                .map(Connection::Mock)
                .map_err(Into::into),
        }
    }
}

impl From<tcp::Connector> for Connector {
    fn from(inner: tcp::Connector) -> Self {
        Self::Tcp(inner)
    }
}

impl From<quic::Connector> for Connector {
    fn from(inner: quic::Connector) -> Self {
        Self::Quic(inner)
    }
}

/// Unified acceptor
pub enum Acceptor {
    Tcp(tcp::Acceptor),
    Quic(quic::Acceptor),
    #[cfg(test)]
    Mock(mock::Acceptor),
}

impl Acceptor {
    pub fn local_addr(&self) -> &SocketAddr {
        match self {
            Self::Tcp(inner) => inner.local_addr(),
            Self::Quic(inner) => inner.local_addr(),
            #[cfg(test)]
            Self::Mock(inner) => inner.local_addr(),
        }
    }

    pub async fn accept(&self) -> Result<Connecting, ConnectionError> {
        match self {
            Self::Tcp(inner) => Ok(Connecting::Tcp(inner.accept().await?)),
            Self::Quic(inner) => Ok(Connecting::Quic(inner.accept().await?)),
            #[cfg(test)]
            Self::Mock(inner) => Ok(Connecting::Mock(inner.accept().await?)),
        }
    }
}

impl From<tcp::Acceptor> for Acceptor {
    fn from(inner: tcp::Acceptor) -> Self {
        Self::Tcp(inner)
    }
}

impl From<quic::Acceptor> for Acceptor {
    fn from(inner: quic::Acceptor) -> Self {
        Self::Quic(inner)
    }
}

/// Incoming connection while being established.
pub enum Connecting {
    // Note TCP doesn't support two phase accept so this is already a fully established
    // connection.
    Tcp(tcp::Connection),
    Quic(quic::Connecting),
    #[cfg(test)]
    Mock(mock::Connection),
}

impl Connecting {
    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            Self::Tcp(inner) => inner.remote_addr(),
            Self::Quic(inner) => inner.remote_addr(),
            #[cfg(test)]
            Self::Mock(inner) => inner.remote_addr(),
        }
    }
}

impl IntoFuture for Connecting {
    type Output = Result<Connection, ConnectionError>;
    type IntoFuture = ConnectingFuture;

    fn into_future(self) -> Self::IntoFuture {
        match self {
            Self::Tcp(inner) => ConnectingFuture::Tcp(future::ready(inner)),
            Self::Quic(inner) => ConnectingFuture::Quic(inner.into_future()),
            #[cfg(test)]
            Self::Mock(inner) => ConnectingFuture::Mock(future::ready(inner)),
        }
    }
}

pub enum ConnectingFuture {
    Tcp(Ready<tcp::Connection>),
    Quic(quic::ConnectingFuture),
    #[cfg(test)]
    Mock(Ready<mock::Connection>),
}

impl Future for ConnectingFuture {
    type Output = Result<Connection, ConnectionError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner)
                .poll(cx)
                .map(|inner| Ok(Connection::Tcp(inner))),
            Self::Quic(inner) => Pin::new(inner)
                .poll(cx)
                .map_ok(Connection::Quic)
                .map_err(Into::into),
            #[cfg(test)]
            Self::Mock(inner) => Pin::new(inner)
                .poll(cx)
                .map(|inner| Ok(Connection::Mock(inner))),
        }
    }
}

/// Unified connection.
pub enum Connection {
    Tcp(tcp::Connection),
    Quic(quic::Connection),
    #[cfg(test)]
    Mock(mock::Connection),
}

impl Connection {
    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            Self::Tcp(inner) => inner.remote_addr(),
            Self::Quic(inner) => inner.remote_addr(),
            #[cfg(test)]
            Self::Mock(inner) => inner.remote_addr(),
        }
    }

    /// Accept a new incoming stream
    pub async fn incoming(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        match self {
            Self::Tcp(inner) => inner
                .incoming()
                .await
                .map(|(send, recv)| (SendStream::Tcp(send), RecvStream::Tcp(recv)))
                .map_err(Into::into),
            Self::Quic(inner) => inner
                .incoming()
                .await
                .map(|(send, recv)| (SendStream::Quic(send), RecvStream::Quic(recv)))
                .map_err(Into::into),
            #[cfg(test)]
            Self::Mock(inner) => inner
                .incoming()
                .await
                .map(|(send, recv)| (SendStream::Mock(send), RecvStream::Mock(recv)))
                .map_err(Into::into),
        }
    }

    /// Open a new outgoing stream
    pub async fn outgoing(&self) -> Result<(SendStream, RecvStream), ConnectionError> {
        match self {
            Self::Tcp(inner) => inner
                .outgoing()
                .await
                .map(|(send, recv)| (SendStream::Tcp(send), RecvStream::Tcp(recv)))
                .map_err(Into::into),
            Self::Quic(inner) => inner
                .outgoing()
                .await
                .map(|(send, recv)| (SendStream::Quic(send), RecvStream::Quic(recv)))
                .map_err(Into::into),
            #[cfg(test)]
            Self::Mock(inner) => inner
                .outgoing()
                .await
                .map(|(send, recv)| (SendStream::Mock(send), RecvStream::Mock(recv)))
                .map_err(Into::into),
        }
    }

    /// Gracefully close the connection
    pub async fn close(&self) {
        match self {
            Self::Tcp(inner) => inner.close().await,
            Self::Quic(inner) => inner.close(),
            #[cfg(test)]
            Self::Mock(inner) => inner.close().await,
        }
    }

    /// Wait for the connection to be closed for any reason (e.g., locally or by the remote peer)
    ///
    /// Note the returned future has a `'static` lifetime, so it can be moved to another task/thread
    /// and awaited there.
    #[cfg(not(test))]
    pub fn closed(&self) -> impl Future<Output = ()> + 'static {
        match self {
            Self::Tcp(inner) => Either::Left(inner.closed()),
            Self::Quic(inner) => Either::Right(inner.closed()),
        }
    }
    #[cfg(test)]
    pub fn closed(&self) -> impl Future<Output = ()> + 'static {
        match self {
            Self::Tcp(inner) => Either::Left(inner.closed()),
            Self::Quic(_inner) => todo!(),
            Self::Mock(inner) => Either::Right(inner.closed()),
        }
    }
}

pub enum SendStream {
    Tcp(tcp::SendStream),
    Quic(quic::SendStream),
    #[cfg(test)]
    Mock(mock::SendStream),
}

impl AsyncWrite for SendStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_write(Pin::new(inner), cx, buf),
            Self::Quic(inner) => AsyncWrite::poll_write(Pin::new(inner), cx, buf),
            #[cfg(test)]
            Self::Mock(inner) => AsyncWrite::poll_write(Pin::new(inner), cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_flush(Pin::new(inner), cx),
            Self::Quic(inner) => AsyncWrite::poll_flush(Pin::new(inner), cx),
            #[cfg(test)]
            Self::Mock(inner) => AsyncWrite::poll_flush(Pin::new(inner), cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_shutdown(Pin::new(inner), cx),
            Self::Quic(inner) => AsyncWrite::poll_shutdown(Pin::new(inner), cx),
            #[cfg(test)]
            Self::Mock(inner) => AsyncWrite::poll_shutdown(Pin::new(inner), cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_write_vectored(Pin::new(inner), cx, bufs),
            Self::Quic(inner) => AsyncWrite::poll_write_vectored(Pin::new(inner), cx, bufs),
            #[cfg(test)]
            Self::Mock(inner) => AsyncWrite::poll_write_vectored(Pin::new(inner), cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(inner) => inner.is_write_vectored(),
            Self::Quic(inner) => inner.is_write_vectored(),
            #[cfg(test)]
            Self::Mock(inner) => inner.is_write_vectored(),
        }
    }
}

pub enum RecvStream {
    Tcp(tcp::RecvStream),
    Quic(quic::RecvStream),
    #[cfg(test)]
    Mock(mock::RecvStream),
}

impl AsyncRead for RecvStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncRead::poll_read(Pin::new(inner), cx, buf),
            Self::Quic(inner) => AsyncRead::poll_read(Pin::new(inner), cx, buf),
            #[cfg(test)]
            Self::Mock(inner) => AsyncRead::poll_read(Pin::new(inner), cx, buf),
        }
    }
}

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("tcp")]
    Tcp(#[from] tcp::Error),
    #[error("quic")]
    Quic(#[from] quic::Error),
    #[cfg(test)]
    #[error("mock")]
    Mock(#[from] mock::Error),
}

#[cfg(test)]
mod tests {
    use super::Connection;
    use crate::test_utils::{
        create_connected_connections, create_connected_peers, init_log, Proto,
    };
    use futures_util::{future, stream::FuturesUnordered, StreamExt};
    use itertools::Itertools;
    use proptest::{arbitrary::any, collection::vec};
    use test_strategy::proptest;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        select, task,
    };
    use tracing::Instrument;

    #[tokio::test]
    async fn ping_tcp() {
        ping_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn ping_quic() {
        ping_case(Proto::Quic).await
    }

    async fn ping_case(proto: Proto) {
        let (client, server) = create_connected_peers(proto);

        let addr = *server.local_addr();

        let server = task::spawn(async move {
            let conn = server.accept().await.unwrap().await.unwrap();
            let (mut tx, mut rx) = conn.incoming().await.unwrap();

            let mut buf = [0; 4];
            rx.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");

            tx.write_all(b"pong").await.unwrap();
        });

        let client = task::spawn(async move {
            let conn = client.connect(addr).await.unwrap();
            let (mut tx, mut rx) = conn.outgoing().await.unwrap();

            tx.write_all(b"ping").await.unwrap();

            let mut buf = [0; 4];

            // Ignore error as it likely means the connection was closed by the peer, which is
            // expected.
            rx.read_exact(&mut buf).await.ok();
        });

        server.await.unwrap();
        client.await.unwrap();
    }

    #[proptest]
    fn multi_streams(
        proto: Proto,
        #[strategy(vec(vec(any::<u8>(), 1..=1024), 1..32))] messages: Vec<Vec<u8>>,
    ) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(multi_streams_case(proto, messages));
    }

    async fn multi_streams_case(proto: Proto, mut messages: Vec<Vec<u8>>) {
        let (client, server) = create_connected_peers(proto);
        let server_addr = *server.local_addr();

        let client = async {
            let conn = client.connect(server_addr).await.unwrap();
            let tasks = FuturesUnordered::new();

            for message in &messages {
                tasks.push(async {
                    let (mut tx, mut rx) = conn.outgoing().await.unwrap();

                    // Send message
                    tx.write_u32(message.len() as u32).await.unwrap();
                    tx.write_all(message).await.unwrap();

                    // Receive response and close the stream
                    let mut buf = [0; 2];
                    rx.read_exact(&mut buf).await.unwrap();
                    assert_eq!(&buf, b"ok");

                    tx.shutdown().await.unwrap();

                    message.clone()
                });
            }

            let sent_messages: Vec<_> = tasks.collect().await;

            conn.close().await;

            sent_messages
        }
        .instrument(tracing::info_span!("client"));

        let server = async {
            let conn = server.accept().await.unwrap().await.unwrap();
            let mut tasks = FuturesUnordered::new();
            let mut received_messages = Vec::new();

            loop {
                let (mut tx, mut rx) = select! {
                    Ok(stream) = conn.incoming() => stream,
                    Some(message) = tasks.next() => {
                        received_messages.push(message);
                        continue;
                    }
                    else => break,
                };

                tasks.push(async move {
                    // Read message len
                    let len = rx.read_u32().await.unwrap() as usize;

                    // Read message content
                    let mut message = vec![0; len];
                    rx.read_exact(&mut message).await.unwrap();

                    // Send response and close the stream
                    tx.write_all(b"ok").await.unwrap();

                    tx.shutdown().await.unwrap();

                    message
                });
            }

            received_messages
        }
        .instrument(tracing::info_span!("server"));

        let (mut sent_messages, mut received_messages) = future::join(client, server).await;

        assert_eq!(sent_messages.len(), messages.len());
        assert_eq!(received_messages.len(), messages.len());

        sent_messages.sort();
        received_messages.sort();
        messages.sort();

        similar_asserts::assert_eq!(sent_messages, messages);
        similar_asserts::assert_eq!(received_messages, messages);
    }

    #[tokio::test]
    async fn concurrent_streams_tcp() {
        concurrent_streams_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn concurrent_streams_quic() {
        concurrent_streams_case(Proto::Quic).await
    }

    // Test concurrent establishment of both incoming and outgoing streams
    async fn concurrent_streams_case(proto: Proto) {
        init_log();

        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        // Exhaustively test all permutations of the operations
        let ops = [
            "ping(client)",
            "ping(server)",
            "pong(client)",
            "pong(server)",
        ];

        for order in ops.iter().permutations(ops.len()) {
            async {
                tracing::info!("init");

                future::try_join_all(order.iter().map(|op| async {
                    match **op {
                        "ping(client)" => ping(&client).await,
                        "ping(server)" => ping(&server).await,
                        "pong(client)" => pong(&client).await,
                        "pong(server)" => pong(&server).await,
                        _ => unreachable!(),
                    }
                }))
                .await?;

                tracing::info!("done");

                Ok::<_, anyhow::Error>(())
            }
            .instrument(tracing::info_span!("order", message = ?order))
            .await
            .unwrap()
        }

        async fn ping(connection: &Connection) -> anyhow::Result<()> {
            let (mut send_stream, mut recv_stream) = connection.outgoing().await?;

            send_stream.write_all(b"ping").await?;

            let mut buffer = [0; 4];
            recv_stream.read_exact(&mut buffer).await?;
            assert_eq!(&buffer, b"pong");

            Ok(())
        }

        async fn pong(connection: &Connection) -> anyhow::Result<()> {
            let (mut send_stream, mut recv_stream) = connection.incoming().await?;

            let mut buffer = [0; 4];
            recv_stream.read_exact(&mut buffer).await?;
            assert_eq!(&buffer, b"ping");

            send_stream.write_all(b"pong").await?;

            Ok(())
        }
    }

    #[tokio::test]
    async fn close_tcp() {
        close_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn close_quic() {
        close_case(Proto::Quic).await
    }

    async fn close_case(proto: Proto) {
        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        future::join(client.closed(), async {
            task::yield_now().await;
            server.close().await;
        })
        .await;
    }

    #[tokio::test]
    async fn drop_tcp() {
        drop_case(Proto::Tcp).await
    }

    #[tokio::test]
    async fn drop_quic() {
        drop_case(Proto::Quic).await
    }

    async fn drop_case(proto: Proto) {
        let (client, server) = create_connected_peers(proto);
        let (client, server) = create_connected_connections(&client, &server).await;

        future::join(client.closed(), async {
            task::yield_now().await;
            drop(server);
        })
        .await;
    }
}
