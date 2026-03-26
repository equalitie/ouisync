//! Networking tests

#[macro_use]
mod common;

use self::common::{DEFAULT_REPO, Env, Proto, TEST_TIMEOUT, actor};
use assert_matches::assert_matches;
use async_trait::async_trait;
use ouisync::{DhtContactsStoreTrait, Network, PeerInfo, PeerSource, PeerState};
use std::{
    collections::HashSet,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Mutex},
};
use tokio::{
    net::UdpSocket,
    sync::{Barrier, watch},
    time,
};

#[test]
fn peer_exchange_basics() {
    let mut env = Env::new();
    let proto = Proto::Quic; // PEX works only with QUIC
    let barrier = Arc::new(Barrier::new(3));

    // Bob and Carol are initially connected only to Alice but eventually they connect to each
    // other via peer exchange.

    env.actor("alice", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(DEFAULT_REPO, &network).await;
            reg.set_pex_enabled(true);

            barrier.wait().await;
        }
    });

    env.actor("bob", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(DEFAULT_REPO, &network).await;

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);

            reg.set_pex_enabled(true);

            expect_peer_known(&network, "carol").await;
            barrier.wait().await;
        }
    });

    env.actor("carol", {
        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo(DEFAULT_REPO, &network).await;

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);

            reg.set_pex_enabled(true);

            expect_peer_known(&network, "bob").await;
            barrier.wait().await;
        }
    });
}

#[test]
fn peer_exchange_discovers_only_peers_sharing_same_repository() {
    let mut env = Env::new();
    let proto = Proto::Quic; // PEX works only with QUIC
    let barrier = Arc::new(Barrier::new(4));

    // Alice has two repos - one shared with Bob and Carol and the other shared with Dave.
    // Bob, Carol and Dave initially know only Alice. Through peer exchange, Bob and Carol learn
    // about each other because they share the same repo, but they don't learn about Carol because
    // they share none with her.

    env.actor("alice", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;

            let (_repo_1, reg_1) = actor::create_linked_repo("repo-1", &network).await;
            reg_1.set_pex_enabled(true);

            let (_repo_2, reg_2) = actor::create_linked_repo("repo-2", &network).await;
            reg_2.set_pex_enabled(true);

            barrier.wait().await;
            barrier.wait().await;
        }
    });

    env.actor("bob", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo("repo-1", &network).await;

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);

            reg.set_pex_enabled(true);

            expect_peer_known(&network, "carol").await;

            barrier.wait().await;

            expect_peer_not_known(&network, "dave").await;

            barrier.wait().await;
        }
    });

    env.actor("carol", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo("repo-1", &network).await;

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);

            reg.set_pex_enabled(true);

            expect_peer_known(&network, "bob").await;

            barrier.wait().await;

            expect_peer_not_known(&network, "dave").await;

            barrier.wait().await;
        }
    });

    env.actor("dave", {
        async move {
            let network = actor::create_network(proto).await;
            let (_repo, reg) = actor::create_linked_repo("repo-2", &network).await;

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);

            reg.set_pex_enabled(true);

            barrier.wait().await;

            expect_peer_not_known(&network, "bob").await;
            expect_peer_not_known(&network, "carol").await;

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
        let (_repo, reg) = actor::create_linked_repo(DEFAULT_REPO, &network).await;

        reg.set_dht_enabled(true);
        reg.set_dht_enabled(false);
        reg.set_dht_enabled(true);
    });
}

#[test]
fn dht_discovery() {
    let mut env = Env::new();
    let proto = Proto::Quic;

    let actors = ["alice", "bob"];

    let bootstrap_addr = watch::Sender::new(None);
    let pause_barrier = Arc::new(Barrier::new(2));
    let stop_barrier = Arc::new(Barrier::new(actors.len() + 1));

    env.actor("bootstrap", {
        let bootstrap_addr = bootstrap_addr.clone();
        let stop_barrier = stop_barrier.clone();

        async move {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();

            bootstrap_addr.send_replace(Some(socket.local_addr().unwrap()));

            let _dht = btdht::MainlineDht::builder()
                .set_read_only(false)
                .start(socket)
                .unwrap();

            stop_barrier.wait().await;
        }
    });

    for actor in actors {
        env.actor(actor, {
            let mut bootstrap_addr = bootstrap_addr.subscribe();
            let pause_barrier = pause_barrier.clone();
            let stop_barrier = stop_barrier.clone();

            async move {
                let bootstrap_addr = bootstrap_addr
                    .wait_for(|addr| addr.is_some())
                    .await
                    .unwrap()
                    .unwrap();
                let dht_contacts = TestDhtContacts::new([bootstrap_addr]);

                let network = Network::builder()
                    .runtime_id(actor::runtime_id())
                    .dht_contacts(Arc::new(dht_contacts))
                    .allow_local_dht(true)
                    .build();
                network.set_dht_routers(HashSet::new());
                actor::bind(&network, proto).await;

                let (_repo, reg) = actor::create_linked_repo(DEFAULT_REPO, &network).await;
                reg.set_dht_enabled(true);

                if pause_barrier.wait().await.is_leader() {
                    time::pause();
                }

                for peer in actors {
                    if peer == actor {
                        continue;
                    }

                    let info = expect_peer_active(&network, peer).await;
                    assert_matches!(info.source, PeerSource::Dht | PeerSource::Listener);
                }

                stop_barrier.wait().await;
            }
        });
    }
}

