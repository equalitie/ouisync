use super::message::{request, Request, Response, ResponseDisambiguator};
use crate::{
    block::{tracker::BlockPromise, BlockData, BlockId, BlockNonce},
    collections::{hash_map::Entry, HashMap},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
    repository_stats::{self, RepositoryStats},
    sync::uninitialized_watch,
};
use scoped_task::ScopedJoinHandle;
use std::sync::{Arc, Mutex};
use tokio::{
    select,
    sync::OwnedSemaphorePermit,
    time::{self, Duration, Instant},
};

// Maximum number of request which have been sent but for which we haven't received a response yet.
// Higher values give better performance but too high risks congesting the network. Also there is a
// point of diminishing returns. 32 seems to be the sweet spot based on a simple experiment.
// TODO: run more precise benchmarks to find the actual optimum.
pub(super) const MAX_REQUESTS_IN_FLIGHT: usize = 512;

// If a response to a pending request is not received within this time, a request timeout error is
// triggered.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) enum PendingRequest {
    RootNode(PublicKey),
    ChildNodes(Hash, ResponseDisambiguator),
    Block(BlockPromise),
}

impl PendingRequest {
    pub fn to_message(&self) -> Request {
        match self {
            Self::RootNode(public_key) => request::RootNode(*public_key).into(),
            Self::ChildNodes(hash, disambiguator) => {
                request::ChildNodes(*hash, *disambiguator).into()
            }
            Self::Block(block_promise) => request::Block(*block_promise.block_id()).into(),
        }
    }
}

pub(super) enum PendingResponse {
    RootNode {
        proof: UntrustedProof,
        summary: Summary,
    },
    InnerNodes(CacheHash<InnerNodeMap>, ResponseDisambiguator),
    LeafNodes(CacheHash<LeafNodeSet>, ResponseDisambiguator),
    Block {
        data: BlockData,
        nonce: BlockNonce,
        block_promise: Option<BlockPromise>,
    },
}

pub(super) struct PendingRequests {
    stats: Arc<RepositoryStats>,
    map: Arc<Mutex<HashMap<Request, RequestData>>>,
    to_tracker_tx: uninitialized_watch::Sender<()>,
    _expiration_tracker: ScopedJoinHandle<()>,
}

impl PendingRequests {
    pub fn new(stats: Arc<RepositoryStats>) -> Self {
        let map = Arc::new(Mutex::new(HashMap::<Request, RequestData>::default()));

        let (expiration_tracker, to_tracker_tx) = run_tracker(stats.clone(), map.clone());

        Self {
            stats,
            map,
            to_tracker_tx,
            _expiration_tracker: expiration_tracker,
        }
    }

    pub fn insert(&self, pending_request: PendingRequest, permit: CompoundPermit) -> bool {
        match self.map.lock().unwrap().entry(pending_request.to_message()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                let msg = pending_request.to_message();

                let block_promise = match pending_request {
                    PendingRequest::RootNode(_) => None,
                    PendingRequest::ChildNodes(_, _) => None,
                    PendingRequest::Block(block_promise) => Some(block_promise),
                };

                entry.insert(RequestData {
                    timestamp: Instant::now(),
                    block_promise,
                    permit,
                });
                self.request_added(&msg);
                true
            }
        }
    }

    pub fn remove(
        &self,
        response: Response,
    ) -> Option<(PendingResponse, Option<OwnedSemaphorePermit>)> {
        let response = ProcessedResponse::from(response);
        let request = response.to_request();

        if let Some(request_data) = self.map.lock().unwrap().remove(&request) {
            self.request_removed(&request, Some(request_data.timestamp));
            // We `drop` the `peer_permit` here but the `Client` will need the `client_permit` and
            // only `drop` it once the request is processed.
            match response {
                ProcessedResponse::Success(success) => {
                    let r = match success {
                        processed_response::Success::RootNode { proof, summary } => {
                            PendingResponse::RootNode { proof, summary }
                        }
                        processed_response::Success::InnerNodes(hash, disambiguator) => {
                            PendingResponse::InnerNodes(hash, disambiguator)
                        }
                        processed_response::Success::LeafNodes(hash, disambiguator) => {
                            PendingResponse::LeafNodes(hash, disambiguator)
                        }
                        processed_response::Success::Block { data, nonce } => {
                            PendingResponse::Block {
                                data,
                                nonce,
                                block_promise: request_data.block_promise,
                            }
                        }
                    };
                    Some((r, Some(request_data.permit.client_permit)))
                }
                ProcessedResponse::Failure(_) => None,
            }
        } else {
            // Only `RootNode` response is allowed to be unsolicited
            match response {
                ProcessedResponse::Success(processed_response::Success::RootNode {
                    proof,
                    summary,
                }) => Some((PendingResponse::RootNode { proof, summary }, None)),
                _ => None,
            }
        }
    }

    fn request_added(&self, request: &Request) {
        stats_request_added(&mut self.stats.write(), request);
        self.notify_tracker_task();
    }

    fn request_removed(&self, request: &Request, timestamp: Option<Instant>) {
        stats_request_removed(&mut self.stats.write(), request, timestamp);
        self.notify_tracker_task();
    }

    fn notify_tracker_task(&self) {
        self.to_tracker_tx.send(()).unwrap_or(());
    }
}

