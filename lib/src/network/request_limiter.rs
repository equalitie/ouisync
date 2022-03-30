use super::message::Request;
use std::{
    collections::HashMap,
    future,
    time::{Duration, Instant},
};
use tokio::time;

// Maximum number of sent request for which we haven't received a response yet.
// Higher values give better performance but too high risks congesting the network. Also there is a
// point of diminishing returns. 32 seems to be the sweet spot based on a simple experiment.
// TODO: run more precise benchmarks to find the actual optimum.
const MAX_PENDING_REQUESTS: usize = 32;

// If a response to a pending request is not received within this time, the request is expired so
// that it doesn't block other requests from being sent.
const PENDING_REQUEST_EXPIRY: Duration = Duration::from_secs(10);

pub(super) struct RequestLimiter(HashMap<Request, Instant>);

impl RequestLimiter {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn is_full(&self) -> bool {
        self.0.len() >= MAX_PENDING_REQUESTS
    }

    pub fn insert(&mut self, request: Request) -> bool {
        self.0.insert(request, Instant::now()).is_none()
    }

    pub fn remove(&mut self, request: &Request) -> bool {
        self.0.remove(request).is_some()
    }

    pub async fn expired(&mut self) {
        if let Some((&request, &timestamp)) =
            self.0.iter().min_by(|(_, lhs), (_, rhs)| lhs.cmp(rhs))
        {
            time::sleep_until((timestamp + PENDING_REQUEST_EXPIRY).into()).await;
            self.0.remove(&request);
        } else {
            future::pending().await
        }
    }
}
