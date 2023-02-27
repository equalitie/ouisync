//! Client and Server than run in the same process but the Client is written in a different
//! language than the Server.

use super::{socket, Server};
use crate::state::ServerState;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::PollSender;

pub struct ForeignServer {
    socket: MemorySocket,
}

impl ForeignServer {
    pub fn new() -> (Self, ForeignClientSender, ForeignClientReceiver) {
        let (socket, client_tx, client_rx) = MemorySocket::new();
        let server = Self { socket };

        (server, client_tx, client_rx)
    }
}

#[async_trait]
impl Server for ForeignServer {
    async fn run(self, state: Arc<ServerState>) {
        socket::server_connection::run(self.socket, state).await
    }
}

pub struct MemorySocket {
    tx: PollSender<Bytes>,
    rx: UnboundedReceiverStream<BytesMut>,
}

// Note: we don't directly implement the `Client` trait. Instead these two types needs to be passed
// across the FFI boundary in some way and the client logic implemented there.
pub type ForeignClientSender = mpsc::UnboundedSender<BytesMut>;
pub type ForeignClientReceiver = mpsc::Receiver<Bytes>;

impl MemorySocket {
    fn new() -> (Self, ForeignClientSender, ForeignClientReceiver) {
        let (server_tx, client_rx) = mpsc::channel(1);
        let (client_tx, server_rx) = mpsc::unbounded_channel();

        let socket = Self {
            tx: PollSender::new(server_tx),
            rx: UnboundedReceiverStream::new(server_rx),
        };

        (socket, client_tx, client_rx)
    }
}

impl futures_util::Stream for MemorySocket {
    type Item = io::Result<BytesMut>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(ready!(self.rx.poll_next_unpin(cx)).map(Ok))
    }
}

impl futures_util::Sink<Bytes> for MemorySocket {
    type Error = io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(
            ready!(self.tx.poll_ready_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
        )
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.tx
            .start_send_unpin(item)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(
            ready!(self.tx.poll_flush_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
        )
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(
            ready!(self.tx.poll_close_unpin(cx)).map_err(|_| io::ErrorKind::BrokenPipe.into()),
        )
    }
}
