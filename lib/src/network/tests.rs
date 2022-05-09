use super::{
    client::Client,
    message::{Content, Request, Response},
    request::MAX_PENDING_REQUESTS,
    server::Server,
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::sign::{Keypair, PublicKey},
    db,
    index::{
        node_test_utils::{receive_blocks, receive_nodes, Snapshot},
        BranchData, Index, Proof, RootNode,
    },
    repository::{self, RepositoryId},
    store, test_utils,
    version_vector::VersionVector,
};
use futures_util::future;
use rand::prelude::*;
use std::{fmt, future::Future, sync::Arc, time::Duration};
use test_strategy::proptest;
use tokio::{
    pin, select,
    sync::{mpsc, Semaphore},
    time,
};

const TIMEOUT: Duration = Duration::from_secs(30);

// Test complete transfer of one snapshot from one replica to another
// Also test a new snapshot transfer is performed after every local branch
// change.
#[proptest]
fn transfer_snapshot_between_two_replicas(
    #[strategy(0usize..32)] leaf_count: usize,
    #[strategy(0usize..2)] changeset_count: usize,
    #[strategy(1usize..4)] changeset_size: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(transfer_snapshot_between_two_replicas_case(
        leaf_count,
        changeset_count,
        changeset_size,
        rng_seed,
    ))
}

async fn transfer_snapshot_between_two_replicas_case(
    leaf_count: usize,
    changeset_count: usize,
    changeset_size: usize,
    rng_seed: u64,
) {
    assert!(changeset_size > 0);

    let mut rng = StdRng::seed_from_u64(rng_seed);

    let write_keys = Keypair::generate(&mut rng);
    let (a_index, a_id) = create_index(&mut rng, &write_keys).await;
    let (b_index, _) = create_index(&mut rng, &write_keys).await;

    let snapshot = Snapshot::generate(&mut rng, leaf_count);
    save_snapshot(&a_index, a_id, &write_keys, &snapshot).await;
    receive_blocks(&a_index, &snapshot).await;

    assert!(load_latest_root_node(&b_index, a_id).await.is_none());

    let mut server = create_server(a_index.clone());
    let mut client = create_client(b_index.clone());

    // Wait until replica B catches up to replica A, then have replica A perform a local change
    // and repeat.
    let drive = async {
        let mut remaining_changesets = changeset_count;

        loop {
            wait_until_snapshots_in_sync(&a_index, a_id, &b_index).await;

            if remaining_changesets > 0 {
                create_changeset(&mut rng, &a_index, &a_id, &write_keys, changeset_size).await;
                remaining_changesets -= 1;
            } else {
                break;
            }
        }
    };

    simulate_connection_until(&mut server, &mut client, drive).await
}

#[proptest]
fn transfer_blocks_between_two_replicas(
    #[strategy(1usize..32)] block_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(transfer_blocks_between_two_replicas_case(
        block_count,
        rng_seed,
    ))
}

async fn transfer_blocks_between_two_replicas_case(block_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let write_keys = Keypair::generate(&mut rng);
    let (a_index, a_id) = create_index(&mut rng, &write_keys).await;
    let (b_index, b_id) = create_index(&mut rng, &write_keys).await;

    // Initially both replicas have the whole snapshot but no blocks.
    let snapshot = Snapshot::generate(&mut rng, block_count);
    save_snapshot(&a_index, a_id, &write_keys, &snapshot).await;
    save_snapshot(&b_index, b_id, &write_keys, &snapshot).await;

    let mut server = create_server(a_index.clone());
    let mut client = create_client(b_index.clone());

    // Keep adding the blocks to replica A and verify they get received by replica B as well.
    let drive = async {
        for (id, block) in snapshot.blocks() {
            // Write the block by replica A.
            store::write_received_block(&a_index, &block.data, &block.nonce)
                .await
                .unwrap();

            // Then wait until replica B receives and writes it too.
            wait_until_block_exists(&b_index, id).await;
        }
    };

    simulate_connection_until(&mut server, &mut client, drive).await
}

