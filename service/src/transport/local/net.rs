use std::{
    io,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream, UnixListener, UnixStream, tcp, unix},
};

use super::{
    LocalAddr,
    auth::{self, AuthKey},
};

pub(super) enum LocalListener {
    Tcp {
        inner: TcpListener,
        auth_key: AuthKey,
    },
    Unix(UnixListener),
}

impl LocalListener {
    pub async fn bind(addr: LocalAddr) -> Result<Self, io::Error> {
        match addr {
            LocalAddr::Tcp { addr, auth_key } => Ok(Self::Tcp {
                inner: TcpListener::bind(addr).await?,
                auth_key,
            }),
            LocalAddr::Unix(path) => Ok(Self::Unix(UnixListener::bind(path)?)),
        }
    }

    pub async fn accept(&self) -> Result<LocalAccept, io::Error> {
        match self {
            Self::Tcp { inner, auth_key } => {
                let (stream, _addr) = inner.accept().await?;
                Ok(LocalAccept::Tcp {
                    stream,
                    auth_key: *auth_key,
                })
            }
            Self::Unix(inner) => {
                let (stream, _addr) = inner.accept().await?;
                Ok(LocalAccept::Unix(stream))
            }
        }
    }

    pub fn local_addr(&self) -> Result<LocalAddr, io::Error> {
        match self {
            Self::Tcp { inner, auth_key } => Ok(LocalAddr::Tcp {
                addr: inner.local_addr()?,
                auth_key: *auth_key,
            }),
            Self::Unix(inner) => {
                let addr = inner.local_addr()?;
                let path = addr.as_pathname().map(Path::to_owned).unwrap_or_default();

                Ok(LocalAddr::Unix(path))
            }
        }
    }
}

pub(super) enum LocalAccept {
    Tcp {
        stream: TcpStream,
        auth_key: AuthKey,
    },
    Unix(UnixStream),
}

impl LocalAccept {
    pub async fn finalize(self) -> Result<LocalStream, io::Error> {
        match self {
            Self::Tcp {
                mut stream,
                auth_key,
            } => {
                auth::server(&mut stream, &auth_key).await?;
                Ok(LocalStream::Tcp(stream))
            }
            Self::Unix(stream) => Ok(LocalStream::Unix(stream)),
        }
    }
}

pub(super) enum LocalIo<Tcp, Unix> {
    Tcp(Tcp),
    Unix(Unix),
}

pub(super) type LocalStream = LocalIo<TcpStream, UnixStream>;
pub(super) type LocalOwnedReadHalf = LocalIo<tcp::OwnedReadHalf, unix::OwnedReadHalf>;
pub(super) type LocalOwnedWriteHalf = LocalIo<tcp::OwnedWriteHalf, unix::OwnedWriteHalf>;

impl LocalStream {
    pub async fn connect(addr: &LocalAddr) -> Result<Self, io::Error> {
        match addr {
            LocalAddr::Tcp { addr, auth_key } => {
                let mut stream = TcpStream::connect(addr).await?;
                auth::client(&mut stream, auth_key).await?;

                Ok(Self::Tcp(stream))
            }
            LocalAddr::Unix(path) => Ok(Self::Unix(UnixStream::connect(path).await?)),
        }
    }

    pub fn into_split(self) -> (LocalOwnedReadHalf, LocalOwnedWriteHalf) {
        match self {
            Self::Tcp(inner) => {
                let (reader, writer) = inner.into_split();
                (
                    LocalOwnedReadHalf::Tcp(reader),
                    LocalOwnedWriteHalf::Tcp(writer),
                )
            }
            Self::Unix(inner) => {
                let (reader, writer) = inner.into_split();
                (
                    LocalOwnedReadHalf::Unix(reader),
                    LocalOwnedWriteHalf::Unix(writer),
                )
            }
        }
    }
}

impl<Tcp, Unix> AsyncRead for LocalIo<Tcp, Unix>
where
    Tcp: AsyncRead + Unpin,
    Unix: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner).poll_read(cx, buf),
            Self::Unix(inner) => Pin::new(inner).poll_read(cx, buf),
        }
    }
}

impl<Tcp, Unix> AsyncWrite for LocalIo<Tcp, Unix>
where
    Tcp: AsyncWrite + Unpin,
    Unix: AsyncWrite + Unpin,
{
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner).poll_flush(cx),
            Self::Unix(inner) => Pin::new(inner).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner).poll_shutdown(cx),
            Self::Unix(inner) => Pin::new(inner).poll_shutdown(cx),
        }
    }

    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner).poll_write(cx, buf),
            Self::Unix(inner) => Pin::new(inner).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(inner) => Pin::new(inner).poll_write_vectored(cx, bufs),
            Self::Unix(inner) => Pin::new(inner).poll_write_vectored(cx, bufs),
        }
    }
}
