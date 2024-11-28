pub mod local;
pub mod remote;

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use thiserror::Error;
use tokio_tungstenite::tungstenite as ws;

use self::{
    local::{LocalServerReader, LocalServerWriter},
    remote::{RemoteServerReader, RemoteServerWriter},
};
use crate::protocol::{
    DecodeError, EncodeError, Message, MessageId, ProtocolError, Request, ServerPayload,
};

pub(crate) enum ServerReader {
    Local(LocalServerReader),
    Remote(RemoteServerReader),
}

impl Stream for ServerReader {
    type Item = Result<Message<Request>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Local(reader) => reader.poll_next_unpin(cx),
            Self::Remote(reader) => reader.poll_next_unpin(cx),
        }
    }
}

pub(crate) enum ServerWriter {
    Local(LocalServerWriter),
    Remote(RemoteServerWriter),
}

impl Sink<Message<ServerPayload>> for ServerWriter {
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_ready_unpin(cx),
            Self::Remote(writer) => writer.poll_ready_unpin(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_flush_unpin(cx),
            Self::Remote(writer) => writer.poll_flush_unpin(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_close_unpin(cx),
            Self::Remote(writer) => writer.poll_close_unpin(cx),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<ServerPayload>) -> Result<(), Self::Error> {
        match self.get_mut() {
            Self::Local(writer) => writer.start_send_unpin(item),
            Self::Remote(writer) => writer.start_send_unpin(item),
        }
    }
}

#[derive(Error, Debug)]
pub enum ReadError {
    #[error("failed to receive message")]
    Receive(#[from] TransportError),
    #[error("failed to decode message")]
    Decode(#[from] DecodeError),
    #[error("failed to validate message")]
    Validate(MessageId, ValidateError),
}

#[derive(Error, Debug)]
pub enum WriteError {
    #[error("failed to send message")]
    Send(#[from] TransportError),
    #[error("failed to encode message")]
    Encode(#[from] EncodeError),
}

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("websocket error")]
    WebSocket(#[from] ws::Error),
}

#[derive(Error, Debug)]
pub enum ValidateError {
    #[error("permission denied")]
    PermissionDenied,
}

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("failed to connect")]
    Connect(#[source] io::Error),
    #[error("connection closed by server")]
    Disconnected,
    #[error("socket address is invalid")]
    InvalidSocketAddr,
    #[error("request argument is invalid")]
    InvalidArgument,
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("failed to receive response")]
    Read(#[from] ReadError),
    #[error("server responded with error")]
    Response(ProtocolError),
    #[error("unexpected response")]
    UnexpectedResponse,
    #[error("failed to send request")]
    Write(#[from] WriteError),
}
