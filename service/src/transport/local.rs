use super::{ClientError, ReadError, WriteError};
use crate::protocol::{Message, Request};
use bytes::BytesMut;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use interprocess::local_socket::{
    tokio::{Listener, RecvHalf, SendHalf, Stream as Socket},
    traits::tokio::{Listener as _, Stream as _},
    GenericFilePath, ListenerOptions, ToFsName,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io,
    marker::PhantomData,
    path::Path,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::ServerPayload;

pub(crate) struct LocalServer {
    listener: Listener,
}

impl LocalServer {
    pub async fn bind(socket_path: &Path) -> io::Result<Self> {
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

        let reader = LocalReader::new(reader);
        let writer = LocalWriter::new(writer);

        Ok((reader, writer))
    }
}

pub async fn connect(
    socket_path: &Path,
) -> Result<(LocalClientReader, LocalClientWriter), ClientError> {
    let socket = Socket::connect(
        socket_path
            .to_fs_name::<GenericFilePath>()
            .map_err(|_| ClientError::InvalidSocketAddr)?,
    )
    .await
    .map_err(ClientError::Connect)?;

    let (reader, writer) = socket.split();

    let reader = LocalReader::new(reader);
    let writer = LocalWriter::new(writer);

    Ok((reader, writer))
}

pub(crate) type LocalServerReader = LocalReader<Request>;
pub(crate) type LocalServerWriter = LocalWriter<ServerPayload>;

pub type LocalClientReader = LocalReader<ServerPayload>;
pub type LocalClientWriter = LocalWriter<Request>;

pub struct LocalReader<T> {
    reader: FramedRead<RecvHalf, LengthDelimitedCodec>,
    _type: PhantomData<fn() -> T>,
}

impl<T> LocalReader<T> {
    pub(super) fn new(inner: RecvHalf) -> Self {
        Self {
            reader: FramedRead::new(inner, LengthDelimitedCodec::new()),
            _type: PhantomData,
        }
    }
}

impl<T> Stream for LocalReader<T>
where
    T: DeserializeOwned,
{
    type Item = Result<Message<T>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let buffer = match ready!(self.get_mut().reader.poll_next_unpin(cx)) {
            Some(Ok(buffer)) => buffer,
            Some(Err(error)) => return Poll::Ready(Some(Err(ReadError::Receive(error.into())))),
            None => return Poll::Ready(None),
        };

        Poll::Ready(Some(Ok(Message::decode(&mut buffer.freeze())?)))
    }
}

pub struct LocalWriter<T> {
    writer: FramedWrite<SendHalf, LengthDelimitedCodec>,
    buffer: BytesMut,
    _type: PhantomData<fn(T)>,
}

impl<T> LocalWriter<T> {
    pub(super) fn new(inner: SendHalf) -> Self {
        Self {
            writer: FramedWrite::new(inner, LengthDelimitedCodec::new()),
            buffer: BytesMut::new(),
            _type: PhantomData,
        }
    }
}

impl<T> Sink<Message<T>> for LocalWriter<T>
where
    T: Serialize,
{
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_ready_unpin(cx)).map_err(into_send_error))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_flush_unpin(cx)).map_err(into_send_error))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_close_unpin(cx)).map_err(into_send_error))
    }

    fn start_send(self: Pin<&mut Self>, item: Message<T>) -> Result<(), Self::Error> {
        let this = self.get_mut();

        item.encode(&mut this.buffer)?;
        this.writer
            .start_send_unpin(this.buffer.split().freeze())
            .map_err(into_send_error)?;

        Ok(())
    }
}

fn into_send_error(src: io::Error) -> WriteError {
    WriteError::Send(src.into())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures_util::SinkExt;
    use tempfile::TempDir;
    use tokio_stream::StreamExt;

    use crate::{
        protocol::{Message, MessageId, Request, ServerPayload},
        test_utils::{self, ServiceRunner},
        transport, Service,
    };

    #[tokio::test]
    async fn sanity_check() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("sock");
        let store_dir = temp_dir.path().join("store");

        let service = Service::init(
            socket_path.clone(),
            temp_dir.path().join("config"),
            store_dir.clone(),
        )
        .await
        .unwrap();

        let runner = ServiceRunner::start(service);

        let (mut client_reader, mut client_writer) =
            transport::local::connect(&socket_path).await.unwrap();

        let message_id = MessageId::next();
        client_writer
            .send(Message {
                id: message_id,
                payload: Request::RepositoryGetStoreDir,
            })
            .await
            .unwrap();

        let message = client_reader.next().await.unwrap().unwrap();
        assert_eq!(message.id, message_id);

        let response = match message.payload {
            ServerPayload::Success(r) => r,
            ServerPayload::Failure(e) => panic!("unexpected failure: {e:?}"),
            ServerPayload::Notification(n) => panic!("unexpected notification: {n:?}"),
        };

        let value: PathBuf = response.try_into().unwrap();
        assert_eq!(value, store_dir);

        runner.stop().await.close().await;
    }
}
