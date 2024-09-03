pub use self::implementation::{OwnedReadHalf, OwnedWriteHalf, TcpStream};

use self::implementation::TcpListener;
use std::{io, net::SocketAddr};

/// Configure TCP endpoint
pub fn configure(bind_addr: SocketAddr) -> Result<(Connector, Acceptor), Error> {
    let listener = TcpListener::bind(bind_addr)?;
    let local_addr = listener.local_addr()?;

    Ok((
        Connector,
        Acceptor {
            listener,
            local_addr,
        },
    ))
}

pub struct Connector;

impl Connector {
    pub async fn connect(&self, addr: SocketAddr) -> Result<TcpStream, Error> {
        Ok(TcpStream::connect(addr).await?)
    }
}

pub struct Acceptor {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl Acceptor {
    pub async fn accept(&self) -> Result<(TcpStream, SocketAddr), Error> {
        Ok(self.listener.accept().await?)
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO error")]
    Io(#[from] io::Error),
}

// Real
#[cfg(not(feature = "simulation"))]
mod implementation {
    pub use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

    use crate::{socket, KEEP_ALIVE_INTERVAL};
    use socket2::{Domain, Socket, TcpKeepalive, Type};
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
        pub fn bind(addr: impl Into<SocketAddr>) -> io::Result<Self> {
            let addr = addr.into();

            let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
            socket.set_nonblocking(true)?;
            // Ignore errors - reuse address is nice to have but not required.
            socket.set_reuse_address(true).ok();
            set_keep_alive(&socket)?;
            socket::bind_with_fallback(&socket, addr)?;

            // Marks the socket as ready for accepting incoming connections. This needs to be set
            // for TCP listeners otherwise we get "Invalid argument" error when calling `accept`.
            //
            // See https://stackoverflow.com/a/10002936/170073 for explanation of the parameter.
            socket.listen(128)?;

            Ok(Self(tokio::net::TcpListener::from_std(socket.into())?))
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
            let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
            socket.set_nonblocking(true)?;
            set_keep_alive(&socket)?;

            Ok(Self(
                tokio::net::TcpSocket::from_std_stream(socket.into())
                    .connect(addr)
                    .await?,
            ))
        }

        pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
            self.0.into_split()
        }
    }

    fn set_keep_alive(socket: &Socket) -> io::Result<()> {
        let options = TcpKeepalive::new()
            .with_time(KEEP_ALIVE_INTERVAL)
            .with_interval(KEEP_ALIVE_INTERVAL);

        // this options is not supported on windows
        #[cfg(any(
            target_os = "android",
            target_os = "ios",
            target_os = "linux",
            target_os = "macos",
        ))]
        let options = options.with_retries(1);

        socket.set_tcp_keepalive(&options)
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
}

// Simulation
#[cfg(feature = "simulation")]
mod implementation {
    pub use turmoil::net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    };
}
