use super::{ClientError, ReadError, WriteError};
use crate::protocol::{Message, Request, ResponseResult};
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
pub(crate) type LocalServerWriter = LocalWriter<ResponseResult>;

pub type LocalClientReader = LocalReader<ResponseResult>;
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
    use std::{
        collections::VecDeque,
        net::Ipv4Addr,
        path::{Path, PathBuf},
    };

    use assert_matches::assert_matches;
    use futures_util::SinkExt;
    use ouisync::{AccessSecrets, PeerAddr, ShareToken};
    use tempfile::TempDir;
    use tokio_stream::StreamExt;
    use tracing::Instrument;

    use crate::{
        file::FileHandle,
        protocol::{
            Message, MessageId, ProtocolError, RepositoryHandle, Request, Response, ResponseResult,
            ToErrorCode,
        },
        test_utils::{self, ServiceRunner},
        transport, Service,
    };

    use super::{LocalClientReader, LocalClientWriter};

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

        let mut client = TestClient::connect(&socket_path).await;

        let message_id = MessageId::next();
        let value: PathBuf = client
            .invoke(message_id, Request::RepositoryGetStoreDir)
            .await
            .unwrap();
        assert_eq!(client.unsolicited_responses, []);
        assert_eq!(value, store_dir);

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn notifications_on_local_changes() {
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

        let mut client = TestClient::connect(&socket_path).await;

        let repo_handle: RepositoryHandle = client
            .invoke(
                MessageId::next(),
                Request::RepositoryCreate {
                    name: "foo".to_string(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    dht: false,
                    pex: false,
                },
            )
            .await
            .unwrap();

        // Subscribe to repository event notifications
        let sub_id = MessageId::next();
        let () = client
            .invoke(sub_id, Request::RepositorySubscribe(repo_handle))
            .await
            .unwrap();
        assert_eq!(client.unsolicited_responses, []);

        // Modify the repo to trigger a notification
        let file_handle: FileHandle = client
            .invoke(
                MessageId::next(),
                Request::FileCreate {
                    repository: repo_handle,
                    path: "a.txt".to_owned(),
                },
            )
            .await
            .unwrap();
        let () = client
            .invoke(MessageId::next(), Request::FileClose(file_handle))
            .await
            .unwrap();

        let message = client.next().await.unwrap();
        assert_eq!(message.id, sub_id);
        assert_matches!(
            message.payload,
            ResponseResult::Success(Response::RepositoryEvent)
        );

        // Unsubscribe
        let () = client
            .invoke(MessageId::next(), Request::Unsubscribe(sub_id))
            .await
            .unwrap();

        // Drain any other notification events sent before the unsubscribe.
        while let Some(message) = client.unsolicited_responses.pop_front() {
            assert_eq!(message.id, sub_id);
            assert_matches!(
                message.payload,
                ResponseResult::Success(Response::RepositoryEvent)
            );
        }

        // Modify the repo to trigger another notification. Because we've unsubscribed, we should
        // not receive this one.
        let file_handle: FileHandle = client
            .invoke(
                MessageId::next(),
                Request::FileCreate {
                    repository: repo_handle,
                    path: "b.txt".to_owned(),
                },
            )
            .await
            .unwrap();
        let () = client
            .invoke(MessageId::next(), Request::FileClose(file_handle))
            .await
            .unwrap();

        // Verify we didn't receive any further notifications by sending some request and checking
        // we only received response to that request and nothing else.
        let _: PathBuf = client
            .invoke(MessageId::next(), Request::RepositoryGetStoreDir)
            .await
            .unwrap();
        assert_eq!(client.unsolicited_responses, []);

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn notifications_on_sync() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();

        // Create two separate services (A and B), each with its own client.
        let (socket_path_a, runner_a) = async {
            let socket_path = temp_dir.path().join("a.sock");
            let service = Service::init(
                socket_path.clone(),
                temp_dir.path().join("config_a"),
                temp_dir.path().join("store_a"),
            )
            .await
            .unwrap();
            let runner = ServiceRunner::start(service);

            (socket_path, runner)
        }
        .instrument(tracing::info_span!("a"))
        .await;

        let (socket_path_b, runner_b) = async {
            let socket_path = temp_dir.path().join("b.sock");
            let service = Service::init(
                socket_path.clone(),
                temp_dir.path().join("config_b"),
                temp_dir.path().join("store_b"),
            )
            .await
            .unwrap();
            let runner = ServiceRunner::start(service);

            (socket_path, runner)
        }
        .instrument(tracing::info_span!("b"))
        .await;

        let mut client_a = TestClient::connect(&socket_path_a).await;
        let mut client_b = TestClient::connect(&socket_path_b).await;

        let bind_addr = PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into());

        let () = client_a
            .invoke(MessageId::next(), Request::NetworkBind(vec![bind_addr]))
            .await
            .unwrap();

        let () = client_b
            .invoke(MessageId::next(), Request::NetworkBind(vec![bind_addr]))
            .await
            .unwrap();

        // Connect A and B
        let addrs_a: Vec<PeerAddr> = client_a
            .invoke(MessageId::next(), Request::NetworkGetListenerAddrs)
            .await
            .unwrap();
        let () = client_b
            .invoke(
                MessageId::next(),
                Request::NetworkAddUserProvidedPeers(addrs_a),
            )
            .await
            .unwrap();

        // Create repo shared between them
        let token = ShareToken::from(AccessSecrets::random_write());

        let repo_handle_a: RepositoryHandle = client_a
            .invoke(
                MessageId::next(),
                Request::RepositoryCreate {
                    name: "repo".to_owned(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(token.clone()),
                    dht: false,
                    pex: false,
                },
            )
            .await
            .unwrap();

        let repo_handle_b: RepositoryHandle = client_b
            .invoke(
                MessageId::next(),
                Request::RepositoryCreate {
                    name: "repo".to_owned(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(token),
                    dht: false,
                    pex: false,
                },
            )
            .await
            .unwrap();

        // B subscribes to the repo notifications
        let sub_id_b = MessageId::next();
        let () = client_b
            .invoke(sub_id_b, Request::RepositorySubscribe(repo_handle_b))
            .await
            .unwrap();

        // A makes some changes
        let _: FileHandle = client_a
            .invoke(
                MessageId::next(),
                Request::FileCreate {
                    repository: repo_handle_a,
                    path: "test.txt".to_owned(),
                },
            )
            .await
            .unwrap();

        // B syncs with A and observes notifications
        let message = client_b.next().await.unwrap();
        assert_eq!(message.id, sub_id_b);
        assert_matches!(
            message.payload,
            ResponseResult::Success(Response::RepositoryEvent)
        );

        runner_a.stop().await.close().await;
        runner_b.stop().await.close().await;
    }

    struct TestClient {
        reader: LocalClientReader,
        writer: LocalClientWriter,
        unsolicited_responses: VecDeque<Message<ResponseResult>>,
    }

    impl TestClient {
        async fn connect(socket_path: &Path) -> Self {
            let (reader, writer) = transport::local::connect(socket_path).await.unwrap();

            Self {
                reader,
                writer,
                unsolicited_responses: VecDeque::new(),
            }
        }

        async fn invoke<T>(
            &mut self,
            message_id: MessageId,
            request: Request,
        ) -> Result<T, ProtocolError>
        where
            T: TryFrom<Response>,
            T::Error: std::error::Error + ToErrorCode,
        {
            self.writer
                .send(Message {
                    id: message_id,
                    payload: request,
                })
                .await
                .unwrap();

            loop {
                let message = self.reader.next().await.unwrap().unwrap();
                if message.id == message_id {
                    break Result::from(message.payload)
                        .and_then(|response| response.try_into().map_err(ProtocolError::from));
                } else {
                    self.unsolicited_responses.push_back(message);
                }
            }
        }

        async fn next(&mut self) -> Option<Message<ResponseResult>> {
            if let Some(message) = self.unsolicited_responses.pop_front() {
                Some(message)
            } else {
                self.reader.next().await.transpose().unwrap()
            }
        }
    }
}
