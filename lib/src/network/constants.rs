// Maximum number of request which have been sent but for which we haven't received a response yet.
// What we want to achieve is to "saturate" the connection in the direction Server -> Client. The
// BitTorrent Economics Paper (PDF), section 2.3 Pipelining uses the number 5 with message sizes
// being 16KB. Our responses don't have a fixed size (blocks are bigger and nodes are smaller) so
// calculating the max-in-flight requests is less straight forward. TODO: Maybe in the future
// Clients can keep track of expected response sizes and limit requests in flight so responses sum
// to 5 * 16KB = 80KB.
pub(super) const MAX_REQUESTS_IN_FLIGHT: usize = 32;

// Maximum number of respones that a `Client` received but had not yet processed before the client
// is allowed to send more requests.
pub(super) const MAX_PENDING_RESPONSES: usize = 512;

// Maximum number of parallel tasks for request handling that the Server is going to create.
pub(super) const MAX_SERVER_PARALLEL_HANDLING: usize = 32;
