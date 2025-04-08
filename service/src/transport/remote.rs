mod client;
mod protocol;
mod server;

pub use client::RemoteClient;
pub(crate) use server::{
    AcceptedRemoteConnection, RemoteServer, RemoteServerReader, RemoteServerWriter,
};

use std::{
    io::Cursor,
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use tokio_rustls::rustls::ConnectionCommon;
use tokio_tungstenite::tungstenite as ws;

use crate::protocol::Message;

use super::{ReadError, WriteError};

fn extract_session_cookie<Data>(connection: &ConnectionCommon<Data>) -> [u8; 32] {
    // unwrap is OK as the function fails only if called before TLS handshake or if the output
    // length is zero, none of which is the case here.
    connection
        .export_keying_material([0; 32], b"ouisync session cookie", None)
        .unwrap()
}

struct RemoteSocket<T, S> {
    inner: S,
    _type: PhantomData<fn(T) -> T>,
}

impl<T, S> RemoteSocket<T, S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            _type: PhantomData,
        }
    }
}

impl<T, S> Stream for RemoteSocket<T, S>
where
    T: DeserializeOwned,
    S: Stream<Item = Result<ws::Message, ws::Error>> + Unpin,
{
    type Item = Result<Message<T>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        let message = loop {
            match ready!(this.inner.poll_next_unpin(cx)) {
                Some(Ok(ws::Message::Binary(payload))) => break payload,
                Some(Ok(ws::Message::Close(_))) => continue,
                Some(Ok(
                    message @ (ws::Message::Text(_)
                    | ws::Message::Ping(_)
                    | ws::Message::Pong(_)
                    | ws::Message::Frame(_)),
                )) => {
                    tracing::debug!(?message, "unexpected message type");
                    continue;
                }
                Some(Err(error)) => {
                    return Poll::Ready(Some(Err(ReadError::Receive(error.into()))));
                }
                None => return Poll::Ready(None),
            }
        };

        let message = Message::decode(&mut Cursor::new(message))?;

        Poll::Ready(Some(Ok(message)))
    }
}

impl<T, S> Sink<Message<T>> for RemoteSocket<T, S>
where
    T: Serialize,
    S: Sink<ws::Message, Error = ws::Error> + Unpin,
{
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().inner.poll_ready_unpin(cx)).map_err(into_write_error))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().inner.poll_flush_unpin(cx)).map_err(into_write_error))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().inner.poll_close_unpin(cx)).map_err(into_write_error))
    }

    fn start_send(self: Pin<&mut Self>, message: Message<T>) -> Result<(), Self::Error> {
        let this = self.get_mut();

        let mut buffer = Vec::new();
        message.encode(&mut buffer)?;

        this.inner
            .start_send_unpin(ws::Message::Binary(buffer))
            .map_err(into_write_error)
    }
}

fn into_write_error(src: ws::Error) -> WriteError {
    WriteError::Send(src.into())
}

#[cfg(test)]
mod tests {
    use super::RemoteClient;
    use std::{
        net::{Ipv4Addr, Ipv6Addr},
        sync::Arc,
    };

    use assert_matches::assert_matches;
    use ouisync::{PeerAddr, WriteSecrets};
    use tempfile::TempDir;
    use tokio::fs;
    use tokio_rustls::rustls::{self, ClientConfig};

    use crate::{
        test_utils::{self, ServiceRunner},
        Service,
    };

