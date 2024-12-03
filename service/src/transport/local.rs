use super::{ClientError, ReadError, WriteError};
use crate::protocol::{Message, Request, ServerPayload};
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
    use assert_matches::assert_matches;
    use futures_util::SinkExt;
    use tempfile::TempDir;
    use tokio_stream::StreamExt;

    use crate::{
        protocol::{Message, MessageId, Request, Response, ServerPayload},
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
        let value = assert_matches!(message.payload, ServerPayload::Success(Response::Path(value)) => value);
        assert_eq!(value, store_dir);

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn notifications() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("sock");
        let store_dir = temp_dir.path().join("store");

        let mut service = Service::init(
            socket_path.clone(),
            temp_dir.path().join("config"),
            store_dir.clone(),
        )
        .await
        .unwrap();

        let repo_handle = service
            .state_mut()
            .create_repository("foo".to_string(), None, None, None, false, false)
            .await
            .unwrap();

        let runner = ServiceRunner::start(service);

        let (mut client_reader, mut client_writer) =
            transport::local::connect(&socket_path).await.unwrap();

        // Subscribe to repository event notifications
        let subscribe_message_id = MessageId::next();
        client_writer
            .send(Message {
                id: subscribe_message_id,
                payload: Request::RepositorySubscribe(repo_handle),
            })
            .await
            .unwrap();

        let message = client_reader.next().await.unwrap().unwrap();
        assert_eq!(message.id, subscribe_message_id);
        assert_matches!(message.payload, ServerPayload::Success(Response::None));

        let service = runner.stop().await;

        // Modify the repo to trigger a notification
        let repo = service.state().get_repository(repo_handle).unwrap();
        repo.create_file("a.txt").await.unwrap();

        let runner = ServiceRunner::start(service);

        let message = client_reader.next().await.unwrap().unwrap();
        assert_eq!(message.id, subscribe_message_id);
        assert_matches!(
            message.payload,
            ServerPayload::Success(Response::RepositoryEvent)
        );

        // Unsubscribe
        let unsubscribe_message_id = MessageId::next();
        client_writer
            .send(Message {
                id: unsubscribe_message_id,
                payload: Request::Unsubscribe(subscribe_message_id),
            })
            .await
            .unwrap();

        // There could have been further notification events sent before the unsubscribe, ignore
        // them.
        loop {
            let message = client_reader.next().await.unwrap().unwrap();
            if message.id == unsubscribe_message_id {
                assert_matches!(message.payload, ServerPayload::Success(Response::None));
                break;
            } else if message.id == subscribe_message_id {
                assert_matches!(
                    message.payload,
                    ServerPayload::Success(Response::RepositoryEvent)
                );
                continue;
            } else {
                panic!(
                    "unexpected message id: {:?}, expecting {:?} or {:?}",
                    message.id, subscribe_message_id, unsubscribe_message_id
                );
            }
        }

        let service = runner.stop().await;

        // Modify the repo to trigger another notification. Because we've unsubscribed, we should
        // not receive this one.
        let repo = service.state().get_repository(repo_handle).unwrap();
        repo.create_file("b.txt").await.unwrap();

        let runner = ServiceRunner::start(service);

        // Send some message whose id is different than what the subscription had. Then when we
        // receive a response with that id we know the subscription has been cancelled before,
        // otherwise we would have received the notification first.
        let other_message_id = MessageId::next();
        client_writer
            .send(Message {
                id: other_message_id,
                payload: Request::RepositoryGetStoreDir,
            })
            .await
            .unwrap();

        let message = client_reader.next().await.unwrap().unwrap();
        assert_eq!(message.id, other_message_id);

        runner.stop().await.close().await;
    }
}
