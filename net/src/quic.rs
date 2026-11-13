use crate::{KEEP_ALIVE_INTERVAL, SocketOptions};
use bytes::BytesMut;
use pin_project_lite::pin_project;
use quinn::{
    UdpPoller,
    crypto::rustls::QuicClientConfig,
    rustls::{
        self, DigitallySignedStruct, SignatureScheme,
        client::danger::{HandshakeSignatureValid, ServerCertVerified},
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    },
};
use std::{
    fmt,
    future::{self, Future, IntoFuture},
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
};
use tokio::sync::{
    Mutex as AsyncMutex,
    broadcast::{self, error::RecvError},
    watch,
};

const CERT_DOMAIN: &str = "ouisync.net";

// Quinn has a problem with GSO on some Android devices
// https://github.com/quinn-rs/quinn/issues/2246
// TODO: Remove this when the issue is resolved as it's likely slowing down the transmission.
#[cfg(target_os = "android")]
const DISABLE_SEGMENTATION_OFFLOAD: bool = true;
#[cfg(not(target_os = "android"))]
const DISABLE_SEGMENTATION_OFFLOAD: bool = false;

//------------------------------------------------------------------------------
pub struct Connector {
    endpoint: quinn::Endpoint,
    socket: Arc<CustomUdpSocket>,
}

impl Connector {
    pub fn connect(
        &self,
        remote_addr: SocketAddr,
    ) -> impl Future<Output = Result<Connection, Error>> + Send + 'static {
        let connect = self.endpoint.connect(remote_addr, CERT_DOMAIN);

        async move {
            connect?
                .await
                .map(|inner| Connection { inner })
                .map_err(Into::into)
        }
    }

    /// Forcefully closes all connections (any pending operation on any connection will
    /// immediately return error) and notifies the peers about it. The notifications are sent in
    /// the background, use [Self::wait_idle] to wait for them. The underlying UDP socket stays
    /// open (and keeps its port) until everything using it is dropped or [Self::hard_close] is
    /// called.
    pub fn graceful_close(&self) {
        self.endpoint.close(quinn::VarInt::from_u32(0), &[]);
    }

    /// Waits until all connections are closed and the peers have been notified about it (see
    /// [Self::graceful_close]).
    pub async fn wait_idle(&self) {
        self.endpoint.wait_idle().await
    }

    /// Closes all connections and the underlying UDP socket immediately, releasing its port even
    /// if something is still using it (any further operation on it will return error). Peers
    /// which haven't been notified by [Self::graceful_close] yet won't be notified.
    pub fn hard_close(&self) {
        self.graceful_close();
        self.socket.close();
    }

    /// Whether the underlying UDP socket has hit a fatal error (e.g. the network interface going
    /// down while the app is backgrounded) and stopped working. Once that happens, quinn treats
    /// the socket as unusable and never rebinds it, so the whole stack needs to be recreated (via
    /// a fresh call to [configure]) to recover.
    pub fn has_failed(&self) -> bool {
        *self.socket.failed_tx.borrow()
    }

    /// Completes when the underlying UDP socket has failed (see [Self::has_failed]). Closing the
    /// socket (see [Self::hard_close]) doesn't count as failing.
    pub fn on_failure(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut failed_rx = self.socket.failed_tx.subscribe();

        async move {
            if failed_rx.wait_for(|failed| *failed).await.is_err() {
                // The socket has been dropped without failing.
                future::pending().await
            }
        }
    }

    /// Simulates the underlying UDP socket hitting a fatal error, as if, e.g., the network
    /// interface went down. The socket stays open (and keeps its port), as it would in reality.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn kill(&self) {
        self.socket.kill();
    }
}

//------------------------------------------------------------------------------
pub struct Acceptor {
    endpoint: quinn::Endpoint,
    local_addr: SocketAddr,
}

impl Acceptor {
    pub async fn accept(&self) -> Result<Connecting, Error> {
        self.endpoint
            .accept()
            .await
            .map(|inner| Connecting { inner })
            .ok_or(Error::EndpointClosed)
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

pub struct Connecting {
    inner: quinn::Incoming,
}

impl Connecting {
    pub fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_address()
    }
}

impl IntoFuture for Connecting {
    type Output = Result<Connection, Error>;
    type IntoFuture = ConnectingFuture;