fn stats_request_added(stats: &mut repository_stats::Writer, request: &Request) {
    match request {
        Request::RootNode(_) | Request::ChildNodes { .. } => stats.index_requests_inflight += 1,
        Request::Block(_) => stats.block_requests_inflight += 1,
    }
}

fn stats_request_removed(
    stats: &mut repository_stats::Writer,
    request: &Request,
    timestamp: Option<Instant>,
) {
    match request {
        Request::RootNode(_) | Request::ChildNodes { .. } => stats.index_requests_inflight -= 1,
        Request::Block(_) => stats.block_requests_inflight -= 1,
    }
    if let Some(timestamp) = timestamp {
        stats.note_request_inflight_duration(Instant::now() - timestamp);
    }
}

fn run_tracker(
    stats: Arc<RepositoryStats>,
    request_map: Arc<Mutex<HashMap<Request, RequestData>>>,
) -> (ScopedJoinHandle<()>, uninitialized_watch::Sender<()>) {
    let (to_tracker_tx, mut to_tracker_rx) = uninitialized_watch::channel::<()>();

    let expiration_tracker = scoped_task::spawn(async move {
        loop {
            let entry = request_map
                .lock()
                .unwrap()
                .iter()
                .min_by(|(_, lhs), (_, rhs)| lhs.timestamp.cmp(&rhs.timestamp))
                .map(|(k, v)| (*k, v.timestamp));

            if let Some((request, timestamp)) = entry {
                select! {
                    r = to_tracker_rx.changed() => {
                        match r {
                            Ok(()) => continue,
                            Err(_) => break,
                        }
                    }
                    _ = time::sleep_until(timestamp + REQUEST_TIMEOUT) => {
                        // Check it hasn't been removed in a meanwhile for cancel safety.
                        if let Some(mut data) = request_map.lock().unwrap().get_mut(&request) {
                            stats.write().request_timeouts += 1;
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
        for request in self.map.lock().unwrap().keys() {
            self.request_removed(request, None);
        }
    }
}

struct RequestData {
    timestamp: Instant,
    block_promise: Option<BlockPromise>,
    permit: CompoundPermit,
}

// When sending requests, we need to limit it in two ways:
//
// 1. Limit how many requests we send to the peer across all repositories, and
// 2. Limit sending requests from a Client if too many responses are queued up.
pub(super) struct CompoundPermit {
    pub _peer_permit: OwnedSemaphorePermit,
    pub client_permit: OwnedSemaphorePermit,
}

mod processed_response {
    use super::*;

    pub(super) enum Success {
        RootNode {
            proof: UntrustedProof,
            summary: Summary,
        },
        InnerNodes(CacheHash<InnerNodeMap>, ResponseDisambiguator),
        LeafNodes(CacheHash<LeafNodeSet>, ResponseDisambiguator),
        Block {
            data: BlockData,
            nonce: BlockNonce,
        },
    }

    #[derive(Debug)]
    pub(super) enum Failure {
        RootNode(PublicKey),
        ChildNodes(Hash, ResponseDisambiguator),
        Block(BlockId),
    }
}

enum ProcessedResponse {
    Success(processed_response::Success),
    Failure(processed_response::Failure),
}

impl ProcessedResponse {
    fn to_request(&self) -> Request {
        match self {
            Self::Success(processed_response::Success::RootNode { proof, .. }) => {
                request::RootNode(proof.writer_id).into()
            }
            Self::Success(processed_response::Success::InnerNodes(nodes, disambiguator)) => {
                request::ChildNodes(nodes.hash(), *disambiguator).into()
            }
            Self::Success(processed_response::Success::LeafNodes(nodes, disambiguator)) => {
                request::ChildNodes(nodes.hash(), *disambiguator).into()
            }
            Self::Success(processed_response::Success::Block { data, .. }) => {
                request::Block(data.id).into()
            }
            Self::Failure(processed_response::Failure::RootNode(branch_id)) => {
                request::RootNode(*branch_id).into()
            }
            Self::Failure(processed_response::Failure::ChildNodes(hash, disambiguator)) => {
                request::ChildNodes(*hash, *disambiguator).into()
            }
            Self::Failure(processed_response::Failure::Block(block_id)) => {
                request::Block(*block_id).into()
            }
        }
    }
}

impl From<Response> for ProcessedResponse {
    fn from(response: Response) -> Self {
        match response {
            Response::RootNode { proof, summary } => {
                Self::Success(processed_response::Success::RootNode { proof, summary })
            }
            Response::InnerNodes(nodes, disambiguator) => Self::Success(
                processed_response::Success::InnerNodes(nodes.into(), disambiguator),
            ),
            Response::LeafNodes(nodes, disambiguator) => Self::Success(
                processed_response::Success::LeafNodes(nodes.into(), disambiguator),
            ),
            Response::Block { content, nonce } => {
                Self::Success(processed_response::Success::Block {
                    data: content.into(),
                    nonce,
                })
            }
            Response::RootNodeError(branch_id) => {
                Self::Failure(processed_response::Failure::RootNode(branch_id))
            }
            Response::ChildNodesError(hash, disambiguator) => {
                Self::Failure(processed_response::Failure::ChildNodes(hash, disambiguator))
            }
            Response::BlockError(block_id) => {
                Self::Failure(processed_response::Failure::Block(block_id))
            }
        }
    }
}
