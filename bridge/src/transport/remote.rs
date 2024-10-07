//! Client and Server than run on different devices.

use super::{socket_server_connection, Handler, SocketClient};
use crate::protocol::{
    remote::{Request, Response, ServerError},
    SessionCookie,
};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    borrow::Cow,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::{
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer},
        ConnectionCommon,
    },
    TlsAcceptor,
};
use tokio_tungstenite::{
    tungstenite::{self, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};
use tracing::Instrument;

/// Shared config for `RemoteServer`
pub fn make_server_config(
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> io::Result<Arc<rustls::ServerConfig>> {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    Ok(Arc::new(config))
}

/// Shared config for `RemoteClient`
pub fn make_client_config(
    additional_root_certs: &[CertificateDer<'_>],
) -> io::Result<Arc<rustls::ClientConfig>> {
    let mut root_cert_store = rustls::RootCertStore::empty();

    // Add default root certificates
    root_cert_store.extend(
        webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .map(|ta| ta.to_owned()),
    );

    // Add custom root certificates (if any)
    for cert in additional_root_certs {
        root_cert_store
            .add(cert.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

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
            tracing::error!(?error, "Failed to bind to {}", addr);
            error
        })?;

        let local_addr = listener.local_addr().map_err(|error| {
            tracing::error!(?error, "Failed to retrieve local address");
            error
        })?;

        tracing::info!("Remote API server listening on {}", local_addr);

        Ok(Self {
            listener,
            local_addr,
            tls_acceptor: TlsAcceptor::from(config),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run<H>(self, handler: H)
    where
        H: Handler<Error = ServerError>,
    {
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
                    tracing::error!(?error, "Failed to accept client");
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
            tracing::error!(?error, "Failed to upgrade to tls");
            return;
        }
    };

    let session_cookie = extract_session_cookie(stream.get_ref().1);

    // Upgrade to websocket
    let stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "Failed to upgrade to websocket");
            return;
        }
    };

    tracing::debug!("Accepted");

    socket_server_connection::run(Socket(stream), handler, session_cookie).await;
}

pub struct RemoteClient {
    inner: SocketClient<Socket<MaybeTlsStream<TcpStream>>, Request, Response, ServerError>,
    session_cookie: SessionCookie,
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

        let session_cookie = match stream.get_ref() {
            MaybeTlsStream::Rustls(stream) => extract_session_cookie(stream.get_ref().1),
            _ => {
                // We created the stream with a rustls connector so the stream should be rustls as
                // well.
                unreachable!()
            }
        };

        let inner = SocketClient::new(Socket(stream));

        Ok(Self {
            inner,
            session_cookie,
        })
    }

    pub async fn invoke(&self, request: impl Into<Request>) -> Result<(), ServerError> {
        match self.inner.invoke(request.into()).await? {
            Response::None => Ok(()),
        }
    }

    pub fn session_cookie(&self) -> &SessionCookie {
        &self.session_cookie
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
                    tracing::debug!(?message, "Unexpected message type");
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

fn extract_session_cookie<Data>(connection: &ConnectionCommon<Data>) -> SessionCookie {
    // unwrap is OK as the function fails only if called before TLS handshake or if the output
    // length is zero, none of which is the case here.
    connection
        .export_keying_material(SessionCookie::DUMMY, b"ouisync session cookie", None)
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{protocol::remote::v1, transport::SessionContext};
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use ouisync_lib::WriteSecrets;
    use std::{
        net::Ipv4Addr,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio::task;
    use tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer;

    #[tokio::test]
    async fn basic() {
        let (server_config, client_config) = make_configs();
        let handler = TestHandler::default();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let port = server.local_addr().port();
        task::spawn(server.run(handler.clone()));

        let client = RemoteClient::connect(&format!("localhost:{port}"), client_config)
            .await
            .unwrap();

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(v1::Request::Create {
                    repository_id: secrets.id,
                    proof,
                })
                .await,
            Ok(())
        );

        assert_eq!(handler.received(), 1);
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
        type Error = ServerError;

        async fn handle(
            &self,
            _: Self::Request,
            _: &SessionContext,
        ) -> Result<Self::Response, Self::Error> {
            self.received.fetch_add(1, Ordering::Relaxed);
            Ok(Response::None)
        }
    }

    fn make_configs() -> (Arc<rustls::ServerConfig>, Arc<rustls::ClientConfig>) {
        let gen = rcgen::generate_simple_self_signed(["localhost".to_owned()]).unwrap();
        let cert = CertificateDer::from(gen.cert);
        let key = PrivatePkcs8KeyDer::from(gen.key_pair.serialize_der());

        let server_config = make_server_config(vec![cert.clone()], key.into()).unwrap();
        let client_config = make_client_config(&[cert]).unwrap();

        (server_config, client_config)
    }
}
