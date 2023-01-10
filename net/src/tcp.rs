pub use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::socket;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// TCP listener
pub struct TcpListener(tokio::net::TcpListener);

impl TcpListener {
    /// Binds TCP socket to the given address. If the port is taken, uses a random one,
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self(socket::bind(addr).await?))
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        self.0
            .accept()
            .await
            .map(|(stream, addr)| (TcpStream(stream), addr))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }
}

/// TCP stream
pub struct TcpStream(tokio::net::TcpStream);

impl TcpStream {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self(tokio::net::TcpStream::connect(addr).await?))
    }

    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        self.0.into_split()
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
