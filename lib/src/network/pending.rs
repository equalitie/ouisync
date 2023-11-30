use super::{
    debug_payload::{DebugResponse, PendingDebugRequest},
    message::{Request, Response, ResponseDisambiguator},
};
use crate::{
    block_tracker::{BlockOffer, BlockPromise},
    collections::{hash_map::Entry, HashMap},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    deadlock::BlockingMutex,
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

pub(crate) enum PendingRequest {
    RootNode(PublicKey, PendingDebugRequest),
    ChildNodes(Hash, ResponseDisambiguator, PendingDebugRequest),
    Block(BlockOffer, PendingDebugRequest),
}

pub(super) enum PendingResponse {
    RootNode {
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        permit: Option<ClientPermit>,
        debug: DebugResponse,
    },
    InnerNodes {
        hash: CacheHash<InnerNodeMap>,
        permit: Option<ClientPermit>,
        debug: DebugResponse,
    },
    LeafNodes {
        hash: CacheHash<LeafNodeSet>,
        permit: Option<ClientPermit>,
        debug: DebugResponse,
    },
    Block {
        block: Block,
        // This will be `None` if the request timeouted but we still received the response
        // afterwards.
        block_promise: Option<BlockPromise>,
        permit: Option<ClientPermit>,
        debug: DebugResponse,
    },
    BlockNotFound {
        block_id: BlockId,
        permit: Option<ClientPermit>,
        debug: DebugResponse,
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
    ) -> Option<Request> {
        let (key, block_promise, request) = match pending_request {
            PendingRequest::RootNode(public_key, debug) => (
                Key::RootNode(public_key),
                None,
                Request::RootNode(public_key, debug.send()),
            ),
            PendingRequest::ChildNodes(hash, disambiguator, debug) => (
                Key::ChildNodes(hash, disambiguator),
                None,
                Request::ChildNodes(hash, disambiguator, debug.send()),
            ),
            PendingRequest::Block(offer, debug) => {
                let promise = offer.accept()?;
                let block_id = *promise.block_id();

                (
                    Key::Block(block_id),
                    Some(promise),
                    Request::Block(block_id, debug.send()),
                )
            }
        };

        match self.map.lock().unwrap().entry(key) {
            Entry::Occupied(_) => None,
            Entry::Vacant(entry) => {
                entry.insert(RequestData {
                    timestamp: Instant::now(),
                    block_promise,
                    _peer_permit: peer_permit,
                    client_permit,
                });

                self.request_added(&key);

                Some(request)
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
                ProcessedResponse::RootNode {
                    proof,
                    block_presence,
                    debug,
                } => Some(PendingResponse::RootNode {
                    proof,
                    block_presence,
                    permit,
                    debug,
                }),
                ProcessedResponse::InnerNodes(hash, _disambiguator, debug) => {
                    Some(PendingResponse::InnerNodes {
                        hash,
                        permit,
                        debug,
                    })
                }
                ProcessedResponse::LeafNodes(hash, _disambiguator, debug) => {
                    Some(PendingResponse::LeafNodes {
                        hash,
                        permit,
                        debug,
                    })
                }
                ProcessedResponse::Block { block, debug } => Some(PendingResponse::Block {
                    block,
                    permit,
                    debug,
                    block_promise: request_data.block_promise,
                }),
                ProcessedResponse::BlockError(block_id, debug) => {
                    Some(PendingResponse::BlockNotFound {
                        block_id,
                        permit,
                        debug,
                    })
                }
                ProcessedResponse::RootNodeError(..) | ProcessedResponse::ChildNodesError(..) => {
                    None
                }
            }
        } else {
            // Only `RootNode` response is allowed to be unsolicited
            match response {
                ProcessedResponse::RootNode {
                    proof,
                    block_presence,
                    debug,
                } => Some(PendingResponse::RootNode {
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
                .filter(|(_, data)| data.block_promise.is_some())
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
                            data.block_promise = None;
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
    block_promise: Option<BlockPromise>,
    _peer_permit: OwnedSemaphorePermit,
    client_permit: OwnedSemaphorePermit,
}

pub(super) struct ClientPermit(OwnedSemaphorePermit, Arc<RepositoryMonitor>);

impl Drop for ClientPermit {
    fn drop(&mut self) {
        *self.1.pending_requests.get() -= 1;
    }
}

enum ProcessedResponse {
    RootNode {
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        debug: DebugResponse,
    },
    InnerNodes(
        CacheHash<InnerNodeMap>,
        ResponseDisambiguator,
        DebugResponse,
    ),
    LeafNodes(CacheHash<LeafNodeSet>, ResponseDisambiguator, DebugResponse),
    Block {
        block: Block,
        debug: DebugResponse,
    },
    RootNodeError(PublicKey, DebugResponse),
    ChildNodesError(Hash, ResponseDisambiguator, DebugResponse),
    BlockError(BlockId, DebugResponse),
}

impl ProcessedResponse {
    fn to_key(&self) -> Key {
        match self {
            Self::RootNode { proof, .. } => Key::RootNode(proof.writer_id),
            Self::InnerNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::LeafNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::Block { block, .. } => Key::Block(block.id),
            Self::RootNodeError(branch_id, _) => Key::RootNode(*branch_id),
            Self::ChildNodesError(hash, disambiguator, _) => Key::ChildNodes(*hash, *disambiguator),
            Self::BlockError(block_id, _) => Key::Block(*block_id),
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
            } => Self::RootNode {
                proof,
                block_presence,
                debug,
            },
            Response::InnerNodes(nodes, disambiguator, debug) => {
                Self::InnerNodes(nodes.into(), disambiguator, debug)
            }
            Response::LeafNodes(nodes, disambiguator, debug) => {
                Self::LeafNodes(nodes.into(), disambiguator, debug)
            }
            Response::Block {
                content,
                nonce,
                debug,
            } => Self::Block {
                block: Block::new(content, nonce),
                debug,
            },
            Response::RootNodeError(branch_id, debug) => Self::RootNodeError(branch_id, debug),
            Response::ChildNodesError(hash, disambiguator, debug) => {
                Self::ChildNodesError(hash, disambiguator, debug)
            }
            Response::BlockError(block_id, debug) => Self::BlockError(block_id, debug),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) enum Key {
    RootNode(PublicKey),
    ChildNodes(Hash, ResponseDisambiguator),
    Block(BlockId),
}
