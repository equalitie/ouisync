//! Client and Server than run on different devices.

use crate::{
    handler::Handler,
    options::{Request, Response},
};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use ouisync_bridge::{
    transport::{
        socket::{self, SocketClient},
        Client,
    },
    Result,
};
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_tungstenite::{
    tungstenite::{self, client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

// TODO: Implement TLS

pub(crate) struct RemoteServer {
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;

        let local_addr = match listener.local_addr() {
            Ok(addr) => {
                tracing::debug!("server bound to {:?}", addr);
                addr
            }
            Err(error) => {
                tracing::error!(?error, "failed to retrieve server address");
                return Err(error);
            }
        };

        Ok(Self {
            listener,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn run(self, handler: Handler) {
        let mut connections = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    // Convert to websocket
                    let socket = match tokio_tungstenite::accept_async(stream).await {
                        Ok(socket) => socket,
                        Err(error) => {
                            tracing::error!(
                                ?error,
                                "failed to upgrade tcp socket to websocket socket"
                            );
                            continue;
                        }
                    };

                    tracing::debug!("client accepted at {:?}", addr);

                    let socket = Socket(socket);
                    connections.spawn(socket::server_connection::run(socket, handler.clone()));
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept client");
                    break;
                }
            }
        }
    }
}

pub(crate) struct RemoteClient {
    inner: SocketClient<Socket<MaybeTlsStream<TcpStream>>, Request, Response>,
}

impl RemoteClient {
    pub async fn connect(request: impl IntoClientRequest + Unpin) -> io::Result<Self> {
        let (inner, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(into_io_error)?;
        let inner = Socket(inner);
        let inner = SocketClient::new(inner);

        Ok(Self { inner })
    }
}

#[async_trait(?Send)]
impl Client for RemoteClient {
    type Request = Request;
    type Response = Response;

    async fn invoke(&self, request: Self::Request) -> Result<Self::Response> {
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
                Some(Ok(
                    message @ (Message::Text(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Close(_)
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
