use super::{
    constants::REQUEST_TIMEOUT,
    debug_payload::{DebugResponse, PendingDebugRequest},
    message::{Request, Response, ResponseDisambiguator},
};
use crate::{
    block_tracker::{BlockOffer, BlockPromise},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    deadlock::BlockingMutex,
    protocol::{Block, BlockId, InnerNodeMap, LeafNodeSet, MultiBlockPresence, UntrustedProof},
    repository::RepositoryMonitor,
    sync::delay_map::DelayMap,
};
use std::{future, sync::Arc, task::ready};
use std::{task::Poll, time::Instant};
use tokio::{sync::OwnedSemaphorePermit, task};

pub(crate) enum PendingRequest {
    RootNode(PublicKey, PendingDebugRequest),
    ChildNodes(Hash, ResponseDisambiguator, PendingDebugRequest),
    Block(BlockOffer, PendingDebugRequest),
}

pub(super) struct PendingResponse {
    pub response: ProcessedResponse,
    // These will be `None` if the request timeouted but we still received the response
    // afterwards.
    pub _client_permit: Option<ClientPermit>,
    pub block_promise: Option<BlockPromise>,
}

pub(super) enum ProcessedResponse {
    RootNode(UntrustedProof, MultiBlockPresence, DebugResponse),
    InnerNodes(
        CacheHash<InnerNodeMap>,
        ResponseDisambiguator,
        DebugResponse,
    ),
    LeafNodes(CacheHash<LeafNodeSet>, ResponseDisambiguator, DebugResponse),
    Block(Block, DebugResponse),
    RootNodeError(PublicKey, DebugResponse),
    ChildNodesError(Hash, ResponseDisambiguator, DebugResponse),
    BlockError(BlockId, DebugResponse),
}

impl ProcessedResponse {
    fn to_key(&self) -> Key {
        match self {
            Self::RootNode(proof, ..) => Key::RootNode(proof.writer_id),
            Self::InnerNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::LeafNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::Block(block, _) => Key::Block(block.id),
            Self::RootNodeError(writer_id, _) => Key::RootNode(*writer_id),
            Self::ChildNodesError(hash, disambiguator, _) => Key::ChildNodes(*hash, *disambiguator),
            Self::BlockError(block_id, _) => Key::Block(*block_id),
        }
    }
}

impl From<Response> for ProcessedResponse {
    fn from(response: Response) -> Self {
        match response {
            Response::RootNode(proof, block_presence, debug) => {
                Self::RootNode(proof, block_presence, debug)
            }
            Response::InnerNodes(nodes, disambiguator, debug) => {
                Self::InnerNodes(nodes.into(), disambiguator, debug)
            }
            Response::LeafNodes(nodes, disambiguator, debug) => {
                Self::LeafNodes(nodes.into(), disambiguator, debug)
            }
            Response::Block(content, nonce, debug) => {
                Self::Block(Block::new(content, nonce), debug)
            }
            Response::RootNodeError(writer_id, debug) => Self::RootNodeError(writer_id, debug),
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

pub(super) struct PendingRequests {
    monitor: Arc<RepositoryMonitor>,
    map: Arc<BlockingMutex<DelayMap<Key, RequestData>>>,
}

impl PendingRequests {
    pub fn new(monitor: Arc<RepositoryMonitor>) -> Self {
        Self {
            monitor,
            map: Arc::new(BlockingMutex::new(DelayMap::default())),
        }
    }

    pub fn insert(
        &self,
        pending_request: PendingRequest,
        link_permit: OwnedSemaphorePermit,
        peer_permit: OwnedSemaphorePermit,
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

        let mut map = self.map.lock().unwrap();

        map.try_insert(key)?.insert(
            RequestData {
                timestamp: Instant::now(),
                block_promise,
                link_permit,
                _peer_permit: peer_permit,
            },
            REQUEST_TIMEOUT,
        );

        // The expiration tracker task is started each time an item is inserted into previously
        // empty map and stopped when the map becomes empty again.
        if map.len() == 1 {
            task::spawn(run_expiration_tracker(
                self.monitor.clone(),
                self.map.clone(),
            ));
        }

        request_added(&self.monitor, &key);

        Some(request)
    }

    pub fn remove(&self, response: Response) -> PendingResponse {
        let response = ProcessedResponse::from(response);
        let key = response.to_key();

        let (client_permit, block_promise) = if let Some(request_data) =
            self.map.lock().unwrap().remove(&key)
        {
            request_removed(&self.monitor, &key, Some(request_data.timestamp));

            // We `drop` the `peer_permit` here but the `Client` will need the `client_permit` and
            // only `drop` it once the request is processed.
            let link_permit = Some(ClientPermit(request_data.link_permit, self.monitor.clone()));
            let block_promise = request_data.block_promise;

            (link_permit, block_promise)
        } else {
            (None, None)
        };

        PendingResponse {
            response,
            _client_permit: client_permit,
            block_promise,
        }
    }
}

fn request_added(monitor: &RepositoryMonitor, key: &Key) {
    *monitor.pending_requests.get() += 1;

    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => *monitor.index_requests_inflight.get() += 1,
        Key::Block(_) => *monitor.block_requests_inflight.get() += 1,
    }
}

fn request_removed(monitor: &RepositoryMonitor, key: &Key, timestamp: Option<Instant>) {
    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => *monitor.index_requests_inflight.get() -= 1,
        Key::Block(_) => *monitor.block_requests_inflight.get() -= 1,
    }

    if let Some(timestamp) = timestamp {
        monitor.request_inflight_metric.record(timestamp.elapsed());
    }
}

async fn run_expiration_tracker(
    monitor: Arc<RepositoryMonitor>,
    request_map: Arc<BlockingMutex<DelayMap<Key, RequestData>>>,
) {
    while let Some((key, _)) = expired(&request_map).await {
        *monitor.request_timeouts.get() += 1;
        request_removed(&monitor, &key, None);
    }
}

// Wait for the next expired request. This does not block the map so it can be inserted / removed
// from while this is being awaited.
async fn expired(map: &BlockingMutex<DelayMap<Key, RequestData>>) -> Option<(Key, RequestData)> {
    future::poll_fn(|cx| Poll::Ready(ready!(map.lock().unwrap().poll_expired(cx)))).await
}

impl Drop for PendingRequests {
    fn drop(&mut self) {
        for (key, ..) in self.map.lock().unwrap().drain() {
            request_removed(&self.monitor, &key, None);
        }
    }
}

struct RequestData {
    timestamp: Instant,
    block_promise: Option<BlockPromise>,
    link_permit: OwnedSemaphorePermit,
    _peer_permit: OwnedSemaphorePermit,
}

pub(super) struct ClientPermit(OwnedSemaphorePermit, Arc<RepositoryMonitor>);

impl Drop for ClientPermit {
    fn drop(&mut self) {
        *self.1.pending_requests.get() -= 1;
    }
}
