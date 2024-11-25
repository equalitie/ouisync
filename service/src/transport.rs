mod common;
pub mod local;
pub mod remote;

pub use self::common::{ReadError, WriteError};

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::{Sink, SinkExt, Stream, StreamExt};

use self::local::{LocalServerReader, LocalServerWriter};
use crate::protocol::{Message, Request, ServerPayload};

pub(crate) enum ServerReader {
    Local(LocalServerReader),
}

impl Stream for ServerReader {
    type Item = Result<Message<Request>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Local(reader) => reader.poll_next_unpin(cx),
        }
    }
}

pub(crate) enum ServerWriter {
    Local(LocalServerWriter),
}

impl Sink<Message<ServerPayload>> for ServerWriter {
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_ready_unpin(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_flush_unpin(cx),
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            Self::Local(writer) => writer.poll_close_unpin(cx),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<ServerPayload>) -> Result<(), Self::Error> {
        match self.get_mut() {
            Self::Local(writer) => writer.start_send_unpin(item),
        }
    }
}
