use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};

use bytes::BytesMut;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use interprocess::local_socket::tokio::{RecvHalf, SendHalf};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{DecodeError, EncodeError, Message};

pub struct Reader<T> {
    reader: FramedRead<RecvHalf, LengthDelimitedCodec>,
    _type: PhantomData<fn() -> T>,
}

impl<T> Reader<T> {
    pub(super) fn new(inner: RecvHalf) -> Self {
        Self {
            reader: FramedRead::new(inner, LengthDelimitedCodec::new()),
            _type: PhantomData,
        }
    }
}

impl<T> Stream for Reader<T>
where
    T: DeserializeOwned,
{
    type Item = Result<Message<T>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let buffer = match ready!(self.get_mut().reader.poll_next_unpin(cx)) {
            Some(Ok(buffer)) => buffer,
            Some(Err(error)) => return Poll::Ready(Some(Err(error.into()))),
            None => return Poll::Ready(None),
        };

        Poll::Ready(Some(Ok(Message::decode(&mut buffer.freeze())?)))
    }
}

pub struct Writer<T> {
    writer: FramedWrite<SendHalf, LengthDelimitedCodec>,
    buffer: BytesMut,
    _type: PhantomData<fn(T)>,
}

impl<T> Writer<T> {
    pub(super) fn new(inner: SendHalf) -> Self {
        Self {
            writer: FramedWrite::new(inner, LengthDelimitedCodec::new()),
            buffer: BytesMut::new(),
            _type: PhantomData,
        }
    }
}

impl<T> Sink<Message<T>> for Writer<T>
where
    T: Serialize,
{
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(ready!(self.get_mut().writer.poll_ready_unpin(cx))?))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(ready!(self.get_mut().writer.poll_flush_unpin(cx))?))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(ready!(self.get_mut().writer.poll_close_unpin(cx))?))
    }

    fn start_send(self: Pin<&mut Self>, item: Message<T>) -> Result<(), Self::Error> {
        let this = self.get_mut();

        item.encode(&mut this.buffer)?;
        this.writer.start_send_unpin(this.buffer.split().freeze())?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ReadError {
    #[error("failed to receive message")]
    Receive(#[from] io::Error),
    #[error("failed to decode message")]
    Decode(#[from] DecodeError),
}

#[derive(Error, Debug)]
pub enum WriteError {
    #[error("failed to send message")]
    Send(#[from] io::Error),
    #[error("failed to encode message")]
    Encode(#[from] EncodeError),
}
