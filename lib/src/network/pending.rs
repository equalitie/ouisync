use super::{
    constants::REQUEST_TIMEOUT,
    debug_payload::{DebugResponse, PendingDebugRequest},
    message::{Request, Response, ResponseDisambiguator},
};
use crate::{
    block_tracker::{BlockOffer, BlockPromise},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    protocol::{Block, BlockId, InnerNodes, LeafNodes, MultiBlockPresence, UntrustedProof},
    repository::RepositoryMonitor,
    sync::delay_map::DelayMap,
};
use deadlock::BlockingMutex;
use scoped_task::ScopedJoinHandle;
use std::{future, sync::Arc, task::ready};
use std::{task::Poll, time::Instant};
use tokio::sync::Notify;

pub(crate) enum PendingRequest {
    RootNode(PublicKey, PendingDebugRequest),
    ChildNodes(Hash, ResponseDisambiguator, PendingDebugRequest),
    Block(BlockOffer, PendingDebugRequest),
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

impl PreparedResponse {
    fn to_key(&self) -> Key {
        match self {
            Self::RootNode(proof, ..) => Key::RootNode(proof.writer_id),
            Self::InnerNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::LeafNodes(nodes, disambiguator, _) => {
                Key::ChildNodes(nodes.hash(), *disambiguator)
            }
            Self::BlockOffer(block_id, _) => Key::BlockOffer(*block_id),
            Self::Block(block, _, _) => Key::Block(block.id),
            Self::RootNodeError(writer_id, _) => Key::RootNode(*writer_id),
            Self::ChildNodesError(hash, disambiguator, _) => Key::ChildNodes(*hash, *disambiguator),
            Self::BlockError(block_id, _) => Key::Block(*block_id),
        }
    }

    fn set_block_promise(&mut self, new_block_promise: BlockPromise) {
        match self {
            Self::Block(_, block_promise, _) => {
                *block_promise = Some(new_block_promise);
            }
            Self::RootNode(..)
            | Self::InnerNodes(..)
            | Self::LeafNodes(..)
            | Self::BlockOffer(..)
            | Self::BlockError(..)
            | Self::RootNodeError(..)
            | Self::ChildNodesError(..) => (),
        }
    }
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
pub(crate) enum Key {
    RootNode(PublicKey),
    ChildNodes(Hash, ResponseDisambiguator),
    BlockOffer(BlockId),
    Block(BlockId),
}

pub(super) struct PendingRequests {
    monitor: Arc<RepositoryMonitor>,
    map: Arc<BlockingMutex<DelayMap<Key, RequestData>>>,
    // Notify when item is inserted into previously empty map. This restarts the expiration tracker
    // task.
    add_notify: Arc<Notify>,
    // This is to ensure the `run_expiration_tracker` task is destroyed with PendingRequests (as
    // opposed to the task being destroyed "sometime after"). This is important because the task
    // holds an Arc to the RepositoryMonitor which must be destroyed prior to reimporting its
    // corresponding repository if the user decides to do so.
    _expiration_tracker_task: ScopedJoinHandle<()>,
}

impl PendingRequests {
    pub fn new(monitor: Arc<RepositoryMonitor>) -> Self {
        let map = Arc::new(BlockingMutex::new(DelayMap::default()));
        let add_notify = Arc::new(Notify::new());

        Self {
            monitor: monitor.clone(),
            map: map.clone(),
            add_notify: add_notify.clone(),
            _expiration_tracker_task: scoped_task::spawn(run_expiration_tracker(
                monitor, map, add_notify,
            )),
        }
    }

    pub fn insert(&self, pending_request: PendingRequest) -> Option<Request> {
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
            },
            REQUEST_TIMEOUT,
        );

        if map.len() == 1 {
            self.add_notify.notify_waiters();
        }

        request_added(&self.monitor, &key);

        Some(request)
    }

    pub fn remove(&self, response: Response) -> PreparedResponse {
        let mut response = PreparedResponse::from(response);
        let key = response.to_key();

        let mut map = self.map.lock().unwrap();

        if let Some(request_data) = map.remove(&key) {
            request_removed(&self.monitor, &key);

            self.monitor
                .request_latency
                .record(request_data.timestamp.elapsed());

            if let Some(block_promise) = request_data.block_promise {
                response.set_block_promise(block_promise);
            }
        }

        response
    }
}

fn request_added(monitor: &RepositoryMonitor, key: &Key) {
    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => {
            monitor.index_requests_sent.increment(1);
            monitor.index_requests_inflight.increment(1.0);
        }
        Key::Block(_) => {
            monitor.block_requests_sent.increment(1);
            monitor.block_requests_inflight.increment(1.0);
        }
        Key::BlockOffer(_) => (),
    }
}

fn request_removed(monitor: &RepositoryMonitor, key: &Key) {
    match key {
        Key::RootNode(_) | Key::ChildNodes { .. } => monitor.index_requests_inflight.decrement(1.0),
        Key::Block(_) => monitor.block_requests_inflight.decrement(1.0),
        Key::BlockOffer(_) => (),
    }
}

async fn run_expiration_tracker(
    monitor: Arc<RepositoryMonitor>,
    map: Arc<BlockingMutex<DelayMap<Key, RequestData>>>,
    add_notify: Arc<Notify>,
) {
    // NOTE: The `expired` fn does not always complete when the last item is removed from the
    // DelayMap. There is an issue in the DelayQueue used by DelayMap, reported here:
    // https://github.com/tokio-rs/tokio/issues/6751

    loop {
        let notified = add_notify.notified();

        while let Some((key, _)) = expired(&map).await {
            monitor.request_timeouts.increment(1);
            request_removed(&monitor, &key);
        }

        // Last item removed from the map. Wait until new item added.
        notified.await;
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
            request_removed(&self.monitor, &key);
        }
    }
}

struct RequestData {
    timestamp: Instant,
    block_promise: Option<BlockPromise>,
}
