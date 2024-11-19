/*
use crate::{
    error::Error,
    repository::RepositoryHolder,
    state::{CreateRepositoryMethod, State},
};
use async_trait::async_trait;
use ouisync_bridge::{
    protocol::remote::{v0, v1, Request, Response, ServerError},
    transport::SessionContext,
};
use ouisync_lib::{crypto::sign::Signature, AccessSecrets, RepositoryId, ShareToken};
use std::{
    iter,
    sync::{Arc, Weak},
};

#[derive(Clone)]
pub(crate) struct RemoteHandler {
    state: Weak<State>,
}

impl RemoteHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self {
            state: Arc::downgrade(&state),
        }
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for RemoteHandler {
    type Request = Request;
    type Response = Response;
    type Error = ServerError;

    async fn handle(
        &self,
        request: Self::Request,
        context: &SessionContext,
    ) -> Result<Self::Response, Self::Error> {
        tracing::debug!(?request);

        let Some(state) = self.state.upgrade() else {
            tracing::error!("can't handle request - shutting down");
            return Err(ServerError::ShuttingDown);
        };

        match request {
            // TODO: disable v0 eventually
            Request::V0(request) => {
                tracing::warn!("deprecated API version: v0");

                match request {
                    v0::Request::Mirror { share_token } => {
                        create_repository(
                            &state,
                            AccessSecrets::Blind {
                                id: *share_token.id(),
                            },
                        )
                        .await?;

                        Ok(().into())
                    }
                }
            }
            Request::V1(request) => match request {
                v1::Request::Create {
                    repository_id,
                    proof,
                } => {
                    verify_proof(context, &repository_id, &proof)?;
                    create_repository(&state, AccessSecrets::Blind { id: repository_id }).await?;

                    Ok(().into())
                }
                v1::Request::Delete {
                    repository_id,
                    proof,
                } => {
                    verify_proof(context, &repository_id, &proof)?;

                    let name = make_name(&repository_id);

                    state
                        .delete_repository(&name)
                        .await
                        .map_err(|error| ServerError::Internal(error.to_string()))?;

                    Ok(().into())
                }
                v1::Request::Exists { repository_id } => {
                    let name = make_name(&repository_id);

                    state
                        .repositories
                        .contains(&name)
                        .then_some(().into())
                        .ok_or(ServerError::NotFound)
                }
            },
        }
    }
}

fn verify_proof(
    context: &SessionContext,
    repository_id: &RepositoryId,
    proof: &Signature,
) -> Result<(), ServerError> {
    if repository_id
        .write_public_key()
        .verify(context.session_cookie.as_ref(), proof)
    {
        Ok(())
    } else {
        tracing::debug!("invalid proof");
        Err(ServerError::PermissionDenied)
    }
}

async fn create_repository(
    state: &State,
    secrets: AccessSecrets,
) -> Result<Option<Arc<RepositoryHolder>>, ServerError> {
    let name = make_name(secrets.id());
    let holder = match state
        .create_repository(
            CreateRepositoryMethod::Import {
                share_token: ShareToken::from(secrets).with_name(name),
            },
            None,
            None,
        )
        .await
    {
        Ok(holder) => holder,
        Err(Error::RepositoryExists) => return Ok(None),
        Err(error) => return Err(ServerError::Internal(error.to_string())),
    };

    // NOTE: DHT is disabled to prevent spamming the DHT when there is a lot of repos.
    // This is fine because the clients add the storage servers as user-provided peers.
    // TODO: After we address https://github.com/equalitie/ouisync/issues/128 we should
    // consider enabling it again.
    holder.registration.set_dht_enabled(false).await;
    holder.registration.set_pex_enabled(true).await;

    Ok(Some(holder))
}

// Derive name from the hash of repository id
fn make_name(id: &RepositoryId) -> String {
    insert_separators(
        &id.salted_hash(b"ouisync server repository name")
            .to_string(),
    )
}

fn insert_separators(input: &str) -> String {
    let chunk_count = 4;
    let chunk_len = 2;
    let sep = '/';

    let (head, tail) = input.split_at(chunk_count * chunk_len);

    head.chars()
        .enumerate()
        .flat_map(|(i, c)| {
            (i > 0 && i < chunk_count * chunk_len && i % chunk_len == 0)
                .then_some(sep)
                .into_iter()
                .chain(iter::once(c))
        })
        .chain(iter::once(sep))
        .chain(tail.chars())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use ouisync_bridge::transport::{
        make_client_config, make_server_config, RemoteClient, RemoteServer,
    };
    use ouisync_lib::{crypto::sign::Keypair, AccessMode, WriteSecrets};
    use state_monitor::StateMonitor;
    use std::net::Ipv4Addr;
    use tempfile::TempDir;
    use tokio::task;
    use tokio_rustls::rustls::{
        pki_types::{CertificateDer, PrivatePkcs8KeyDer},
        ClientConfig,
    };

    #[test]
    fn insert_separators_test() {
        let input = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

        let expected_output = format!(
            "{}/{}/{}/{}/{}",
            &input[0..2],
            &input[2..4],
            &input[4..6],
            &input[6..8],
            &input[8..],
        );
        let actual_output = insert_separators(input);

        assert_eq!(actual_output, expected_output);
    }

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
}
*/
