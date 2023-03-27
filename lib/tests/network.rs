//! Networking tests

// These test mostly require QUIC / UDP which the simulator doesn't support yet.
#![cfg(not(feature = "simulation"))]

mod common;

use self::common::{actor, Env, NetworkExt, Proto, TEST_TIMEOUT};
use ouisync::network::Network;
use std::{net::Ipv4Addr, sync::Arc};
use tokio::{sync::Barrier, time};

// This test requires QUIC which is not yet supported in simulation
#[test]
fn peer_exchange() {
    let mut env = Env::new();
    let proto = Proto::Quic; // PEX works only with QUIC
    let barrier = Arc::new(Barrier::new(3));

    // Bob and Carol are initially connected only to Alice but eventually they connect to each
    // other via peer exchange.

    env.actor("alice", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(&network).await;
            reg.enable_pex();

            barrier.wait().await;
        }
    });

    env.actor("bob", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(&network).await;
            network.connect("alice");
            reg.enable_pex();

            expect_knows(&network, "carol").await;
            barrier.wait().await;
        }
    });

    env.actor("carol", {
        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(&network).await;
            network.connect("alice");
            reg.enable_pex();

            expect_knows(&network, "bob").await;
            barrier.wait().await;
        }
    });
}

// Test for an edge case that used to panic.
#[test]
fn dht_toggle() {
    let mut env = Env::new();
    let proto = Proto::Quic;

    env.actor("eric", async move {
        let network = actor::create_network(proto).await;
        let (_repo, reg) = actor::create_linked_repo(&network).await;

        reg.enable_dht();
        reg.disable_dht();
        reg.enable_dht();
    });
}

#[test]
fn local_discovery() {
    let mut env = Env::new();
    let proto = Proto::Quic;
    let barrier = Arc::new(Barrier::new(2));

    // The peers are initially disconnected and don't know each other's socket addesses.
    // They eventually discover each other via local discovery.

    for (src_port, dst_port) in [(7001, 7002), (7002, 7001)] {
        let barrier = barrier.clone();

        env.actor(&format!("node-{src_port}"), async move {
            let network = actor::create_unbound_network();
            network
                .handle()
                .bind(&[proto.wrap((Ipv4Addr::LOCALHOST, src_port))])
                .await;

            network.set_local_discovery_enabled(true);

            // Note we compare only the ports because we bind to `LOCALHOST` (127.0.0.1) but local
            // discovery produces the actual LAN addresses which we don't know in advance (or
            // rather can't be bothered to find out). Comparing the ports should be enough to test
            // that local discovery works.
            expect_knows_port(&network, dst_port).await;

            barrier.wait().await;
        });
    }
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

async fn expect_knows_port(network: &Network, peer_port: u16) {
    time::timeout(TEST_TIMEOUT, async move {
        let mut rx = network.on_peer_set_change();

        loop {
            let mut peer_ports = network
                .collect_peer_info()
                .into_iter()
                .map(|info| info.port);

            if peer_ports.any(|port| port == peer_port) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
