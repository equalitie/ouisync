use ouisync_bridge::{
    protocol::{Request, Response},
    transport::{
        // foreign::ForeignServer,
        local::{LocalClient, LocalServer},
        native::NativeClient,
        // remote::RemoteServer,
        Client,
        Server,
    },
    Handle, ServerState,
};
use ouisync_lib::{AccessMode, AccessSecrets, StateMonitor};
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use tokio::task;

#[tokio::test]
async fn native() {
    let (base_dir, server_state) = setup();
    let native_client = NativeClient::new(server_state.clone());
    sanity_check(base_dir.path(), &native_client).await
}

#[tokio::test]
async fn local() {
    let (base_dir, server_state) = setup();

    let local_socket_path = base_dir.path().join("socket");

    let local_server = LocalServer::bind(local_socket_path.as_path()).unwrap();
    task::spawn(local_server.run(server_state));

    let local_client = LocalClient::connect(local_socket_path.as_path())
        .await
        .unwrap();

    sanity_check(base_dir.path(), &local_client).await
}

// // let (_foreign_server, foreign_client_tx, foreign_client_rx) = ForeignServer::new();

// let remote_server = RemoteServer::bind((Ipv4Addr::LOCALHOST, 0).into())
//     .await
//     .unwrap();
// let remote_addr = remote_server.local_addr();
// // let remote_client = RemoteClient::connect(remote_addr).await.unwrap();

fn setup() -> (TempDir, Arc<ServerState>) {
    init_log();

    let base_dir = TempDir::new().unwrap();
    let root_monitor = StateMonitor::make_root();
    let server_state = ServerState::new(base_dir.path().join("config"), root_monitor).unwrap();
    let server_state = Arc::new(server_state);

    (base_dir, server_state)
}

async fn sanity_check(base_dir: &Path, client: &dyn Client) {
    let repo_path = base_dir.join("repo.db");
    let secrets = AccessSecrets::random_write();

    let response = client
        .invoke(Request::RepositoryCreate {
            path: repo_path.try_into().unwrap(),
            read_password: None,
            write_password: None,
            share_token: Some(secrets.clone().into()),
        })
        .await;
    let repo_handle = match response {
        Ok(Response::Handle(handle)) => handle,
        _ => panic!("unexpected response: {response:?}"),
    };

    let response = client
        .invoke(Request::RepositoryAccessMode(Handle::from_id(repo_handle)))
        .await;
    let access_mode = match response {
        Ok(Response::U8(n)) => AccessMode::try_from(n).unwrap(),
        _ => panic!("unexpected response: {response:?}"),
    };
    assert_eq!(access_mode, secrets.access_mode());

    // TODO: add more tests
}

fn init_log() {
    use tracing::metadata::LevelFilter;

    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                // Only show the logs if explicitly enabled with the `RUST_LOG` env variable.
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        // log output is captured by default and only shown on failure. Run tests with
        // `--nocapture` to override.
        .with_test_writer()
        .try_init()
        // error here most likely means the logger is already initialized. We can ignore that.
        .ok();
}

// struct FakeForeignClient {
//     inner:
// }