    fn into_future(self) -> Self::IntoFuture {
        ConnectingFuture {
            inner: self.inner.into_future(),
        }
    }
}

pub struct ConnectingFuture {
    inner: quinn::IncomingFuture,
}

impl Future for ConnectingFuture {
    type Output = Result<Connection, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner)
            .poll(cx)
            .map_ok(|inner| Connection { inner })
            .map_err(Into::into)
    }
}

pub struct Connection {
    inner: quinn::Connection,
}

impl Connection {
    pub fn remote_addr(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    pub async fn incoming(&self) -> Result<(SendStream, RecvStream), Error> {
        self.inner.accept_bi().await.map_err(Into::into)
    }

    pub async fn outgoing(&self) -> Result<(SendStream, RecvStream), Error> {
        self.inner.open_bi().await.map_err(Into::into)
    }

    pub fn close(&self) {
        self.inner.close(0u8.into(), &[]);
    }

    pub fn closed(&self) -> impl Future<Output = ()> + 'static {
        let inner = self.inner.clone();
        async move {
            inner.closed().await;
        }
    }
}

pub type SendStream = quinn::SendStream;
pub type RecvStream = quinn::RecvStream;

//------------------------------------------------------------------------------
pub fn configure(
    bind_addr: SocketAddr,
    options: SocketOptions,
) -> Result<(Connector, Acceptor, SideChannelMaker), Error> {
    let server_config = make_server_config()?;
    let custom_socket = Arc::new(CustomUdpSocket::bind(bind_addr, options)?);
    let side_channel_maker = custom_socket.clone().side_channel_maker();

    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config),
        custom_socket.clone(),
        Arc::new(quinn::TokioRuntime),
    )?;

    endpoint.set_default_client_config(make_client_config());

    let local_addr = endpoint.local_addr()?;

    let connector = Connector {
        endpoint: endpoint.clone(),
        socket: custom_socket,
    };
    let acceptor = Acceptor {
        endpoint,
        local_addr,
    };

    Ok((connector, acceptor, side_channel_maker))
}

//------------------------------------------------------------------------------
pub use quinn::{ConnectError, ConnectionError, WriteError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connect error")]
    Connect(#[from] ConnectError),
    #[error("connection error")]
    Connection(#[from] ConnectionError),
    #[error("endpoint closed")]
    EndpointClosed,
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("TLS error")]
    Tls(#[from] rustls::Error),
}

//------------------------------------------------------------------------------
// Dummy certificate verifier that treats any certificate as valid. In our P2P system there are no
// certification authorities. TODO: I think this still makes the TLS encryption provided by QUIC
// usefull against passive MitM attacks (eavesdropping), but not against the active ones.
#[derive(Debug)]
struct SkipServerVerification(rustls::crypto::CryptoProvider);

impl SkipServerVerification {
    fn new() -> Self {
        Self(rustls::crypto::ring::default_provider())
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn make_client_config() -> quinn::ClientConfig {
    let crypto_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification::new()))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        // `expect` should be OK because we made sure we constructed the rustls::ClientConfig in a
        // compliant way.
        QuicClientConfig::try_from(crypto_config).expect("failed to create quic client config"),
    ));

    let mut transport_config = quinn::TransportConfig::default();

    transport_config
        .max_concurrent_uni_streams(0_u8.into())
        // Documentation says that only one side needs to set the keep alive interval, chosing this
        // to be on the client side with the reasoning that the server side has a better chance of
        // being behind a non restrictive NAT, and so that sending the packets from the client side
        // shall assist in hole punching.
        .keep_alive_interval(Some(KEEP_ALIVE_INTERVAL))
        .max_idle_timeout((2 * KEEP_ALIVE_INTERVAL).try_into().ok());

    if DISABLE_SEGMENTATION_OFFLOAD {
        transport_config.enable_segmentation_offload(false);
    }

    client_config.transport_config(Arc::new(transport_config));
    client_config
}

fn make_server_config() -> Result<quinn::ServerConfig, Error> {
    // Generate a self signed certificate.
    let cert = rcgen::generate_simple_self_signed(vec![CERT_DOMAIN.into()]).unwrap();
    let cert_der = CertificateDer::from(cert.cert);
    let priv_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());

    let mut server_config =
        quinn::ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;

    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config
        .max_concurrent_uni_streams(0_u8.into())
        .max_idle_timeout((2 * KEEP_ALIVE_INTERVAL).try_into().ok());

    if DISABLE_SEGMENTATION_OFFLOAD {
        transport_config.enable_segmentation_offload(false);
    }

    Ok(server_config)
}

