//! Networking tests

mod common;

use self::common::{Env, Node, Proto, DEFAULT_TIMEOUT};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{select, time};

#[tokio::test(flavor = "multi_thread")]
async fn peer_exchange() {
    let mut env = Env::with_seed(0);
    let proto = Proto::Quic;

    // B and C are initially connected only to A...
    let node_a = Node::new(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;
    let node_b = Node::new(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;
    let node_c = Node::new(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;

    node_b
        .network
        .add_user_provided_peer(&proto.listener_local_addr_v4(&node_a.network));
    node_c
        .network
        .add_user_provided_peer(&proto.listener_local_addr_v4(&node_a.network));

    let repo_a = env.create_repo().await;
    let reg_a = node_a.network.handle().register(repo_a.store().clone());
    reg_a.enable_pex();

    let repo_b = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_b = node_b.network.handle().register(repo_b.store().clone());
    reg_b.enable_pex();

    let repo_c = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_c = node_c.network.handle().register(repo_c.store().clone());
    reg_c.enable_pex();

    let addr_b = proto.listener_local_addr_v4(&node_b.network);
    let addr_c = proto.listener_local_addr_v4(&node_c.network);

    let mut rx_b = node_b.network.on_peer_set_change();
    let mut rx_c = node_c.network.on_peer_set_change();

    // ...eventually B and C connect to each other via peer exchange.
    let connected = async {
        loop {
            if node_b.network.knows_peer(addr_c) && node_c.network.knows_peer(addr_b) {
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
    let bind = proto.wrap((Ipv4Addr::LOCALHOST, 0));

    let node = Node::new(bind).await;
    let local_addr_0 = proto.listener_local_addr_v4(&node.network);

    node.network.handle().bind(&[]).await;
    node.network.handle().bind(&[bind]).await;

    let local_addr_1 = proto.listener_local_addr_v4(&node.network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_pending_connection() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;
    let bind = proto.wrap((Ipv4Addr::LOCALHOST, 0));

    let node = Node::new(bind).await;
    let local_addr_0 = proto.listener_local_addr_v4(&node.network);

    let remote_addr = proto.wrap((Ipv4Addr::LOCALHOST, 12345));
    node.network.add_user_provided_peer(&remote_addr);

    // Wait until the connection starts begin established.
    let mut rx = node.network.on_peer_set_change();
    time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if node.network.knows_peer(remote_addr) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();

    node.network.handle().bind(&[]).await;
    node.network.handle().bind(&[bind]).await;

    let local_addr_1 = proto.listener_local_addr_v4(&node.network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_addr_takeover() {
    use tokio::net::UdpSocket;

    let _env = Env::with_seed(0);
    let proto = Proto::Quic;
    let bind = proto.wrap((Ipv4Addr::LOCALHOST, 0));

    let node = Node::new(bind).await;
    let local_addr_0 = proto.listener_local_addr_v4(&node.network);

    node.network.handle().bind(&[]).await;

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

    // Enabling the network binds it to a different port.
    node.network.handle().bind(&[bind]).await;

    let local_addr_1 = proto.listener_local_addr_v4(&node.network);
    assert_ne!(local_addr_1, local_addr_0);
}

// Test for an edge case that used to panic.
#[tokio::test(flavor = "multi_thread")]
async fn dht_toggle() {
    let mut env = Env::with_seed(0);
    let proto = Proto::Quic;

    let node = Node::new(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;

    let repo = env.create_repo().await;
    let reg = node.network.handle().register(repo.store().clone());

    reg.enable_dht();
    reg.disable_dht();
    reg.enable_dht();
}

#[tokio::test(flavor = "multi_thread")]
async fn local_discovery() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    // A and B are initially disconnected and don't know each other's socket addesses.
    let node_a = Node::new(proto.wrap((Ipv4Addr::UNSPECIFIED, 0))).await;
    let node_b = Node::new(proto.wrap((Ipv4Addr::UNSPECIFIED, 0))).await;

    node_a.network.enable_local_discovery();
    node_b.network.enable_local_discovery();

    // Note we compare only the ports because we bind to `UNSPECIFIED` (0.0.0.0) and that's what
    // `listener_local_addr_v4` returns as well, but local discovery produces the actual LAN
    // addresses. Comparing the ports should be enough to test that local discovery works.
    let port_a = proto.listener_local_addr_v4(&node_a.network).port();
    let port_b = proto.listener_local_addr_v4(&node_b.network).port();

    let mut rx_a = node_a.network.on_peer_set_change();
    let mut rx_b = node_b.network.on_peer_set_change();

    // ...eventually they discover each other via local discovery.
    let connected = async {
        loop {
            let mut peer_ports_a = node_a
                .network
                .collect_peer_info()
                .into_iter()
                .map(|info| info.addr.port());
            let mut peer_ports_b = node_b
                .network
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

    time::timeout(DEFAULT_TIMEOUT, connected).await.unwrap();
}
