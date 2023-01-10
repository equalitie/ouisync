//! Integration tests that use simulated environment for repeatable test results

#![cfg(feature = "simulation")]

mod common;

use common::{Env, Node, Proto};
use ouisync::{network::Network, AccessSecrets, EntryType, PeerAddr};
use rand::Rng;
use std::{
    net::{Ipv4Addr, SocketAddr},
    rc::Rc,
};
use tokio::sync::Barrier;

const PORT: u16 = 12345;

#[test]
fn transfer_large_file() {
    let env = Env::new();

    let barrier = Rc::new(Barrier::new(2));
    let secrets = AccessSecrets::random_write();

    let file_size = 4 * 1024 * 1024;
    let content = {
        let mut content = vec![0; file_size];
        rand::thread_rng().fill(&mut content[..]);
        Rc::new(content)
    };

    let mut sim = turmoil::Builder::new().build_with_rng(Box::new(rand::thread_rng()));

    sim.client("writer", {
        let secrets = secrets.clone();
        let store = env.next_store();
        let content = content.clone();
        let barrier = barrier.clone();

        async move {
            let node = Node::new(0, default_peer_addr()).await;
            let repo = common::create_repo(&store, secrets).await;
            let _reg = node.network.handle().register(repo.store().clone());

            let mut file = repo.create_file("test.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            barrier.wait().await;

            Ok(())
        }
    });

    sim.client("reader", {
        let store = env.next_store();

        async move {
            let node = Node::new(1, default_peer_addr()).await;
            let repo = common::create_repo(&store, secrets).await;
            let _reg = node.network.handle().register(repo.store().clone());

            connect(&node.network, "writer");

            common::expect_file_content(&repo, "test.dat", &content).await;

            barrier.wait().await;

            Ok(())
        }
    });

    sim.run().unwrap();
}

#[test]
fn transfer_directory_with_file() {
    let env = Env::new();

    let barrier = Rc::new(Barrier::new(2));
    let secrets = AccessSecrets::random_write();

    let mut sim = turmoil::Builder::new().build_with_rng(Box::new(rand::thread_rng()));

    sim.client("writer", {
        let secrets = secrets.clone();
        let store = env.next_store();
        let barrier = barrier.clone();

        async move {
            let node = Node::new(0, default_peer_addr()).await;
            let repo = common::create_repo(&store, secrets).await;
            let _reg = node.network.handle().register(repo.store().clone());

            let mut dir = repo.create_directory("food").await.unwrap();
            dir.create_file("pizza.jpg".into()).await.unwrap();

            barrier.wait().await;

            Ok(())
        }
    });

    sim.client("reader", {
        let store = env.next_store();

        async move {
            let node = Node::new(1, default_peer_addr()).await;
            let repo = common::create_repo(&store, secrets).await;
            let _reg = node.network.handle().register(repo.store().clone());

            connect(&node.network, "writer");
            common::expect_entry_exists(&repo, "food/pizza.jpg", EntryType::File).await;

            barrier.wait().await;

            Ok(())
        }
    });

    sim.run().unwrap();
}

fn default_peer_addr() -> PeerAddr {
    Proto::Tcp.wrap((Ipv4Addr::UNSPECIFIED, PORT))
}

fn connect(network: &Network, target: &str) {
    network.add_user_provided_peer(&Proto::Tcp.wrap(SocketAddr::new(turmoil::lookup(target), PORT)))
}
