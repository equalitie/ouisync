use super::{
    constants::REQUEST_TIMEOUT,
    debug_payload::{DebugRequest, DebugResponse},
    message::{Request, Response, ResponseDisambiguator},
};
use crate::{
    block_tracker::{BlockOffer, BlockPromise},
    collections::HashMap,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    protocol::{Block, BlockId, InnerNodes, LeafNodes, MultiBlockPresence, UntrustedProof},
    repository::RepositoryMonitor,
    sync::delay_map::DelayMap,
};
use deadlock::BlockingMutex;
use scoped_task::ScopedJoinHandle;
use std::{collections::hash_map::Entry, future, sync::Arc, task::ready};
use std::{task::Poll, time::Instant};
use tokio::sync::Notify;

pub(crate) enum PendingRequest {
    RootNode(PublicKey, DebugRequest),
    ChildNodes(Hash, ResponseDisambiguator, DebugRequest),
    Block(BlockOffer, DebugRequest),
}

/// Response that's been prepared for processing.
pub(super) enum PreparedResponse {
    RootNode(UntrustedProof, MultiBlockPresence, DebugResponse),
    InnerNodes(CacheHash<InnerNodes>, ResponseDisambiguator, DebugResponse),
    LeafNodes(CacheHash<LeafNodes>, ResponseDisambiguator, DebugResponse),
    BlockOffer(BlockId, DebugResponse),
    // The `BlockPromise` is `None` if the request timeouted but we still received the response
    // afterwards.
    Block(Block, Option<BlockPromise>, DebugResponse),
    RootNodeError(PublicKey, DebugResponse),
    ChildNodesError(Hash, ResponseDisambiguator, DebugResponse),
    BlockError(BlockId, DebugResponse),
}

impl From<Response> for PreparedResponse {
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
            Response::BlockOffer(block_id, debug) => Self::BlockOffer(block_id, debug),
            Response::Block(content, nonce, debug) => {
                Self::Block(Block::new(content, nonce), None, debug)
            }
            Response::RootNodeError(writer_id, debug) => Self::RootNodeError(writer_id, debug),
            Response::ChildNodesError(hash, disambiguator, debug) => {
                Self::ChildNodesError(hash, disambiguator, debug)
            }
            Response::BlockError(block_id, debug) => Self::BlockError(block_id, debug),
        }
    }
}

/// Response whose processing requires only read access to the store or no access at all.
pub(super) enum EphemeralResponse {
    BlockOffer(BlockId, DebugResponse),
}

/// Response whose processing requires write access to the store.
pub(super) enum PersistableResponse {
    RootNode(UntrustedProof, MultiBlockPresence, DebugResponse),
    InnerNodes(CacheHash<InnerNodes>, DebugResponse),
    LeafNodes(CacheHash<LeafNodes>, DebugResponse),
    Block(Block, Option<BlockPromise>, DebugResponse),
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) enum IndexKey {
    RootNode(PublicKey),
    ChildNodes(Hash, ResponseDisambiguator),
}

/// Tracks sent requests whose responses have not been received yet.
///
/// This has multiple purposes:
///
/// - To prevent sending duplicate requests.
/// - To know how many requests are in flight which in turn is used to indicate activity.
/// - To track round trip time / latency.
/// - To timeout block requests so that we can send them to other peers instead.
///
/// Note that only block requests are currently timeouted. This is because we currently send block
/// request to only one peer at a time. So if this peer was faulty, without the timeout it could
/// prevent us from receiving that block indefinitely. Index requests, on the other hand, are
/// currently sent to all peers so even with some of the peers being faulty, the responses from the
/// non-faulty ones should eventually be received.
pub(super) struct PendingRequests {
    monitor: Arc<RepositoryMonitor>,
    index: PendingIndexRequests,
    block: Arc<PendingBlockRequests>,
    // This is to ensure the `run_expiration_tracker` task is destroyed with PendingRequests (as
    // opposed to the task being destroyed "sometime after"). This is important because the task
    // holds an Arc to the RepositoryMonitor which must be destroyed prior to reimporting its
    // corresponding repository if the user decides to do so.
    _expiration_tracker_task: ScopedJoinHandle<()>,
}

impl PendingRequests {
    pub fn new(monitor: Arc<RepositoryMonitor>) -> Self {
        let index = PendingIndexRequests::default();
        let block = Arc::new(PendingBlockRequests::default());

        Self {
            monitor: monitor.clone(),
            index,
            block: block.clone(),
            _expiration_tracker_task: scoped_task::spawn(run_expiration_tracker(monitor, block)),
        }
    }

    pub fn insert(&self, pending_request: PendingRequest) -> Option<Request> {
        let request = match pending_request {
            PendingRequest::RootNode(writer_id, debug) => self
                .index
                .try_insert(IndexKey::RootNode(writer_id))
                .then_some(Request::RootNode(writer_id, debug))?,
            PendingRequest::ChildNodes(hash, disambiguator, debug) => self
                .index
                .try_insert(IndexKey::ChildNodes(hash, disambiguator))
                .then_some(Request::ChildNodes(hash, disambiguator, debug))?,
            PendingRequest::Block(block_offer, debug) => {
                let block_promise = block_offer.accept()?;
                let block_id = *block_promise.block_id();
                self.block
                    .try_insert(block_promise)
                    .then_some(Request::Block(block_id, debug))?
            }
        };

        match request {
            Request::RootNode(..) | Request::ChildNodes(..) => {
                self.monitor.index_requests_sent.increment(1);
                self.monitor.index_requests_inflight.increment(1.0);
            }
            Request::Block(..) => {
                self.monitor.block_requests_sent.increment(1);
                self.monitor.block_requests_inflight.increment(1.0);
            }
        }

        Some(request)
    }