// Receive a `LeafNode` with non-missing block, then drop the connection before the block itself is
// received, then re-establish the connection and make sure the block gets received then.
#[tokio::test]
async fn failed_block_only_peer() {
    let mut rng = StdRng::seed_from_u64(0);

    let write_keys = Keypair::generate(&mut rng);
    let (a_index, a_id) = create_index(&mut rng, &write_keys).await;
    let (b_index, _) = create_index(&mut rng, &write_keys).await;

    let snapshot = Snapshot::generate(&mut rng, 1);
    save_snapshot(&a_index, a_id, &write_keys, &snapshot).await;
    receive_blocks(&a_index, &snapshot).await;

    let mut server = create_server(a_index.clone());
    let mut client = create_client(b_index.clone());

    simulate_connection_until(
        &mut server,
        &mut client,
        wait_until_snapshots_in_sync(&a_index, a_id, &b_index),
    )
    .await;

    // Simulate peer disconnecting and reconnecting.
    drop(server);
    drop(client);

    let mut server = create_server(a_index.clone());
    let mut client = create_client(b_index.clone());

    simulate_connection_until(&mut server, &mut client, async {
        for id in snapshot.blocks().keys() {
            wait_until_block_exists(&b_index, id).await
        }
    })
    .await;
}

// Same as `failed_block_only_peer` test but this time there is a second peer who remains connected
// for the whole duration of the test. This is to uncover any potential request caching issues.
#[tokio::test]
async fn failed_block_same_peer() {
    let mut rng = StdRng::seed_from_u64(0);

    let write_keys = Keypair::generate(&mut rng);
    let (a_index, a_id) = create_index(&mut rng, &write_keys).await;
    let (b_index, _) = create_index(&mut rng, &write_keys).await;
    let (c_index, _) = create_index(&mut rng, &write_keys).await;

    let snapshot = Snapshot::generate(&mut rng, 1);
    save_snapshot(&a_index, a_id, &write_keys, &snapshot).await;
    receive_blocks(&a_index, &snapshot).await;

    // [A]-(server_ac)---+
    //                   |
    //               (client_ca)
    //                   |
    //                  [C]
    //                   |
    //               (client_cb)
    //                   |
    // [B]-(server_bc)---+

    let mut server_ac = create_server(a_index.clone());
    let mut client_ca = create_client(c_index.clone());

    let mut server_bc = create_server(b_index.clone());
    let mut client_cb = create_client(c_index.clone());

    // Run both connections in parallel until C syncs its index (but not blocks) with B
    let conn_bc = simulate_connection_until(&mut server_bc, &mut client_cb, future::pending());
    pin!(conn_bc);

    let conn_ac = simulate_connection_until(
        &mut server_ac,
        &mut client_ca,
        wait_until_snapshots_in_sync(&a_index, a_id, &c_index),
    );

    select! {
        _ = conn_ac => (),
        _ = &mut conn_bc => unreachable!(),
    }

    // Drop and recreate the B-C connection but keep the A-C connection up.
    drop(server_ac);
    drop(client_ca);

    let mut server_ac = create_server(a_index.clone());
    let mut client_ca = create_client(c_index.clone());

    // Run the new B-C connection in parallel with the existing A-C connection until all blocks are
    // received.
    let conn_ac = simulate_connection_until(&mut server_ac, &mut client_ca, async {
        for id in snapshot.blocks().keys() {
            wait_until_block_exists(&c_index, id).await
        }
    });

    select! {
        _ = conn_ac => (),
        _ = conn_bc => unreachable!(),
    }
}

async fn create_index<R: Rng + CryptoRng>(rng: &mut R, write_keys: &Keypair) -> (Index, PublicKey) {
    let db = repository::create_db(&db::Store::Temporary).await.unwrap();
    let writer_id = PublicKey::generate(rng);
    let repository_id = RepositoryId::from(write_keys.public);

    let index = Index::load(db, repository_id).await.unwrap();
    let proof = Proof::first(writer_id, write_keys);
    index.create_branch(proof).await.unwrap();

    (index, writer_id)
}

// Enough capacity to prevent deadlocks.
// TODO: find the actual minimum necessary capacity.
const CAPACITY: usize = 256;

async fn save_snapshot(
    index: &Index,
    writer_id: PublicKey,
    write_keys: &Keypair,
    snapshot: &Snapshot,
) {
    // If the snapshot is empty then there is nothing else to save in addition to the initial root
    // node the index already has.
    if snapshot.leaf_count() == 0 {
        return;
    }

    let mut version_vector = VersionVector::new();
    version_vector.insert(writer_id, 2); // to force overwrite the initial root node

    receive_nodes(index, write_keys, writer_id, version_vector, snapshot).await;
}

