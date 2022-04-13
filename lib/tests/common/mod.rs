use ouisync::{
    AccessSecrets, ConfigStore, MasterSecret, Network, NetworkOptions, Repository, Store,
};
use rand::{rngs::StdRng, Rng};
use std::net::{Ipv4Addr, SocketAddr};

// Create two `Network` instances connected together.
pub(crate) async fn create_connected_peers() -> (Network, Network) {
    let a = Network::new(&test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let b = create_peer_connected_to(*a.listener_local_addr()).await;

    (a, b)
}

// Create a `Network` instance connected only to the given address.
pub(crate) async fn create_peer_connected_to(addr: SocketAddr) -> Network {
    Network::new(
        &NetworkOptions {
            peers: vec![addr],
            ..test_network_options()
        },
        ConfigStore::null(),
    )
    .await
    .unwrap()
}

pub(crate) async fn create_repo(rng: &mut StdRng) -> Repository {
    let secrets = AccessSecrets::generate_write(rng);
    create_repo_with_secrets(rng, secrets).await
}

pub(crate) async fn create_repo_with_secrets(
    rng: &mut StdRng,
    secrets: AccessSecrets,
) -> Repository {
    Repository::create(
        &Store::Temporary,
        rng.gen(),
        MasterSecret::generate(rng),
        secrets,
        false,
    )
    .await
    .unwrap()
}

pub(crate) fn test_network_options() -> NetworkOptions {
    NetworkOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        disable_local_discovery: true,
        disable_upnp: true,
        disable_dht: true,
        ..Default::default()
    }
}
