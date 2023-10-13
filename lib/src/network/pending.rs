use super::{
    debug_payload::{DebugReceivedResponse, DebugRequest},
    message::{Request, Response, ResponseDisambiguator},
};
use crate::{
    collections::{hash_map::Entry, HashMap},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    deadlock::BlockingMutex,
    missing_parts::PartPromise,
    protocol::{Block, BlockId, InnerNodeMap, LeafNodeSet, MultiBlockPresence, UntrustedProof},
    repository::RepositoryMonitor,
    sync::uninitialized_watch,
};
use scoped_task::ScopedJoinHandle;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::{select, sync::OwnedSemaphorePermit, time};

// If a response to a pending request is not received within this time, a request timeout error is
// triggered.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) enum PendingRequest {
    RootNode(PublicKey, DebugRequest),
    ChildNodes(Hash, ResponseDisambiguator, DebugRequest),
    Block(PartPromise, DebugRequest),
}

impl PendingRequest {
    pub fn to_key(&self) -> Key {
        // Debug payloads are ignored in keys.
        match self {
            Self::RootNode(public_key, _) => Key::RootNode(*public_key),
            Self::ChildNodes(hash, disambiguator, _) => Key::ChildNodes(*hash, *disambiguator),
            Self::Block(part_promise, _) => Key::Block(*part_promise.block_id()),
        }
    }

    pub fn to_message(&self) -> Request {
        match self {
            Self::RootNode(public_key, debug) => Request::RootNode(*public_key, debug.send()),
            Self::ChildNodes(hash, disambiguator, debug) => {
                Request::ChildNodes(*hash, *disambiguator, debug.send())
            }
            Self::Block(part_promise, debug) => {
                Request::Block(*part_promise.block_id(), debug.send())
            }
        }
    }
}

pub(super) enum PendingResponse {
    RootNode {
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        permit: Option<ClientPermit>,
        debug: DebugReceivedResponse,
    },
    InnerNodes {
        hash: CacheHash<InnerNodeMap>,
        permit: Option<ClientPermit>,
        debug: DebugReceivedResponse,
    },
    LeafNodes {
        hash: CacheHash<LeafNodeSet>,
        permit: Option<ClientPermit>,
        debug: DebugReceivedResponse,
    },
    Block {
        block: Block,
        // This will be `None` if the request timeouted but we still received the response
        // afterwards.
        part_promise: Option<PartPromise>,
        permit: Option<ClientPermit>,
        debug: DebugReceivedResponse,
    },
    BlockNotFound {
        block_id: BlockId,
        permit: Option<ClientPermit>,
        debug: DebugReceivedResponse,
    },
}

pub(super) struct PendingRequests {
    monitor: Arc<RepositoryMonitor>,
    map: Arc<BlockingMutex<HashMap<Key, RequestData>>>,
    to_tracker_tx: uninitialized_watch::Sender<()>,
    _expiration_tracker: ScopedJoinHandle<()>,
}

impl PendingRequests {
    pub fn new(monitor: Arc<RepositoryMonitor>) -> Self {
        let map = Arc::new(BlockingMutex::new(HashMap::<Key, RequestData>::default()));

        let (expiration_tracker, to_tracker_tx) = run_tracker(monitor.clone(), map.clone());

        Self {
            monitor,
            map,
            to_tracker_tx,
            _expiration_tracker: expiration_tracker,
        }
    }