//------------------------------------------------------------------------------
use futures_util::{ready, task::AtomicWaker};
use tokio::io::Interest;

use crate::udp::DatagramSocket;

// TODO: I saw this number mentioned somewhere in quinn as a standard MTU size rounded to 8 bytes.
// I believe it's also more than is needed for BtDHT, but more rigorous definition of this number
// would be welcome.
const MAX_SIDE_CHANNEL_PACKET_SIZE: usize = 1480;
// TODO: Another made up constant. Can/should it be smaller?
const MAX_SIDE_CHANNEL_PENDING_PACKETS: usize = 1024;

#[derive(Clone)]
struct Packet {
    data: [u8; MAX_SIDE_CHANNEL_PACKET_SIZE],
    len: usize,
    from: SocketAddr,
}

#[derive(Debug)]
struct CustomUdpSocket {
    // `None` once closed (see `close`). It's in `Arc` so the async operations don't need to hold
    // the lock across `await`.
    io: RwLock<Option<Arc<tokio::net::UdpSocket>>>,
    local_addr: SocketAddr,
    state: quinn::udp::UdpSocketState,
    side_channel_tx: broadcast::Sender<Packet>,
    // Set to `true` when the socket hits a fatal error.
    failed_tx: watch::Sender<bool>,
    // Waker of the last `poll_recv` call. When the socket is dropped, tokio clears the wakers
    // registered on it without waking them, so `close` (and `kill`) wake this one explicitly to
    // not leave quinn's endpoint driver waiting forever.
    recv_waker: AtomicWaker,
    #[cfg(any(test, feature = "test-utils"))]
    killed: std::sync::atomic::AtomicBool,
}

