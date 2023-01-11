//! Networking tests

mod common;

use self::common::{old, Env, NetworkExt, Proto, TEST_TIMEOUT};
use ouisync::{network::Network, AccessSecrets};
use std::{net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{select, sync::Barrier, time};

// This test requires QUIC which is not yet supported in simulation
#[cfg(not(feature = "simulation"))]
#[test]
fn peer_exchange() {
    let mut env = Env::new();
    env.set_proto(Proto::Quic); // PEX doesn't work with TCP

    let secrets = AccessSecrets::random_write();
    let barrier = Arc::new(Barrier::new(3));

    // Bob and Carol are initially connected only to Alice but eventually they connect to each
    // other via peer exchange.

    env.actor("alice", {
        let secrets = secrets.clone();
        let barrier = barrier.clone();

        async move {
            let (_network, _repo, reg) = common::setup_actor(secrets).await;
            reg.enable_pex();

            barrier.wait().await;
        }
    });

    env.actor("bob", {
        let secrets = secrets.clone();
        let barrier = barrier.clone();

        async move {
            let (network, _repo, reg) = common::setup_actor(secrets).await;
            network.connect("alice");
            reg.enable_pex();

            expect_knows(&network, "carol").await;
            barrier.wait().await;
        }
    });

    env.actor("carol", {
        async move {
            let (network, _repo, reg) = common::setup_actor(secrets).await;
            network.connect("alice");
            reg.enable_pex();

            expect_knows(&network, "bob").await;
            barrier.wait().await;
        }
    });
}

#[test]
fn network_disable_enable_idle() {
    let mut env = Env::new();
    let proto = Proto::Quic;
    env.set_proto(proto);

    env.actor("only", async move {
        let bind_addr = proto.wrap((Ipv4Addr::LOCALHOST, 0));
        let network = common::create_unbound_network();

        network.handle().bind(&[bind_addr]).await;
        let local_addr_0 = proto.listener_local_addr_v4(&network);

        network.handle().bind(&[]).await;
        network.handle().bind(&[bind_addr]).await;
        let local_addr_1 = proto.listener_local_addr_v4(&network);

        assert_eq!(local_addr_1, local_addr_0);
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_pending_connection() {
    let mut env = old::Env::new();
    let proto = Proto::Quic;
    let bind = proto.wrap((Ipv4Addr::LOCALHOST, 0));

    let node = env.create_node(bind).await;
    let local_addr_0 = proto.listener_local_addr_v4(&node);

    let remote_addr = proto.wrap((Ipv4Addr::LOCALHOST, 12345));
    node.add_user_provided_peer(&remote_addr);

    // Wait until the connection starts begin established.
    let mut rx = node.on_peer_set_change();
    time::timeout(TEST_TIMEOUT, async {
        loop {
            if node.knows_peer(remote_addr) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();

    node.handle().bind(&[]).await;
    node.handle().bind(&[bind]).await;

    let local_addr_1 = proto.listener_local_addr_v4(&node);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_addr_takeover() {
    use tokio::net::UdpSocket;

    let mut env = old::Env::new();
    let proto = Proto::Quic;
    let bind = proto.wrap((Ipv4Addr::LOCALHOST, 0));

    let node = env.create_node(bind).await;
    let local_addr_0 = proto.listener_local_addr_v4(&node);

    node.handle().bind(&[]).await;

    // Bind some other socket to the same address while the network is disabled.
    let _socket = time::timeout(TEST_TIMEOUT, async {
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

    // Enabling the network binds it to a different port.
    node.handle().bind(&[bind]).await;

    let local_addr_1 = proto.listener_local_addr_v4(&node);
    assert_ne!(local_addr_1, local_addr_0);
}

// Test for an edge case that used to panic.
#[tokio::test(flavor = "multi_thread")]
async fn dht_toggle() {
    let mut env = old::Env::new();
    let proto = Proto::Quic;

    let node = env.create_node(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;

    let repo = env.create_repo().await;
    let reg = node.handle().register(repo.store().clone());

    reg.enable_dht();
    reg.disable_dht();
    reg.enable_dht();
}

#[tokio::test(flavor = "multi_thread")]
async fn local_discovery() {
    let mut env = old::Env::new();
    let proto = Proto::Quic;

    // A and B are initially disconnected and don't know each other's socket addesses.
    let node_a = env
        .create_node(proto.wrap((Ipv4Addr::UNSPECIFIED, 0)))
        .await;
    let node_b = env
        .create_node(proto.wrap((Ipv4Addr::UNSPECIFIED, 0)))
        .await;

    node_a.enable_local_discovery();
    node_b.enable_local_discovery();

    // Note we compare only the ports because we bind to `UNSPECIFIED` (0.0.0.0) and that's what
    // `listener_local_addr_v4` returns as well, but local discovery produces the actual LAN
    // addresses. Comparing the ports should be enough to test that local discovery works.
    let port_a = proto.listener_local_addr_v4(&node_a).port();
    let port_b = proto.listener_local_addr_v4(&node_b).port();

    let mut rx_a = node_a.on_peer_set_change();
    let mut rx_b = node_b.on_peer_set_change();

    // ...eventually they discover each other via local discovery.
    let connected = async {
        loop {
            let mut peer_ports_a = node_a
                .collect_peer_info()
                .into_iter()
                .map(|info| info.addr.port());
            let mut peer_ports_b = node_b
                .collect_peer_info()
                .into_iter()
                .map(|info| info.addr.port());

            if peer_ports_a.any(|port| port == port_b) && peer_ports_b.any(|port| port == port_a) {
                break;
            }

            select! {
                result = rx_a.changed() => result.unwrap(),
                result = rx_b.changed() => result.unwrap(),
            }
        }
    };

    time::timeout(TEST_TIMEOUT, connected).await.unwrap();
}

async fn expect_knows(network: &Network, peer_name: &str) {
    time::timeout(TEST_TIMEOUT, async move {
        let mut rx = network.on_peer_set_change();

        loop {
            if network.knows(peer_name) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
