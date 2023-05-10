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

/// Shared config for `RemoteServer`
#[derive(Clone)]
pub struct ServerConfig {
    inner: Arc<rustls::ServerConfig>,
}

impl ServerConfig {
    pub fn new(cert_chain: Vec<rustls::Certificate>, key: rustls::PrivateKey) -> Result<Self> {
        let inner = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let inner = Arc::new(inner);

        Ok(Self { inner })
    }
}

/// Shared config for `RemoteClient`
#[derive(Clone)]
pub struct ClientConfig {
    inner: Arc<rustls::ClientConfig>,
}

impl ClientConfig {
    pub fn new(additional_root_certs: &[rustls::Certificate]) -> Result<Self> {
        let mut root_cert_store = rustls::RootCertStore::empty();

        // Add default root certificates
        root_cert_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
            |ta| {
                rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                    ta.subject,
                    ta.spki,
                    ta.name_constraints,
                )
            },
        ));

        // Add custom root certificates (if any)
        for cert in additional_root_certs {
            root_cert_store
                .add(cert)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        }

        let inner = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();
        let inner = Arc::new(inner);

        Ok(Self { inner })
    }
}

pub struct RemoteServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    tls_acceptor: TlsAcceptor,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr, config: ServerConfig) -> io::Result<Self> {
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
            tls_acceptor: TlsAcceptor::from(config.inner),
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
    pub async fn connect(host: &str, config: ClientConfig) -> io::Result<Self> {
        let host = if host.contains("://") {
            Cow::Borrowed(host)
        } else {
            Cow::Owned(format!("wss://{host}"))
        };

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            host.as_ref(),
            None,
            Some(Connector::Rustls(config.inner)),
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
