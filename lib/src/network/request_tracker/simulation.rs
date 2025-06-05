use super::{
    super::message::{Request, Response},
    CandidateRequest, MessageKey, PendingRequest, RequestTracker, RequestTrackerClient,
    RequestTrackerReceiver, RequestVariant, TryRecvError,
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
                    match peer.client.poll_request() {
                        Ok(PendingRequest { payload, variant }) => {
                            let key = MessageKey::from(&payload);
                            let inserted = self.requests.entry(key).or_default().insert(variant);

                            match payload {
                                Request::RootNode { .. }
                                | Request::ChildNodes(..)
                                | Request::Block(..) => {
                                    assert!(
                                        inserted,
                                        "request sent more than once: {payload:?} ({variant:?})"
                                    );
                                }
                                // `Idle` is actually a notification, not a request, so it's allowed to
                                // be sent more than once.
                                Request::Idle => (),
                            }

                            peer.requests.insert(key, variant);
                            peer.server.handle_request(payload);

                            return true;
                        }
                        Err(TryRecvError::InProgress) => return true,
                        Err(TryRecvError::Empty | TryRecvError::Closed) => (),
                    }
                }
                Side::Server => {
                    if let Some(response) = peer.server.poll_response() {
                        // In case of failure,  cancel the request so it can be retried without it
                        // triggering assertion failure.
                        let key = match response {
                            Response::RootNodeError {
                                writer_id, cookie, ..
                            } => Some(MessageKey::RootNode(writer_id, cookie)),
                            Response::ChildNodesError(hash, _) => {
                                Some(MessageKey::ChildNodes(hash))
                            }
                            Response::BlockError(block_id, _) => Some(MessageKey::Block(block_id)),
                            Response::RootNode { .. }
                            | Response::InnerNodes(..)
                            | Response::LeafNodes(..)
                            | Response::Block(..)
                            | Response::BlockOffer(..)
                            | Response::Choke
                            | Response::Unchoke => None,
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
    tracker_request_rx: RequestTrackerReceiver,
}

impl TestClient {
    fn new(
        tracker_client: RequestTrackerClient,
        tracker_request_rx: RequestTrackerReceiver,
    ) -> Self {
        Self {
            tracker_client,
            tracker_request_rx,
        }
    }

    fn handle_response(&mut self, response: Response, snapshot: &mut Snapshot) {
        match response {
            Response::RootNode {
                proof,
                block_presence,
                cookie,
                debug,
            } => {
                let requests = snapshot
                    .insert_root(proof.hash, block_presence)
                    .then_some(
                        CandidateRequest::new(Request::ChildNodes(proof.hash, debug.follow_up()))
                            .variant(RequestVariant::new(
                                MultiBlockPresence::None,
                                block_presence,
                            )),
                    )
                    .into_iter()
                    .collect();

                let key = MessageKey::RootNode(proof.writer_id, cookie);

                self.tracker_client.receive(key);
                self.tracker_client.success(key, requests);
            }
            Response::InnerNodes(nodes, debug_payload) => {
                let parent_hash = nodes.hash();
                let nodes = snapshot.insert_inners(nodes);

                let requests: Vec<_> = nodes
                    .into_iter()
                    .map(|(_, node)| {
                        CandidateRequest::new(Request::ChildNodes(
                            node.hash,
                            debug_payload.follow_up(),
                        ))
                        .variant(RequestVariant::new(
                            MultiBlockPresence::None,
                            node.summary.block_presence,
                        ))
                    })
                    .collect();

                let key = MessageKey::ChildNodes(parent_hash);

                self.tracker_client.receive(key);
                self.tracker_client.success(key, requests);
            }
            Response::LeafNodes(nodes, debug_payload) => {
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

                let key = MessageKey::ChildNodes(parent_hash);

                self.tracker_client.receive(key);
                self.tracker_client.success(key, requests);
            }
            Response::Block(content, nonce, _debug_payload) => {
                let block = Block::new(content, nonce);
                let block_id = block.id;

                snapshot.insert_block(block);

                let key = MessageKey::Block(block_id);

                self.tracker_client.receive(key);
                self.tracker_client.success(key, vec![]);
            }
            Response::RootNodeError {
                writer_id, cookie, ..
            } => {
                let key = MessageKey::RootNode(writer_id, cookie);

                self.tracker_client.receive(key);
                self.tracker_client.failure(key);
            }
            Response::ChildNodesError(hash, _debug_payload) => {
                let key = MessageKey::ChildNodes(hash);

                self.tracker_client.receive(key);
                self.tracker_client.failure(key);
            }
            Response::BlockError(block_id, _debug_payload) => {
                let key = MessageKey::Block(block_id);

                self.tracker_client.receive(key);
                self.tracker_client.failure(key);
            }
            Response::BlockOffer(..) | Response::Choke | Response::Unchoke => unimplemented!(),
        };

        // Note: for simplicity, in this simulation we `commit` after every operation. To test
        // committing properly, separate tests not based on this simulation need to be used.
        self.tracker_client.new_committer().commit();
    }

    fn poll_request(&mut self) -> Result<PendingRequest, TryRecvError> {
        self.tracker_request_rx.try_recv()
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

        let outbox = [Response::RootNode {
            proof: proof.clone(),
            block_presence: snapshot.root_summary().block_presence,
            cookie: 0,
            debug: DebugResponse::unsolicited(),
        }]
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
            Request::RootNode {
                writer_id,
                cookie,
                debug,
            } => {
                if writer_id == self.writer_id {
                    let proof = Proof::new(
                        writer_id,
                        VersionVector::first(writer_id),
                        *self.snapshot.root_hash(),
                        &self.write_keys,
                    );

                    self.outbox.push_back(Response::RootNode {
                        proof: proof.into(),
                        block_presence: self.snapshot.root_summary().block_presence,
                        cookie,
                        debug: debug.reply(),
                    });
                } else {
                    self.outbox.push_back(Response::RootNodeError {
                        writer_id,
                        cookie,
                        debug: debug.reply(),
                    });
                }
            }
            Request::ChildNodes(hash, debug_payload) => {
                if let Some(nodes) = self.snapshot.get_inner_set(&hash) {
                    self.outbox
                        .push_back(Response::InnerNodes(nodes.clone(), debug_payload.reply()));
                } else if let Some(nodes) = self.snapshot.get_leaf_set(&hash) {
                    self.outbox
                        .push_back(Response::LeafNodes(nodes.clone(), debug_payload.reply()));
                } else {
                    self.outbox
                        .push_back(Response::ChildNodesError(hash, debug_payload.reply()));
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
            Request::Idle => (),
        }
    }

    fn poll_response(&mut self) -> Option<Response> {
        self.outbox.pop_front()
    }
}
