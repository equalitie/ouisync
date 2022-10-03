//! Networking tests

mod common;

use self::common::{Env, Proto, DEFAULT_TIMEOUT};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{select, time};

#[tokio::test(flavor = "multi_thread")]
async fn peer_exchange() {
    let mut env = Env::with_seed(0);
    let proto = Proto::Quic;

    // B and C are initially connected only to A...
    let network_a = common::create_disconnected_peer(proto).await;
    let network_b =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_a)).await;
    let network_c =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_a)).await;

    let repo_a = env.create_repo().await;
    let reg_a = network_a.handle().register(repo_a.store().clone());
    reg_a.enable_pex();

    let repo_b = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_b = network_b.handle().register(repo_b.store().clone());
    reg_b.enable_pex();

    let repo_c = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_c = network_c.handle().register(repo_c.store().clone());
    reg_c.enable_pex();

    let addr_b = proto.listener_local_addr_v4(&network_b);
    let addr_c = proto.listener_local_addr_v4(&network_c);

    let mut rx_b = network_b.on_peer_set_change();
    let mut rx_c = network_c.on_peer_set_change();

    // ...eventually B and C connect to each other via peer exchange.
    let connected = async {
        loop {
            if network_b.knows_peer(addr_c) && network_c.knows_peer(addr_b) {
                break;
            }

            select! {
                result = rx_b.changed() => result.unwrap(),
                result = rx_c.changed() => result.unwrap(),
            }
        }
    };

    time::timeout(DEFAULT_TIMEOUT, connected).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_idle() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    network.handle().disable();
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_pending_connection() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    let remote_addr = proto.wrap_addr((Ipv4Addr::LOCALHOST, 12345).into());
    network.add_user_provided_peer(&remote_addr);

    // Wait until the connection starts begin established.
    let mut rx = network.on_peer_set_change();
    time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if network.knows_peer(remote_addr) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();

    network.handle().disable();
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_addr_takeover() {
    use tokio::net::UdpSocket;

    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    network.handle().disable();

    // Bind some other socket to the same address while the network is disabled.
    let _socket = time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if let Ok(socket) = UdpSocket::bind(local_addr_0.socket_addr()).await {
                break socket;
            } else {
                time::sleep(Duration::from_millis(250)).await;
            }
        }
    })
    .await
    .unwrap();

    // Enabling the network binds it to a different address.
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_ne!(local_addr_1, local_addr_0);
}

// Test for an edge case that used to panic.
#[tokio::test(flavor = "multi_thread")]
async fn dht_toggle() {
    let mut env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;

    let repo = env.create_repo().await;
    let reg = network.handle().register(repo.store().clone());

    reg.enable_dht();
    reg.disable_dht();
    reg.enable_dht();
}
