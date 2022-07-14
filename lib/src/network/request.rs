use super::message::Request;
use crate::repository;
use std::{
    collections::{hash_map::Entry, HashMap},
    future,
    sync::Weak,
    time::{Duration, Instant},
};
use tokio::{sync::OwnedSemaphorePermit, time};

// Maximum number of sent request for which we haven't received a response yet.
// Higher values give better performance but too high risks congesting the network. Also there is a
// point of diminishing returns. 32 seems to be the sweet spot based on a simple experiment.
// TODO: run more precise benchmarks to find the actual optimum.
pub(super) const MAX_PENDING_REQUESTS: usize = 32;

// If a response to a pending request is not received within this time, a request timeout error is
// triggered.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct PendingRequests {
    monitored: Weak<repository::MonitoredValues>,
    map: HashMap<Request, RequestData>,
}

impl PendingRequests {
    pub fn new(monitored: Weak<repository::MonitoredValues>) -> Self {
        Self {
            monitored,
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, request: Request, permit: OwnedSemaphorePermit) -> bool {
        match self.map.entry(request) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(RequestData {
                    timestamp: Instant::now(),
                    _permit: permit,
                });
                self.request_added(&request);
                true
            }
        }
    }

    pub fn remove(&mut self, request: &Request) -> bool {
        if self.map.remove(request).is_some() {
            self.request_removed(request);
            true
        } else {
            false
        }
    }

    /// Wait until a pending request expires and remove it from the collection. If there are
    /// currently no pending requests, this method waits forever.
    ///
    /// This method is cancel-safe in the sense that no request is removed if the returned future
    /// is dropped before being driven to completion.
    pub async fn expired(&mut self) {
        if let Some((&request, &RequestData { timestamp, .. })) = self
            .map
            .iter()
            .min_by(|(_, lhs), (_, rhs)| lhs.timestamp.cmp(&rhs.timestamp))
        {
            time::sleep_until((timestamp + REQUEST_TIMEOUT).into()).await;
            self.remove(&request);
        } else {
            future::pending().await
        }
    }

    fn request_added(&self, request: &Request) {
        if let Some(monitored) = self.monitored.upgrade() {
            match request {
                Request::ChildNodes(_) => *monitored.index_requests_inflight.get() += 1,
                Request::Block(_) => *monitored.block_requests_inflight.get() += 1,
            }
        }
    }

    fn request_removed(&self, request: &Request) {
        if let Some(monitored) = self.monitored.upgrade() {
            match request {
                Request::ChildNodes(_) => *monitored.index_requests_inflight.get() -= 1,
                Request::Block(_) => *monitored.block_requests_inflight.get() -= 1,
            }
        }
    }
}

impl Drop for PendingRequests {
    fn drop(&mut self) {
        for request in self.map.keys() {
            self.request_removed(request);
        }
    }
}

struct RequestData {
    timestamp: Instant,
    _permit: OwnedSemaphorePermit,
}
