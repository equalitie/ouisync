use futures_util::StreamExt;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const CERT_DOMAIN: &str = "ouisync.net";
const KEEP_ALIVE_INTERVAL_MS: u32 = 15_000;
const MAX_IDLE_TIMEOUT_MS: u32 = 3 * KEEP_ALIVE_INTERVAL_MS + 2_000;

pub type Result<T> = std::result::Result<T, Error>;

//------------------------------------------------------------------------------
pub struct Connector {
    endpoint: quinn::Endpoint,
}

impl Connector {
    pub async fn connect(&self, remote_addr: SocketAddr) -> Result<Connection> {
        let quinn::NewConnection { connection, .. } =
            self.endpoint.connect(remote_addr, CERT_DOMAIN)?.await?;
        let (tx, rx) = connection.open_bi().await?;
        Ok(Connection::new(connection, rx, tx))
    }
}

//------------------------------------------------------------------------------
pub struct Acceptor {
    incoming: quinn::Incoming,
    local_addr: SocketAddr,
}

impl Acceptor {
    pub async fn accept(&mut self) -> Result<Connection> {
        let incoming_conn = match self.incoming.next().await {
            Some(incoming_conn) => incoming_conn,
            None => return Err(Error::DoneAccepting),
        };
        let quinn::NewConnection {
            connection,
            mut bi_streams,
            ..
        } = incoming_conn.await?;
        let (tx, rx) = match bi_streams.next().await {
            Some(r) => r?,
            None => return Err(Error::DoneAccepting),
        };
        Ok(Connection::new(connection, rx, tx))
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }
}

//------------------------------------------------------------------------------
pub struct Connection {
    connection: Arc<quinn::Connection>,
    rx: Option<quinn::RecvStream>,
    tx: Option<quinn::SendStream>,
}

impl Connection {
    pub fn new(
        connection: quinn::Connection,
        rx: quinn::RecvStream,
        tx: quinn::SendStream,
    ) -> Self {
        Self {
            connection: Arc::new(connection),
            rx: Some(rx),
            tx: Some(tx),
        }
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    pub fn into_split(mut self) -> (OwnedReadHalf, OwnedWriteHalf) {
        let conn = self.connection.clone();
        // Unwrap OK because `self` can't be split more than once and we're not `taking` from `rx`
        // anywhere else.
        let rx = self.rx.take().unwrap();
        let tx = self.tx.take();
        (OwnedReadHalf(rx, conn.clone()), OwnedWriteHalf(tx, conn))
    }

    /// Make sure all data is sent, no more data can be sent afterwards.
    #[cfg(test)]
    pub async fn finish(&mut self) -> Result<()> {
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
        match &mut self.get_mut().rx {
            Some(rx) => Pin::new(rx).poll_read(cx, buf),
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
        match &mut self.get_mut().tx {
            Some(tx) => Pin::new(tx).poll_write(cx, buf),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match &mut self.get_mut().tx {
            Some(tx) => Pin::new(tx).poll_flush(cx),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match &mut self.get_mut().tx {
            Some(tx) => Pin::new(tx).poll_shutdown(cx),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(mut tx) = self.tx.take() {
            tokio::task::spawn(async move { tx.finish().await.unwrap_or(()) });
        }
    }
}

//------------------------------------------------------------------------------
pub struct OwnedReadHalf(quinn::RecvStream, Arc<quinn::Connection>);
pub struct OwnedWriteHalf(Option<quinn::SendStream>, Arc<quinn::Connection>);

impl AsyncRead for OwnedReadHalf {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncWrite for OwnedWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            Some(tx) => Pin::new(tx).poll_write(cx, buf),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Some(tx) => Pin::new(tx).poll_flush(cx),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            Some(tx) => Pin::new(tx).poll_shutdown(cx),
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "already finished",
            ))),
        }
    }
}

impl Drop for OwnedWriteHalf {
    fn drop(&mut self) {
        if let Some(mut tx) = self.0.take() {
            tokio::task::spawn(async move { tx.finish().await.unwrap_or(()) });
        }
    }
}

//------------------------------------------------------------------------------
pub fn configure(socket: std::net::UdpSocket) -> Result<(Connector, Acceptor)> {
    let server_config = make_server_config()?;
    let (mut endpoint, incoming) = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
    )?;
    endpoint.set_default_client_config(make_client_config());

    let local_addr = endpoint.local_addr()?;

    let connector = Connector { endpoint };
    let acceptor = Acceptor {
        incoming,
        local_addr,
    };

    Ok((connector, acceptor))
}

//------------------------------------------------------------------------------
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connect error")]
    Connect,
    #[error("connection error")]
    Connection,
    #[error("write error")]
    Write(quinn::WriteError),
    #[error("done accepting error")]
    DoneAccepting,
    #[error("IO error")]
    Io(std::io::Error),
    #[error("TLS error")]
    Tls(rustls::Error),
}

impl From<quinn::ConnectionError> for Error {
    fn from(_: quinn::ConnectionError) -> Self {
        Self::Connection
    }
}

impl From<quinn::ConnectError> for Error {
    fn from(_: quinn::ConnectError) -> Self {
        Self::Connect
    }
}

impl From<quinn::WriteError> for Error {
    fn from(e: quinn::WriteError) -> Self {
        Self::Write(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rustls::Error> for Error {
    fn from(e: rustls::Error) -> Self {
        Self::Tls(e)
    }
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

    Arc::get_mut(&mut client_config.transport)
        .unwrap()
        .max_concurrent_uni_streams(0_u8.into())
        // Documentation says that only one side needs to set the keep alive interval, chosing this
        // to be on the client side with the reasoning that the server side has a better chance of
        // being behind a non restrictive NAT, and so that sending the packets from the client side
        // shall assist in hole punching.
        .keep_alive_interval(Some(Duration::from_millis(KEEP_ALIVE_INTERVAL_MS.into())))
        .max_idle_timeout(Some(quinn::VarInt::from_u32(MAX_IDLE_TIMEOUT_MS).into()));

    client_config
}

fn make_server_config() -> Result<quinn::ServerConfig> {
    // Generate self signed certificate, it won't be checked, but QUIC doesn't have an option to
    // get rid of TLS completely.
    let cert = rcgen::generate_simple_self_signed(vec![CERT_DOMAIN.into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(priv_key);
    let cert_chain = vec![rustls::Certificate(cert_der.clone())];

    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, priv_key)?;

    Arc::get_mut(&mut server_config.transport)
        .unwrap()
        .max_concurrent_uni_streams(0_u8.into())
        .max_idle_timeout(Some(quinn::VarInt::from_u32(MAX_IDLE_TIMEOUT_MS).into()));

    Ok(server_config)
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
    async fn small_data_exchange() {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();

        let (connector, mut acceptor) = configure(socket).unwrap();

        let addr = *acceptor.local_addr();

        let message = b"hello world";

        let h1 = task::spawn(async move {
            let mut conn = acceptor.accept().await.unwrap();
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
}
