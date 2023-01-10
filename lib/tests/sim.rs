//! Integration tests that use simulated environment for repeatable test results

#![cfg(feature = "simulation")]

mod common;

use common::sim::{self, NetworkExt, Sim};
use ouisync::{AccessSecrets, EntryType};
use rand::Rng;
use std::rc::Rc;
use tokio::sync::Barrier;

#[test]
fn transfer_large_file() {
    let mut sim = Sim::new();

    let barrier = Rc::new(Barrier::new(2));
    let secrets = AccessSecrets::random_write();

    let file_size = 4 * 1024 * 1024;
    let content = {
        let mut content = vec![0; file_size];
        rand::thread_rng().fill(&mut content[..]);
        Rc::new(content)
    };

    sim.actor("writer", {
        let secrets = secrets.clone();
        let content = content.clone();
        let barrier = barrier.clone();

        async move {
            let network = sim::create_network().await;
            let repo = sim::create_repo(secrets).await;
            let _reg = network.handle().register(repo.store().clone());

            let mut file = repo.create_file("test.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            barrier.wait().await;
        }
    });

    sim.actor("reader", async move {
        let network = sim::create_network().await;
        let repo = sim::create_repo(secrets).await;
        let _reg = network.handle().register(repo.store().clone());

        network.connect("writer");

        common::expect_file_content(&repo, "test.dat", &content).await;

        barrier.wait().await;
    });

    sim.run();
}

#[test]
fn transfer_directory_with_file() {
    let mut sim = Sim::new();

    let barrier = Rc::new(Barrier::new(2));
    let secrets = AccessSecrets::random_write();

    sim.actor("writer", {
        let secrets = secrets.clone();
        let barrier = barrier.clone();

        async move {
            let network = sim::create_network().await;
            let repo = sim::create_repo(secrets).await;
            let _reg = network.handle().register(repo.store().clone());

            let mut dir = repo.create_directory("food").await.unwrap();
            dir.create_file("pizza.jpg".into()).await.unwrap();

            barrier.wait().await;
        }
    });

    sim.actor("reader", async move {
        let network = sim::create_network().await;
        let repo = sim::create_repo(secrets).await;
        let _reg = network.handle().register(repo.store().clone());

        network.connect("writer");

        common::expect_entry_exists(&repo, "food/pizza.jpg", EntryType::File).await;

        barrier.wait().await;
    });

    sim.run();
}
