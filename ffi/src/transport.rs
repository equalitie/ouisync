//! Client and Server than run in the same process but the Client is written in a different
//! language than the Server.

use crate::{handler::Handler, sender::Sender};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use ouisync_bridge::{protocol::SessionCookie, transport::socket_server_connection};
use std::{
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

pub(crate) struct Server<T> {
    socket: Socket<T>,
}

impl<T> Server<T> {
    pub fn new(sender: T) -> (Self, ClientSender) {
        let (socket, client_tx) = Socket::new(sender);
        let server = Self { socket };

        (server, client_tx)
    }
}

impl<T> Server<T>
where
    T: Sender,
{
    pub async fn run(self, handler: Handler) {
        socket_server_connection::run(self.socket, handler, SessionCookie::DUMMY).await
    }
}

pub(crate) type ClientSender = mpsc::UnboundedSender<BytesMut>;

struct Socket<T> {
    tx: T,
    rx: UnboundedReceiverStream<BytesMut>,
}

impl<T> Socket<T> {
    fn new(sender: T) -> (Self, ClientSender) {
        let (client_tx, server_rx) = mpsc::unbounded_channel();

        let socket = Self {
            tx: sender,
            rx: UnboundedReceiverStream::new(server_rx),
        };

        (socket, client_tx)
    }
}

impl<T> futures_util::Stream for Socket<T>
where
    T: Unpin,
{
    type Item = io::Result<BytesMut>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(ready!(self.rx.poll_next_unpin(cx)).map(Ok))
    }
}

impl<T> futures_util::Sink<Bytes> for Socket<T>
where
    T: Sender,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.tx.send(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}