    pub fn remove(&self, response: Response) -> PreparedResponse {
        let mut response = PreparedResponse::from(response);

        enum ResponseKind {
            Index,
            Block,
        }

        let status = match &mut response {
            PreparedResponse::RootNode(proof, ..) => self
                .index
                .remove(&IndexKey::RootNode(proof.writer_id))
                .map(|timestamp| (timestamp, ResponseKind::Index)),
            PreparedResponse::RootNodeError(writer_id, ..) => self
                .index
                .remove(&IndexKey::RootNode(*writer_id))
                .map(|timestamp| (timestamp, ResponseKind::Index)),
            PreparedResponse::InnerNodes(nodes, disambiguator, ..) => self
                .index
                .remove(&IndexKey::ChildNodes(nodes.hash(), *disambiguator))
                .map(|timestamp| (timestamp, ResponseKind::Index)),
            PreparedResponse::LeafNodes(nodes, disambiguator, ..) => self
                .index
                .remove(&IndexKey::ChildNodes(nodes.hash(), *disambiguator))
                .map(|timestamp| (timestamp, ResponseKind::Index)),
            PreparedResponse::ChildNodesError(hash, disambiguator, ..) => self
                .index
                .remove(&IndexKey::ChildNodes(*hash, *disambiguator))
                .map(|timestamp| (timestamp, ResponseKind::Index)),
            PreparedResponse::Block(block, block_promise, ..) => {
                self.block
                    .remove(&block.id)
                    .map(|(timestamp, new_block_promise)| {
                        *block_promise = Some(new_block_promise);
                        (timestamp, ResponseKind::Block)
                    })
            }
            PreparedResponse::BlockError(block_id, ..) => self
                .block
                .remove(block_id)
                .map(|(timestamp, _)| (timestamp, ResponseKind::Block)),
            PreparedResponse::BlockOffer(..) => None,
        };

        if let Some((timestamp, kind)) = status {
            self.monitor.request_latency.record(timestamp.elapsed());

            match kind {
                ResponseKind::Index => self.monitor.index_requests_inflight.decrement(1.0),
                ResponseKind::Block => self.monitor.block_requests_inflight.decrement(1.0),
            }
        }

        response
    }
}

impl Drop for PendingRequests {
    fn drop(&mut self) {
        self.monitor
            .index_requests_inflight
            .decrement(self.index.map.lock().unwrap().len() as f64);
        self.monitor
            .block_requests_inflight
            .decrement(self.block.map.lock().unwrap().len() as f64);
    }
}

#[derive(Default)]
struct PendingIndexRequests {
    map: BlockingMutex<HashMap<IndexKey, Instant>>,
}

impl PendingIndexRequests {
    fn try_insert(&self, key: IndexKey) -> bool {
        match self.map.lock().unwrap().entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(Instant::now());
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    fn remove(&self, key: &IndexKey) -> Option<Instant> {
        self.map.lock().unwrap().remove(key)
    }
}

#[derive(Default)]
struct PendingBlockRequests {
    map: BlockingMutex<DelayMap<BlockId, (Instant, BlockPromise)>>,
    // Notify when item is inserted into previously empty map. This restarts the expiration tracker
    // task.
    notify: Notify,
}

impl PendingBlockRequests {
    fn try_insert(&self, block_promise: BlockPromise) -> bool {
        let mut map = self.map.lock().unwrap();

        if let Some(entry) = map.try_insert(*block_promise.block_id()) {
            entry.insert((Instant::now(), block_promise), REQUEST_TIMEOUT);

            if map.len() == 1 {
                drop(map);
                self.notify.notify_waiters();
            }

            true
        } else {
            false
        }
    }

    fn remove(&self, block_id: &BlockId) -> Option<(Instant, BlockPromise)> {
        self.map.lock().unwrap().remove(block_id)
    }
}

async fn run_expiration_tracker(
    monitor: Arc<RepositoryMonitor>,
    pending: Arc<PendingBlockRequests>,
) {
    // NOTE: The `expired` fn does not always complete when the last item is removed from the
    // DelayMap. There is an issue in the DelayQueue used by DelayMap, reported here:
    // https://github.com/tokio-rs/tokio/issues/6751

    loop {
        let notified = pending.notify.notified();

        while expired(&pending.map).await {
            monitor.request_timeouts.increment(1);
            monitor.block_requests_inflight.decrement(1.0);
        }

        // Last item removed from the map. Wait until new item added.
        notified.await;
    }
}

// Wait for the next expired request. This does not block the map so it can be inserted / removed
// from while this is being awaited.
// Returns `true` if a request expired and `false` if there are no more pending requests.
async fn expired(map: &BlockingMutex<DelayMap<BlockId, (Instant, BlockPromise)>>) -> bool {
    future::poll_fn(|cx| Poll::Ready(ready!(map.lock().unwrap().poll_expired(cx))))
        .await
        .is_some()
}