    #[tokio::test]
    async fn create_ok() {
        test_utils::init_log();

        let (_temp_dir, runner, mut client) = setup().await;
        let secrets = WriteSecrets::random();

        client.create_mirror(&secrets).await.unwrap();

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn create_duplicate() {
        let (_temp_dir, runner, mut client) = setup().await;
        let secrets = WriteSecrets::random();

        assert_matches!(client.create_mirror(&secrets).await, Ok(_));

        let service = runner.stop().await;
        let repos_before = service.state().session_list_repositories();

        let runner = ServiceRunner::start(service);

        // Create is idempotent so this still returns Ok
        assert_matches!(client.create_mirror(&secrets).await, Ok(()));

        let mut service = runner.stop().await;
        let repos_after = service.state().session_list_repositories();
        assert_eq!(repos_before, repos_after);

        service.close().await;
    }

    #[tokio::test]
    async fn delete_present() {
        let (_temp_dir, runner, mut client) = setup().await;
        let secrets = WriteSecrets::random();

        client.create_mirror(&secrets).await.unwrap();

        let service = runner.stop().await;
        assert_eq!(service.state().session_list_repositories().len(), 1);

        let runner = ServiceRunner::start(service);
        client.delete_mirror(&secrets).await.unwrap();

        let mut service = runner.stop().await;
        assert_eq!(service.state().session_list_repositories().len(), 0);

        service.close().await;
    }

    #[tokio::test]
    async fn delete_missing() {
        let (_temp_dir, runner, mut client) = setup().await;

        let secrets = WriteSecrets::random();

        // Delete is idempotent so this still return Ok
        assert_matches!(client.delete_mirror(&secrets).await, Ok(()));

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn exists_present() {
        test_utils::init_log();

        let (_temp_dir, runner, mut client) = setup().await;
        let secrets = WriteSecrets::random();

        client.create_mirror(&secrets).await.unwrap();

        assert_matches!(client.mirror_exists(&secrets.id).await, Ok(true));

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn exists_missing() {
        let (_temp_dir, runner, mut client) = setup().await;
        let repository_id = WriteSecrets::random().id;

        assert_matches!(client.mirror_exists(&repository_id).await, Ok(false));

        runner.stop().await.close().await;
    }

    #[tokio::test]
    async fn get_listener_addrs() {
        let (_temp_dir, runner, mut client) = setup().await;

        assert_matches!(
            client.get_listener_addrs().await,
            Ok(addrs) => assert_eq!(addrs, vec![])
        );

        let service = runner.stop().await;
        service
            .state()
            .session_bind_network(vec![
                PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into()),
                PeerAddr::Quic((Ipv6Addr::LOCALHOST, 0).into()),
            ])
            .await;
        let expected_addrs = service.state().network.listener_local_addrs();

        let runner = ServiceRunner::start(service);

        assert_matches!(
            client.get_listener_addrs().await,
            Ok(actual_addrs) => assert_eq!(actual_addrs, expected_addrs)
        );

        runner.stop().await.close().await;
    }

    pub(super) async fn setup() -> (TempDir, ServiceRunner, RemoteClient) {
        let (temp_dir, service, server_addr, client_config) = setup_service().await;
        let runner = ServiceRunner::start(service);

        let client = RemoteClient::connect(&server_addr, client_config)
            .await
            .unwrap();

        (temp_dir, runner, client)
    }

    pub(super) async fn setup_service() -> (TempDir, Service, String, Arc<ClientConfig>) {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().join("config");

        let cert_key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();

        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("cert.pem"), cert_key.cert.pem())
            .await
            .unwrap();
        fs::write(
            config_dir.join("key.pem"),
            cert_key.key_pair.serialize_pem(),
        )
        .await
        .unwrap();

        let mut root_cert_store = rustls::RootCertStore::empty();
        root_cert_store.add(cert_key.cert.into()).unwrap();

        let remote_client_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_cert_store)
                .with_no_client_auth(),
        );

        let mut service = Service::init(config_dir).await.unwrap();

        service
            .set_store_dir(temp_dir.path().join("store"))
            .await
            .unwrap();

        let remote_port = service
            .state()
            .session_bind_remote_control(Some((Ipv4Addr::LOCALHOST, 0).into()))
            .await
            .unwrap();

        (
            temp_dir,
            service,
            format!("localhost:{remote_port}"),
            remote_client_config,
        )
    }
}
