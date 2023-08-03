//! Client and Server than run on different devices.

use super::{socket_server_connection, Handler, SocketClient};
use crate::{
    error::Result,
    protocol::remote::{Request, Response},
};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    borrow::Cow,
    io,
    net::SocketAddr,
    ops::RangeInclusive,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::{rustls, TlsAcceptor};
use tokio_tungstenite::{
    tungstenite::{self, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};
use tracing::Instrument;

// Range (inclusive) of supported protocol versions.
const MIN_VERSION: u64 = 0;
const MAX_VERSION: u64 = 0;

/// Shared config for `RemoteServer`
pub fn make_server_config(
    cert_chain: Vec<rustls::Certificate>,
    key: rustls::PrivateKey,
) -> Result<Arc<rustls::ServerConfig>> {
    make_server_config_with_versions(cert_chain, key, MIN_VERSION..=MAX_VERSION)
}

fn make_server_config_with_versions(
    cert_chain: Vec<rustls::Certificate>,
    key: rustls::PrivateKey,
    versions: RangeInclusive<u64>,
) -> Result<Arc<rustls::ServerConfig>> {
    let mut config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    // (ab)use ALPN (https://en.wikipedia.org/wiki/Application-Layer_Protocol_Negotiation) for
    // protocol version negotation
    config.alpn_protocols = [b"h2".to_vec(), b"http/1.1".to_vec(), b"http/1.0".to_vec()]
        .into_iter()
        .chain(to_alpn_protocols(versions))
        .collect();

    Ok(Arc::new(config))
}

/// Shared config for `RemoteClient`
pub fn make_client_config(
    additional_root_certs: &[rustls::Certificate],
) -> Result<Arc<rustls::ClientConfig>> {
    make_client_config_with_versions(additional_root_certs, MIN_VERSION..=MAX_VERSION)
}

fn make_client_config_with_versions(
    additional_root_certs: &[rustls::Certificate],
    versions: RangeInclusive<u64>,
) -> Result<Arc<rustls::ClientConfig>> {
    let mut root_cert_store = rustls::RootCertStore::empty();

    // Add default root certificates
    root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    // Add custom root certificates (if any)
    for cert in additional_root_certs {
        root_cert_store
            .add(cert)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    }

    let mut config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    config.alpn_protocols = to_alpn_protocols(versions).collect();

    Ok(Arc::new(config))
}

pub struct RemoteServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    tls_acceptor: TlsAcceptor,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr, config: Arc<rustls::ServerConfig>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await.map_err(|error| {
            tracing::error!(?error, "failed to bind to {}", addr);
            error
        })?;

        let local_addr = listener.local_addr().map_err(|error| {
            tracing::error!(?error, "failed to retrieve local address");
            error
        })?;

        tracing::info!("remote API server listening on {}", local_addr);

        Ok(Self {
            listener,
            local_addr,
            tls_acceptor: TlsAcceptor::from(config),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run<H: Handler>(self, handler: H) {
        let mut connections = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    connections.spawn(
                        run_connection(stream, self.tls_acceptor.clone(), handler.clone())
                            .instrument(tracing::info_span!("remote client", %addr)),
                    );
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept client");
                    break;
                }
            }
        }
    }
}

async fn run_connection<H: Handler>(stream: TcpStream, tls_acceptor: TlsAcceptor, handler: H) {
    // Upgrade to TLS
    let stream = match tls_acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to upgrade to tls");
            return;
        }
    };

    // Upgrade to websocket
    let stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to upgrade to websocket");
            return;
        }
    };

    tracing::debug!("accepted");

    socket_server_connection::run(Socket(stream), handler).await;
}

pub struct RemoteClient {
    inner: SocketClient<Socket<MaybeTlsStream<TcpStream>>, Request, Response>,
}

impl RemoteClient {
    pub async fn connect(host: &str, config: Arc<rustls::ClientConfig>) -> io::Result<Self> {
        let host = if host.contains("://") {
            Cow::Borrowed(host)
        } else {
            Cow::Owned(format!("wss://{host}"))
        };

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            host.as_ref(),
            None,
            false,
            Some(Connector::Rustls(config)),
        )
        .await
        .map_err(into_io_error)?;
        let inner = SocketClient::new(Socket(stream));

        Ok(Self { inner })
    }

    pub async fn invoke(&self, request: Request) -> Result<Response> {
        self.inner.invoke(request).await
    }
}

