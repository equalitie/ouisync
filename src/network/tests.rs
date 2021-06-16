use super::{
    client::Client,
    message::{Request, Response},
    message_broker::{ClientStream, Command, ServerStream},
    server::Server,
};
use crate::{
    db,
    index::{self, node_test_utils::Snapshot, Index, RootNode, INNER_LAYER_COUNT},
    test_utils,
};
use rand::prelude::*;
use test_strategy::proptest;
use tokio::{select, sync::mpsc};

// test complete transfer of one snapshot from one replica to another.
#[proptest]
fn transfer_snapshot_between_two_replicas(
    #[strategy(0usize..=32)] leaf_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(transfer_snapshot_between_two_replicas_case(
        leaf_count, rng_seed,
    ))
}

async fn transfer_snapshot_between_two_replicas_case(leaf_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let a_index = create_index(&mut rng).await;
    let b_index = create_index(&mut rng).await;

    let snapshot = Snapshot::generate(&mut rng, leaf_count);
    save_snapshot(&a_index, &snapshot).await;

    {
        let mut tx = b_index.pool.begin().await.unwrap();
        assert!(RootNode::load_latest(&mut tx, &a_index.this_replica_id)
            .await
            .unwrap()
            .is_none());
    }

    // Enough capacity to prevent deadlocks.
    // TODO: find the actual minimum necessary capacity.
    let capacity = 256;

    let (a_send_tx, a_send_rx) = mpsc::channel(1);
    let (a_recv_tx, a_recv_rx) = mpsc::channel(capacity);
    let server_stream = ServerStream {
        tx: a_send_tx,
        rx: a_recv_rx,
    };
    let mut server = Server::new(a_index.clone(), server_stream);

    let (b_send_tx, b_send_rx) = mpsc::channel(1);
    let (b_recv_tx, b_recv_rx) = mpsc::channel(capacity);
    let client_stream = ClientStream {
        tx: b_send_tx,
        rx: b_recv_rx,
    };
    let mut client = Client::new(b_index.clone(), a_index.this_replica_id, client_stream);

    select! {
        _ = server.run() => {},
        _ = client.run() => {},
        _ = simulate_connection(b_send_rx, b_recv_tx, a_send_rx, a_recv_tx) => {},
    }

    let mut tx = b_index.pool.begin().await.unwrap();
    let root = RootNode::load_latest(&mut tx, &a_index.this_replica_id)
        .await
        .unwrap()
        .unwrap();

    assert!(root.is_complete);
}

async fn create_index<R: Rng>(rng: &mut R) -> Index {
    let db = db::init(db::Store::Memory).await.unwrap();
    let id = rng.gen();

    Index::load(db, id).await.unwrap()
}

async fn save_snapshot(index: &Index, snapshot: &Snapshot) {
    let mut tx = index.pool.begin().await.unwrap();

    RootNode::create(&mut tx, &index.this_replica_id, *snapshot.root_hash())
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            for (bucket, node) in nodes {
                node.save(&mut tx, parent_hash, bucket).await.unwrap();
            }
        }
    }

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        for node in nodes {
            node.save(&mut tx, parent_hash).await.unwrap();
        }

        index::detect_complete_snapshots(&mut tx, *parent_hash, INNER_LAYER_COUNT)
            .await
            .unwrap();
    }

    tx.commit().await.unwrap()
}

// Simulate connection between `Client` and `Server` by forwarding the messages between the
// corresponding streams.
async fn simulate_connection(
    mut client_send_rx: mpsc::Receiver<Command>,
    client_recv_tx: mpsc::Sender<Response>,
    mut server_send_rx: mpsc::Receiver<Command>,
    server_recv_tx: mpsc::Sender<Request>,
) {
    // If the client sends second `RootNode` request that means it's done fetching the whole
    // snapshot.
    let mut root_request_count = 0usize;

    while root_request_count < 2 {
        select! {
            command = client_send_rx.recv() => {
                let request = command.unwrap().into_send_message().into();

                if matches!(request, Request::RootNode(_)) {
                    root_request_count += 1;
                }

                server_recv_tx.send(request).await.unwrap();
            }
            command = server_send_rx.recv() => {
                let response = command.unwrap().into_send_message().into();
                client_recv_tx.send(response).await.unwrap();
            }
        }
    }
}
