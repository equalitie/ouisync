use super::{
    client::Client,
    message::{Request, Response},
    message_broker::{ClientStream, Command, ServerStream},
    server::Server,
};
use crate::{
    crypto::Hashable,
    db,
    index::{self, node_test_utils::Snapshot, Index, RootNode, INNER_LAYER_COUNT},
    replica_id::ReplicaId,
    test_utils,
    version_vector::VersionVector,
};
use rand::prelude::*;
use test_strategy::proptest;
use tokio::{select, sync::mpsc};

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

#[tokio::test(flavor = "multi_thread")]
async fn debug() {
    transfer_snapshot_between_two_replicas_case(32, 1, 0).await
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

    assert!(load_latest_root_node(&b_index, &a_index.this_replica_id)
        .await
        .is_none());

    let (mut server, a_send_rx, a_recv_tx) = create_server(a_index.clone()).await;
    let (mut client, b_send_rx, b_recv_tx) =
        create_client(b_index.clone(), a_index.this_replica_id);

    let drive = async {
        let mut simulator = ConnectionSimulator::new(b_send_rx, b_recv_tx, a_send_rx, a_recv_tx);
        let mut remaining_changes = change_count;

        loop {
            simulator.step().await;

            if remaining_changes > 0 {
                insert_random_block(&mut rng, &a_index).await;
                remaining_changes -= 1;
            } else {
                break;
            }
        }
    };

    select! {
        result = server.run() => result.unwrap(),
        result = client.run() => result.unwrap(),
        _ = drive => (),
    }

    let root_per_b = load_latest_root_node(&b_index, &a_index.this_replica_id)
        .await
        .unwrap();
    let root_per_a = load_latest_root_node(&a_index, &a_index.this_replica_id)
        .await
        .unwrap();

    assert!(root_per_b.is_complete);
    assert_eq!(root_per_b.hash, root_per_a.hash);
    assert_eq!(root_per_b.versions, root_per_a.versions);
}

async fn create_index<R: Rng>(rng: &mut R) -> Index {
    let db = db::init(db::Store::Memory).await.unwrap();
    let id = rng.gen();

    Index::load(db, id).await.unwrap()
}

// Enough capacity to prevent deadlocks.
// TODO: find the actual minimum necessary capacity.
const CAPACITY: usize = 256;

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
    )
    .await
    .unwrap();

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            let mut tx = index.pool.begin().await.unwrap();
            for (bucket, node) in nodes {
                node.save(&mut tx, parent_hash, bucket).await.unwrap();
            }
            tx.commit().await.unwrap()
        }
    }

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        let mut tx = index.pool.begin().await.unwrap();
        for node in nodes {
            node.save(&mut tx, parent_hash).await.unwrap();
        }
        tx.commit().await.unwrap();

        index::detect_complete_snapshots(&index.pool, *parent_hash, INNER_LAYER_COUNT)
            .await
            .unwrap();
    }
}

async fn insert_random_block(rng: &mut impl Rng, index: &Index) {
    let branch = index.local_branch().await;
    let encoded_locator = rng.gen::<u64>().hash();
    let block_id = rng.gen();

    let mut tx = index.pool.begin().await.unwrap();
    branch
        .insert(&mut tx, &block_id, &encoded_locator)
        .await
        .unwrap();
    tx.commit().await.unwrap();
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

    // Simulate the connection until two `RootNode` requests sent because when the client sends
    // the second `RootNode` request that means it's done fetching the whole snapshot.
    async fn step(&mut self) {
        let mut root_node_requests = 0;

        while root_node_requests < 2 {
            select! {
                command = self.client_send_rx.recv() => {
                    let request = command.unwrap().into_send_message().into();

                    if matches!(request, Request::RootNode(_)) {
                        root_node_requests += 1;
                    }

                    self.server_recv_tx.send(request).await.unwrap();
                }
                command = self.server_send_rx.recv() => {
                    let response = command.unwrap().into_send_message().into();
                    self.client_recv_tx.send(response).await.unwrap();
                }
            }
        }
    }
}