struct Socket<T>(WebSocketStream<T>);

impl<T> futures_util::Stream for Socket<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Item = io::Result<BytesMut>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(payload))) => {
                    return Poll::Ready(Some(Ok(payload.into_iter().collect())));
                }
                Some(Ok(Message::Close(_))) => continue,
                Some(Ok(
                    message @ (Message::Text(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_)),
                )) => {
                    tracing::debug!(?message, "unexpected message type");
                    continue;
                }
                Some(Err(error)) => {
                    return Poll::Ready(Some(Err(into_io_error(error))));
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<T> futures_util::Sink<Bytes> for Socket<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Error = io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.0.poll_ready_unpin(cx)).map_err(into_io_error))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.0
            .start_send_unpin(Message::Binary(item.into()))
            .map_err(into_io_error)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.0.poll_flush_unpin(cx)).map_err(into_io_error))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.0.poll_close_unpin(cx)).map_err(into_io_error))
    }
}

fn into_io_error(src: tungstenite::Error) -> io::Error {
    match src {
        tungstenite::Error::Io(error) => error,
        _ => io::Error::new(io::ErrorKind::Other, src),
    }
}

fn to_alpn_protocols(versions: RangeInclusive<u64>) -> impl Iterator<Item = Vec<u8>> {
    versions.map(|version| version.to_be_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::NotificationSender;
    use async_trait::async_trait;
    use ouisync_lib::{AccessMode, AccessSecrets, ShareToken};
    use std::{
        net::Ipv4Addr,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio::task;

    #[tokio::test]
    async fn basic() {
        let (server_config, client_config) =
            make_configs(MIN_VERSION..=MAX_VERSION, MIN_VERSION..=MAX_VERSION);
        let handler = TestHandler::default();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let port = server.local_addr().port();
        task::spawn(server.run(handler.clone()));

        let client = RemoteClient::connect(&format!("localhost:{port}"), client_config)
            .await
            .unwrap();

        let share_token =
            ShareToken::from(AccessSecrets::random_write().with_mode(AccessMode::Blind));

        match client
            .invoke(Request::Mirror { share_token })
            .await
            .unwrap()
        {
            Response::None => (),
        }

        assert_eq!(handler.received(), 1);
    }

    #[tokio::test]
    async fn version_overlap() {
        let (server_config, client_config) = make_configs(1..=2, 0..=1);
        let handler = TestHandler::default();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let port = server.local_addr().port();
        task::spawn(server.run(handler.clone()));

        let client = RemoteClient::connect(&format!("localhost:{port}"), client_config)
            .await
            .unwrap();

        let share_token =
            ShareToken::from(AccessSecrets::random_write().with_mode(AccessMode::Blind));

        match client
            .invoke(Request::Mirror { share_token })
            .await
            .unwrap()
        {
            Response::None => (),
        }

        assert_eq!(handler.received(), 1);
    }

    #[tokio::test]
    async fn version_mismatch() {
        let (server_config, client_config) = make_configs(2..=3, 0..=1);
        let handler = TestHandler::default();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let port = server.local_addr().port();
        task::spawn(server.run(handler.clone()));

        match RemoteClient::connect(&format!("localhost:{port}"), client_config).await {
            Err(error) if error.kind() == io::ErrorKind::InvalidData => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[derive(Default, Clone)]
    struct TestHandler {
        received: Arc<AtomicUsize>,
    }

    impl TestHandler {
        fn received(&self) -> usize {
            self.received.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl Handler for TestHandler {
        type Request = Request;
        type Response = Response;

        async fn handle(&self, _: Self::Request, _: &NotificationSender) -> Result<Self::Response> {
            self.received.fetch_add(1, Ordering::Relaxed);
            Ok(Response::None)
        }
    }

    fn make_configs(
        server_versions: RangeInclusive<u64>,
        client_versions: RangeInclusive<u64>,
    ) -> (Arc<rustls::ServerConfig>, Arc<rustls::ClientConfig>) {
        let gen = rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
        let cert = rustls::Certificate(gen.serialize_der().unwrap());
        let key = rustls::PrivateKey(gen.serialize_private_key_der());

        let server_config =
            make_server_config_with_versions(vec![cert.clone()], key, server_versions).unwrap();
        let client_config = make_client_config_with_versions(&[cert], client_versions).unwrap();

        (server_config, client_config)
    }
}
