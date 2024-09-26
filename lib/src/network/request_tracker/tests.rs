use super::{
    super::{debug_payload::DebugResponse, message::Response},
    *,
};
use crate::{
    collections::HashSet,
    crypto::{sign::Keypair, Hashable},
    network::message::ResponseDisambiguator,
    protocol::{
        test_utils::{assert_snapshots_equal, BlockState, Snapshot},
        Block, MultiBlockPresence, Proof, UntrustedProof,
    },
    version_vector::VersionVector,
};
use rand::{
    distributions::{Bernoulli, Distribution, Standard},
    rngs::StdRng,
    seq::SliceRandom,
    CryptoRng, Rng, SeedableRng,
};
use std::collections::{hash_map::Entry, VecDeque};

// Test syncing while peers keep joining and leaving the swarm.
//
// Note: We need `tokio::test` here because the `RequestTracker` uses `DelayQueue` internaly which
// needs a tokio runtime.
#[tokio::test]
async fn dynamic_swarm() {
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

// TODO: test failure/timeout

struct Simulation {
    peers: Vec<TestPeer>,
    // All requests sent by live peers. This is used to verify that every request is sent only once
    // unless the peer that sent it died or the request failed. In those cases the request may be
    // sent by another peer. It's also allowed to sent the same request more than once as long as
    // each one has a different block presence.
    requests: HashMap<MessageKey, HashSet<MultiBlockPresence>>,
    snapshot: Snapshot,
}

impl Simulation {
    fn new() -> Self {
        Self {
            peers: Vec::new(),
            requests: HashMap::default(),
            snapshot: Snapshot::default(),
        }
    }

    fn peer_count(&self) -> usize {
        self.peers.len()
    }

    fn insert_peer<R: Rng + CryptoRng>(
        &mut self,
        rng: &mut R,
        tracker: &RequestTracker,
        snapshot: Snapshot,
    ) {
        let (tracker_client, tracker_request_rx) = tracker.new_client();
        let client = TestClient::new(tracker_client, tracker_request_rx);

        let writer_id = PublicKey::generate(rng);
        let write_keys = Keypair::generate(rng);
        let server = TestServer::new(writer_id, write_keys, snapshot);

        self.peers.push(TestPeer {
            client,
            server,
            requests: HashMap::default(),
        });
    }

    fn remove_peer<R: Rng>(&mut self, rng: &mut R) {
        let index = rng.gen_range(0..self.peers.len());
        let peer = self.peers.remove(index);

        for (key, block_presence) in peer.requests {
            cancel_request(&mut self.requests, key, block_presence);
        }
    }

    // Polls random client or server once
    #[track_caller]
    fn poll<R: Rng>(&mut self, rng: &mut R) -> bool {
        enum Side {
            Client,
            Server,
        }

        let mut order: Vec<_> = (0..self.peers.len())
            .flat_map(|index| [(Side::Client, index), (Side::Server, index)])
            .collect();

        order.shuffle(rng);

        for (side, index) in order {
            let peer = &mut self.peers[index];

            match side {
                Side::Client => {
                    if let Some(SendPermit {
                        request,
                        block_presence,
                    }) = peer.client.poll_request()
                    {
                        let key = MessageKey::from(&request);

                        assert!(
                            self.requests.entry(key).or_default().insert(block_presence),
                            "request sent more than once: {request:?} ({block_presence:?})"
                        );

                        peer.requests.insert(key, block_presence);
                        peer.server.handle_request(request);

                        return true;
                    }
                }
                Side::Server => {
                    if let Some(response) = peer.server.poll_response() {
                        // In case of failure,  cancel the request so it can be retried without it
                        // triggering assertion failure.
                        let key = match response {
                            Response::RootNodeError(writer_id, _) => {
                                Some(MessageKey::RootNode(writer_id))
                            }
                            Response::ChildNodesError(hash, _, _) => {
                                Some(MessageKey::ChildNodes(hash))
                            }
                            Response::BlockError(block_id, _) => Some(MessageKey::Block(block_id)),
                            Response::RootNode(..)
                            | Response::InnerNodes(..)
                            | Response::LeafNodes(..)
                            | Response::Block(..)
                            | Response::BlockOffer(..) => None,
                        };

                        if let Some(key) = key {
                            if let Some(block_presence) = peer.requests.get(&key) {
                                cancel_request(&mut self.requests, key, *block_presence);
                            }
                        }

                        peer.client.handle_response(response, &mut self.snapshot);
                        return true;
                    }
                }
            }
        }

        false
    }

    #[track_caller]
    fn verify(&self, expected_snapshot: &Snapshot) {
        assert_snapshots_equal(&self.snapshot, expected_snapshot)
    }
}

fn cancel_request(
    requests: &mut HashMap<MessageKey, HashSet<MultiBlockPresence>>,
    key: MessageKey,
    block_presence: MultiBlockPresence,
) {
    if let Entry::Occupied(mut entry) = requests.entry(key) {
        entry.get_mut().remove(&block_presence);

        if entry.get().is_empty() {
            entry.remove();
        }
    }
}

struct TestPeer {
    client: TestClient,
    server: TestServer,
    // All requests sent by this peer.
    requests: HashMap<MessageKey, MultiBlockPresence>,
}

struct TestClient {
    tracker_client: RequestTrackerClient,
    tracker_request_rx: mpsc::UnboundedReceiver<SendPermit>,
}

impl TestClient {
    fn new(
        tracker_client: RequestTrackerClient,
        tracker_request_rx: mpsc::UnboundedReceiver<SendPermit>,
    ) -> Self {
        Self {
            tracker_client,
            tracker_request_rx,
        }
    }

    fn handle_response(&mut self, response: Response, snapshot: &mut Snapshot) {
        match response {
            Response::RootNode(proof, block_presence, debug_payload) => {
                let requests = snapshot
                    .insert_root(proof.hash, block_presence)
                    .then_some((
                        Request::ChildNodes(
                            proof.hash,
                            ResponseDisambiguator::new(block_presence),
                            debug_payload.follow_up(),
                        ),
                        block_presence,
                    ))
                    .into_iter()
                    .collect();

                self.tracker_client
                    .success(MessageKey::RootNode(proof.writer_id), requests);
            }
            Response::InnerNodes(nodes, _disambiguator, debug_payload) => {
                let parent_hash = nodes.hash();
                let nodes = snapshot.insert_inners(nodes);

                let requests: Vec<_> = nodes
                    .into_iter()
                    .map(|(_, node)| {
                        (
                            Request::ChildNodes(
                                node.hash,
                                ResponseDisambiguator::new(node.summary.block_presence),
                                debug_payload.follow_up(),
                            ),
                            node.summary.block_presence,
                        )
                    })
                    .collect();

                self.tracker_client
                    .success(MessageKey::ChildNodes(parent_hash), requests);
            }
            Response::LeafNodes(nodes, _disambiguator, debug_payload) => {
                let parent_hash = nodes.hash();
                let nodes = snapshot.insert_leaves(nodes);
                let requests = nodes
                    .into_iter()
                    .map(|node| {
                        (
                            Request::Block(node.block_id, debug_payload.follow_up()),
                            MultiBlockPresence::None,
                        )
                    })
                    .collect();

                self.tracker_client
                    .success(MessageKey::ChildNodes(parent_hash), requests);
            }
            Response::Block(content, nonce, _debug_payload) => {
                let block = Block::new(content, nonce);
                let block_id = block.id;

                snapshot.insert_block(block);

                self.tracker_client
                    .success(MessageKey::Block(block_id), vec![]);
            }
            Response::RootNodeError(writer_id, _debug_payload) => {
                self.tracker_client.failure(MessageKey::RootNode(writer_id));
            }
            Response::ChildNodesError(hash, _disambiguator, _debug_payload) => {
                self.tracker_client.failure(MessageKey::ChildNodes(hash));
            }
            Response::BlockError(block_id, _debug_payload) => {
                self.tracker_client.failure(MessageKey::Block(block_id));
            }
            Response::BlockOffer(_block_id, _debug_payload) => unimplemented!(),
        };
    }

    fn poll_request(&mut self) -> Option<SendPermit> {
        self.tracker_request_rx.try_recv().ok()
    }
}

struct TestServer {
    writer_id: PublicKey,
    write_keys: Keypair,
    snapshot: Snapshot,
    outbox: VecDeque<Response>,
}

impl TestServer {
    fn new(writer_id: PublicKey, write_keys: Keypair, snapshot: Snapshot) -> Self {
        let proof = UntrustedProof::from(Proof::new(
            writer_id,
            VersionVector::first(writer_id),
            *snapshot.root_hash(),
            &write_keys,
        ));

        let outbox = [Response::RootNode(
            proof.clone(),
            snapshot.root_summary().block_presence,
            DebugResponse::unsolicited(),
        )]
        .into();

        Self {
            writer_id,
            write_keys,
            snapshot,
            outbox,
        }
    }

    fn handle_request(&mut self, request: Request) {
        match request {
            Request::RootNode(writer_id, debug_payload) => {
                if writer_id == self.writer_id {
                    let proof = Proof::new(
                        writer_id,
                        VersionVector::first(writer_id),
                        *self.snapshot.root_hash(),
                        &self.write_keys,
                    );

                    self.outbox.push_back(Response::RootNode(
                        proof.into(),
                        self.snapshot.root_summary().block_presence,
                        debug_payload.reply(),
                    ));
                } else {
                    self.outbox
                        .push_back(Response::RootNodeError(writer_id, debug_payload.reply()));
                }
            }
            Request::ChildNodes(hash, disambiguator, debug_payload) => {
                if let Some(nodes) = self.snapshot.get_inner_set(&hash) {
                    self.outbox.push_back(Response::InnerNodes(
                        nodes.clone(),
                        disambiguator,
                        debug_payload.reply(),
                    ));
                } else if let Some(nodes) = self.snapshot.get_leaf_set(&hash) {
                    self.outbox.push_back(Response::LeafNodes(
                        nodes.clone(),
                        disambiguator,
                        debug_payload.reply(),
                    ));
                } else {
                    self.outbox.push_back(Response::ChildNodesError(
                        hash,
                        disambiguator,
                        debug_payload.reply(),
                    ));
                }
            }
            Request::Block(block_id, debug_payload) => {
                if let Some(block) = self.snapshot.blocks().get(&block_id) {
                    self.outbox.push_back(Response::Block(
                        block.content.clone(),
                        block.nonce,
                        debug_payload.reply(),
                    ));
                } else {
                    self.outbox
                        .push_back(Response::BlockError(block_id, debug_payload.reply()));
                }
            }
        }
    }

    fn poll_response(&mut self) -> Option<Response> {
        self.outbox.pop_front()
    }
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
