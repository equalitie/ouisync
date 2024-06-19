use crate::KEEP_ALIVE_INTERVAL;
use bytes::BytesMut;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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

pub type Result<T> = std::result::Result<T, Error>;

//------------------------------------------------------------------------------
pub struct Connector {
    endpoint: quinn::Endpoint,
}

impl Connector {
    pub async fn connect(&self, remote_addr: SocketAddr) -> Result<Connection> {
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
    pub async fn accept(&mut self) -> Option<Connecting> {
        self.endpoint
            .accept()
            .await
            .map(|connecting| Connecting { connecting })
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

pub struct Connecting {
    connecting: quinn::Connecting,
}

impl Connecting {
    pub async fn finish(self) -> Result<Connection> {
        let connection = self.connecting.await?;
        let (tx, rx) = connection.accept_bi().await?;
        Ok(Connection::new(rx, tx, connection.remote_address()))
    }
}

//------------------------------------------------------------------------------
pub struct Connection {
    rx: Option<quinn::RecvStream>,
    tx: Option<quinn::SendStream>,
    remote_address: SocketAddr,
    can_finish: bool,
}

impl Connection {
    pub fn new(rx: quinn::RecvStream, tx: quinn::SendStream, remote_address: SocketAddr) -> Self {
        Self {
            rx: Some(rx),
            tx: Some(tx),
            remote_address,
            can_finish: true,
        }
    }

    pub fn remote_address(&self) -> &SocketAddr {
        &self.remote_address
    }

    pub fn into_split(mut self) -> (OwnedReadHalf, OwnedWriteHalf) {
        // Unwrap OK because `self` can't be split more than once and we're not `taking` from `rx`
        // anywhere else.
        let rx = self.rx.take().unwrap();
        let tx = self.tx.take();
        let can_finish = Arc::new(AtomicBool::new(self.can_finish));
        (
            OwnedReadHalf {
                rx,
                can_finish: can_finish.clone(),
            },
            OwnedWriteHalf { tx, can_finish },
        )
    }

    /// Make sure all data is sent, no more data can be sent afterwards.
    #[cfg(test)]
    pub async fn finish(&mut self) -> Result<()> {
        if !self.can_finish {
            return Err(Error::Write(quinn::WriteError::UnknownStream));
        }

        self.can_finish = false;

        match self.tx.take() {
            Some(mut tx) => {
                tx.finish().await?;
                Ok(())
            }
            None => Err(Error::Write(quinn::WriteError::UnknownStream)),
        }
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.rx {
            Some(rx) => {
                let poll = Pin::new(rx).poll_read(cx, buf);
                if let Poll::Ready(r) = &poll {
                    this.can_finish &= r.is_ok();
                };
                poll
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection was split",
            ))),
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                let poll = Pin::new(tx).poll_write(cx, buf);
                if let Poll::Ready(r) = &poll {
                    this.can_finish &= r.is_ok();
                }
                poll
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                let poll = Pin::new(tx).poll_flush(cx);
                if let Poll::Ready(r) = &poll {
                    this.can_finish &= r.is_ok();
                }
                poll
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                this.can_finish = false;
                Pin::new(tx).poll_shutdown(cx)
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if !self.can_finish {
            return;
        }

        if let Some(mut tx) = self.tx.take() {
            tokio::task::spawn(async move { tx.finish().await.unwrap_or(()) });
        }
    }
}

//------------------------------------------------------------------------------
pub struct OwnedReadHalf {
    rx: quinn::RecvStream,
    can_finish: Arc<AtomicBool>,
}
pub struct OwnedWriteHalf {
    tx: Option<quinn::SendStream>,
    can_finish: Arc<AtomicBool>,
}

impl AsyncRead for OwnedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.rx).poll_read(cx, buf);
        if let Poll::Ready(r) = &poll {
            if r.is_err() {
                this.can_finish.store(false, Ordering::SeqCst);
            }
        }
        poll
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                let poll = Pin::new(tx).poll_write(cx, buf);

                if let Poll::Ready(r) = &poll {
                    if r.is_err() {
                        this.can_finish.store(false, Ordering::SeqCst);
                    }
                }

                poll
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                let poll = Pin::new(tx).poll_flush(cx);

                if let Poll::Ready(r) = &poll {
                    if r.is_err() {
                        this.can_finish.store(false, Ordering::SeqCst);
                    }
                }

                poll
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match &mut this.tx {
            Some(tx) => {
                this.can_finish.store(false, Ordering::SeqCst);
                Pin::new(tx).poll_shutdown(cx)
            }
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }
}

impl Drop for OwnedWriteHalf {
    fn drop(&mut self) {
        if !self.can_finish.load(Ordering::SeqCst) {
            return;
        }

        if let Some(mut tx) = self.tx.take() {
            tokio::task::spawn(async move { tx.finish().await.unwrap_or(()) });
        }
    }
}

