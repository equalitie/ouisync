use super::{
    client::Client,
    message::{Message, Request, Response},
    message_broker::{ClientStream, Command, ServerStream},
    server::Server,
};
use crate::{
    block::{self, BLOCK_SIZE},
    crypto::{AuthTag, Hashable},
    db,
    index::{self, node_test_utils::Snapshot, Index, RootNode, Summary, INNER_LAYER_COUNT},
    replica_id::ReplicaId,
    store, test_utils,
    version_vector::VersionVector,
};
use rand::prelude::*;
use std::time::Duration;
use test_strategy::proptest;
use tokio::{join, select, sync::mpsc, time};

// Test complete transfer of one snapshot from one replica to another
// Also test a new snapshot transfer is performed after every local branch
// change.
#[proptest]
fn transfer_snapshot_between_two_replicas(
    #[strategy(0usize..32)] leaf_count: usize,
    #[strategy(0usize..2)] change_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(transfer_snapshot_between_two_replicas_case(
        leaf_count,
        change_count,
        rng_seed,
    ))
}

async fn transfer_snapshot_between_two_replicas_case(
    leaf_count: usize,
    change_count: usize,
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

    let (mut server, mut client, mut simulator) =
        create_network(a_index.clone(), b_index.clone()).await;

    let drive = async {
        let mut remaining_changes = change_count;
        let mut first_root_request = false;

        loop {
            simulator
                .run_until(|message| matches!(message, Message::Request(Request::RootNode { .. })))
                .await;

            if !first_root_request {
                first_root_request = true;
            } else if remaining_changes > 0 {
                create_block(&mut rng, &a_index).await;
                remaining_changes -= 1;
            } else {
                break;
            }
        }
    };

    // NOTE: using `join` instead of `select` to make sure all tasks run to completion even after
    // one of them finishes. This seems to prevent a memory corruption issue which is triggered by
    // (what seems to be) a bug in sqlx and which happens when a future that contains a sqlx query
    // is interrupted mid query instead of being let to run to completion.
    let (server_result, client_result, _) = join!(server.run(), client.run(), drive);
    server_result.unwrap();
    client_result.unwrap();

    let root_per_b = load_latest_root_node(&b_index, &a_index.this_replica_id)
        .await
        .unwrap();
    let root_per_a = load_latest_root_node(&a_index, &a_index.this_replica_id)
        .await
        .unwrap();

    assert!(root_per_b.summary.is_complete());
    assert_eq!(root_per_b.hash, root_per_a.hash);
    assert_eq!(root_per_b.versions, root_per_a.versions);
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

#[tokio::test(flavor = "multi_thread")]
async fn debug() {
    env_logger::init();
    transfer_blocks_between_two_replicas_case(1, 0).await
}

async fn transfer_blocks_between_two_replicas_case(block_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let a_index = create_index(&mut rng).await;
    let b_index = create_index(&mut rng).await;

    // Initially both replicas have the whole snapshot but no blocks.
    let snapshot = Snapshot::generate(&mut rng, block_count);
    save_snapshot(&a_index, &snapshot).await;
    save_snapshot(&b_index, &snapshot).await;

    let (mut server, mut client, mut simulator) =
        create_network(a_index.clone(), b_index.clone()).await;

    let drive = async move {
        let content = vec![0; BLOCK_SIZE];

        for block_id in snapshot.block_ids() {
            // Write the block by replica A.
            store::write_received_block(&a_index, block_id, &content, &AuthTag::default())
                .await
                .unwrap();

            // Wait until replica B receives and writes the block too.
            simulator
                .run_until(|message| matches!(message, Message::Response(Response::Block { .. })))
                .await;

            // HACK: Find a better way to do this.
            while !block::exists(&b_index.pool, block_id).await.unwrap() {
                time::sleep(Duration::from_millis(25)).await;
            }
        }
    };

    let (server_result, client_result, _) = join!(server.run(), client.run(), drive);
    server_result.unwrap();
    client_result.unwrap();
}

async fn create_index<R: Rng>(rng: &mut R) -> Index {
    let db = db::init(db::Store::Memory).await.unwrap();
    let id = rng.gen();

    Index::load(db, id).await.unwrap()
}

// Enough capacity to prevent deadlocks.
// TODO: find the actual minimum necessary capacity.
const CAPACITY: usize = 256;

async fn create_network(
    server_index: Index,
    client_index: Index,
) -> (Server, Client, ConnectionSimulator) {
    let server_replica_id = server_index.this_replica_id;
    let (server, server_send_rx, server_recv_tx) = create_server(server_index).await;
    let (client, client_send_rx, client_recv_tx) = create_client(client_index, server_replica_id);

    let simulator = ConnectionSimulator::new(
        client_send_rx,
        client_recv_tx,
        server_send_rx,
        server_recv_tx,
    );

    (server, client, simulator)
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
        nodes.save(&index.pool, &parent_hash).await.unwrap();
        index::update_summaries(&index.pool, *parent_hash, INNER_LAYER_COUNT)
            .await
            .unwrap();
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

// Simulate connection between `Client` and `Server` by forwarding the messages between the
// corresponding streams.
struct ConnectionSimulator {
    client_send_rx: mpsc::Receiver<Command>,
    client_recv_tx: mpsc::Sender<Response>,
    server_send_rx: mpsc::Receiver<Command>,
    server_recv_tx: mpsc::Sender<Request>,
}

impl ConnectionSimulator {
    fn new(
        client_send_rx: mpsc::Receiver<Command>,
        client_recv_tx: mpsc::Sender<Response>,
        server_send_rx: mpsc::Receiver<Command>,
        server_recv_tx: mpsc::Sender<Request>,
    ) -> Self {
        Self {
            client_send_rx,
            client_recv_tx,
            server_send_rx,
            server_recv_tx,
        }
    }

    // Keep simulating the network until a message is sent for which `pred` returns `true`.
    async fn run_until<F>(&mut self, pred: F)
    where
        F: Fn(&Message) -> bool,
    {
        loop {
            select! {
                command = self.client_send_rx.recv() => {
                    let message = command.unwrap().into_send_message();
                    let stop = pred(&message);
                    let request = message.into();
                    self.server_recv_tx.send(request).await.unwrap();
                    if stop { break }
                }
                command = self.server_send_rx.recv() => {
                    let message = command.unwrap().into_send_message();
                    let stop = pred(&message);
                    let response = message.into();
                    self.client_recv_tx.send(response).await.unwrap();
                    if stop { break }
                }
            }
        }
    }
}
