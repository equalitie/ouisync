use crate::{quic, tcp};
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
}

impl Connector {
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, Error> {
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
}

impl Acceptor {
    pub fn local_addr(&self) -> &SocketAddr {
        match self {
            Self::Tcp(inner) => inner.local_addr(),
            Self::Quic(inner) => inner.local_addr(),
        }
    }

    pub async fn accept(&self) -> Result<Connecting, Error> {
        match self {
            Self::Tcp(inner) => Ok(Connecting::Tcp(inner.accept().await?)),
            Self::Quic(inner) => Ok(Connecting::Quic(inner.accept().await?)),
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
}

impl Connecting {
    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            Self::Tcp(inner) => inner.remote_addr(),
            Self::Quic(inner) => inner.remote_addr(),
        }
    }
}

impl IntoFuture for Connecting {
    type Output = Result<Connection, Error>;
    type IntoFuture = ConnectingFuture;

    fn into_future(self) -> Self::IntoFuture {
        match self {
            Self::Tcp(inner) => ConnectingFuture::Tcp(future::ready(inner)),
            Self::Quic(inner) => ConnectingFuture::Quic(inner.into_future()),
        }
    }
}

pub enum ConnectingFuture {
    Tcp(Ready<tcp::Connection>),
    Quic(quic::ConnectingFuture),
}

impl Future for ConnectingFuture {
    type Output = Result<Connection, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner)
                .poll(cx)
                .map(|inner| Ok(Connection::Tcp(inner))),
            Self::Quic(inner) => Pin::new(inner)
                .poll(cx)
                .map_ok(Connection::Quic)
                .map_err(Into::into),
        }
    }
}

/// Unified connection.
pub enum Connection {
    Tcp(tcp::Connection),
    Quic(quic::Connection),
}

impl Connection {
    pub fn remote_addr(&self) -> SocketAddr {
        match self {
            Self::Tcp(inner) => inner.remote_addr(),
            Self::Quic(inner) => inner.remote_addr(),
        }
    }

    /// Accept a new incoming stream
    pub async fn incoming(&self) -> Result<(SendStream, RecvStream), Error> {
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
        }
    }

    /// Open a new outgoing stream
    pub async fn outgoing(&self) -> Result<(SendStream, RecvStream), Error> {
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
        }
    }

    /// Gracefully close the connection
    pub async fn close(&self) {
        match self {
            Self::Tcp(inner) => inner.close().await,
            Self::Quic(inner) => inner.close(),
        }
    }
}

pub enum SendStream {
    Tcp(tcp::SendStream),
    Quic(quic::SendStream),
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
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_flush(Pin::new(inner), cx),
            Self::Quic(inner) => AsyncWrite::poll_flush(Pin::new(inner), cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Self::Tcp(inner) => AsyncWrite::poll_shutdown(Pin::new(inner), cx),
            Self::Quic(inner) => AsyncWrite::poll_shutdown(Pin::new(inner), cx),
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
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(inner) => inner.is_write_vectored(),
            Self::Quic(inner) => inner.is_write_vectored(),
        }
    }
}

pub enum RecvStream {
    Tcp(tcp::RecvStream),
    Quic(quic::RecvStream),
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
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("tcp")]
    Tcp(#[from] tcp::Error),
    #[error("quic")]
    Quic(#[from] quic::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{future, stream::FuturesUnordered, StreamExt};
    use rand::{distributions::Standard, Rng};
    use std::net::Ipv4Addr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        select, task,
    };
    use tracing::Instrument;

    #[tokio::test]
    async fn ping_tcp() {
        let (client, server) = setup_tcp_peers();
        ping_case(client, server).await
    }

    #[tokio::test]
    async fn ping_quic() {
        let (client, server) = setup_quic_peers();
        ping_case(client, server).await
    }

    async fn ping_case(client_connector: Connector, server_acceptor: Acceptor) {
        let addr = *server_acceptor.local_addr();

        let server = task::spawn(async move {
            let conn = server_acceptor.accept().await.unwrap().await.unwrap();
            let (mut tx, mut rx) = conn.incoming().await.unwrap();

            let mut buf = [0; 4];
            rx.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");

            tx.write_all(b"pong").await.unwrap();
        });

        let client = task::spawn(async move {
            let conn = client_connector.connect(addr).await.unwrap();
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

    #[tokio::test]
    async fn multi_streams_tcp() {
        let (client, server) = setup_tcp_peers();
        multi_streams_case(client, server).await;
    }

    #[tokio::test]
    async fn multi_streams_quic() {
        let (client, server) = setup_quic_peers();
        multi_streams_case(client, server).await;
    }

    async fn multi_streams_case(client_connector: Connector, server_acceptor: Acceptor) {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .compact()
            .init();

        let num_messages = 32;
        let min_message_size = 1;
        let max_message_size = 256 * 1024;

        let mut rng = rand::thread_rng();
        let mut messages: Vec<Vec<u8>> = (0..num_messages)
            .map(|_| {
                let size = rng.gen_range(min_message_size..=max_message_size);
                (&mut rng).sample_iter(Standard).take(size).collect()
            })
            .collect();

        let server_addr = *server_acceptor.local_addr();

        let client = async {
            let conn = client_connector.connect(server_addr).await.unwrap();
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
            let conn = server_acceptor.accept().await.unwrap().await.unwrap();
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

    fn setup_tcp_peers() -> (Connector, Acceptor) {
        let (client, _) = tcp::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let (_, server) = tcp::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();

        (Connector::Tcp(client), Acceptor::Tcp(server))
    }

    fn setup_quic_peers() -> (Connector, Acceptor) {
        let (client, _, _) = quic::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let (_, server, _) = quic::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();

        (Connector::Quic(client), Acceptor::Quic(server))
    }
}
