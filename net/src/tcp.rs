use self::implementation::{TcpListener, TcpStream};
use crate::{sync::rendezvous, SocketOptions};
use std::{
    future::{self, Future},
    io,
    net::SocketAddr,
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    select,
    sync::{mpsc, oneshot},
    task,
};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::{Instrument, Span};

/// Configure TCP endpoint
pub fn configure(
    bind_addr: SocketAddr,
    options: SocketOptions,
) -> Result<(Connector, Acceptor), Error> {
    let listener = TcpListener::bind(bind_addr, options)?;
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
    // Using explicit return type instead of `async fn` to ensure it's `'static`.
    #[expect(clippy::manual_async_fn)]
    pub fn connect(
        &self,
        addr: SocketAddr,
    ) -> impl Future<Output = Result<Connection, Error>> + Send + 'static {
        async move {
            let stream = TcpStream::connect(addr).await?;
            Ok(Connection::new(stream, yamux::Mode::Client, addr))
        }
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
            remote_addr,
            command_tx,
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
    pub async fn close(&self) {
        let (reply_tx, reply_rx) = oneshot::channel();

        if self
            .command_tx
            .send(Command::Close(Some(reply_tx)))
            .await
            .is_err()
        {
            return;
        }

        match reply_rx.await {
            Ok(Ok(())) | Err(_) => (),
            Ok(Err(error)) => tracing::debug!(?error, "failed to close connection"),
        }
    }

    /// Waits for the connection to be closed.
    pub fn closed(&self) -> impl Future<Output = ()> + 'static {
        let command_tx = self.command_tx.clone();
        async move { command_tx.closed().await }
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
    // Incoming streams are being polled continuously and placed here, then taken from here one by
    // one on the next call to `Connection::incoming`. Note that yamux has a limit on the number of
    // simultaneously open streams per connection which effectively puts a bound on this collection
    // as well.
    let mut incoming_results = Vec::<Result<_, _>>::new();
    let mut incoming_senders = Vec::<rendezvous::Sender<_>>::new();

    // If an `Connection::outgoing` calls gets cancelled after the outgoing stream's been already
    // created, we store the stream here and use it next time `Connection::outgoing` is called.
    // This ensures no outgoing stream gets lost and thus makes `outgoing` cancel safe.
    let mut outgoing_result = None;

    loop {
        while let Some(tx) = incoming_senders.pop() {
            if let Some(result) = incoming_results.pop() {
                if let Err(result) = tx.send(result).await {
                    incoming_results.push(result);
                }
            } else {
                incoming_senders.push(tx);
                break;
            }
        }

        let command = select! {
            command = command_rx.recv() => command,
            result = incoming(&mut conn) => {
                if let Some(result) = result {
                    // Store at most one error
                    if result.is_err() {
                        incoming_results.retain(|result| result.is_ok());
                    }

                    incoming_results.push(result);

                    continue;
                } else {
                    // Connection closed by the peer
                    break;
                }
            }
        };

        match command.unwrap_or(Command::Close(None)) {
            Command::Incoming(reply_tx) => {
                incoming_senders.push(reply_tx);
            }
            Command::Outgoing(reply_tx) => {
                let result = if let Some(result) = outgoing_result.take() {
                    result
                } else {
                    select! {
                        result = outgoing(&mut conn) => result,
                        _ = reply_tx.closed() => continue,
                    }
                };

                if let Err(result) = reply_tx.send(result).await {
                    outgoing_result = Some(result);
                }
            }
            Command::Close(reply_tx) => {
                let result = close(&mut conn).await;

                if let Some(reply_tx) = reply_tx {
                    reply_tx.send(result).ok();
                }

                break;
            }
        }
    }
}

async fn incoming(
    conn: &mut yamux::Connection<Compat<TcpStream>>,
) -> Option<Result<yamux::Stream, yamux::ConnectionError>> {
    future::poll_fn(|cx| conn.poll_next_inbound(cx)).await
}

async fn outgoing(
    conn: &mut yamux::Connection<Compat<TcpStream>>,
) -> Result<yamux::Stream, yamux::ConnectionError> {
    future::poll_fn(|cx| conn.poll_new_outbound(cx)).await
}

async fn close(
    conn: &mut yamux::Connection<Compat<TcpStream>>,
) -> Result<(), yamux::ConnectionError> {
    future::poll_fn(|cx| conn.poll_close(cx)).await
}

enum Command {
    // Using rendezvous to guarantee the reply is either received or we get it back if the receive
    // got cancelled.
    Incoming(rendezvous::Sender<Result<yamux::Stream, yamux::ConnectionError>>),
    Outgoing(rendezvous::Sender<Result<yamux::Stream, yamux::ConnectionError>>),
    // Using regular oneshot as we don't care about cancellation here
    Close(Option<oneshot::Sender<Result<(), yamux::ConnectionError>>>),
}

fn connection_config() -> yamux::Config {
    yamux::Config::default()
}

// Real
#[cfg(not(feature = "simulation"))]
mod implementation {
    use crate::{socket, SocketOptions, KEEP_ALIVE_INTERVAL};
    use socket2::{Domain, Socket, TcpKeepalive, Type};
    use std::{
        fmt, io,
        net::SocketAddr,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    /// TCP listener
    pub(super) struct TcpListener(tokio::net::TcpListener);

    impl TcpListener {
        /// Configures a TCP socket with the given options and binds it to the given address. If the
        /// port is taken, uses a random one,
        pub fn bind(addr: SocketAddr, options: SocketOptions) -> io::Result<Self> {
            let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
            socket.set_nonblocking(true)?;

            if options.reuse_addr {
                // Ignore errors - reuse address is nice to have but not required.
                socket.set_reuse_address(true).ok();
            }

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

    impl fmt::Debug for TcpStream {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:?}", self.0)
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
