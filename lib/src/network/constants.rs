use std::time::Duration;

/// If a response to a pending request is not received within this time, the request timeouts and is
/// resent to another peer (if one exists).
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum number of unchoked peers at the same time.
pub(super) const MAX_UNCHOKED_COUNT: usize = 3;
/// Maximum duration that a peer remains unchoked.
pub(super) const MAX_UNCHOKED_DURATION: Duration = Duration::from_secs(30);

/// If we don't receive any message from the peer for this long while the peer is unchoked, we
/// consider the peer as "idle" and we choke them even before their regular unchoke period ends.
pub(super) const UNCHOKED_IDLE_TIMEOUT: Duration = Duration::from_secs(3);

/// Capacity of the buffer where incoming responses wait before they are processed.
pub(super) const RESPONSE_BUFFER_SIZE: usize = RESPONSE_BATCH_SIZE;

/// Max number of responses to process in a singe batch (that is, in a single db write transaction).
pub(super) const RESPONSE_BATCH_SIZE: usize = 1024;

/// Max number of requests being processed concurrently per server.
///
/// Note that the request processing is already limited by the number of database connections that
/// can be acquired at the same time. This further limit is necessary however, to put a bound on
/// the number of tasks that already released their db connection but are waiting for the response
/// to be sent. Without this limit we would be vulnerable to memory exhaustion by malicious peers.
pub(super) const MAX_CONCURRENT_REQUESTS: usize = 16;

const _: () = assert!(RESPONSE_BUFFER_SIZE >= RESPONSE_BATCH_SIZE);
