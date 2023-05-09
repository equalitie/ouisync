//! Client and Server than run on different devices.

use super::{socket_server_connection, Handler, SocketClient};
use crate::{
    error::Result,
    protocol::remote::{Request, Response},
};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    io::{self, IoSlice},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::{
    rustls::{ClientConfig, ServerConfig},
    TlsAcceptor,
};
use tokio_tungstenite::{
    tungstenite::{self, client::IntoClientRequest, Message},
    Connector, WebSocketStream,
};
use tracing::Instrument;

pub struct RemoteServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    tls_acceptor: Option<TlsAcceptor>,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr, config: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await.map_err(|error| {
            tracing::error!(?error, "failed to bind to {}", addr);
            error
        })?;

        let local_addr = listener.local_addr().map_err(|error| {
            tracing::error!(?error, "failed to retrieve local address");
            error
        })?;

        tracing::info!("remote API server listening on {}", local_addr);

        let tls_acceptor = config.map(TlsAcceptor::from);

        Ok(Self {
            listener,
            local_addr,
            tls_acceptor,
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

async fn run_connection<H: Handler>(
    stream: TcpStream,
    tls_acceptor: Option<TlsAcceptor>,
    handler: H,
) {
    // Upgrade to TLS
    let stream = if let Some(tls_acceptor) = tls_acceptor {
        match tls_acceptor.accept(stream).await {
            Ok(stream) => MaybeTlsServerStream::Rustls(stream),
            Err(error) => {
                tracing::error!(?error, "failed to upgrade to tls");
                return;
            }
        }
    } else {
        MaybeTlsServerStream::Plain(stream)
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
    inner: SocketClient<Socket<MaybeTlsClientStream>, Request, Response>,
}

impl RemoteClient {
    pub async fn connect(
        request: impl IntoClientRequest + Unpin,
        config: Option<Arc<ClientConfig>>,
    ) -> io::Result<Self> {
        let connector = config.map(Connector::Rustls);
        let (stream, _) =
            tokio_tungstenite::connect_async_tls_with_config(request, None, connector)
                .await
                .map_err(into_io_error)?;
        let inner = SocketClient::new(Socket(stream));

        Ok(Self { inner })
    }

    pub async fn invoke(&self, request: Request) -> Result<Response> {
        self.inner.invoke(request).await
    }
}

type MaybeTlsClientStream = tokio_tungstenite::MaybeTlsStream<TcpStream>;

enum MaybeTlsServerStream {
    Plain(TcpStream),
    Rustls(tokio_rustls::server::TlsStream<TcpStream>),
}

impl AsyncRead for MaybeTlsServerStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Rustls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsServerStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Rustls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Rustls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Rustls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
            Self::Rustls(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(stream) => stream.is_write_vectored(),
            Self::Rustls(stream) => stream.is_write_vectored(),
        }
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