    pub fn insert(
        &self,
        pending_request: PendingRequest,
        peer_permit: OwnedSemaphorePermit,
        client_permit: OwnedSemaphorePermit,
    ) -> bool {
        match self.map.lock().unwrap().entry(pending_request.to_key()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                let msg = pending_request.to_key();

                let part_promise = match pending_request {
                    PendingRequest::RootNode(_, _) => None,
                    PendingRequest::ChildNodes(_, _, _) => None,
                    PendingRequest::Block(part_promise, _) => Some(part_promise),
                };

                entry.insert(RequestData {
                    timestamp: Instant::now(),
                    part_promise,
                    _peer_permit: peer_permit,
                    client_permit,
                });
                self.request_added(&msg);
                true
            }
        }
    }

    pub fn remove(&self, response: Response) -> Option<PendingResponse> {
        let response = ProcessedResponse::from(response);
        let key = response.to_key();

        if let Some(request_data) = self.map.lock().unwrap().remove(&key) {
            self.request_removed(&key, Some(request_data.timestamp));

            // We `drop` the `peer_permit` here but the `Client` will need the `client_permit` and
            // only `drop` it once the request is processed.
            let permit = Some(ClientPermit(
                request_data.client_permit,
                self.monitor.clone(),
            ));

            match response {
                ProcessedResponse::Success(success) => {
                    let r = match success {
                        processed_response::Success::RootNode {
                            proof,
                            block_presence,
                            debug,
                        } => PendingResponse::RootNode {
                            proof,
                            block_presence,
                            permit,
                            debug,
                        },
                        processed_response::Success::InnerNodes(hash, _disambiguator, debug) => {
                            PendingResponse::InnerNodes {
                                hash,
                                permit,
                                debug,
                            }
                        }
                        processed_response::Success::LeafNodes(hash, _disambiguator, debug) => {
                            PendingResponse::LeafNodes {
                                hash,
                                permit,
                                debug,
                            }
                        }
                        processed_response::Success::Block { block, debug } => {
                            PendingResponse::Block {
                                block,
                                permit,
                                debug,
                                part_promise: request_data.part_promise,
                            }
                        }
                    };
                    Some(r)
                }
                ProcessedResponse::Failure(processed_response::Failure::Block(block_id, debug)) => {
                    Some(PendingResponse::BlockNotFound {
                        block_id,
                        permit,
                        debug,
                    })
                }
                ProcessedResponse::Failure(_) => None,
            }
        } else {
            // Only `RootNode` response is allowed to be unsolicited
            match response {
                ProcessedResponse::Success(processed_response::Success::RootNode {
                    proof,
                    block_presence,
                    debug,
                }) => Some(PendingResponse::RootNode {
                    proof,
                    block_presence,
                    permit: None,
                    debug,
                }),
                _ => None,
            }
        }
    }

    fn request_added(&self, key: &Key) {
        stats_request_added(&self.monitor, key);
        self.notify_tracker_task();
    }

    fn request_removed(&self, key: &Key, timestamp: Option<Instant>) {
        stats_request_removed(&self.monitor, key, timestamp);
        self.notify_tracker_task();
    }

    fn notify_tracker_task(&self) {
        self.to_tracker_tx.send(()).unwrap_or(());
    }
}

fn stats_request_added(monitor: &RepositoryMonitor, key: &Key) {
    *monitor.pending_requests.get() += 1;

    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => *monitor.index_requests_inflight.get() += 1,
        Key::Block(_) => *monitor.block_requests_inflight.get() += 1,
    }
}

fn stats_request_removed(monitor: &RepositoryMonitor, key: &Key, timestamp: Option<Instant>) {
    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => *monitor.index_requests_inflight.get() -= 1,
        Key::Block(_) => *monitor.block_requests_inflight.get() -= 1,
    }
    if let Some(timestamp) = timestamp {
        monitor.request_inflight_metric.record(timestamp.elapsed());
    }
}

fn run_tracker(
    monitor: Arc<RepositoryMonitor>,
    request_map: Arc<BlockingMutex<HashMap<Key, RequestData>>>,
) -> (ScopedJoinHandle<()>, uninitialized_watch::Sender<()>) {
    let (to_tracker_tx, mut to_tracker_rx) = uninitialized_watch::channel::<()>();

    let expiration_tracker = scoped_task::spawn(async move {
        loop {
            let entry = request_map
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, data)| data.part_promise.is_some())
                .min_by(|(_, lhs), (_, rhs)| lhs.timestamp.cmp(&rhs.timestamp))
                .map(|(k, v)| (*k, v.timestamp));

            if let Some((key, timestamp)) = entry {
                select! {
                    r = to_tracker_rx.changed() => {
                        match r {
                            Ok(()) => continue,
                            Err(_) => break,
                        }
                    }
                    _ = time::sleep_until((timestamp + REQUEST_TIMEOUT).into()) => {
                        // Check it hasn't been removed in a meanwhile for cancel safety.
                        if let Some(data) = request_map.lock().unwrap().get_mut(&key) {
                            *monitor.request_timeouts.get() += 1;
                            data.part_promise = None;
                        }
                    }
                };
            } else {
                match to_tracker_rx.changed().await {
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
        }
    });

    (expiration_tracker, to_tracker_tx)
}

