use super::{
    super::{debug_payload::DebugResponse, message::Response},
    *,
};
use crate::{
    collections::HashSet,
    crypto::{sign::Keypair, Hashable},
    network::message::ResponseDisambiguator,
    protocol::{
        test_utils::{BlockState, Snapshot},
        Block, MultiBlockPresence, Proof, SingleBlockPresence, UntrustedProof,
    },
    version_vector::VersionVector,
};
use rand::{
    distributions::{Bernoulli, Distribution, Standard},
    rngs::StdRng,
    seq::SliceRandom,
    CryptoRng, Rng, SeedableRng,
};
use std::collections::{BTreeMap, VecDeque};

// Test syncing while peers keep joining and leaving the swarm.
//
// Note: We need `tokio::test` here because the `RequestTracker` uses `DelayQueue` internaly which
// needs a tokio runtime.
#[ignore = "fails due to problems with the test setup"]
#[tokio::test]
async fn dynamic_swarm() {
    let seed = rand::random();
    case(seed, 64, 4);

    fn case(seed: u64, max_blocks: usize, expected_peer_changes: usize) {
        let mut rng = StdRng::seed_from_u64(seed);
        let num_blocks = rng.gen_range(1..=max_blocks);
        let snapshot = Snapshot::generate(&mut rng, num_blocks);
        let mut summary = Summary::new(snapshot.blocks().len());

        println!(
            "seed = {seed}, blocks = {}/{max_blocks}, expected_peer_changes = {expected_peer_changes}",
            snapshot.blocks().len()
        );

        let (tracker, mut tracker_worker) = build();
        let mut peers = Vec::new();

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
                    peers.push(make_peer(&mut rng, &tracker, snapshot.clone()));
                }
                Action::Remove => {
                    if peers.len() < 2 {
                        continue;
                    }

                    let index = rng.gen_range(0..peers.len());
                    peers.remove(index);
                }
                Action::Keep => {
                    if peers.is_empty() {
                        continue;
                    }
                }
            }

            let polled = poll_peers(&mut rng, &mut peers, &mut summary);

            if polled || matches!(action, Action::Remove) {
                tracker_worker.step();
            } else {
                break;
            }
        }

        summary.verify(&snapshot);
        assert_eq!(tracker_worker.requests().len(), 0);
    }
}

// Test syncing with multiple peers where no peer has all the blocks but every block is present in
// at least one peer.
#[ignore = "fails due to problems with the test setup"]
#[tokio::test]
async fn missing_blocks() {
    // let seed = rand::random();
    let seed = 830380000365750606;
    case(seed, 8, 2);

    fn case(seed: u64, max_blocks: usize, max_peers: usize) {
        crate::test_utils::init_log();

        let mut rng = StdRng::seed_from_u64(seed);
        let num_blocks = rng.gen_range(2..=max_blocks);
        let num_peers = rng.gen_range(2..=max_peers);
        let (master_snapshot, peer_snapshots) =
            generate_snapshots_with_missing_blocks(&mut rng, num_peers, num_blocks);
        let mut summary = Summary::new(master_snapshot.blocks().len());

        println!(
            "seed = {seed}, blocks = {num_blocks}/{max_blocks}, peers = {num_peers}/{max_peers}"
        );

        let (tracker, mut tracker_worker) = build();
        let mut peers: Vec<_> = peer_snapshots
            .into_iter()
            .map(|snapshot| make_peer(&mut rng, &tracker, snapshot))
            .collect();

        for tick in 0.. {
            let _enter = tracing::info_span!("tick", message = tick).entered();

            if poll_peers(&mut rng, &mut peers, &mut summary) {
                tracker_worker.step();
            } else {
                break;
            }
        }

        summary.verify(&master_snapshot);
        assert_eq!(tracker_worker.requests().cloned().collect::<Vec<_>>(), []);
    }
}

// TODO: test failure/timeout

struct Summary {
    expected_block_count: usize,

    // Using `BTreeMap` so any potential failures are printed in the same order in different test
    // runs.
    requests: BTreeMap<MessageKey, usize>,

    nodes: HashMap<Hash, HashSet<MultiBlockPresence>>,
    blocks: HashSet<BlockId>,

    node_failures: HashMap<Hash, usize>,
    block_failures: HashMap<BlockId, usize>,
}

impl Summary {
    fn new(expected_block_count: usize) -> Self {
        Self {
            expected_block_count,
            requests: BTreeMap::default(),
            nodes: HashMap::default(),
            blocks: HashSet::default(),
            node_failures: HashMap::default(),
            block_failures: HashMap::default(),
        }
    }

    fn send_request(&mut self, request: &Request) {
        *self.requests.entry(MessageKey::from(request)).or_default() += 1;
    }

    fn receive_node(&mut self, hash: Hash, block_presence: MultiBlockPresence) -> bool {
        self.nodes.entry(hash).or_default().insert(block_presence);
        self.blocks.len() < self.expected_block_count
    }

    fn receive_node_failure(&mut self, hash: Hash) {
        *self.node_failures.entry(hash).or_default() += 1;
    }

