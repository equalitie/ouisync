//! Client and Server than run on different devices.

use super::{utils, Server};
use crate::state::ServerState;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_tungstenite::{
    tungstenite::{self, Message},
    WebSocketStream,
};

pub struct RemoteServer {
    listener: TcpListener,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;

        match listener.local_addr() {
            Ok(addr) => tracing::debug!("server bound to {:?}", addr),
            Err(error) => {
                tracing::error!(?error, "failed to retrieve server address")
            }
        }

        Ok(Self { listener })
    }
}

#[async_trait]
impl Server for RemoteServer {
    async fn run(self, state: Arc<ServerState>) {
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
                    let state = state.clone();

                    connections.spawn(async move {
                        utils::handle_server_connection(socket, &state).await
                    });
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept client");
                    break;
                }
            }
        }
    }
}

// TODO: implement client

struct Socket(WebSocketStream<TcpStream>);

impl futures_util::Stream for Socket {
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

impl futures_util::Sink<Bytes> for Socket {
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
    io::Error::new(io::ErrorKind::Other, src)
}
