use std::time::Duration;

/// If a response to a pending request is not received within this time, a request timeout error is
/// triggered.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of unchoked peers at the same time.
pub(super) const MAX_UNCHOKED_COUNT: usize = 3;
/// Maximum duration that a peer remains unchoked.
pub(super) const MAX_UNCHOKED_DURATION: Duration = Duration::from_secs(30);

/// If we don't receive any message from the peer for this long while the peer is unchoked, we
/// consider the peer as "idle" and we choke them even before their regular unchoke period ends.
pub(super) const UNCHOKED_IDLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Max number of responses to process in a singe batch (that is, in a single db write transaction).
pub(super) const RESPONSE_BATCH_SIZE: usize = 1024;

/// Max number of buffered incoming responses per client.
pub(super) const RESPONSE_BUFFER_SIZE: usize = 12 * RESPONSE_BATCH_SIZE;

/// Max number of buffered incoming requests per server.
pub(super) const REQUEST_BUFFER_SIZE: usize = 1024;
