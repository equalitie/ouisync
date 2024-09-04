use crate::sync::rendezvous;

use self::implementation::{TcpListener, TcpStream};
use std::{future, io, net::SocketAddr};
use tokio::{
    io::{ReadHalf, WriteHalf},
    select,
    sync::{mpsc, oneshot},
    task,
};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{Instrument, Span};

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
    pub async fn connect(&self, addr: SocketAddr) -> Result<Connection, Error> {
        let stream = TcpStream::connect(addr).await?;

        Ok(Connection::new(stream, yamux::Mode::Client, addr))
    }
}

/// TCP acceptor
pub struct Acceptor {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl Acceptor {
    pub async fn accept(&self) -> Result<Connection, Error> {
        let (stream, addr) = self.listener.accept().await?;

        Ok(Connection::new(stream, yamux::Mode::Server, addr))
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

/// TCP connection
pub struct Connection {
    remote_addr: SocketAddr,
    command_tx: mpsc::Sender<Command>,
}

impl Connection {
    fn new(stream: TcpStream, mode: yamux::Mode, remote_addr: SocketAddr) -> Self {
        let connection = yamux::Connection::new(stream.compat(), connection_config(), mode);
        let (command_tx, command_rx) = mpsc::channel(1);

        task::spawn(drive_connection(connection, command_rx).instrument(Span::current()));

        Self {
            command_tx,
            remote_addr,
        }
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Accept the next incoming stream.
    ///
    /// # Cancel safety
    ///
    /// In case this function is cancelled, no stream gets lost and the call can be safely retried.
    pub async fn incoming(&self) -> Result<(SendStream, RecvStream), Error> {
        let (reply_tx, reply_rx) = rendezvous::channel();

        self.command_tx
            .send(Command::Incoming(reply_tx))
            .await
            .map_err(|_| yamux::ConnectionError::Closed)?;

        let stream = reply_rx
            .recv()
            .await
            .map_err(|_| yamux::ConnectionError::Closed)??;
        let (recv, send) = tokio::io::split(stream.compat());

        Ok((send, recv))
    }

    /// Open a new outgoing stream
    ///
    /// # Cancel safety
    ///
    /// In case this function is cancelled, no stream gets lost and the call can be safely retried.
    pub async fn outgoing(&self) -> Result<(SendStream, RecvStream), Error> {
        let (reply_tx, reply_rx) = rendezvous::channel();

        self.command_tx
            .send(Command::Outgoing(reply_tx))
            .await
            .map_err(|_| yamux::ConnectionError::Closed)?;

        let stream = reply_rx
            .recv()
            .await
            .map_err(|_| yamux::ConnectionError::Closed)??;
        let (recv, send) = tokio::io::split(stream.compat());

        Ok((send, recv))
    }

    /// Gracefully close the connection
    ///
    /// # Cancel safety
    ///
    /// This function is idempotent even in the presence of cancellation.
    pub async fn close(&self) -> Result<(), Error> {
        let (reply_tx, reply_rx) = oneshot::channel();

        // If send or receive return an error it means the connection is already closed. Returning
        // `Ok` in that case to make this function idempotent.
        self.command_tx
            .send(Command::Close(Some(reply_tx)))
            .await
            .ok();

        reply_rx.await.unwrap_or(Ok(())).map_err(Into::into)
    }
}

pub type SendStream = WriteHalf<Compat<yamux::Stream>>;
pub type RecvStream = ReadHalf<Compat<yamux::Stream>>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO error")]
    Io(#[from] io::Error),
    #[error("connection error")]
    Connection(#[from] yamux::ConnectionError),
}

// Yamux requires that `poll_next_inbound` is called continuously in order to drive the connection
// forward. We spawn a task a do it there.
async fn drive_connection(
    mut conn: yamux::Connection<Compat<TcpStream>>,
    mut command_rx: mpsc::Receiver<Command>,
) {
    // Buffers for incoming and outgoing streams. These guarantee that no streams are ever lost,
    // even if `Connection::incoming` or `Connection::outgoing` are cancelled. Due to the limit on
    // the number of streams per connection, these buffers are effectively bounded.
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();

    loop {
        let command = select! {
            command = command_rx.recv() => command,
            result = future::poll_fn(|cx| conn.poll_next_inbound(cx)) => {
                match result {
                    Some(result) => {
                        incoming.push(result);
                        continue;
                    }
                    None => break,
                }
            }
        };

        match command.unwrap_or(Command::Close(None)) {
            Command::Incoming(reply_tx) => {
                let result = if let Some(result) = incoming.pop() {
                    result
                } else if let Some(result) = future::poll_fn(|cx| conn.poll_next_inbound(cx)).await
                {
                    result
                } else {
                    break;
                };

                if let Err(result) = reply_tx.send(result).await {
                    // reply_rx dropped before receiving the result, save it for next time.
                    incoming.push(result);
                }

                continue;
            }
            Command::Outgoing(reply_tx) => {
                let result = if let Some(result) = outgoing.pop() {
                    result
                } else {
                    future::poll_fn(|cx| conn.poll_new_outbound(cx)).await
                };

                if let Err(result) = reply_tx.send(result).await {
                    // reply_rx dropped before receiving the result, save it for next time.
                    outgoing.push(result);
                }
            }
            Command::Close(reply_tx) => {
                let result = future::poll_fn(|cx| conn.poll_close(cx)).await;

                if let Some(reply_tx) = reply_tx {
                    reply_tx.send(result).ok();
                }

                break;
            }
        }
    }
}

enum Command {
    // Using rendezvous to guarantee the reply is either received or we get it back if the receive
    // got cancelled.
    Incoming(rendezvous::Sender<Result<yamux::Stream, yamux::ConnectionError>>),
    Outgoing(rendezvous::Sender<Result<yamux::Stream, yamux::ConnectionError>>),
    // Using regular oneshot as we don't care about cancellation here:
    Close(Option<oneshot::Sender<Result<(), yamux::ConnectionError>>>),
}

fn connection_config() -> yamux::Config {
    yamux::Config::default()
}

// Real
#[cfg(not(feature = "simulation"))]
mod implementation {
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
    pub(super) struct TcpListener(tokio::net::TcpListener);

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
    pub(super) struct TcpStream(tokio::net::TcpStream);

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
    pub(super) use turmoil::net::{TcpListener, TcpStream};
}