    fn receive_block(&mut self, block_id: BlockId) {
        self.blocks.insert(block_id);
    }

    fn receive_block_failure(&mut self, block_id: BlockId) {
        *self.block_failures.entry(block_id).or_default() += 1;
    }

    fn verify(self, snapshot: &Snapshot) {
        assert!(
            self.nodes
                .get(snapshot.root_hash())
                .into_iter()
                .flatten()
                .count()
                > 0,
            "root node not received"
        );

        for hash in snapshot
            .inner_nodes()
            .map(|node| &node.hash)
            .chain(snapshot.leaf_nodes().map(|node| &node.locator))
        {
            assert!(
                self.nodes.get(hash).into_iter().flatten().count() > 0,
                "child node not received: {hash:?}"
            );
        }

        for block_id in snapshot.blocks().keys() {
            assert!(
                self.blocks.contains(block_id),
                "block not received: {block_id:?}"
            );
        }

        for (request, &actual_count) in &self.requests {
            let expected_max = match request {
                MessageKey::RootNode(_) => 0,
                MessageKey::ChildNodes(hash) => {
                    self.nodes.get(hash).map(HashSet::len).unwrap_or(0)
                        + self.node_failures.get(hash).copied().unwrap_or(0)
                }
                MessageKey::Block(block_id) => {
                    (if self.blocks.contains(block_id) { 1 } else { 0 })
                        + self.block_failures.get(block_id).copied().unwrap_or(0)
                }
            };

            assert!(
                actual_count <= expected_max,
                "request sent too many times ({} instead of {}): {:?}",
                actual_count,
                expected_max,
                request
            );
        }
    }
}

struct TestClient {
    tracker_client: RequestTrackerClient,
    tracker_request_rx: mpsc::UnboundedReceiver<Request>,
}

impl TestClient {
    fn new(
        tracker_client: RequestTrackerClient,
        tracker_request_rx: mpsc::UnboundedReceiver<Request>,
    ) -> Self {
        Self {
            tracker_client,
            tracker_request_rx,
        }
    }

    fn handle_response(&mut self, response: Response, summary: &mut Summary) {
        match response {
            Response::RootNode(proof, block_presence, debug_payload) => {
                let requests = summary
                    .receive_node(proof.hash, block_presence)
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
                let requests: Vec<_> = nodes
                    .into_iter()
                    .filter(|(_, node)| {
                        summary.receive_node(node.hash, node.summary.block_presence)
                    })
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
                let requests = nodes
                    .into_iter()
                    .filter(|node| {
                        summary.receive_node(
                            node.locator,
                            match node.block_presence {
                                SingleBlockPresence::Present => MultiBlockPresence::Full,
                                SingleBlockPresence::Missing => MultiBlockPresence::None,
                                SingleBlockPresence::Expired => unimplemented!(),
                            },
                        )
                    })
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

                summary.receive_block(block.id);

                self.tracker_client
                    .success(MessageKey::Block(block.id), vec![]);
            }
            Response::RootNodeError(writer_id, _debug_payload) => {
                self.tracker_client.failure(MessageKey::RootNode(writer_id));
            }
            Response::ChildNodesError(hash, _disambiguator, _debug_payload) => {
                summary.receive_node_failure(hash);
                self.tracker_client.failure(MessageKey::ChildNodes(hash));
            }
            Response::BlockError(block_id, _debug_payload) => {
                summary.receive_block_failure(block_id);
                self.tracker_client.failure(MessageKey::Block(block_id));
            }
            Response::BlockOffer(_block_id, _debug_payload) => unimplemented!(),
        };
    }

    fn poll_request(&mut self) -> Option<Request> {
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

    fn handle_request(&mut self, request: Request, summary: &mut Summary) {
        summary.send_request(&request);

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

fn make_peer<R: Rng + CryptoRng>(
    rng: &mut R,
    tracker: &RequestTracker,
    snapshot: Snapshot,
) -> (TestClient, TestServer) {
    let (tracker_client, tracker_request_rx) = tracker.new_client();
    let client = TestClient::new(tracker_client, tracker_request_rx);

    let writer_id = PublicKey::generate(rng);
    let write_keys = Keypair::generate(rng);
    let server = TestServer::new(writer_id, write_keys, snapshot);

    (client, server)
}

// Polls every client and server once, in random order
fn poll_peers<R: Rng>(
    rng: &mut R,
    peers: &mut [(TestClient, TestServer)],
    summary: &mut Summary,
) -> bool {
    #[derive(Debug)]
    enum Side {
        Client,
        Server,
    }

    let mut order: Vec<_> = (0..peers.len())
        .flat_map(|index| [(Side::Client, index), (Side::Server, index)])
        .collect();

    order.shuffle(rng);

    let mut changed = false;

    for (side, index) in order {
        let (client, server) = &mut peers[index];

        match side {
            Side::Client => {
                if let Some(request) = client.poll_request() {
                    server.handle_request(request, summary);
                    changed = true;
                }
            }
            Side::Server => {
                if let Some(response) = server.poll_response() {
                    client.handle_response(response, summary);
                    changed = true;
                }
            }
        }
    }

    changed
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
