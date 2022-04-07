use super::message::Request;
use std::{
    collections::{hash_map::Entry, HashMap},
    future,
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

pub(super) struct PendingRequests(HashMap<Request, RequestData>);

impl PendingRequests {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(&mut self, request: Request, permit: OwnedSemaphorePermit) -> bool {
        match self.0.entry(request) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(RequestData {
                    timestamp: Instant::now(),
                    _permit: permit,
                });
                true
            }
        }
    }

    pub fn remove(&mut self, request: &Request) -> bool {
        self.0.remove(request).is_some()
    }

    /// Wait until a pending request expires and remove it from the collection. If there are
    /// currently no pending requests, this method waits forever.
    ///
    /// This method is cancel-safe in the sense that no request is removed if the returned future
    /// is dropped before being driven to completion.
    pub async fn expired(&mut self) {
        if let Some((&request, &RequestData { timestamp, .. })) = self
            .0
            .iter()
            .min_by(|(_, lhs), (_, rhs)| lhs.timestamp.cmp(&rhs.timestamp))
        {
            time::sleep_until((timestamp + REQUEST_TIMEOUT).into()).await;
            self.0.remove(&request);
        } else {
            future::pending().await
        }
    }
}

struct RequestData {
    timestamp: Instant,
    _permit: OwnedSemaphorePermit,
}