impl Drop for PendingRequests {
    fn drop(&mut self) {
        for key in self.map.lock().unwrap().keys() {
            self.request_removed(key, None);
        }
    }
}

struct RequestData {
    timestamp: Instant,
    part_promise: Option<PartPromise>,
    _peer_permit: OwnedSemaphorePermit,
    client_permit: OwnedSemaphorePermit,
}

pub(super) struct ClientPermit(OwnedSemaphorePermit, Arc<RepositoryMonitor>);

impl Drop for ClientPermit {
    fn drop(&mut self) {
        *self.1.pending_requests.get() -= 1;
    }
}

mod processed_response {
    use super::*;

    pub(super) enum Success {
        RootNode {
            proof: UntrustedProof,
            block_presence: MultiBlockPresence,
            debug: DebugReceivedResponse,
        },
        InnerNodes(
            CacheHash<InnerNodeMap>,
            ResponseDisambiguator,
            DebugReceivedResponse,
        ),
        LeafNodes(
            CacheHash<LeafNodeSet>,
            ResponseDisambiguator,
            DebugReceivedResponse,
        ),
        Block {
            block: Block,
            debug: DebugReceivedResponse,
        },
    }

    #[derive(Debug)]
    pub(super) enum Failure {
        RootNode(PublicKey, DebugReceivedResponse),
        ChildNodes(Hash, ResponseDisambiguator, DebugReceivedResponse),
        Block(BlockId, DebugReceivedResponse),
    }
}

enum ProcessedResponse {
    Success(processed_response::Success),
    Failure(processed_response::Failure),
}

impl ProcessedResponse {
    fn to_key(&self) -> Key {
        match self {
            Self::Success(processed_response::Success::RootNode { proof, .. }) => {
                Key::RootNode(proof.writer_id)
            }
            Self::Success(processed_response::Success::InnerNodes(nodes, disambiguator, _)) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::Success(processed_response::Success::LeafNodes(nodes, disambiguator, _)) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::Success(processed_response::Success::Block { block, .. }) => Key::Block(block.id),
            Self::Failure(processed_response::Failure::RootNode(branch_id, _)) => {
                Key::RootNode(*branch_id)
            }
            Self::Failure(processed_response::Failure::ChildNodes(hash, disambiguator, _)) => {
                Key::ChildNodes(*hash, *disambiguator)
            }
            Self::Failure(processed_response::Failure::Block(block_id, _)) => Key::Block(*block_id),
        }
    }
}

impl From<Response> for ProcessedResponse {
    fn from(response: Response) -> Self {
        match response {
            Response::RootNode {
                proof,
                block_presence,
                debug,
            } => Self::Success(processed_response::Success::RootNode {
                proof,
                block_presence,
                debug: debug.received(),
            }),
            Response::InnerNodes(nodes, disambiguator, debug) => {
                Self::Success(processed_response::Success::InnerNodes(
                    nodes.into(),
                    disambiguator,
                    debug.received(),
                ))
            }
            Response::LeafNodes(nodes, disambiguator, debug) => {
                Self::Success(processed_response::Success::LeafNodes(
                    nodes.into(),
                    disambiguator,
                    debug.received(),
                ))
            }
            Response::Block {
                content,
                nonce,
                debug,
            } => Self::Success(processed_response::Success::Block {
                block: Block::new(content, nonce),
                debug: debug.received(),
            }),
            Response::RootNodeError(branch_id, debug) => Self::Failure(
                processed_response::Failure::RootNode(branch_id, debug.received()),
            ),
            Response::ChildNodesError(hash, disambiguator, debug) => Self::Failure(
                processed_response::Failure::ChildNodes(hash, disambiguator, debug.received()),
            ),
            Response::BlockError(block_id, debug) => Self::Failure(
                processed_response::Failure::Block(block_id, debug.received()),
            ),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) enum Key {
    RootNode(PublicKey),
    ChildNodes(Hash, ResponseDisambiguator),
    Block(BlockId),
}
