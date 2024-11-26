mod client;
mod protocol;
mod server;

pub use client::{RemoteClient, RemoteClientError};
pub(crate) use server::{RemoteServer, RemoteServerReader, RemoteServerWriter};

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
    use std::{net::Ipv4Addr, slice};

    use crate::{
        test_utils::{self, ServiceRunner},
        transport::remote::RemoteClient,
        Defaults, Service,
    };
    use ouisync::WriteSecrets;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn create_ok() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("sock");
        let config_dir = temp_dir.path().join("config");

        let gen = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = gen.cert.der();

        fs::create_dir_all(&config_dir).await.unwrap();
        fs::write(config_dir.join("cert.pem"), gen.cert.pem())
            .await
            .unwrap();
        fs::write(config_dir.join("key.pem"), gen.key_pair.serialize_pem())
            .await
            .unwrap();

        let client_config =
            ouisync_bridge::transport::make_client_config(slice::from_ref(cert)).unwrap();

        let mut service = Service::init(
            socket_path.clone(),
            config_dir,
            Defaults {
                store_dir: temp_dir.path().join("store"),
                mount_dir: temp_dir.path().join("mnt"),
                bind: vec![],
                local_discovery_enabled: false,
                port_forwarding_enabled: false,
            },
        )
        .await
        .unwrap();

        let port = service
            .bind_remote_control(Some((Ipv4Addr::LOCALHOST, 0).into()))
            .await
            .unwrap();

        let runner = ServiceRunner::start(service);

        let mut remote_client = RemoteClient::connect(&format!("localhost:{port}"), client_config)
            .await
            .unwrap();

        let secrets = WriteSecrets::random();
        remote_client.create_mirror(&secrets).await.unwrap();

        runner.stop().await.close().await;
    }

    /*
    use super::*;
    use assert_matches::assert_matches;
    use ouisync::{crypto::sign::Keypair, AccessMode, WriteSecrets};
    use ouisync_bridge::transport::{
        make_client_config, make_server_config, RemoteClient, RemoteServer,
    };
    use state_monitor::StateMonitor;
    use std::net::Ipv4Addr;
    use tokio::task;
    use tokio_rustls::rustls::{
        pki_types::{CertificateDer, PrivatePkcs8KeyDer},
        ClientConfig,
    };

    #[tokio::test]
    async fn create_ok() {
        let (_temp_dir, state, client) = setup().await;

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(v1::Request::Create {
                    repository_id: secrets.id,
                    proof,
                })
                .await,
            Ok(())
        );

        let repo = state
            .repositories
            .get_all()
            .into_iter()
            .find(|repo| repo.repository.secrets().id() == &secrets.id)
            .unwrap();

        assert_eq!(repo.repository.access_mode(), AccessMode::Blind);
    }

    #[tokio::test]
    async fn create_is_idempotent() {
        let (_temp_dir, state, client) = setup().await;

        let secrets = WriteSecrets::random();

        let holder = create_repository(&state, AccessSecrets::Write(secrets.clone()))
            .await
            .unwrap()
            .unwrap();

        // Add some content to the repo so we can verify that it's not overwritten on repeated
        // creations.
        let mut file = holder.repository.create_file("test.txt").await.unwrap();
        file.write(b"hello world").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        // Create is idempotent so this still returns `Ok`.
        assert_matches!(
            client
                .invoke(v1::Request::Create {
                    repository_id: secrets.id,
                    proof,
                })
                .await,
            Ok(())
        );

        assert_eq!(
            holder
                .repository
                .open_file("test.txt")
                .await
                .unwrap()
                .read_to_end()
                .await
                .unwrap(),
            b"hello world"
        );
    }

    #[tokio::test]
    async fn create_invalid_proof() {
        let (_temp_dir, state, client) = setup().await;

        let repository_id = WriteSecrets::random().id;
        let invalid_write_keys = Keypair::random();
        let invalid_proof = invalid_write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(v1::Request::Create {
                    repository_id,
                    proof: invalid_proof
                })
                .await,
            Err(ServerError::PermissionDenied)
        );

        assert!(state.repositories.get_all().is_empty());
    }

    #[tokio::test]
    async fn delete_present() {
        let (_temp_dir, state, client) = setup().await;

        let secrets = WriteSecrets::random();

        create_repository(&state, AccessSecrets::Blind { id: secrets.id })
            .await
            .unwrap()
            .unwrap();

        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(v1::Request::Delete {
                    repository_id: secrets.id,
                    proof
                })
                .await,
            Ok(())
        );

        assert!(state.repositories.get_all().is_empty());
    }

    #[tokio::test]
    async fn delete_missing() {
        let (_temp_dir, _state, client) = setup().await;

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        // Delete is idempotent so this still returns `Ok`
        assert_matches!(
            client
                .invoke(v1::Request::Delete {
                    repository_id: secrets.id,
                    proof
                })
                .await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn delete_invalid_proof() {
        let (_temp_dir, state, client) = setup().await;

        let secrets = WriteSecrets::random();

        create_repository(&state, AccessSecrets::Blind { id: secrets.id })
            .await
            .unwrap()
            .unwrap();

        let invalid_write_keys = Keypair::random();
        let invalid_proof = invalid_write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(v1::Request::Delete {
                    repository_id: secrets.id,
                    proof: invalid_proof,
                })
                .await,
            Err(ServerError::PermissionDenied)
        );

        assert!(state.repositories.get_all().into_iter().any(|holder| holder
            .repository
            .secrets()
            .id()
            == &secrets.id));
    }

    #[tokio::test]
    async fn exists_present() {
        let (_temp_dir, state, client) = setup().await;
        let repository_id = WriteSecrets::random().id;

        create_repository(&state, AccessSecrets::Blind { id: repository_id })
            .await
            .unwrap()
            .unwrap();

        assert_matches!(
            client.invoke(v1::Request::Exists { repository_id }).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn exists_missing() {
        let (_temp_dir, _state, client) = setup().await;
        let repository_id = WriteSecrets::random().id;

        assert_matches!(
            client.invoke(v1::Request::Exists { repository_id }).await,
            Err(ServerError::NotFound)
        );
    }

    #[tokio::test]
    async fn proof_replay_attack() {
        let (_temp_dir, _state, server_addr, client_config) = setup_server().await;

        let client0 = RemoteClient::connect(&server_addr, client_config.clone())
            .await
            .unwrap();
        let client1 = RemoteClient::connect(&server_addr, client_config.clone())
            .await
            .unwrap();

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(client0.session_cookie().as_ref());

        // Attempt to invoke the request using a proof leaked from another client.
        assert_matches!(
            client1
                .invoke(v1::Request::Create {
                    repository_id: secrets.id,
                    proof
                })
                .await,
            Err(ServerError::PermissionDenied)
        );
    }

    async fn setup() -> (TempDir, Arc<State>, RemoteClient) {
        let (temp_dir, state, server_addr, client_config) = setup_server().await;

        let client = RemoteClient::connect(&server_addr, client_config)
            .await
            .unwrap();

        (temp_dir, state, client)
    }

    async fn setup_server() -> (TempDir, Arc<State>, String, Arc<ClientConfig>) {
        let temp_dir = TempDir::new().unwrap();
        let dirs = Dirs {
            config_dir: temp_dir.path().join("config"),
            store_dir: temp_dir.path().join("store"),
            mount_dir: temp_dir.path().join("mount"),
        };

        let gen = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = CertificateDer::from(gen.cert);
        let private_key = PrivatePkcs8KeyDer::from(gen.key_pair.serialize_der());

        let server_config = make_server_config(vec![cert.clone()], private_key.into()).unwrap();
        let client_config = make_client_config(&[cert]).unwrap();

        let state = State::init(&dirs, StateMonitor::make_root()).await.unwrap();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let server_addr = format!("localhost:{}", server.local_addr().port());

        task::spawn(server.run(RemoteHandler::new(state.clone())));

        (temp_dir, state, server_addr, client_config)
    }
    */
}
