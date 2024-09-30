use super::{simulation::Simulation, *};
use crate::{
    network::{debug_payload::DebugRequest, message::ResponseDisambiguator},
    protocol::{
        test_utils::{BlockState, Snapshot},
        Block,
    },
};
use rand::{
    distributions::{Bernoulli, Distribution, Standard},
    rngs::StdRng,
    seq::SliceRandom,
    Rng, SeedableRng,
};
use std::{pin::pin, time::Duration};
use tokio::{sync::mpsc::error::TryRecvError, time};

// Test syncing while peers keep joining and leaving the swarm.
//
// Note: We need `tokio::test` here because the `RequestTracker` uses `DelayQueue` internaly which
// needs a tokio runtime.
#[tokio::test]
async fn dynamic_swarm() {
    // crate::test_utils::init_log();

    let seed = rand::random();
    case(seed, 64, 4);

    fn case(seed: u64, max_blocks: usize, expected_peer_changes: usize) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut sim = Simulation::new();

        let num_blocks = rng.gen_range(1..=max_blocks);
        let snapshot = Snapshot::generate(&mut rng, num_blocks);

        println!(
            "seed = {seed}, blocks = {}/{max_blocks}, expected_peer_changes = {expected_peer_changes}",
            snapshot.blocks().len()
        );

        let (tracker, mut tracker_worker) = build();

        // Action to perform on the set of peers.
        #[derive(Debug)]
        enum Action {
            // Insert a new peer
            Insert,
            // Remove a random peer
            Remove,
            // Keep the peer set intact
            Keep,
        }

        // Total number of simulation steps is the number of index nodes plus the number of blocks
        // in the snapshot. This is used to calculate the probability of the next action.
        let steps = 1 + snapshot.inner_count() + snapshot.leaf_count() + snapshot.blocks().len();

        for tick in 0.. {
            let _enter = tracing::info_span!("tick", message = tick).entered();

            // Generate the next action. The probability of `Insert` or `Remove` is chosen such that
            // the expected number of such actions in the simulation is equal to
            // `expected_peer_changes`. Both `Insert` and `Remove` have currently the same
            // probability.
            let action = if rng.gen_range(0..steps) < expected_peer_changes {
                if rng.gen() {
                    Action::Insert
                } else {
                    Action::Remove
                }
            } else {
                Action::Keep
            };

            match action {
                Action::Insert => {
                    sim.insert_peer(&mut rng, &tracker, snapshot.clone());
                }
                Action::Remove => {
                    if sim.peer_count() < 2 {
                        continue;
                    }

                    sim.remove_peer(&mut rng);
                }
                Action::Keep => {
                    if sim.peer_count() == 0 {
                        continue;
                    }
                }
            }

            let polled = sim.poll(&mut rng);

            if polled || matches!(action, Action::Remove) {
                tracker_worker.step();
            } else {
                break;
            }
        }

        sim.verify(&snapshot);
        assert_eq!(tracker_worker.requests().len(), 0);
    }
}

// Test syncing with multiple peers where no peer has all the blocks but every block is present in
// at least one peer.
#[tokio::test]
async fn missing_blocks() {
    let seed = rand::random();
    case(seed, 32, 4);

    fn case(seed: u64, max_blocks: usize, max_peers: usize) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut sim = Simulation::new();

        let num_blocks = rng.gen_range(2..=max_blocks);
        let num_peers = rng.gen_range(2..=max_peers);
        let (master_snapshot, peer_snapshots) =
            generate_snapshots_with_missing_blocks(&mut rng, num_peers, num_blocks);

        println!(
            "seed = {seed}, blocks = {num_blocks}/{max_blocks}, peers = {num_peers}/{max_peers}"
        );

        let (tracker, mut tracker_worker) = build();
        for snapshot in peer_snapshots {
            sim.insert_peer(&mut rng, &tracker, snapshot);
        }

        for tick in 0.. {
            let _enter = tracing::info_span!("tick", message = tick).entered();

            if sim.poll(&mut rng) {
                tracker_worker.step();
            } else {
                break;
            }
        }

        sim.verify(&master_snapshot);
        assert_eq!(tracker_worker.requests().cloned().collect::<Vec<_>>(), []);
    }
}

