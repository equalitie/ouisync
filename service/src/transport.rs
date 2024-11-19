use crate::protocol::{DecodeError, EncodeError, Message, Request};
use bytes::{buf::Writer, Buf, BufMut, BytesMut};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use interprocess::local_socket::{
    tokio::{Listener, RecvHalf, SendHalf},
    traits::tokio::{Listener as _, Stream as _},
    GenericFilePath, ListenerOptions, ToFsName,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io,
    marker::PhantomData,
    path::PathBuf,
    pin::Pin,
    task::{ready, Context, Poll},
};
use thiserror::Error;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::ServerPayload;

pub(crate) struct LocalServer {
    listener: Listener,
}

impl LocalServer {
    pub async fn bind(socket_path: PathBuf) -> io::Result<Self> {
        let listener = socket_path
            .to_fs_name::<GenericFilePath>()
            .and_then(|name| {
                ListenerOptions::new()
                    .name(name)
                    .reclaim_name(true)
                    .create_tokio()
            })?;

        Ok(Self { listener })
    }

    pub async fn accept(&self) -> io::Result<(LocalServerReader, LocalServerWriter)> {
        let stream = self.listener.accept().await?;
        let (reader, writer) = stream.split();

        let reader = LocalReader {
            reader: FramedRead::new(reader, LengthDelimitedCodec::new()),
            _type: PhantomData,
        };

        let writer = LocalWriter {
            writer: FramedWrite::new(writer, LengthDelimitedCodec::new()),
            buffer: BytesMut::new().writer(),
            _type: PhantomData,
        };

        Ok((reader, writer))
    }
}

pub(crate) type LocalServerReader = LocalReader<Request>;
pub(crate) type LocalServerWriter = LocalWriter<ServerPayload>;

pub(crate) struct LocalReader<T> {
    reader: FramedRead<RecvHalf, LengthDelimitedCodec>,
    _type: PhantomData<fn() -> T>,
}

impl<T> Stream for LocalReader<T>
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

        Poll::Ready(Some(Ok(Message::decode(&mut buffer.reader())?)))
    }
}

pub(crate) struct LocalWriter<T> {
    writer: FramedWrite<SendHalf, LengthDelimitedCodec>,
    buffer: Writer<BytesMut>,
    _type: PhantomData<fn(&T)>,
}

impl<T> Sink<Message<T>> for LocalWriter<T>
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
        this.writer
            .start_send_unpin(this.buffer.get_mut().split().freeze())?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub(crate) enum ReadError {
    #[error("failed to receive message")]
    Receive(#[from] io::Error),
    #[error("failed to decode message")]
    Decode(#[from] DecodeError),
}

#[derive(Error, Debug)]
pub(crate) enum WriteError {
    #[error("failed to send message")]
    Send(#[from] io::Error),
    #[error("failed to encode message")]
    Encode(#[from] EncodeError),
}
