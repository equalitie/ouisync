use super::{constants::REQUEST_TIMEOUT, message::Request};
use crate::{
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    protocol::BlockId,
};
use std::{
    collections::hash_map::Entry,
    iter,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{select, sync::mpsc, task};
use tokio_stream::StreamExt;
use tokio_util::time::{delay_queue, DelayQueue};
use tracing::instrument;

/// Keeps track of in-flight requests. Falls back on another peer in case the request failed (due to
/// error response, timeout or disconnection). Evenly distributes the requests between the peers
/// and ensures every request is only sent to one peer at a time.
pub(super) struct RequestTracker {
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTracker {
    #[expect(dead_code)]
    pub fn new() -> Self {
        let (this, worker) = build();
        task::spawn(worker.run());
        this
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn new_client(&self) -> (RequestTrackerClient, mpsc::UnboundedReceiver<Request>) {
        let client_id = ClientId::next();
        let (request_tx, request_rx) = mpsc::unbounded_channel();

        self.command_tx
            .send(Command::InsertClient {
                client_id,
                request_tx,
            })
            .ok();

        (
            RequestTrackerClient {
                client_id,
                command_tx: self.command_tx.clone(),
            },
            request_rx,
        )
    }
}

pub(super) struct RequestTrackerClient {
    client_id: ClientId,
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTrackerClient {
    /// Handle sending a request that does not follow from any previously received response.
    #[expect(dead_code)]
    pub fn initial(&self, request: Request) {
        self.command_tx
            .send(Command::HandleInitial {
                client_id: self.client_id,
                request,
            })
            .ok();
    }

    /// Handle sending requests that follow from a received success response.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn success(&self, response_key: MessageKey, requests: Vec<Request>) {
        self.command_tx
            .send(Command::HandleSuccess {
                client_id: self.client_id,
                response_key,
                requests,
            })
            .ok();
    }

    /// Handle failure response.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn failure(&self, response_key: MessageKey) {
        self.command_tx
            .send(Command::HandleFailure {
                client_id: self.client_id,
                response_key,
            })
            .ok();
    }
}

impl Drop for RequestTrackerClient {
    fn drop(&mut self) {
        self.command_tx
            .send(Command::RemoveClient {
                client_id: self.client_id,
            })
            .ok();
    }
}

/// Key identifying a request and its corresponding response.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(super) enum MessageKey {
    RootNode(PublicKey),
    ChildNodes(Hash),
    Block(BlockId),
}

impl<'a> From<&'a Request> for MessageKey {
    fn from(request: &'a Request) -> Self {
        match request {
            Request::RootNode(writer_id, _) => MessageKey::RootNode(*writer_id),
            Request::ChildNodes(hash, _, _) => MessageKey::ChildNodes(*hash),
            Request::Block(block_id, _) => MessageKey::Block(*block_id),
        }
    }
}

fn build() -> (RequestTracker, Worker) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    (RequestTracker { command_tx }, Worker::new(command_rx))
}

struct Worker {
    clients: HashMap<ClientId, ClientState>,
    requests: HashMap<MessageKey, RequestState>,
    timeouts: DelayQueue<(ClientId, MessageKey)>,
    command_rx: mpsc::UnboundedReceiver<Command>,
}

impl Worker {
    fn new(command_rx: mpsc::UnboundedReceiver<Command>) -> Self {
        Self {
            clients: HashMap::default(),
            requests: HashMap::default(),
            timeouts: DelayQueue::new(),
            command_rx,
        }
    }

