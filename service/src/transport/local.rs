mod addr;
mod auth;
mod client;
mod net;

pub use addr::LocalAddr;
pub use auth::AuthKey;
pub use client::LocalClient;

use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, ready},
};

use bytes::BytesMut;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use super::{ClientError, ReadError, WriteError};
use crate::protocol::{Message, Request, ResponseResult};
use net::{LocalAccept, LocalListener, LocalOwnedReadHalf, LocalOwnedWriteHalf, LocalStream};

pub enum LocalTransport {
    Tcp,
    Unix,
}

#[expect(clippy::derivable_impls)] // false positive
impl Default for LocalTransport {
    fn default() -> Self {
        // TODO: use unix sockets on more platforms
        cfg_select! {
            target_os = "linux" => Self::Unix,
            _ => Self::Tcp,
        }
    }
}

pub(crate) struct LocalServer {
    listener: LocalListener,
    addr: LocalAddr,
}

impl LocalServer {
    pub async fn bind(addr: LocalAddr) -> io::Result<Self> {
        let listener = LocalListener::bind(addr).await?;
        let addr = listener.local_addr()?;

        Ok(Self { listener, addr })
    }

    /// Accept the next local client connection. The returned value needs to be finalized (
    /// [AcceptedLocalConnection::finalize]) before use.
    pub async fn accept(&self) -> io::Result<AcceptedLocalConnection> {
        let accept = self.listener.accept().await?;

        Ok(AcceptedLocalConnection { accept })
    }

    pub fn addr(&self) -> &LocalAddr {
        &self.addr
    }
}

pub(crate) struct AcceptedLocalConnection {
    accept: LocalAccept,
}

impl AcceptedLocalConnection {
    /// Finalize accepting the connection.
    pub async fn finalize(self) -> Option<(LocalServerReader, LocalServerWriter)> {
        let socket = self
            .accept
            .finalize()
            .await
            .inspect_err(|error| {
                tracing::debug!(?error, "failed to finalize accepted client connection")
            })
            .ok()?;

        let (reader, writer) = socket.into_split();
        let reader = LocalReader::new(reader);
        let writer = LocalWriter::new(writer);

        Some((reader, writer))
    }
}

pub async fn connect(
    addr: &LocalAddr,
) -> Result<(LocalClientReader, LocalClientWriter), ClientError> {
    let socket = LocalStream::connect(addr)
        .await
        .map_err(ClientError::Connect)?;

    let (reader, writer) = socket.into_split();
    let reader = LocalReader::new(reader);
    let writer = LocalWriter::new(writer);

    Ok((reader, writer))
}

pub(crate) type LocalServerReader = LocalReader<Request>;
pub(crate) type LocalServerWriter = LocalWriter<ResponseResult>;

pub type LocalClientReader = LocalReader<ResponseResult>;
pub type LocalClientWriter = LocalWriter<Request>;

pub struct LocalReader<T> {
    reader: FramedRead<LocalOwnedReadHalf, LengthDelimitedCodec>,
    _type: PhantomData<fn() -> T>,
}

