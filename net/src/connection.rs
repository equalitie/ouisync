use crate::{quic, tcp};
use std::{
    future::Future,
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
            Self::Tcp(inner) => Ok(Connecting::Tcp(Some(inner.accept().await?))),
            Self::Quic(inner) => Ok(Connecting::Quic(inner.accept().await?)),
        }
    }
}

/// Incoming connection which being established.
pub enum Connecting {
    // Note TCP doesn't support two phase accept so this is already a fully established
    // connection.
    Tcp(Option<tcp::Connection>),
    Quic(quic::Connecting),
}

impl Future for Connecting {
    type Output = Result<Connection, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Self::Tcp(connection) => Poll::Ready(Ok(Connection::Tcp(
                connection.take().expect("future polled after completion"),
            ))),
            Self::Quic(connecting) => Pin::new(connecting)
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
    pub async fn close(&self) -> Result<(), Error> {
        match self {
            Self::Tcp(inner) => inner.close().await?,
            Self::Quic(inner) => inner.close(),
        }

        Ok(())
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
    use std::net::Ipv4Addr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        task,
    };

    #[tokio::test]
    async fn ping_tcp() {
        let (client, _) = tcp::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let (_, server) = tcp::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();

        ping_case(Connector::Tcp(client), Acceptor::Tcp(server)).await
    }

    #[tokio::test]
    async fn ping_quic() {
        let (client, _, _) = quic::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let (_, server, _) = quic::configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();

        ping_case(Connector::Quic(client), Acceptor::Quic(server)).await
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
}
