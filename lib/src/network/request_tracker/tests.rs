use super::{
    super::{debug_payload::DebugResponse, message::Response},
    *,
};
use crate::{
    crypto::{sign::Keypair, Hashable},
    network::message::ResponseDisambiguator,
    protocol::{test_utils::Snapshot, Block, MultiBlockPresence, Proof, UntrustedProof},
    test_utils,
    version_vector::VersionVector,
};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use std::collections::VecDeque;

// Note: We need `tokio::test` here because the `RequestTracker` uses `DelayQueue` internaly which
// needs a tokio runtime.
#[tokio::test]
async fn simulation() {
    let seed = rand::random();
    simulation_case(seed, 64, 4);
}

fn simulation_case(seed: u64, max_blocks: usize, expected_peer_changes: usize) {
    test_utils::init_log();

    tracing::info!(seed, max_blocks, expected_peer_changes);

    let mut rng = StdRng::seed_from_u64(seed);

    let (tracker, mut tracker_worker) = build();

    let block_count = rng.gen_range(1..=max_blocks);
    let snapshot = Snapshot::generate(&mut rng, block_count);
    let mut summary = Summary::new(snapshot.blocks().len());

    tracing::info!(?snapshot);

    let mut peers = Vec::new();
    let mut total_peer_count = 0; // total number of peers that participated in the simulation

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

    // Total number of simulation steps is the number of index nodes plus the number of blocks in
    // the snapshot. This is used to calculate the probability of the next action.
    let steps = 1 + snapshot.inner_count() + snapshot.leaf_count() + snapshot.blocks().len();

    for tick in 0.. {
        let _enter = tracing::info_span!("tick", message = tick);

        // Generate the next action. The probability of `Insert` or `Remove` is chosen such that the
        // expected number of such actions in the simulation is equal to `expected_peer_changes`.
        // Both `Insert` and `Remove` have currently the same probability.
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
                let (tracker_client, tracker_request_rx) = tracker.new_client();
                let client = TestClient::new(tracker_client, tracker_request_rx);

                let writer_id = PublicKey::generate(&mut rng);
                let write_keys = Keypair::generate(&mut rng);
                let server = TestServer::new(writer_id, write_keys, &snapshot);

                peers.push((client, server));
                total_peer_count += 1;
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

        let polled = poll_peers(&mut rng, &mut peers, &snapshot, &mut summary);

        if polled || matches!(action, Action::Remove) {
            tracker_worker.step();
        } else {
            break;
        }
    }

    summary.verify(total_peer_count, &snapshot);
    assert_eq!(tracker_worker.request_count(), 0);
}

// TODO: test multiple peers with different block summaries
// TODO: test failure/timeout

struct Summary {
    expected_blocks: usize,
    nodes: HashMap<Hash, usize>,
    blocks: HashMap<BlockId, usize>,
}

impl Summary {
    fn new(expected_blocks: usize) -> Self {
        Self {
            expected_blocks,
            nodes: HashMap::default(),
            blocks: HashMap::default(),
        }
    }

    fn receive_node(&mut self, hash: Hash) -> bool {
        *self.nodes.entry(hash).or_default() += 1;
        self.blocks.len() < self.expected_blocks
    }

    fn receive_block(&mut self, block_id: BlockId) {
        *self.blocks.entry(block_id).or_default() += 1;
    }

    fn verify(&mut self, num_peers: usize, snapshot: &Snapshot) {
        assert_eq!(
            self.nodes.remove(snapshot.root_hash()).unwrap_or(0),
            num_peers,
            "root node not received exactly {num_peers} times: {:?}",
            snapshot.root_hash()
        );

        for hash in snapshot
            .inner_nodes()
            .map(|node| &node.hash)
            .chain(snapshot.leaf_nodes().map(|node| &node.locator))
        {
            assert_eq!(
                self.nodes.remove(hash).unwrap_or(0),
                1,
                "child node not received exactly once: {hash:?}"
            );
        }

        for block_id in snapshot.blocks().keys() {
            assert_eq!(
                self.blocks.remove(block_id).unwrap_or(0),
                1,
                "block not received exactly once: {block_id:?}"
            );
        }

        // Verify we received only the expected nodes and blocks
        assert!(
            self.nodes.is_empty(),
            "unexpected nodes received: {:?}",
            self.nodes
        );
        assert!(
            self.blocks.is_empty(),
            "unexpected blocks received: {:?}",
            self.blocks
        );
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
                    .receive_node(proof.hash)
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
                    .filter_map(|(_, node)| {
                        summary.receive_node(node.hash).then_some((
                            Request::ChildNodes(
                                node.hash,
                                ResponseDisambiguator::new(node.summary.block_presence),
                                debug_payload.follow_up(),
                            ),
                            node.summary.block_presence,
                        ))
                    })
                    .collect();

                self.tracker_client
                    .success(MessageKey::ChildNodes(parent_hash), requests);
            }
            Response::LeafNodes(nodes, _disambiguator, debug_payload) => {
                let parent_hash = nodes.hash();
                let requests = nodes
                    .into_iter()
                    .filter_map(|node| {
                        summary.receive_node(node.locator).then_some((
                            Request::Block(node.block_id, debug_payload.follow_up()),
                            MultiBlockPresence::None,
                        ))
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
                self.tracker_client.failure(MessageKey::ChildNodes(hash));
            }
            Response::BlockError(block_id, _debug_payload) => {
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
    outbox: VecDeque<Response>,
}

impl TestServer {
    fn new(writer_id: PublicKey, write_keys: Keypair, snapshot: &Snapshot) -> Self {
        let proof = UntrustedProof::from(Proof::new(
            writer_id,
            VersionVector::first(writer_id),
            *snapshot.root_hash(),
            &write_keys,
        ));

        let outbox = [Response::RootNode(
            proof.clone(),
            MultiBlockPresence::Full,
            DebugResponse::unsolicited(),
        )]
        .into();

        Self {
            writer_id,
            write_keys,
            outbox,
        }
    }

    fn handle_request(&mut self, request: Request, snapshot: &Snapshot) {
        match request {
            Request::RootNode(writer_id, debug_payload) => {
                if writer_id == self.writer_id {
                    let proof = Proof::new(
                        writer_id,
                        VersionVector::first(writer_id),
                        *snapshot.root_hash(),
                        &self.write_keys,
                    );

                    self.outbox.push_back(Response::RootNode(
                        proof.into(),
                        MultiBlockPresence::Full,
                        debug_payload.reply(),
                    ));
                } else {
                    self.outbox
                        .push_back(Response::RootNodeError(writer_id, debug_payload.reply()));
                }
            }
            Request::ChildNodes(hash, disambiguator, debug_payload) => {
                if let Some(nodes) = snapshot
                    .inner_layers()
                    .flat_map(|layer| layer.inner_maps())
                    .find_map(|(parent_hash, nodes)| (*parent_hash == hash).then_some(nodes))
                {
                    self.outbox.push_back(Response::InnerNodes(
                        nodes.clone(),
                        disambiguator,
                        debug_payload.reply(),
                    ));
                } else if let Some(nodes) = snapshot
                    .leaf_sets()
                    .find_map(|(parent_hash, nodes)| (*parent_hash == hash).then_some(nodes))
                {
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
                if let Some(block) = snapshot.blocks().get(&block_id) {
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

// Polls every client and server once, in random order
fn poll_peers<R: Rng>(
    rng: &mut R,
    peers: &mut [(TestClient, TestServer)],
    snapshot: &Snapshot,
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
                    server.handle_request(request, snapshot);
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
