use std::time::Duration;

/// If a response to a pending request is not received within this time, a request timeout error is
/// triggered.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of requests that have been sent to a given peer but for which we haven't received
/// a response yet. Higher values give better performance but too high risks congesting the
/// network. There is also a point of diminishing returns. 32 seems to be the sweet spot based on a
/// simple experiment.
/// NOTE: This limit is protecting the peer against being overhelmed by too many requests from us.
// TODO: run more precise benchmarks to find the actual optimum.
pub(super) const MAX_IN_FLIGHT_REQUESTS_PER_PEER: usize = 32;

/// Maximum number of requests that have been sent on a given `Client` but for which the response
/// hasn't yet been processed (although it may have been received).
/// NOTE: This limit is protecting us against being overhelmed by too many responses from the peer.
pub(super) const MAX_PENDING_REQUESTS_PER_CLIENT: usize = 2 * MAX_IN_FLIGHT_REQUESTS_PER_PEER;

/// Maximum number of unchoked peers at the same time.
pub(super) const MAX_UNCHOKED_COUNT: usize = 3;
/// Maximum duration that a peer remains unchoked.
pub(super) const MAX_UNCHOKED_DURATION: Duration = Duration::from_secs(30);

/// Maximum time to wait for a request before the peer is considered inactive. Inactive peers can be
/// choked before their `MAX_UNCHOKED_DURATION` is up.
pub(super) const MAX_INACTIVITY_DURATION: Duration = Duration::from_secs(1);