async fn wait_until_snapshots_in_sync(
    server_index: &Index,
    server_id: PublicKey,
    client_index: &Index,
) {
    let mut rx = client_index.subscribe();

    let server_root = load_latest_root_node(server_index, server_id)
        .await
        .unwrap();

    loop {
        if let Some(client_root) = load_latest_root_node(client_index, server_id).await {
            if client_root.summary.is_complete() && client_root.proof.hash == server_root.proof.hash
            {
                // client has now fully downloaded server's latest snapshot.
                assert_eq!(
                    client_root.proof.version_vector,
                    server_root.proof.version_vector
                );
                break;
            }
        }

        rx.recv().await.unwrap();
    }
}

async fn wait_until_block_exists(index: &Index, block_id: &BlockId) {
    let mut rx = index.subscribe();

    while !block::exists(&mut index.pool.acquire().await.unwrap(), block_id)
        .await
        .unwrap()
    {
        rx.recv().await.unwrap();
    }
}

// Simulate a changeset, e.g. create a file, write to it and flush it.
async fn create_changeset(
    rng: &mut StdRng,
    index: &Index,
    writer_id: &PublicKey,
    write_keys: &Keypair,
    size: usize,
) {
    use sqlx::Connection;

    let branch = index.branches().await.get(writer_id).unwrap().clone();

    for _ in 0..size {
        create_block(rng, index, &branch, write_keys).await;
    }

    let mut cx = index.pool.acquire().await.unwrap();
    let tx = cx.begin().await.unwrap();
    branch
        .update_root_version_vector(tx, &VersionVector::first(*writer_id), write_keys)
        .await
        .unwrap();
}

async fn create_block(rng: &mut StdRng, index: &Index, branch: &BranchData, write_keys: &Keypair) {
    let encoded_locator = rng.gen();

    let mut content = vec![0; BLOCK_SIZE];
    rng.fill(&mut content[..]);

    let block_id = BlockId::from_content(&content);
    let nonce = rng.gen();

    let mut tx = index.pool.begin().await.unwrap();
    branch
        .insert(&mut tx, &block_id, &encoded_locator, write_keys)
        .await
        .unwrap();
    block::write(&mut tx, &block_id, &content, &nonce)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn load_latest_root_node(index: &Index, writer_id: PublicKey) -> Option<RootNode> {
    RootNode::load_latest_by_writer(&mut index.pool.acquire().await.unwrap(), writer_id)
        .await
        .unwrap()
}

// Simulate connection between two replicas until the given future completes.
async fn simulate_connection_until<F>(server: &mut ServerData, client: &mut ClientData, until: F)
where
    F: Future<Output = ()>,
{
    let (server, server_send_rx, server_recv_tx) = server;
    let (client, client_send_rx, client_recv_tx) = client;

    let mut server_conn = Connection {
        send_rx: server_send_rx,
        recv_tx: client_recv_tx,
    };

    let mut client_conn = Connection {
        send_rx: client_send_rx,
        recv_tx: server_recv_tx,
    };

    select! {
        biased; // deterministic poll order for repeatable tests

        result = server.run() => result.unwrap(),
        result = client.run() => result.unwrap(),
        _ = server_conn.run() => panic!("connection closed prematurely"),
        _ = client_conn.run() => panic!("connection closed prematurely"),
        _ = until => (),
        _ = time::sleep(TIMEOUT) => panic!("test timed out"),
    }
}

type ServerData = (Server, mpsc::Receiver<Content>, mpsc::Sender<Request>);
type ClientData = (Client, mpsc::Receiver<Content>, mpsc::Sender<Response>);

fn create_server(index: Index) -> ServerData {
    let (send_tx, send_rx) = mpsc::channel(1);
    let (recv_tx, recv_rx) = mpsc::channel(CAPACITY);
    let server = Server::new(index, send_tx, recv_rx);

    (server, send_rx, recv_tx)
}

fn create_client(index: Index) -> ClientData {
    let (send_tx, send_rx) = mpsc::channel(1);
    let (recv_tx, recv_rx) = mpsc::channel(CAPACITY);
    let client = Client::new(
        index,
        send_tx,
        recv_rx,
        Arc::new(Semaphore::new(MAX_PENDING_REQUESTS)),
    );

    (client, send_rx, recv_tx)
}

// Simulated connection between a server and a client.
struct Connection<'a, T> {
    send_rx: &'a mut mpsc::Receiver<Content>,
    recv_tx: &'a mut mpsc::Sender<T>,
}

impl<T> Connection<'_, T>
where
    T: From<Content> + fmt::Debug,
{
    async fn run(&mut self) {
        while let Some(content) = self.send_rx.recv().await {
            self.recv_tx.send(content.into()).await.unwrap();
        }
    }
}
