use crate::{
    repository::{RepositoryHolder, RepositoryName, OPEN_ON_START},
    state::State,
};
use async_trait::async_trait;
use ouisync_bridge::{
    protocol::remote::{Request, Response, ServerError},
    transport::SessionContext,
};
use ouisync_lib::{AccessSecrets, RepositoryId, ShareToken};
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
            Request::Mirror {
                repository_id,
                proof,
            } => {
                if !repository_id
                    .write_public_key()
                    .verify(context.session_cookie.as_ref(), &proof)
                {
                    tracing::debug!("invalid mirror proof");
                    return Err(ServerError::InvalidArgument);
                }

                // Mirroring is supported for blind replicas only.
                let share_token = ShareToken::from(AccessSecrets::Blind { id: repository_id });
                let name = make_name(share_token.id());

                // Mirror is idempotent
                if state.repositories.contains(&name) {
                    return Ok(().into());
                }

                let store_path = state.store_path(name.as_ref());

                let repository = ouisync_bridge::repository::create(
                    store_path,
                    None,
                    None,
                    Some(share_token),
                    &state.config,
                    &state.repositories_monitor,
                )
                .await
                .map_err(|error| ServerError::CreateRepository(error.to_string()))?;

                tracing::info!(%name, "repository created");

                let holder = RepositoryHolder::new(repository, name, &state.network).await;
                let holder = Arc::new(holder);

                // Mirror is idempotent
                if !state.repositories.try_insert(holder.clone()) {
                    return Ok(().into());
                }

                holder
                    .repository
                    .metadata()
                    .set(OPEN_ON_START, true)
                    .await
                    .ok();

                // NOTE: DHT is disabled to prevent spamming the DHT when there is a lot of repos.
                // This is fine because the clients add the storage servers as user-provided peers.
                // TODO: After we address https://github.com/equalitie/ouisync/issues/128 we should
                // consider enabling it again.
                holder.registration.set_dht_enabled(false).await;
                holder.registration.set_pex_enabled(true).await;

                Ok(().into())
            }
        }
    }
}

// Derive name from the hash of repository id
fn make_name(id: &RepositoryId) -> RepositoryName {
    RepositoryName::try_from(insert_separators(
        &id.salted_hash(b"ouisync server repository name")
            .to_string(),
    ))
    .unwrap()
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
    use crate::options::Dirs;
    use assert_matches::assert_matches;
    use ouisync_bridge::transport::{
        make_client_config, make_server_config, RemoteClient, RemoteServer,
    };
    use ouisync_lib::{crypto::sign::Keypair, AccessMode, WriteSecrets};
    use rustls::{Certificate, PrivateKey};
    use state_monitor::StateMonitor;
    use std::net::Ipv4Addr;
    use tempfile::TempDir;
    use tokio::task;

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
    async fn mirror_ok() {
        let (_temp_dir, state, client) = setup().await;

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());

        client
            .invoke(Request::Mirror {
                repository_id: secrets.id,
                proof,
            })
            .await
            .unwrap();

        let repo = state
            .repositories
            .get_all()
            .into_iter()
            .find(|repo| repo.repository.secrets().id() == &secrets.id)
            .unwrap();

        assert_eq!(repo.repository.access_mode(), AccessMode::Blind);
    }

    #[tokio::test]
    async fn mirror_invalid_proof() {
        let (_temp_dir, state, client) = setup().await;

        let repository_id = WriteSecrets::random().id;
        let invalid_write_keys = Keypair::random();
        let invalid_proof = invalid_write_keys.sign(client.session_cookie().as_ref());

        assert_matches!(
            client
                .invoke(Request::Mirror {
                    repository_id,
                    proof: invalid_proof
                })
                .await,
            Err(ServerError::InvalidArgument)
        );

        assert!(state.repositories.get_all().is_empty());
    }

    async fn setup() -> (TempDir, Arc<State>, RemoteClient) {
        let temp_dir = TempDir::new().unwrap();
        let dirs = Dirs {
            config_dir: temp_dir.path().join("config"),
            store_dir: temp_dir.path().join("store"),
            mount_dir: temp_dir.path().join("mount"),
        };

        let certs = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = Certificate(certs.serialize_der().unwrap());
        let private_key = PrivateKey(certs.serialize_private_key_der());

        let server_config = make_server_config(vec![cert.clone()], private_key).unwrap();
        let client_config = make_client_config(&[cert]).unwrap();

        let state = State::init(&dirs, StateMonitor::make_root()).await.unwrap();

        let server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into(), server_config)
            .await
            .unwrap();
        let server_addr = format!("localhost:{}", server.local_addr().port());
        task::spawn(server.run(RemoteHandler::new(state.clone())));

        let client = RemoteClient::connect(&server_addr, client_config)
            .await
            .unwrap();

        (temp_dir, state, client)
    }
}
