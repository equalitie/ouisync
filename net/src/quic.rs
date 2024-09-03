use crate::KEEP_ALIVE_INTERVAL;
use bytes::BytesMut;
use pin_project_lite::pin_project;
use quinn::{
    crypto::rustls::QuicClientConfig,
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified},
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
        DigitallySignedStruct, SignatureScheme,
    },
    UdpPoller,
};
use std::{
    fmt,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{
        broadcast::{self, error::RecvError},
        Mutex as AsyncMutex,
    },
};

const CERT_DOMAIN: &str = "ouisync.net";

//------------------------------------------------------------------------------
pub struct Connector {
    endpoint: quinn::Endpoint,
}

impl Connector {
    pub async fn connect(&self, remote_addr: SocketAddr) -> Result<Connection, Error> {
        let connection = self.endpoint.connect(remote_addr, CERT_DOMAIN)?.await?;
        let (tx, rx) = connection.open_bi().await?;
        Ok(Connection::new(rx, tx, connection.remote_address()))
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
    pub async fn accept(&self) -> Option<Connecting> {
        self.endpoint
            .accept()
            .await
            .map(|incoming| Connecting { incoming })
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

pub struct Connecting {
    incoming: quinn::Incoming,
}

impl Connecting {
    pub fn remote_addr(&self) -> SocketAddr {
        self.incoming.remote_address()
    }

    pub async fn complete(self) -> Result<Connection, Error> {
        let connection = self.incoming.await?;
        let (tx, rx) = connection.accept_bi().await?;
        Ok(Connection::new(rx, tx, connection.remote_address()))
    }
}

//------------------------------------------------------------------------------
pub struct Connection {
    reader: quinn::RecvStream,
    writer: quinn::SendStream,
    remote_addr: SocketAddr,
}

impl Connection {
    pub fn new(
        reader: quinn::RecvStream,
        writer: quinn::SendStream,
        remote_addr: SocketAddr,
    ) -> Self {
        Self {
            reader,
            writer,
            remote_addr,
        }
    }

    pub fn remote_addr(&self) -> &SocketAddr {
        &self.remote_addr
    }

    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        (
            OwnedReadHalf { inner: self.reader },
            OwnedWriteHalf { inner: self.writer },
        )
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.reader), cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.writer), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.writer), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.writer), cx)
    }
}

//------------------------------------------------------------------------------
pub struct OwnedReadHalf {
    inner: quinn::RecvStream,
}

pub struct OwnedWriteHalf {
    inner: quinn::SendStream,
}

impl AsyncRead for OwnedReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.inner), cx, buf)
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.inner), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.inner), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.inner), cx)
    }
}

//------------------------------------------------------------------------------
pub fn configure(bind_addr: SocketAddr) -> Result<(Connector, Acceptor, SideChannelMaker), Error> {
    let server_config = make_server_config()?;
    let custom_socket = Arc::new(CustomUdpSocket::bind(bind_addr)?);
    let side_channel_maker = custom_socket.clone().side_channel_maker();

    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config),
        custom_socket,
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
    #[error("write error")]
    Write(#[from] WriteError),
    #[error("done accepting error")]
    DoneAccepting,
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

    client_config.transport_config(Arc::new(transport_config));
    client_config
}

fn make_server_config() -> Result<quinn::ServerConfig, Error> {
    // Generate a self signed certificate.
    let cert = rcgen::generate_simple_self_signed(vec![CERT_DOMAIN.into()]).unwrap();
    let cert_der = CertificateDer::from(cert.cert);
    let priv_key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let mut server_config =
        quinn::ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;

    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config
        .max_concurrent_uni_streams(0_u8.into())
        .max_idle_timeout((2 * KEEP_ALIVE_INTERVAL).try_into().ok());

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

#[derive(Debug)]
struct CustomUdpSocket {
    io: tokio::net::UdpSocket,
    state: quinn::udp::UdpSocketState,
    side_channel_tx: broadcast::Sender<Packet>,
}

impl CustomUdpSocket {
    fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = crate::udp::UdpSocket::bind(addr)?;
        let socket = socket.into_std()?;

        let state = quinn::udp::UdpSocketState::new((&socket).into())?;

        Ok(Self {
            io: tokio::net::UdpSocket::from_std(socket)?,
            state,
            side_channel_tx: broadcast::channel(MAX_SIDE_CHANNEL_PENDING_PACKETS).0,
        })
    }

    fn side_channel_maker(self: Arc<Self>) -> SideChannelMaker {
        SideChannelMaker { socket: self }
    }
}

impl quinn::AsyncUdpSocket for CustomUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(UdpPollHelper::new(move || {
            let socket = self.clone();
            async move { socket.io.writable().await }
        }))
    }

    fn try_send(&self, transmit: &quinn::udp::Transmit) -> io::Result<()> {
        self.io.try_io(Interest::WRITABLE, || {
            self.state.send((&self.io).into(), transmit)
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [std::io::IoSliceMut<'_>],
        metas: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            if let Ok(res) = self.io.try_io(Interest::READABLE, || {
                let res = self.state.recv((&self.io).into(), bufs, metas);

                if let Ok(msg_count) = res {
                    send_to_side_channels(&self.side_channel_tx, bufs, metas, msg_count);
                }

                res
            }) {
                return Poll::Ready(Ok(res));
            }
        }
    }

    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.io.local_addr()
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

pub struct SideChannel {
    socket: Arc<CustomUdpSocket>,
    packet_rx: AsyncMutex<broadcast::Receiver<Packet>>,
}

impl SideChannel {
    pub fn sender(&self) -> SideChannelSender {
        SideChannelSender {
            socket: self.socket.clone(),
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
                    ))
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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        task,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn small_data_exchange() {
        let (connector, acceptor, _) = configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();

        let addr = *acceptor.local_addr();

        let h1 = task::spawn(async move {
            let mut conn = acceptor.accept().await.unwrap().complete().await.unwrap();

            let mut buf = [0; 4];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");

            conn.write_all(b"pong").await.unwrap();
        });

        let h2 = task::spawn(async move {
            let mut conn = connector.connect(addr).await.unwrap();
            conn.write_all(b"ping").await.unwrap();

            let mut buf = [0; 4];
            match conn.read_exact(&mut buf).await {
                Ok(_) => (),
                Err(error) => match error.downcast::<quinn::ReadError>() {
                    Ok(error) => match error {
                        quinn::ReadError::ConnectionLost(
                            quinn::ConnectionError::ApplicationClosed(_),
                        ) => {
                            // connection gracefully closed by the peer, this is expected.
                        }
                        error => panic!("unexpected error: {:?}", error),
                    },
                    Err(error) => panic!("unexpected error: {:?}", error),
                },
            }
        });

        h1.await.unwrap();
        h2.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn side_channel() {
        let (_connector, acceptor, side_channel_maker) =
            configure((Ipv4Addr::LOCALHOST, 0).into()).unwrap();
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
