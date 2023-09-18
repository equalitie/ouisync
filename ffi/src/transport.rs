//! Client and Server than run in the same process but the Client is written in a different
//! language than the Server.

use crate::{
    dart::{Port, PortSender},
    handler::Handler,
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use ouisync_bridge::transport::socket_server_connection;
use std::{
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

pub(crate) struct Server {
    socket: Socket,
}

impl Server {
    pub fn new(port_sender: PortSender, sender_tx_port: Port) -> (Self, ClientSender) {
        let (socket, client_tx) = Socket::new(port_sender, sender_tx_port);
        let server = Self { socket };

        (server, client_tx)
    }

    pub async fn run(self, handler: Handler) {
        socket_server_connection::run(self.socket, handler).await
    }
}

pub(crate) type ClientSender = mpsc::UnboundedSender<BytesMut>;

struct Socket {
    port_sender: PortSender,
    server_tx_port: Port,
    server_rx: UnboundedReceiverStream<BytesMut>,
}

impl Socket {
    fn new(port_sender: PortSender, server_tx_port: Port) -> (Self, ClientSender) {
        let (client_tx, server_rx) = mpsc::unbounded_channel();

        let socket = Self {
            port_sender,
            server_tx_port,
            server_rx: UnboundedReceiverStream::new(server_rx),
        };

        (socket, client_tx)
    }
}

impl futures_util::Stream for Socket {
    type Item = io::Result<BytesMut>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(ready!(self.server_rx.poll_next_unpin(cx)).map(Ok))
    }
}

impl futures_util::Sink<Bytes> for Socket {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.port_sender.send_bytes(self.server_tx_port, item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}