impl CustomUdpSocket {
    fn bind(addr: SocketAddr, options: SocketOptions) -> io::Result<Self> {
        let socket = crate::udp::UdpSocket::bind_with_options(addr, options)?;
        let socket = socket.into_std()?;

        #[cfg(windows)]
        disable_icmp_errors(&socket);

        let state = quinn::udp::UdpSocketState::new((&socket).into())?;
        let local_addr = socket.local_addr()?;

        Ok(Self {
            io: RwLock::new(Some(Arc::new(tokio::net::UdpSocket::from_std(socket)?))),
            local_addr,
            state,
            side_channel_tx: broadcast::channel(MAX_SIDE_CHANNEL_PENDING_PACKETS).0,
            failed_tx: watch::Sender::new(false),
            recv_waker: AtomicWaker::new(),
            #[cfg(any(test, feature = "test-utils"))]
            killed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn side_channel_maker(self: Arc<Self>) -> SideChannelMaker {
        SideChannelMaker { socket: self }
    }

    /// Returns the underlying socket, or error if it's been closed.
    fn io(&self) -> io::Result<Arc<tokio::net::UdpSocket>> {
        self.io.read().unwrap().clone().ok_or_else(closed_error)
    }

    /// Closes the underlying socket, releasing its port. The socket is dropped immediately, except
    /// when an async operation (e.g. `send_to`) is in progress, in which case it's dropped once
    /// that completes (normally right away).
    fn close(&self) {
        self.io.write().unwrap().take();
        self.recv_waker.wake();
    }

    fn mark_failed(&self) {
        self.failed_tx.send_replace(true);
    }

    #[cfg(any(test, feature = "test-utils"))]
    fn kill(&self) {
        self.killed
            .store(true, std::sync::atomic::Ordering::Release);
        self.recv_waker.wake();
    }
}

fn closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "socket closed")
}

impl quinn::AsyncUdpSocket for CustomUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(UdpPollHelper::new(move || {
            let io = self.io();
            async move { io?.writable().await }
        }))
    }

    fn try_send(&self, transmit: &quinn::udp::Transmit) -> io::Result<()> {
        let io = self.io.read().unwrap();
        let io = io.as_ref().ok_or_else(closed_error)?;

        io.try_io(Interest::WRITABLE, || {
            self.state.send(io.as_ref().into(), transmit)
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [std::io::IoSliceMut<'_>],
        metas: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.recv_waker.register(cx.waker());

        #[cfg(any(test, feature = "test-utils"))]
        if self.killed.load(std::sync::atomic::Ordering::Acquire) {
            self.mark_failed();
            return Poll::Ready(Err(io::Error::other("socket killed")));
        }

        let io = self.io.read().unwrap();
        let Some(io) = io.as_ref() else {
            // The socket has been closed on purpose (see `close`). Don't return error, otherwise
            // quinn's endpoint driver would log it as an I/O error. It terminates on its own once
            // the endpoint is dropped.
            return Poll::Pending;
        };

        let mut recoverable_errors = 0;

        loop {
            if let Err(e) = ready!(io.poll_recv_ready(cx)) {
                self.mark_failed();
                return Poll::Ready(Err(e));
            }

            match io.try_io(Interest::READABLE, || {
                let res = self.state.recv(io.as_ref().into(), bufs, metas);

                if let Ok(msg_count) = res {
                    send_to_side_channels(&self.side_channel_tx, bufs, metas, msg_count);
                }

                res
            }) {
                Ok(res) => return Poll::Ready(Ok(res)),
                // `WouldBlock` means the readiness we just observed was stale (the socket isn't
                // actually readable yet) - `try_io` already cleared it, so looping back to
                // `poll_recv_ready` is correct: it'll wait for the next real readiness event.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                // Recoverable errors are reported only once (the `recv` call which returns them
                // also clears them) so retrying won't spin. Still, cap the number of retries in
                // case some platform keeps returning one of them, to not spin forever.
                Err(e)
                    if is_recoverable_recv_error(&e)
                        && recoverable_errors < MAX_CONSECUTIVE_RECOVERABLE_RECV_ERRORS =>
                {
                    tracing::debug!(error = ?e, "recoverable QUIC socket recv error");
                    recoverable_errors += 1;
                    continue;
                }
                // Any other error (e.g. the network interface going down while the app is
                // backgrounded) is a real failure, not a spurious wakeup. `try_io` doesn't clear
                // readiness for these, so looping back to `poll_recv_ready` would immediately
                // observe the socket as "ready" again and re-enter `recv` synchronously,
                // spinning this task at 100% CPU forever instead of ever yielding. Propagate it
                // instead so the caller can handle/recreate the connection. Mark the socket as
                // failed so the owner knows this endpoint needs to be rebuilt - quinn never does
                // this on its own once its driver sees a fatal error.
                Err(e) => {
                    tracing::warn!(error = ?e, "fatal QUIC socket recv error");
                    self.mark_failed();
                    return Poll::Ready(Err(e));
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        Ok(self.local_addr)
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }

    fn max_transmit_segments(&self) -> usize {
        self.state.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }
}

fn send_to_side_channels(
    channel: &broadcast::Sender<Packet>,
    bufs: &[std::io::IoSliceMut<'_>],
    metas: &[quinn::udp::RecvMeta],
    msg_count: usize,
) {
    for (meta, buf) in metas.iter().zip(bufs.iter()).take(msg_count) {
        let mut data: BytesMut = buf[0..meta.len].into();
        while !data.is_empty() {
            let src = data.split_to(meta.stride.min(data.len()));
            let mut dst = [0; MAX_SIDE_CHANNEL_PACKET_SIZE];
            let len = src.len().min(dst.len());

            dst[..len].copy_from_slice(&src[..len]);

            channel
                .send(Packet {
                    data: dst,
                    len,
                    from: meta.addr,
                })
                .unwrap_or(0);
        }
    }
}

// This is copied verbatim from [quinn]
// (https://github.com/quinn-rs/quinn/blob/main/quinn/src/runtime.rs) as it's unfortunatelly not
// exposed from there.
pin_project! {
    struct UdpPollHelper<MakeFut, Fut> {
        make_fut: MakeFut,
        #[pin]
        fut: Option<Fut>,
    }
}

impl<MakeFut, Fut> UdpPollHelper<MakeFut, Fut> {
    fn new(make_fut: MakeFut) -> Self {
        Self {
            make_fut,
            fut: None,
        }
    }
}

impl<MakeFut, Fut> UdpPoller for UdpPollHelper<MakeFut, Fut>
where
    MakeFut: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = io::Result<()>> + Send + Sync + 'static,
{
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let mut this = self.project();

        if this.fut.is_none() {
            this.fut.set(Some((this.make_fut)()));
        }

        let result = this.fut.as_mut().as_pin_mut().unwrap().poll(cx);

        if result.is_ready() {
            this.fut.set(None);
        }

        result
    }
}

impl<MakeFut, Fut> fmt::Debug for UdpPollHelper<MakeFut, Fut> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpPollHelper").finish_non_exhaustive()
    }
}
//------------------------------------------------------------------------------

/// Makes new `SideChannel`s.
#[derive(Clone)]
pub struct SideChannelMaker {
    socket: Arc<CustomUdpSocket>,
}

impl SideChannelMaker {
    pub fn make(&self) -> SideChannel {
        SideChannel {
            socket: self.socket.clone(),
            packet_rx: AsyncMutex::new(self.socket.side_channel_tx.subscribe()),
        }
    }
}

/// Side channel for the underlying UDP socket which allows sending and receiving raw UDP
/// datagrams.
pub struct SideChannel {
    socket: Arc<CustomUdpSocket>,
    packet_rx: AsyncMutex<broadcast::Receiver<Packet>>,
}

impl SideChannel {
    pub fn into_sender(self) -> SideChannelSender {
        SideChannelSender {
            socket: self.socket,
        }
    }
}

impl DatagramSocket for SideChannel {
    async fn send_to<'a>(&'a self, buf: &'a [u8], target: SocketAddr) -> io::Result<usize> {
        self.socket.io()?.send_to(buf, target).await
    }

    // Note: receiving on side channels will only work when quinn is calling `poll_recv`.  This
    // normally shouldn't be a problem because by default we'll always be accepting new connections
    // on the `Acceptor`, but should be kept in mind if we decide to disable QUIC for some reason.
    async fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let packet = loop {
            match self.packet_rx.lock().await.recv().await {
                Ok(packet) => break packet,
                Err(RecvError::Lagged(_)) => {
                    // We missed one or more packets due to channel overflow. This is ok as this is
                    // an unreliable socket anyway. Let's try again.
                    continue;
                }
                Err(RecvError::Closed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "side channel closed",
                    ));
                }
            }
        };

        let len = packet.len.min(buf.len());
        buf[..len].copy_from_slice(&packet.data[..len]);

        Ok((len, packet.from))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.socket.local_addr)
    }
}