//------------------------------------------------------------------------------
pub async fn configure(bind_addr: SocketAddr) -> Result<(Connector, Acceptor, SideChannelMaker)> {
    let server_config = make_server_config()?;
    let custom_socket = CustomUdpSocket::bind(bind_addr).await?;
    let side_channel_maker = custom_socket.side_channel_maker();

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
struct SkipServerVerification;

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> std::result::Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

fn make_client_config() -> quinn::ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification {}))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(Arc::new(crypto));

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

fn make_server_config() -> Result<quinn::ServerConfig> {
    // Generate a self signed certificate.
    let cert = rcgen::generate_simple_self_signed(vec![CERT_DOMAIN.into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(priv_key);
    let cert_chain = vec![rustls::Certificate(cert_der)];

    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, priv_key)?;

    let mut transport_config = quinn::TransportConfig::default();

    transport_config
        .max_concurrent_uni_streams(0_u8.into())
        .max_idle_timeout((2 * KEEP_ALIVE_INTERVAL).try_into().ok());

    server_config.transport_config(Arc::new(transport_config));

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
    io: Arc<tokio::net::UdpSocket>,
    quinn_socket_state: quinn::udp::UdpSocketState,
    side_channel_tx: broadcast::Sender<Packet>,
}

impl CustomUdpSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let socket = crate::udp::UdpSocket::bind(addr).await?;
        let socket = socket.into_std()?;

        quinn::udp::UdpSocketState::configure((&socket).into())?;

        Ok(Self {
            io: Arc::new(tokio::net::UdpSocket::from_std(socket)?),
            quinn_socket_state: quinn::udp::UdpSocketState::new(),
            side_channel_tx: broadcast::channel(MAX_SIDE_CHANNEL_PENDING_PACKETS).0,
        })
    }

    fn side_channel_maker(&self) -> SideChannelMaker {
        SideChannelMaker {
            io: self.io.clone(),
            packet_tx: self.side_channel_tx.clone(),
        }
    }
}

impl quinn::AsyncUdpSocket for CustomUdpSocket {
    fn poll_send(
        &self,
        state: &quinn::udp::UdpState,
        cx: &mut Context,
        transmits: &[quinn::udp::Transmit],
    ) -> Poll<io::Result<usize>> {
        let quinn_socket_state = &self.quinn_socket_state;
        let io = &*self.io;
        loop {
            ready!(io.poll_send_ready(cx))?;
            if let Ok(res) = io.try_io(Interest::WRITABLE, || {
                quinn_socket_state.send(io.into(), state, transmits)
            }) {
                return Poll::Ready(Ok(res));
            }
        }
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
                let res = self
                    .quinn_socket_state
                    .recv((&*self.io).into(), bufs, metas);

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

//------------------------------------------------------------------------------

/// Makes new `SideChannel`s.
pub struct SideChannelMaker {
    io: Arc<tokio::net::UdpSocket>,
    packet_tx: broadcast::Sender<Packet>,
}

impl SideChannelMaker {
    pub fn make(&self) -> SideChannel {
        SideChannel {
            io: self.io.clone(),
            packet_rx: AsyncMutex::new(self.packet_tx.subscribe()),
        }
    }
}

pub struct SideChannel {
    io: Arc<tokio::net::UdpSocket>,
    packet_rx: AsyncMutex<broadcast::Receiver<Packet>>,
}

impl SideChannel {
    pub fn sender(&self) -> SideChannelSender {
        SideChannelSender {
            io: self.io.clone(),
        }
    }
}

impl DatagramSocket for SideChannel {
    async fn send_to<'a>(&'a self, buf: &'a [u8], target: SocketAddr) -> io::Result<usize> {
        self.io.send_to(buf, target).await
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
        self.io.local_addr()
    }
}

// `SideChannelSender` is less expensive than the `SideChannel` because there is no additional
// `broadcast::Receiver` that the `CustomUdpSocket` would need to pass messages to.
#[derive(Clone)]
pub struct SideChannelSender {
    io: Arc<tokio::net::UdpSocket>,
}

impl SideChannelSender {
    pub async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.io.send_to(buf, target).await.map(|_| ())
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
        let (connector, mut acceptor, _) =
            configure((Ipv4Addr::LOCALHOST, 0).into()).await.unwrap();

        let addr = *acceptor.local_addr();

        let message = b"hello world";

        let h1 = task::spawn(async move {
            let mut conn = acceptor.accept().await.unwrap().finish().await.unwrap();
            let mut buf = [0; 32];
            let n = conn.read(&mut buf).await.unwrap();
            assert_eq!(message, &buf[..n]);
        });

        let h2 = task::spawn(async move {
            let mut conn = connector.connect(addr).await.unwrap();
            conn.write_all(message).await.unwrap();
            conn.finish().await.unwrap();
        });

        h1.await.unwrap();
        h2.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn side_channel() {
        let (_connector, mut acceptor, side_channel_maker) =
            configure((Ipv4Addr::LOCALHOST, 0).into()).await.unwrap();
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
