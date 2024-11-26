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