// `SideChannelSender` is less expensive than the `SideChannel` because there is no additional
// `broadcast::Receiver` that the `CustomUdpSocket` would need to pass messages to.
#[derive(Clone)]
pub struct SideChannelSender {
    socket: Arc<CustomUdpSocket>,
}

impl SideChannelSender {
    pub async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.socket.io()?.send_to(buf, target).await.map(|_| ())
    }
}

/// Max number of recoverable errors `CustomUdpSocket::poll_recv` retries in a row before treating
/// the error as fatal.
const MAX_CONSECUTIVE_RECOVERABLE_RECV_ERRORS: usize = 32;

/// Whether the error returned from receiving on the UDP socket is recoverable, that is, it doesn't
/// mean the socket stopped working.
///
/// TODO: Temporary lack of memory / buffer space in the kernel (`ENOMEM`, `ENOBUFS`, `WSAENOBUFS`)
/// is in fact recoverable too. But unlike the errors below it's a condition which lasts for some
/// time, not a one-off event, so retrying immediately would just hit the retry limit. Handling it
/// properly would require backing off with a timer which would complicate the code, and these
/// errors are rare on recv, so they are treated as fatal for now.
fn is_recoverable_recv_error(error: &io::Error) -> bool {
    match error.kind() {
        // ICMP errors caused by previously sent packets (e.g., "port unreachable" when the peer
        // went offline, reported as `WSAECONNRESET` on Windows).
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable => true,
        // ICMP "TTL expired" caused by a previously sent packet.
        #[cfg(windows)]
        _ if error.raw_os_error()
            == Some(windows_sys::Win32::Networking::WinSock::WSAENETRESET) =>
        {
            true
        }
        _ => false,
    }
}

