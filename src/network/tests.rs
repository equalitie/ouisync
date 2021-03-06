use super::{
    client::Client,
    message::{Message, Request, Response},
    message_broker::{ClientStream, Command, ServerStream},
    server::Server,
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::{AuthTag, Hashable},
    db,
    index::{node_test_utils::Snapshot, Index, RootNode, Summary},
    replica_id::ReplicaId,
    store, test_utils,
    version_vector::VersionVector,
};
use rand::prelude::*;
use std::{fmt, future::Future, time::Duration};
use test_strategy::proptest;
use tokio::{select, sync::mpsc, time};

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
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let a_index = create_index(&mut rng).await;
    let b_index = create_index(&mut rng).await;

    let snapshot = Snapshot::generate(&mut rng, leaf_count);
    save_snapshot(&a_index, &snapshot).await;
    write_all_blocks(&a_index, &snapshot).await;

    assert!(load_latest_root_node(&b_index, &a_index.this_replica_id)
        .await
        .is_none());

    // Wait until replica B catches up to replica A, then have replica A perform a local change
    // (create one new block) and repeat.
    let drive = async {
        let mut remaining_changesets = changeset_count;

        loop {
            wait_until_snapshots_in_sync(&a_index, &b_index).await;

            if remaining_changesets > 0 {
                for _ in 0..changeset_size {
                    create_block(&mut rng, &a_index).await;
                }

                remaining_changesets -= 1;
            } else {
                break;
            }
        }
    };

    simulate_connection_until(a_index.clone(), b_index.clone(), drive).await
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

    let a_index = create_index(&mut rng).await;
    let b_index = create_index(&mut rng).await;

    // Initially both replicas have the whole snapshot but no blocks.
    let snapshot = Snapshot::generate(&mut rng, block_count);
    save_snapshot(&a_index, &snapshot).await;
    save_snapshot(&b_index, &snapshot).await;

    // Keep adding the blocks to replica A and verify they get received by replica B as well.
    let drive = async {
        let content = vec![0; BLOCK_SIZE];

        for block_id in snapshot.block_ids() {
            // Write the block by replica A.
            store::write_received_block(&a_index, block_id, &content, &AuthTag::default())
                .await
                .unwrap();

            // Then wait until replica B receives and writes it too.
            wait_until_block_exists(&b_index, block_id).await;
        }
    };

    simulate_connection_until(a_index.clone(), b_index.clone(), drive).await
}

async fn create_index<R: Rng>(rng: &mut R) -> Index {
    let db = db::init(db::Store::Memory).await.unwrap();
    let id = rng.gen();

    Index::load(db, id).await.unwrap()
}

// Enough capacity to prevent deadlocks.
// TODO: find the actual minimum necessary capacity.
const CAPACITY: usize = 256;

async fn save_snapshot(index: &Index, snapshot: &Snapshot) {
    RootNode::create(
        &index.pool,
        &index.this_replica_id,
        VersionVector::new(),
        *snapshot.root_hash(),
        Summary::INCOMPLETE,
    )
    .await
    .unwrap();

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            nodes.save(&index.pool, &parent_hash).await.unwrap();
        }
    }

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        index
            .receive_leaf_nodes(*parent_hash, nodes.clone())
            .await
            .unwrap();
    }
}

async fn wait_until_snapshots_in_sync(server_index: &Index, client_index: &Index) {
    let server_root = load_latest_root_node(server_index, &server_index.this_replica_id)
        .await
        .unwrap();

    loop {
        if let Some(client_root) =
            load_latest_root_node(client_index, &server_index.this_replica_id).await
        {
            if client_root.summary.is_complete() && client_root.hash == server_root.hash {
                // client has now fully downloaded server's latest snapshot.
                assert_eq!(client_root.versions, server_root.versions);
                break;
            }
        }

        // TODO: find a better way to do this than `sleep`.
        time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_until_block_exists(index: &Index, block_id: &BlockId) {
    while !block::exists(&index.pool, block_id).await.unwrap() {
        // TODO: find a better way to do this than `sleep`.
        time::sleep(Duration::from_millis(25)).await;
    }
}

async fn create_block(rng: &mut impl Rng, index: &Index) {
    let branch = index.local_branch().await;
    let encoded_locator = rng.gen::<u64>().hash();
    let block_id = rng.gen();
    let content = vec![0; BLOCK_SIZE];

    let mut tx = index.pool.begin().await.unwrap();
    branch
        .insert(&mut tx, &block_id, &encoded_locator)
        .await
        .unwrap();
    block::write(&mut tx, &block_id, &content, &AuthTag::default())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn write_all_blocks(index: &Index, snapshot: &Snapshot) {
    let content = vec![0; BLOCK_SIZE];

    for (_, nodes) in snapshot.leaf_sets() {
        for node in nodes {
            store::write_received_block(index, &node.block_id, &content, &AuthTag::default())
                .await
                .unwrap();
        }
    }
}

async fn load_latest_root_node(index: &Index, replica_id: &ReplicaId) -> Option<RootNode> {
    RootNode::load_latest(&index.pool, replica_id)
        .await
        .unwrap()
}

// Simulate connection between two replicas until the given future completes.
async fn simulate_connection_until<F>(server_index: Index, client_index: Index, until: F)
where
    F: Future<Output = ()>,
{
    let server_replica_id = server_index.this_replica_id;
    let (mut server, server_send_rx, server_recv_tx) = create_server(server_index).await;
    let (mut client, client_send_rx, client_recv_tx) =
        create_client(client_index, server_replica_id);

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
        _ = server_conn.run() => (),
        _ = client_conn.run() => (),
        _ = until => (),
        _ = time::sleep(TIMEOUT) => panic!("test timed out"),
    }
}

async fn create_server(index: Index) -> (Server, mpsc::Receiver<Command>, mpsc::Sender<Request>) {
    let (send_tx, send_rx) = mpsc::channel(1);
    let (recv_tx, recv_rx) = mpsc::channel(CAPACITY);
    let stream = ServerStream::new(send_tx, recv_rx);
    let server = Server::new(index, stream).await;

    (server, send_rx, recv_tx)
}

fn create_client(
    index: Index,
    their_replica_id: ReplicaId,
) -> (Client, mpsc::Receiver<Command>, mpsc::Sender<Response>) {
    let (send_tx, send_rx) = mpsc::channel(1);
    let (recv_tx, recv_rx) = mpsc::channel(CAPACITY);
    let stream = ClientStream::new(send_tx, recv_rx);
    let client = Client::new(index, their_replica_id, stream);

    (client, send_rx, recv_tx)
}

// Simulated connection between a server and a client.
struct Connection<T> {
    send_rx: mpsc::Receiver<Command>,
    recv_tx: mpsc::Sender<T>,
}

impl<T> Connection<T>
where
    T: From<Message> + fmt::Debug,
{
    async fn run(&mut self) {
        while let Some(command) = self.send_rx.recv().await {
            let message = command.into_send_message().into();
            self.recv_tx.send(message).await.unwrap();
        }
    }
}
