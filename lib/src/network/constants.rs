use std::time::Duration;

// If a response to a pending request is not received within this time, a request timeout error is
// triggered.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// Maximum number of request which have been sent but for which we haven't received a response yet.
// Higher values give better performance but too high risks congesting the network. Also there is a
// point of diminishing returns. 32 seems to be the sweet spot based on a simple experiment.
// TODO: run more precise benchmarks to find the actual optimum.
pub(super) const MAX_REQUESTS_IN_FLIGHT: usize = 32;

// Maximum number of respones that a `Client` received but had not yet processed before the client
// is allowed to send more requests.
pub(super) const MAX_PENDING_RESPONSES: usize = 2 * MAX_REQUESTS_IN_FLIGHT;