#[test]
fn local_discovery() {
    let mut env = Env::new();
    let proto = Proto::Quic;
    let barrier = Arc::new(Barrier::new(2));

    // The peers are initially disconnected and don't know each other's socket addesses.
    // They eventually discover each other via local discovery.

    for (src, dst) in [("alice", "bob"), ("bob", "alice")] {
        let barrier = barrier.clone();

        env.actor(src, async move {
            let network = actor::create_network(proto).await;
            network.set_local_discovery_enabled(true);

            let dst_port = actor::lookup_addr(dst).await.port();

            // Note we compare only the ports because we bind to `LOCALHOST` (127.0.0.1) but local
            // discovery produces the actual LAN addresses which we don't know in advance (or
            // rather can't be bothered to find out). Comparing the ports should be enough to test
            // that local discovery works.
            expect_knows_port(&network, dst_port).await;

            barrier.wait().await;
        });
    }
}

#[test]
fn add_peer_before_bind() {
    let mut env = Env::new();
    let proto = Proto::Quic;
    let barrier = Arc::new(Barrier::new(2));

    env.actor("alice", {
        let barrier = barrier.clone();

        async move {
            let network = actor::create_network(proto).await;
            expect_peer_active(&network, "bob").await;

            barrier.wait().await;
        }
    });

    env.actor("bob", {
        async move {
            let network = actor::create_unbound_network();

            let peer_addr = actor::lookup_addr("alice").await;
            network.add_user_provided_peer(&peer_addr);
            expect_peer_known(&network, "alice").await;

            actor::bind(&network, proto).await;
            expect_peer_active(&network, "alice").await;

            barrier.wait().await;
        }
    });
}

async fn expect_peer_known(network: &Network, peer_name: &str) -> PeerInfo {
    expect_peer_state(network, peer_name, |_| true).await
}

async fn expect_peer_active(network: &Network, peer_name: &str) -> PeerInfo {
    expect_peer_state(network, peer_name, |state| {
        matches!(state, PeerState::Active { .. })
    })
    .await
}

async fn expect_peer_state<F>(network: &Network, peer_name: &str, expected_state_fn: F) -> PeerInfo
where
    F: Fn(&PeerState) -> bool,
{
    time::timeout(*TEST_TIMEOUT, async move {
        let mut rx = network.subscribe();
        let peer_addr = actor::lookup_addr(peer_name).await;

        loop {
            if let Some(info) = network.peer_info(peer_addr)
                && expected_state_fn(&info.state)
            {
                break info;
            }

            rx.recv().await.unwrap();
        }
    })
    .await
    .unwrap()
}

async fn expect_knows_port(network: &Network, peer_port: u16) {
    let collector = network.peer_info_collector();

    time::timeout(*TEST_TIMEOUT, async move {
        let mut rx = network.subscribe();

        loop {
            let mut peer_ports = collector.collect().into_iter().map(|info| info.addr.port());

            if peer_ports.any(|port| port == peer_port) {
                break;
            }

            rx.recv().await.unwrap();
        }
    })
    .await
    .unwrap()
}

async fn expect_peer_not_known(network: &Network, peer_name: &str) {
    let peer_addr = actor::lookup_addr(peer_name).await;

    if let Some(info) = network.peer_info(peer_addr) {
        error!("unexpected known peer {peer_name}: {info:?}");
        panic!("unexpected known peer {peer_name}: {info:?}");
    }
}

struct TestDhtContacts {
    v4: Mutex<HashSet<SocketAddrV4>>,
    v6: Mutex<HashSet<SocketAddrV6>>,
}

impl TestDhtContacts {
    fn new(contacts: impl IntoIterator<Item = SocketAddr>) -> Self {
        let mut v4 = HashSet::new();
        let mut v6 = HashSet::new();

        for addr in contacts {
            match addr {
                SocketAddr::V4(addr) => v4.insert(addr),
                SocketAddr::V6(addr) => v6.insert(addr),
            };
        }

        Self {
            v4: Mutex::new(v4),
            v6: Mutex::new(v6),
        }
    }
}

#[async_trait]
impl DhtContactsStoreTrait for TestDhtContacts {
    async fn load_v4(&self) -> io::Result<HashSet<SocketAddrV4>> {
        Ok(self.v4.lock().unwrap().clone())
    }

    async fn load_v6(&self) -> io::Result<HashSet<SocketAddrV6>> {
        Ok(self.v6.lock().unwrap().clone())
    }

    async fn store_v4(&self, contacts: HashSet<SocketAddrV4>) -> io::Result<()> {
        *self.v4.lock().unwrap() = contacts;
        Ok(())
    }

    async fn store_v6(&self, contacts: HashSet<SocketAddrV6>) -> io::Result<()> {
        *self.v6.lock().unwrap() = contacts;
        Ok(())
    }
}
