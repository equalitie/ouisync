use super::{
    super::message::{Request, Response, ResponseDisambiguator},
    CandidateRequest, MessageKey, PendingRequest, RequestTracker, RequestTrackerClient,
    RequestVariant,
};
use crate::{
    collections::{HashMap, HashSet},
    crypto::{
        sign::{Keypair, PublicKey},
        Hashable,
    },
    network::debug_payload::DebugResponse,
    protocol::{
        test_utils::{assert_snapshots_equal, Snapshot},
        Block, MultiBlockPresence, Proof, UntrustedProof,
    },
    version_vector::VersionVector,
};
use rand::{seq::SliceRandom, CryptoRng, Rng};
use std::collections::{hash_map::Entry, VecDeque};
use tokio::sync::mpsc;

/// Simple network simulation for testing `RequestTracker`.
pub(super) struct Simulation {
    peers: Vec<TestPeer>,
    // All requests sent by live peers. This is used to verify that every request is sent only once
    // unless the peer that sent it died or the request failed. In those cases the request may be
    // sent by another peer. It's also allowed to sent the same request more than once as long as
    // each one has a different variant.
    requests: HashMap<MessageKey, HashSet<RequestVariant>>,
    snapshot: Snapshot,
}

impl Simulation {
    pub fn new() -> Self {
        Self {
            peers: Vec::new(),
            requests: HashMap::default(),
            snapshot: Snapshot::default(),
        }
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn insert_peer<R: Rng + CryptoRng>(
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

    pub fn remove_peer<R: Rng>(&mut self, rng: &mut R) {
        let index = rng.gen_range(0..self.peers.len());
        let peer = self.peers.remove(index);

        for (key, variant) in peer.requests {
            cancel_request(&mut self.requests, key, variant);
        }
    }

    // Polls random client or server once
    #[track_caller]
    pub fn poll<R: Rng>(&mut self, rng: &mut R) -> bool {
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
                    if let Some(PendingRequest { payload, variant }) = peer.client.poll_request() {
                        let key = MessageKey::from(&payload);

                        assert!(
                            self.requests.entry(key).or_default().insert(variant),
                            "request sent more than once: {payload:?} ({variant:?})"
                        );

                        peer.requests.insert(key, variant);
                        peer.server.handle_request(payload);

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
                            if let Some(variant) = peer.requests.get(&key) {
                                cancel_request(&mut self.requests, key, *variant);
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
    pub fn verify(&self, expected_snapshot: &Snapshot) {
        assert_snapshots_equal(&self.snapshot, expected_snapshot)
    }
}

fn cancel_request(
    requests: &mut HashMap<MessageKey, HashSet<RequestVariant>>,
    key: MessageKey,
    variant: RequestVariant,
) {
    if let Entry::Occupied(mut entry) = requests.entry(key) {
        entry.get_mut().remove(&variant);

        if entry.get().is_empty() {
            entry.remove();
        }
    }
}

struct TestPeer {
    client: TestClient,
    server: TestServer,
    // All requests sent by this peer.
    requests: HashMap<MessageKey, RequestVariant>,
}

struct TestClient {
    tracker_client: RequestTrackerClient,
    tracker_request_rx: mpsc::UnboundedReceiver<PendingRequest>,
}

impl TestClient {
    fn new(
        tracker_client: RequestTrackerClient,
        tracker_request_rx: mpsc::UnboundedReceiver<PendingRequest>,
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
                    .then_some(
                        CandidateRequest::new(Request::ChildNodes(
                            proof.hash,
                            ResponseDisambiguator::new(block_presence),
                            debug_payload.follow_up(),
                        ))
                        .variant(RequestVariant::new(
                            MultiBlockPresence::None,
                            block_presence,
                        )),
                    )
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
                        CandidateRequest::new(Request::ChildNodes(
                            node.hash,
                            ResponseDisambiguator::new(node.summary.block_presence),
                            debug_payload.follow_up(),
                        ))
                        .variant(RequestVariant::new(
                            MultiBlockPresence::None,
                            node.summary.block_presence,
                        ))
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
                        CandidateRequest::new(Request::Block(
                            node.block_id,
                            debug_payload.follow_up(),
                        ))
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

        // Note: for simplicity, in this simulation we `commit` after every operation. To test
        // committing properly, separate tests not based on this simulation need to be used.
        self.tracker_client.new_committer().commit();
    }

    fn poll_request(&mut self) -> Option<PendingRequest> {
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
