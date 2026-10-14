use crate::{KEEP_ALIVE_INTERVAL, SocketOptions};
use bytes::BytesMut;
use pin_project_lite::pin_project;
use quinn::{
    UdpSender, crypto::rustls::QuicClientConfig, rustls::{
        self, DigitallySignedStruct, SignatureScheme,
        client::danger::{HandshakeSignatureValid, ServerCertVerified},
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    }, udp::Transmit,
};
use std::{
    fmt,
    future::{Future, IntoFuture},
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{self, Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::{
    Mutex as AsyncMutex,
    broadcast::{self, error::RecvError},
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

    // forcefully close all connections (any pending operation on any connection will immediatelly
    // return error)
    pub fn close(&self) {
        self.endpoint.close(quinn::VarInt::from_u32(0), &[]);
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
    let custom_socket = CustomUdpSocket::bind(bind_addr, options)?;
    let side_channel_maker = Arc::new(custom_socket.clone()).side_channel_maker();

    let endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config),
        Box::new(custom_socket),
        Arc::new(quinn::TokioRuntime),
    )?;

    endpoint.set_default_client_config(make_client_config());

    let local_addr = endpoint.local_addr()?;

    let connector = Connector {
        endpoint: endpoint.clone(),
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
use futures_util::ready;
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

#[derive(Debug, Clone)]
struct CustomUdpSocket {
    io: Arc<tokio::net::UdpSocket>,
    state: Arc<quinn::udp::UdpSocketState>,
    side_channel_tx: broadcast::Sender<Packet>,
}

impl CustomUdpSocket {
    fn bind(addr: SocketAddr, options: SocketOptions) -> io::Result<Self> {
        let socket = crate::udp::UdpSocket::bind_with_options(addr, options)?;
        let socket = socket.into_std()?;

        let state = quinn::udp::UdpSocketState::new((&socket).into())?;

        Ok(Self {
            io: Arc::new(tokio::net::UdpSocket::from_std(socket)?),
            state: Arc::new(state),
            side_channel_tx: broadcast::channel(MAX_SIDE_CHANNEL_PENDING_PACKETS).0,
        })
    }

    fn side_channel_maker(self: Arc<Self>) -> SideChannelMaker {
        SideChannelMaker { socket: self }
    }
}

impl quinn::AsyncUdpSocket for CustomUdpSocket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(UdpSenderHelper::new(
            self.clone(),
            |socket: &CustomUdpSocket| {
                let socket = socket.clone();
                async move { socket.io.writable().await }
            },
        ))
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [std::io::IoSliceMut<'_>],
        metas: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            match self.io.try_io(Interest::READABLE, || {
                let res = self.state.recv((&*self.io).into(), bufs, metas);

                if let Ok(msg_count) = res {
                    send_to_side_channels(&self.side_channel_tx, bufs, metas, msg_count);
                }

                res
            }) {
                Ok(res) => return Poll::Ready(Ok(res)),
                // `try_io` clears readiness only for `WouldBlock`. Looping on any other
                // error would spin because readiness stays asserted and the receive fails again.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.io.local_addr()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
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

// This is copied from
// [quinn](https://github.com/quinn-rs/quinn/blob/main/quinn/src/runtime/mod.rs)
// as it's not exposed from there.
pin_project! {
    /// A helper for constructing [`UdpSender`]s from an underlying `Socket` type.
    ///
    /// This struct implements [`UdpSender`] if `MakeWritableFut` produces a `WritableFut`.
    ///
    /// Also serves as a trick, since `WritableFut` doesn't need to be a named future,
    /// it can be an anonymous async block, as long as `MakeWritableFut` produces that
    /// anonymous async block type.
    ///
    /// The `UdpSenderHelper` generic type parameters don't need to named, as it will be
    /// used in its dyn-compatible form as a `Pin<Box<dyn UdpSender>>`.
    struct UdpSenderHelper<MakeWritableFutFn, WritableFut> {
        socket: CustomUdpSocket,
        last_send_error: Option<Instant>,
        make_writable_fut_fn: MakeWritableFutFn,
        #[pin]
        writable_fut: Option<WritableFut>,
    }
}

impl<MakeWritableFutFn, WritableFut> UdpSenderHelper<MakeWritableFutFn, WritableFut> {
    /// Create helper that implements [`UdpSender`] from a socket.
    ///
    /// Additionally you need to provide what is essentially an async function
    /// that resolves once the socket is write-ready.
    ///
    /// See also the bounds on this struct's [`UdpSender`] implementation.
    fn new(inner: CustomUdpSocket, make_fut: MakeWritableFutFn) -> Self {
        Self {
            socket: inner,
            last_send_error: None,
            make_writable_fut_fn: make_fut,
            writable_fut: None,
        }
    }
}

impl<MakeWritableFutFn, WritableFut> UdpSender for UdpSenderHelper<MakeWritableFutFn, WritableFut>
where
    MakeWritableFutFn: Fn(&CustomUdpSocket) -> WritableFut + Send + Sync + 'static,
    WritableFut: Future<Output = io::Result<()>> + Send + Sync + 'static,
{
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();
        loop {
            if this.writable_fut.is_none() {
                this.writable_fut
                    .set(Some((this.make_writable_fut_fn)(this.socket)));
            }
            // We're forced to `unwrap` here because `Fut` may be `!Unpin`, which means we can't safely
            // obtain an `&mut WritableFut` after storing it in `self.writable_fut` when `self` is already behind `Pin`,
            // and if we didn't store it then we wouldn't be able to keep it alive between
            // `poll_send` calls.
            let result = task::ready!(this.writable_fut.as_mut().as_pin_mut().unwrap().poll(cx));

            // Polling an arbitrary `Future` after it becomes ready is a logic error, so arrange for
            // a new `Future` to be created on the next call.
            this.writable_fut.set(None);

            // If .writable() fails, propagate the error
            result?;

            match this.socket.try_send(transmit) {
                // We thought the socket was writable, but it wasn't, then retry so that either another
                // `writable().await` call determines that the socket is indeed not writable and
                // registers us for a wakeup, or the send succeeds if this really was just a
                // transient failure.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => {
                    log_sendmsg_error(this.last_send_error, &e, transmit);
                    return Poll::Ready(Ok(()));
                }
                Ok(()) => return Poll::Ready(Ok(())),
            }
        }
    }

    fn max_transmit_segments(&self) -> usize {
        self.socket.max_transmit_segments()
    }
}

impl<MakeWritableFutFn, WritableFut> fmt::Debug
    for UdpSenderHelper<MakeWritableFutFn, WritableFut>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UdpSender")
    }
}

impl CustomUdpSocket {
    fn try_send(&self, transmit: &quinn::udp::Transmit<'_>) -> io::Result<()> {
        self.io.try_io(Interest::WRITABLE, || {
            self.state.try_send((&*self.io).into(), transmit)
        })
    }

    fn max_transmit_segments(&self) -> usize {
        self.state.max_gso_segments()
    }
}

/// Limits I/O error logging to one message per minute.
const IO_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);

fn log_sendmsg_error(
    last_send_error: &mut Option<Instant>,
    error: &io::Error,
    transmit: &quinn::udp::Transmit<'_>,
) {
    #[cfg(unix)]
    // Unix `EMSGSIZE` is expected for MTU probes.
    if quinn::udp::is_msg_size_err(error) {
        return;
    }

    let now = Instant::now();
    if last_send_error
        .is_some_and(|last| now.saturating_duration_since(last) <= IO_ERROR_LOG_INTERVAL)
    {
        return;
    }
    *last_send_error = Some(now);

    tracing::warn!(
        "sendmsg error: {:?}, Transmit: {{ destination: {:?}, src_ip: {:?}, ecn: {:?}, len: {:?}, segment_size: {:?} }}",
        error,
        transmit.destination,
        transmit.src_ip,
        transmit.ecn,
        transmit.contents.len(),
        transmit.segment_size
    );
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
        self.socket.io.send_to(buf, target).await
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
        self.socket.io.local_addr()
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
        self.socket.io.send_to(buf, target).await.map(|_| ())
    }
}

//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::task;

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