    pub async fn run(mut self) {
        loop {
            select! {
                command = self.command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command);
                    } else {
                        break;
                    }
                }
                Some(expired) = self.timeouts.next() => {
                    let (client_id, request_key) = expired.into_inner();
                    self.handle_failure(client_id, request_key);
                    continue;
                }
            }
        }
    }

    /// Process all currently queued commands.
    #[cfg(test)]
    pub fn step(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            self.handle_command(command);
        }

        // TODO: Check timeouts
    }

    #[cfg(test)]
    pub fn request_count(&self) -> usize {
        self.requests.len()
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::InsertClient {
                client_id,
                request_tx,
            } => {
                self.insert_client(client_id, request_tx);
            }
            Command::RemoveClient { client_id } => {
                self.remove_client(client_id);
            }
            Command::HandleInitial { client_id, request } => {
                self.handle_initial(client_id, request);
            }
            Command::HandleSuccess {
                client_id,
                response_key,
                requests,
            } => {
                self.handle_success(client_id, response_key, requests);
            }
            Command::HandleFailure {
                client_id,
                response_key,
            } => {
                self.handle_failure(client_id, response_key);
            }
        }
    }

    fn insert_client(&mut self, client_id: ClientId, request_tx: mpsc::UnboundedSender<Request>) {
        self.clients.insert(
            client_id,
            ClientState {
                request_tx,
                requests: HashSet::default(),
            },
        );
    }

    #[instrument(skip(self))]
    fn remove_client(&mut self, client_id: ClientId) {
        let Some(client_state) = self.clients.remove(&client_id) else {
            return;
        };

        for message_key in client_state.requests {
            let Entry::Occupied(mut entry) = self.requests.entry(message_key) else {
                continue;
            };

            let Some(client_request_state) = entry.get_mut().clients.remove(&client_id) else {
                continue;
            };

            if let Some(timeout_key) = client_request_state.timeout_key {
                self.timeouts.try_remove(&timeout_key);
            }

            if entry.get().clients.is_empty() {
                entry.remove();
                continue;
            }

            // TODO: fallback to another client, if any
        }
    }

    #[instrument(skip(self))]
    fn handle_initial(&mut self, client_id: ClientId, request: Request) {
        let Some(client_state) = self.clients.get_mut(&client_id) else {
            // client not inserted
            return;
        };

        let request_key = MessageKey::from(&request);

        client_state.requests.insert(request_key);

        let request_state = self
            .requests
            .entry(request_key)
            .or_insert_with(|| RequestState {
                request,
                clients: HashMap::default(),
            });

        let timeout_key = if request_state
            .clients
            .values()
            .all(|state| state.timeout_key.is_none())
        {
            client_state
                .request_tx
                .send(request_state.request.clone())
                .ok();

            Some(
                self.timeouts
                    .insert((client_id, request_key), REQUEST_TIMEOUT),
            )
        } else {
            None
        };

        request_state
            .clients
            .insert(client_id, RequestClientState { timeout_key });
    }

    #[instrument(skip(self))]
    fn handle_success(
        &mut self,
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<Request>,
    ) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.requests.remove(&response_key);
        }

        let mut followup_client_ids = if requests.is_empty() {
            vec![]
        } else {
            vec![client_id]
        };

        if let Entry::Occupied(mut entry) = self.requests.entry(response_key) {
            entry.get_mut().clients.retain(|other_client_id, state| {
                // TODO: remove only those with the same or worse block presence.

                if let Some(timeout_key) = state.timeout_key {
                    self.timeouts.try_remove(&timeout_key);
                }

                if !requests.is_empty() && *other_client_id != client_id {
                    followup_client_ids.push(*other_client_id);
                }

                false
            });

            if entry.get().clients.is_empty() {
                entry.remove();
            }
        }

        for request in requests {
            for (client_id, request) in followup_client_ids
                .iter()
                .copied()
                // TODO: use `repeat_n` once it gets stabilized.
                .zip(iter::repeat(request))
            {
                self.handle_initial(client_id, request);
            }

            // round-robin the requests across the clients
            followup_client_ids.rotate_right(1);
        }
    }

    #[instrument(skip(self))]
    fn handle_failure(&mut self, client_id: ClientId, response_key: MessageKey) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.requests.remove(&response_key);
        }

        let Entry::Occupied(mut entry) = self.requests.entry(response_key) else {
            return;
        };

        if let Some(state) = entry.get_mut().clients.remove(&client_id) {
            if let Some(timeout_key) = state.timeout_key {
                self.timeouts.try_remove(&timeout_key);
            }
        }

        // TODO: prefer one with the same or better block presence as `client_id`.
        if let Some((fallback_client_id, state)) = entry
            .get_mut()
            .clients
            .iter_mut()
            .find(|(_, state)| state.timeout_key.is_none())
        {
            state.timeout_key = Some(
                self.timeouts
                    .insert((*fallback_client_id, response_key), REQUEST_TIMEOUT),
            );

            // TODO: send the request
        }

        if entry.get().clients.is_empty() {
            entry.remove();
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct ClientId(usize);

impl ClientId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

enum Command {
    InsertClient {
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<Request>,
    },
    RemoveClient {
        client_id: ClientId,
    },
    HandleInitial {
        client_id: ClientId,
        request: Request,
    },
    HandleSuccess {
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<Request>,
    },
    HandleFailure {
        client_id: ClientId,
        response_key: MessageKey,
    },
}

struct ClientState {
    request_tx: mpsc::UnboundedSender<Request>,
    requests: HashSet<MessageKey>,
}

struct RequestState {
    request: Request,
    clients: HashMap<ClientId, RequestClientState>,
}

struct RequestClientState {
    // disambiguator: ResponseDisambiguator,
    timeout_key: Option<delay_queue::Key>,
}

#[cfg(test)]
mod tests {
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
    use test_strategy::proptest;

    #[proptest]
    fn sanity_check(
        #[strategy(test_utils::rng_seed_strategy())] seed: u64,
        #[strategy(1usize..=32)] num_blocks: usize,
        #[strategy(1usize..=3)] num_peers: usize,
    ) {
        sanity_check_case(seed, num_blocks, num_peers);
    }

    fn sanity_check_case(seed: u64, num_blocks: usize, num_peers: usize) {
        test_utils::init_log();

        // Tokio runtime needed for `DelayQueue`.
        let _runtime_guard = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .enter();

        let mut rng = StdRng::seed_from_u64(seed);

        let snapshot = Snapshot::generate(&mut rng, num_blocks);
        let mut summary = Summary::default();

        let (tracker, mut tracker_worker) = build();

        let mut peers: Vec<_> = (0..num_peers)
            .map(|_| {
                let (tracker_client, tracker_request_rx) = tracker.new_client();
                let client = TestClient::new(tracker_client, tracker_request_rx);

                let writer_id = PublicKey::generate(&mut rng);
                let write_keys = Keypair::generate(&mut rng);
                let server = TestServer::new(writer_id, write_keys, &snapshot);

                (client, server)
            })
            .collect();

        while poll_peers(&mut rng, &mut peers, &snapshot, &mut summary) {
            tracker_worker.step();
        }

        summary.verify(peers.len(), &snapshot);

        assert_eq!(tracker_worker.request_count(), 0);
    }

    #[derive(Default)]
    struct Summary {
        nodes: HashMap<Hash, usize>,
        blocks: HashMap<BlockId, usize>,
    }

    impl Summary {
        fn receive_node(&mut self, hash: Hash) {
            *self.nodes.entry(hash).or_default() += 1;
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
                    summary.receive_node(proof.hash);

                    let requests = vec![Request::ChildNodes(
                        proof.hash,
                        ResponseDisambiguator::new(block_presence),
                        debug_payload.follow_up(),
                    )];

                    self.tracker_client
                        .success(MessageKey::RootNode(proof.writer_id), requests);
                }
                Response::InnerNodes(nodes, _disambiguator, debug_payload) => {
                    let parent_hash = nodes.hash();
                    let requests: Vec<_> = nodes
                        .into_iter()
                        .map(|(_, node)| {
                            summary.receive_node(node.hash);

                            Request::ChildNodes(
                                node.hash,
                                ResponseDisambiguator::new(node.summary.block_presence),
                                debug_payload.follow_up(),
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
                        .map(|node| {
                            summary.receive_node(node.locator);

                            Request::Block(node.block_id, debug_payload.follow_up())
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
                    }

                    if let Some(nodes) = snapshot
                        .leaf_sets()
                        .find_map(|(parent_hash, nodes)| (*parent_hash == hash).then_some(nodes))
                    {
                        self.outbox.push_back(Response::LeafNodes(
                            nodes.clone(),
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
}