/// By default, Windows reports ICMP "port unreachable" and "TTL expired" caused by a previously
/// sent packet as `WSAECONNRESET` / `WSAENETRESET` on the next recv. These are meaningless for
/// QUIC (and common in P2P, e.g. when a peer goes offline), so disable them.
#[cfg(windows)]
fn disable_icmp_errors(socket: &std::net::UdpSocket) {
    use std::{os::windows::io::AsRawSocket, ptr};
    use windows_sys::Win32::Networking::WinSock::{
        SIO_UDP_CONNRESET, SIO_UDP_NETRESET, SOCKET_ERROR, WSAIoctl,
    };

    for (name, code) in [
        ("SIO_UDP_CONNRESET", SIO_UDP_CONNRESET),
        ("SIO_UDP_NETRESET", SIO_UDP_NETRESET),
    ] {
        let enable: u32 = 0;
        let mut bytes_returned: u32 = 0;

        // SAFETY: The socket is valid for the duration of the call and the pointers point to
        // properly sized and aligned local variables. No overlapped I/O is used.
        let res = unsafe {
            WSAIoctl(
                socket.as_raw_socket() as _,
                code,
                (&raw const enable).cast(),
                size_of_val(&enable) as u32,
                ptr::null_mut(),
                0,
                &mut bytes_returned,
                ptr::null_mut(),
                None,
            )
        };

        // Ignore errors - the recoverable ones are handled in `CustomUdpSocket::poll_recv` anyway.
        if res == SOCKET_ERROR {
            tracing::warn!(error = ?io::Error::last_os_error(), "failed to disable {name}");
        }
    }
}

//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future;
    use std::{net::Ipv4Addr, time::Duration};
    use tokio::task;

    // Sending to a closed port triggers an ICMP "port unreachable" which some platforms (e.g.,
    // Windows) report as an error on the next recv. That must not kill the endpoint.
    #[tokio::test(flavor = "multi_thread")]
    async fn send_to_closed_port() {
        let (connector, _acceptor, _) =
            configure((Ipv4Addr::LOCALHOST, 0).into(), Default::default()).unwrap();

        let closed_addr = {
            let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            socket.local_addr().unwrap()
        };

        // The handshake never completes, so don't wait for it. Just give it some time to send the
        // initial packets and receive the ICMP responses.
        let connect = task::spawn(connector.connect(closed_addr));
        tokio::time::sleep(Duration::from_millis(500)).await;
        connect.abort();

        assert!(!connector.has_failed());

        // The endpoint still works.
        let (_connector, acceptor, _) =
            configure((Ipv4Addr::LOCALHOST, 0).into(), Default::default()).unwrap();
        let addr = *acceptor.local_addr();

        task::spawn(async move {
            let _connection = acceptor.accept().await.unwrap().await.unwrap();
            future::pending::<()>().await;
        });

        connector.connect(addr).await.unwrap();
        assert!(!connector.has_failed());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn strict_port() {
        let taken = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = taken.local_addr().unwrap();

        assert!(configure(addr, SocketOptions::default().with_strict_port()).is_err());

        // Without strict port, falls back to a random port.
        let (_connector, acceptor, _) = configure(addr, SocketOptions::default()).unwrap();
        assert_ne!(acceptor.local_addr().port(), addr.port());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hard_close_releases_port() {
        let (connector, acceptor, side_channel_maker) =
            configure((Ipv4Addr::LOCALHOST, 0).into(), Default::default()).unwrap();
        let addr = *acceptor.local_addr();

        // Keep some references to the socket alive.
        let side_channel = side_channel_maker.make();

        connector.hard_close();

        std::net::UdpSocket::bind(addr).unwrap();

        assert!(side_channel.send_to(b"hello", addr).await.is_err());

        // Closing doesn't count as failing.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!connector.has_failed());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill() {
        let (connector, acceptor, _) =
            configure((Ipv4Addr::LOCALHOST, 0).into(), Default::default()).unwrap();
        let addr = *acceptor.local_addr();

        assert!(!connector.has_failed());

        let on_failure = connector.on_failure();
        connector.kill();

        tokio::time::timeout(Duration::from_secs(5), on_failure)
            .await
            .unwrap();
        assert!(connector.has_failed());

        // The socket keeps its port.
        assert!(std::net::UdpSocket::bind(addr).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn side_channel() {
        let (_connector, acceptor, side_channel_maker) =
            configure((Ipv4Addr::LOCALHOST, 0).into(), Default::default()).unwrap();
        let addr = *acceptor.local_addr();
        let side_channel = side_channel_maker.make();

        // We must ensure quinn polls on the socket for side channel to be able to receive data.
        task::spawn(async move {
            let _connection = acceptor.accept().await.unwrap();
        });

        const MSG: &[u8; 18] = b"hello side channel";

        task::spawn(async move {
            let socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            socket.send_to(MSG, addr).await.unwrap();
        });

        let mut buf = [0; MAX_SIDE_CHANNEL_PACKET_SIZE];
        let (len, _from) = side_channel.recv_from(&mut buf).await.unwrap();

        assert_eq!(len, MSG.len());
        assert_eq!(buf[..len], MSG[..]);
    }
}