impl<T> LocalReader<T> {
    pub(self) fn new(inner: LocalOwnedReadHalf) -> Self {
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
    writer: FramedWrite<LocalOwnedWriteHalf, LengthDelimitedCodec>,
    buffer: BytesMut,
    _type: PhantomData<fn(T)>,
}

impl<T> LocalWriter<T> {
    pub(self) fn new(inner: LocalOwnedWriteHalf) -> Self {
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
    use std::{collections::VecDeque, io, net::Ipv4Addr, path::PathBuf};

    use assert_matches::assert_matches;
    use futures_util::SinkExt;
    use ouisync::{AccessSecrets, PeerAddr, ShareToken};
    use tempfile::TempDir;
    use tokio_stream::StreamExt;
    use tracing::Instrument;

    use crate::{
        Service,
        file::FileHandle,
        protocol::{
            Message, MessageId, ProtocolError, RepositoryHandle, Request, Response, ResponseResult,
            ToErrorCode,
        },
        test_utils::{self, ServiceRunner},
        transport::{self, ClientError},
    };

    use super::{AuthKey, LocalAddr, LocalClientReader, LocalClientWriter, LocalTransport};

    #[tokio::test]
    async fn sanity_check_tcp() {
        sanity_check(LocalTransport::Tcp).await;
    }

    #[tokio::test]
    async fn sanity_check_unix() {
        sanity_check(LocalTransport::Unix).await;
    }

    async fn sanity_check(local_transport: LocalTransport) {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();
        let store_dir = temp_dir.path().join("store");

        let service = Service::init(temp_dir.path().join("config"), local_transport)
            .await
            .unwrap();
        let service_addr = service.local_addr().clone();
        service
            .state()
            .session_insert_store_dirs(vec![store_dir.clone()])
            .await
            .unwrap();

        let runner = ServiceRunner::start(service);

        let mut client = TestClient::connect(&service_addr).await;

        let message_id = MessageId::next();
        let value: Vec<PathBuf> = client
            .invoke(message_id, Request::SessionGetStoreDirs)
            .await
            .unwrap();
        assert_eq!(client.unsolicited_responses, []);
        assert_eq!(value, [store_dir]);

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn authentication() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();

        let service = Service::init(temp_dir.path().join("config"), LocalTransport::Tcp)
            .await
            .unwrap();

        let invalid_addr = LocalAddr::Tcp {
            addr: match service.local_addr() {
                LocalAddr::Tcp { addr, .. } => *addr,
                LocalAddr::Unix(_) => unreachable!(),
            },
            auth_key: AuthKey::random(),
        };

        let runner = ServiceRunner::start(service);

        match transport::local::connect(&invalid_addr).await {
            Err(ClientError::Connect(error)) if error.kind() == io::ErrorKind::PermissionDenied => {
            }
            Err(error) => panic!("unexpected error: {error:?}"),
            Ok(_) => panic!("unexpected success"),
        }

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn notifications_on_local_changes() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();
        let store_dir = temp_dir.path().join("store");

        let service = Service::init(temp_dir.path().join("config"), LocalTransport::Tcp)
            .await
            .unwrap();
        let service_addr = service.local_addr().clone();
        service
            .state()
            .session_insert_store_dirs(vec![store_dir])
            .await
            .unwrap();

        let runner = ServiceRunner::start(service);

        let mut client = TestClient::connect(&service_addr).await;

        let repo_handle: RepositoryHandle = client
            .invoke(
                MessageId::next(),
                Request::SessionCreateRepository {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    sync_enabled: false,
                    dht_enabled: false,
                    pex_enabled: false,
                },
            )
            .await
            .unwrap();

        // Subscribe to repository event notifications
        let sub_id = MessageId::next();
        let () = client
            .invoke(sub_id, Request::RepositorySubscribe { repo: repo_handle })
            .await
            .unwrap();
        assert_eq!(client.unsolicited_responses, []);

        // Modify the repo to trigger a notification
        let file_handle: FileHandle = client
            .invoke(
                MessageId::next(),
                Request::RepositoryCreateFile {
                    repo: repo_handle,
                    path: "a.txt".to_owned(),
                },
            )
            .await
            .unwrap();
        let () = client
            .invoke(MessageId::next(), Request::FileClose { file: file_handle })
            .await
            .unwrap();

        let message = client.next().await.unwrap();
        assert_eq!(message.id, sub_id);
        assert_matches!(message.payload, ResponseResult::Success(Response::Unit));

        // Unsubscribe
        let response: bool = client
            .invoke(MessageId::next(), Request::Cancel { id: sub_id })
            .await
            .unwrap();
        assert!(response);

        // Drain any other notification events sent before the unsubscribe.
        while let Some(message) = client.unsolicited_responses.pop_front() {
            assert_eq!(message.id, sub_id);
            assert_matches!(message.payload, ResponseResult::Success(Response::Unit));
        }

        // Modify the repo to trigger another notification. Because we've unsubscribed, we should
        // not receive this one.
        let file_handle: FileHandle = client
            .invoke(
                MessageId::next(),
                Request::RepositoryCreateFile {
                    repo: repo_handle,
                    path: "b.txt".to_owned(),
                },
            )
            .await
            .unwrap();
        let () = client
            .invoke(MessageId::next(), Request::FileClose { file: file_handle })
            .await
            .unwrap();

        // Verify we didn't receive any further notifications by sending some request and checking
        // we only received response to that request and nothing else.
        let _: Vec<PathBuf> = client
            .invoke(MessageId::next(), Request::SessionGetStoreDirs)
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
        let (addr_a, runner_a) = async {
            let service = Service::init(temp_dir.path().join("config_a"), LocalTransport::Tcp)
                .await
                .unwrap();
            let addr = service.local_addr().clone();
            service
                .state()
                .session_insert_store_dirs(vec![temp_dir.path().join("store_a")])
                .await
                .unwrap();
            let runner = ServiceRunner::start(service);

            (addr, runner)
        }
        .instrument(tracing::info_span!("a"))
        .await;

        let (addr_b, runner_b) = async {
            let service = Service::init(temp_dir.path().join("config_b"), LocalTransport::Tcp)
                .await
                .unwrap();
            let addr = service.local_addr().clone();
            service
                .state()
                .session_insert_store_dirs(vec![temp_dir.path().join("store_b")])
                .await
                .unwrap();
            let runner = ServiceRunner::start(service);

            (addr, runner)
        }
        .instrument(tracing::info_span!("b"))
        .await;

        let mut client_a = TestClient::connect(&addr_a).await;
        let mut client_b = TestClient::connect(&addr_b).await;

        let bind_addr = PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into());

        let () = client_a
            .invoke(
                MessageId::next(),
                Request::SessionBindNetwork {
                    addrs: vec![bind_addr],
                },
            )
            .await
            .unwrap();

        let () = client_b
            .invoke(
                MessageId::next(),
                Request::SessionBindNetwork {
                    addrs: vec![bind_addr],
                },
            )
            .await
            .unwrap();

        // Connect A and B
        let addrs_a: Vec<PeerAddr> = client_a
            .invoke(MessageId::next(), Request::SessionGetLocalListenerAddrs)
            .await
            .unwrap();
        let () = client_b
            .invoke(
                MessageId::next(),
                Request::SessionAddUserProvidedPeers { addrs: addrs_a },
            )
            .await
            .unwrap();

        // Create repo shared between them
        let token = ShareToken::from(AccessSecrets::random_write());

        let repo_handle_a: RepositoryHandle = client_a
            .invoke(
                MessageId::next(),
                Request::SessionCreateRepository {
                    path: "repo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(token.clone()),
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
            )
            .await
            .unwrap();

        let repo_handle_b: RepositoryHandle = client_b
            .invoke(
                MessageId::next(),
                Request::SessionCreateRepository {
                    path: "repo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(token),
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
            )
            .await
            .unwrap();

        // B subscribes to the repo notifications
        let sub_id_b = MessageId::next();
        let () = client_b
            .invoke(
                sub_id_b,
                Request::RepositorySubscribe {
                    repo: repo_handle_b,
                },
            )
            .await
            .unwrap();

        // A makes some changes
        let _: FileHandle = client_a
            .invoke(
                MessageId::next(),
                Request::RepositoryCreateFile {
                    repo: repo_handle_a,
                    path: "test.txt".to_owned(),
                },
            )
            .await
            .unwrap();

        // B syncs with A and observes notifications
        let message = client_b.next().await.unwrap();
        assert_eq!(message.id, sub_id_b);
        assert_matches!(message.payload, ResponseResult::Success(Response::Unit));

        runner_a.stop().await.close().await;
        runner_b.stop().await.close().await;
    }

    struct TestClient {
        reader: LocalClientReader,
        writer: LocalClientWriter,
        unsolicited_responses: VecDeque<Message<ResponseResult>>,
    }

    impl TestClient {
        async fn connect(addr: &LocalAddr) -> Self {
            let (reader, writer) = transport::local::connect(addr).await.unwrap();

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