#[tokio::test(start_paused = true)]
async fn timeout() {
    let mut rng = StdRng::seed_from_u64(0);
    let (tracker, tracker_worker) = build();

    let mut work = pin!(tracker_worker.run());

    let (client_a, mut request_rx_a) = tracker.new_client();
    let (client_b, mut request_rx_b) = tracker.new_client();

    let preceding_request_key = MessageKey::RootNode(PublicKey::generate(&mut rng));
    let request = Request::ChildNodes(
        rng.gen(),
        ResponseDisambiguator::new(MultiBlockPresence::Full),
        DebugRequest::start(),
    );

    // Register the request with both clients.
    client_a.success(
        preceding_request_key,
        vec![PendingRequest {
            request: request.clone(),
            block_presence: MultiBlockPresence::Full,
        }],
    );

    client_b.success(
        preceding_request_key,
        vec![PendingRequest {
            request: request.clone(),
            block_presence: MultiBlockPresence::Full,
        }],
    );

    time::timeout(Duration::from_millis(1), &mut work)
        .await
        .ok();

    // Only the first client gets the request.
    assert_eq!(
        request_rx_a.try_recv().map(|r| r.request),
        Ok(request.clone())
    );

    assert_eq!(
        request_rx_b.try_recv().map(|r| r.request),
        Err(TryRecvError::Empty)
    );

    // Wait until the request timeout passes
    time::timeout(REQUEST_TIMEOUT + Duration::from_millis(1), &mut work)
        .await
        .ok();

    // The first client timeouted so the second client now gets the request.
    assert_eq!(
        request_rx_b.try_recv().map(|r| r.request),
        Ok(request.clone())
    );
}

#[tokio::test]
async fn drop_uncommitted_client() {
    let mut rng = StdRng::seed_from_u64(0);
    let (tracker, mut tracker_worker) = build();

    let (client_a, mut request_rx_a) = tracker.new_client();
    let (client_b, mut request_rx_b) = tracker.new_client();

    let preceding_request_key = MessageKey::RootNode(PublicKey::generate(&mut rng));
    let request = Request::ChildNodes(
        rng.gen(),
        ResponseDisambiguator::new(MultiBlockPresence::Full),
        DebugRequest::start(),
    );
    let request_key = MessageKey::from(&request);

    for client in [&client_a, &client_b] {
        client.success(
            preceding_request_key,
            vec![PendingRequest {
                request: request.clone(),
                block_presence: MultiBlockPresence::Full,
            }],
        );
    }

    tracker_worker.step();

    assert_eq!(
        request_rx_a.try_recv().map(|r| r.request),
        Ok(request.clone())
    );
    assert_eq!(
        request_rx_b.try_recv().map(|r| r.request),
        Err(TryRecvError::Empty)
    );

    // Complete the request by the first client.
    client_a.success(request_key, vec![]);
    tracker_worker.step();

    assert_eq!(
        request_rx_a.try_recv().map(|r| r.request),
        Err(TryRecvError::Empty)
    );
    assert_eq!(
        request_rx_b.try_recv().map(|r| r.request),
        Err(TryRecvError::Empty)
    );

    // Drop the first client without commiting.
    drop(client_a);
    tracker_worker.step();

    // The request falls back to the other client because although the request was completed, it
    // wasn't committed.
    assert_eq!(request_rx_b.try_recv().map(|r| r.request), Ok(request));
}

/// Generate `count + 1` copies of the same snapshot. The first one will have all the blocks
/// present (the "master copy"). The remaining ones will have some blocks missing but in such a
/// way that every block is present in at least one of the snapshots.
fn generate_snapshots_with_missing_blocks(
    mut rng: &mut impl Rng,
    count: usize,
    num_blocks: usize,
) -> (Snapshot, Vec<Snapshot>) {
    let all_blocks: Vec<(Hash, Block)> = rng.sample_iter(Standard).take(num_blocks).collect();

    let mut partial_block_sets = Vec::with_capacity(count);
    partial_block_sets.resize_with(count, || Vec::with_capacity(num_blocks));

    // Every block is present in one snapshot and has a 50% (1:2) chance of being present in any of
    // the other shapshots respectively.
    let bernoulli = Bernoulli::from_ratio(1, 2).unwrap();

    let mut batch = Vec::with_capacity(count);

    for (locator, block) in &all_blocks {
        // Poor man's Binomial distribution
        let num_present = 1 + (1..count).filter(|_| bernoulli.sample(&mut rng)).count();
        let num_missing = count - num_present;

        batch.extend(
            iter::repeat(block.clone())
                .map(BlockState::Present)
                .take(num_present)
                .chain(
                    iter::repeat(block.id)
                        .map(BlockState::Missing)
                        .take(num_missing),
                ),
        );
        batch.shuffle(&mut rng);

        for (index, block) in batch.drain(..).enumerate() {
            partial_block_sets[index].push((*locator, block));
        }
    }

    (
        Snapshot::from_present_blocks(all_blocks),
        partial_block_sets
            .into_iter()
            .map(Snapshot::from_blocks)
            .collect(),
    )
}
