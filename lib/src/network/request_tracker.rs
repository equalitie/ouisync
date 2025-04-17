mod graph;

#[cfg(test)]
mod simulation;
#[cfg(test)]
mod tests;

use self::graph::{Graph, Key as GraphKey};
use super::message::Request;
use crate::{
    collections::HashMap,
    crypto::{sign::PublicKey, Hash},
    protocol::{BlockId, MultiBlockPresence},
    repository::monitor::{RequestEvent, RequestKind, TrafficMonitor},
};
use std::{
    collections::{hash_map::Entry, VecDeque},
    fmt, iter, mem,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tokio::{select, sync::mpsc, task};
use tokio_stream::StreamExt;
use tokio_util::time::{delay_queue, DelayQueue};
use tracing::{instrument, Instrument, Span};
use xxhash_rust::xxh3::Xxh3Default;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Keeps track of in-flight requests. Falls back on another peer in case the request failed (due to
/// error response, timeout or disconnection). Evenly distributes the requests between the peers
/// and ensures every request is only sent to one peer at a time.
#[derive(Clone)]
pub(super) struct RequestTracker {
    command_tx: CommandSender,
}

impl RequestTracker {
    pub fn new(monitor: TrafficMonitor) -> Self {
        let (this, worker) = build(monitor);
        task::spawn(worker.run().instrument(Span::current()));
        this
    }

    pub fn set_timeout(&self, timeout: Duration) {
        self.command_tx.send(Command::SetTimeout { timeout });
    }

    pub fn new_client(
        &self,
    ) -> (
        RequestTrackerClient,
        mpsc::UnboundedReceiver<PendingRequest>,
    ) {
        let client_id = ClientId::next();
        let (request_tx, request_rx) = mpsc::unbounded_channel();

        self.command_tx.send(Command::InsertClient {
            client_id,
            request_tx,
        });

        (
            RequestTrackerClient {
                client_id,
                command_tx: self.command_tx.clone(),
            },
            request_rx,
        )
    }
}

pub(super) struct RequestTrackerClient {
    client_id: ClientId,
    command_tx: CommandSender,
}

impl RequestTrackerClient {
    /// Handle sending a request that does not follow from any previously received response.
    pub fn initial(&self, request: CandidateRequest) {
        self.command_tx.send(Command::HandleInitial {
            client_id: self.client_id,
            request,
        });
    }

    /// Handle sending requests that follow from a received success response.
    pub fn success(&self, request_key: MessageKey, requests: Vec<CandidateRequest>) {
        self.command_tx.send(Command::HandleSuccess {
            client_id: self.client_id,
            request_key,
            requests,
        });
    }

    /// Handle failure response.
    pub fn failure(&self, request_key: MessageKey) {
        self.command_tx.send(Command::HandleFailure {
            client_id: self.client_id,
            request_key,
        });
    }

    /// Resume suspended request.
    pub fn resume(&self, request_key: MessageKey, variant: RequestVariant) {
        self.command_tx.send(Command::Resume {
            request_key,
            variant,
        });
    }

    /// Obtain a handle to commit all the requests that were successfully completed by this client.
    /// The handle can be sent to other tasks/threads before invoking the commit.
    pub fn new_committer(&self) -> RequestTrackerCommitter {
        RequestTrackerCommitter {
            client_id: self.client_id,
            command_tx: self.command_tx.clone(),
        }
    }

    /// Choke this client.
    pub fn choke(&self) {
        self.command_tx.send(Command::Choke {
            client_id: self.client_id,
        });
    }

    /// Unchoke this client.
    pub fn unchoke(&self) {
        self.command_tx.send(Command::Unchoke {
            client_id: self.client_id,
        });
    }
}

impl Drop for RequestTrackerClient {
    fn drop(&mut self) {
        self.command_tx.send(Command::RemoveClient {
            client_id: self.client_id,
        });
    }
}

pub(crate) struct RequestTrackerCommitter {
    client_id: ClientId,
    command_tx: CommandSender,
}

impl RequestTrackerCommitter {
    /// Commit all successfully completed requests.
    ///
    /// If the client associated with this committer is dropped before this is called, all requests
    /// successfully completed by the client will be considered failed and will be made available
    /// for retry by other clients.
    pub fn commit(self) {
        self.command_tx.send(Command::Commit {
            client_id: self.client_id,
        });
    }
}

/// Request that we want to send to the peer.
#[derive(Clone, Debug)]
pub(super) struct CandidateRequest {
    pub payload: Request,
    pub variant: RequestVariant,
    pub state: InitialRequestState,
}

impl CandidateRequest {
    /// Create new candiate for the given request.
    pub fn new(payload: Request) -> Self {
        Self {
            payload,
            variant: RequestVariant::default(),
            state: InitialRequestState::InFlight,
        }
    }

    pub fn variant(self, variant: RequestVariant) -> Self {
        Self { variant, ..self }
    }

    /// Start this request in suspended state.
    pub fn suspended(self) -> Self {
        Self {
            state: InitialRequestState::Suspended,
            ..self
        }
    }
}

#[derive(Default, Clone, Copy, Eq, PartialEq, Hash)]
pub(super) struct RequestVariant(u128);

impl RequestVariant {
    pub fn new(
        local_block_presence: MultiBlockPresence,
        remote_block_presence: MultiBlockPresence,
    ) -> Self {
        let mut hasher = Xxh3Default::default();

        hasher.update(local_block_presence.checksum());
        hasher.update(remote_block_presence.checksum());

        Self(hasher.digest128())
    }
}

impl fmt::Debug for RequestVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<8x}", hex_fmt::HexFmt(&self.0.to_le_bytes()))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum InitialRequestState {
    InFlight,
    Suspended,
}

/// Request that is ready to be sent to the peer.
///
/// It also contains the block presence from the response that triggered this request. This is
/// mostly useful for diagnostics and testing.
#[derive(Clone, Eq, PartialEq, Debug)]
pub(super) struct PendingRequest {
    pub payload: Request,
    pub variant: RequestVariant,
}

impl PendingRequest {
    pub fn new(payload: Request) -> Self {
        Self {
            payload,
            variant: RequestVariant::default(),
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn variant(self, variant: RequestVariant) -> Self {
        Self { variant, ..self }
    }
}

/// Key identifying a request and its corresponding response.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Debug)]
pub(super) enum MessageKey {
    RootNode(PublicKey, u64),
    ChildNodes(Hash),
    Block(BlockId),
    Idle,
}

impl MessageKey {
    pub fn kind(&self) -> RequestKind {
        match self {
            Self::RootNode(..) | Self::ChildNodes(..) => RequestKind::Index,
            Self::Block(..) => RequestKind::Block,
            Self::Idle => RequestKind::Other,
        }
    }
}

impl<'a> From<&'a Request> for MessageKey {
    fn from(request: &'a Request) -> Self {
        match request {
            Request::RootNode {
                writer_id, cookie, ..
            } => MessageKey::RootNode(*writer_id, *cookie),
            Request::ChildNodes(hash, _) => MessageKey::ChildNodes(*hash),
            Request::Block(block_id, _) => MessageKey::Block(*block_id),
            Request::Idle => MessageKey::Idle,
        }
    }
}

fn build(monitor: TrafficMonitor) -> (RequestTracker, Worker) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let command_tx = command_tx.into();

    (
        RequestTracker { command_tx },
        Worker::new(command_rx, monitor),
    )
}

const BROKEN_INVARIANT: &str = "broken invariant";
const CLIENT_NOT_FOUND: &str = "client not found";

struct Worker {
    command_rx: mpsc::UnboundedReceiver<SpannedCommand>,
    clients: HashMap<ClientId, ClientState>,
    requests: Graph<RequestState>,
    flight: Flight,
}

impl Worker {
    fn new(command_rx: mpsc::UnboundedReceiver<SpannedCommand>, monitor: TrafficMonitor) -> Self {
        Self {
            command_rx,
            clients: HashMap::default(),
            requests: Graph::new(),
            flight: Flight::new(monitor),
        }
    }

    pub async fn run(mut self) {
        loop {
            select! {
                command = self.command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command);
                    } else {
                        break;
                    }
                }
                Some((client_id, request_key)) = self.flight.next_timeout() => {
                    self.handle_failure(client_id, request_key, FailureReason::Timeout);
                }
            }
        }
    }

    /// Process all currently queued commands.
    #[cfg(test)]
    pub fn step(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            self.handle_command(command);
        }
    }

    #[cfg(test)]
    pub fn requests(&self) -> impl ExactSizeIterator<Item = &Request> {
        self.requests.requests()
    }

    fn handle_command(&mut self, command: SpannedCommand) {
        let SpannedCommand { command, span } = command;
        let _enter = span.enter();

        match command {
            Command::InsertClient {
                client_id,
                request_tx,
            } => {
                self.insert_client(client_id, request_tx);
            }
            Command::RemoveClient { client_id } => {
                self.remove_client(client_id);
            }
            Command::SetTimeout { timeout } => {
                // Note: for simplicity, the new timeout is be applied to future requests only,
                // not the ones that've been already scheduled.
                self.flight.set_timeout(timeout);
            }
            Command::HandleInitial { client_id, request } => {
                self.handle_initial(client_id, request);
            }
            Command::HandleSuccess {
                client_id,
                request_key,
                requests,
            } => {
                self.handle_success(client_id, request_key, requests);
            }
            Command::HandleFailure {
                client_id,
                request_key,
            } => {
                self.handle_failure(client_id, request_key, FailureReason::Response);
            }
            Command::Resume {
                request_key,
                variant,
            } => self.resume(request_key, variant),
            Command::Commit { client_id } => {
                self.commit(client_id);
            }
            Command::Choke { client_id } => self.set_choked(client_id, true),
            Command::Unchoke { client_id } => self.set_choked(client_id, false),
        }
    }

    #[instrument(skip(self, request_tx))]
    fn insert_client(
        &mut self,
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<PendingRequest>,
    ) {
        self.clients.insert(client_id, ClientState::new(request_tx));
    }

    #[instrument(skip(self))]
    fn remove_client(&mut self, client_id: ClientId) {
        let Some(client_state) = self.clients.remove(&client_id) else {
            return;
        };

        for (_, node_key) in client_state.requests {
            self.cancel_request(client_id, node_key, None);
        }
    }

    #[instrument(skip(self))]
    fn handle_initial(&mut self, client_id: ClientId, request: CandidateRequest) {
        self.insert_request(client_id, request, None)
    }

    #[instrument(skip(self))]
    fn handle_success(
        &mut self,
        client_id: ClientId,
        request_key: MessageKey,
        requests: Vec<CandidateRequest>,
    ) {
        let node_key = self
            .clients
            .get(&client_id)
            .expect(CLIENT_NOT_FOUND)
            .requests
            .get(&request_key)
            .copied();

        let (mut client_ids, decrement_pending) = if let Some(node_key) = node_key {
            let node = self.requests.get_mut(node_key).expect(BROKEN_INVARIANT);

            let waiters = match node.value_mut() {
                RequestState::InFlight {
                    sender_client_id,
                    sender_timer_key,
                    sent_at,
                    waiters,
                } if *sender_client_id == client_id => {
                    self.flight
                        .complete(request_key, *sender_timer_key, *sent_at);
                    Some(mem::take(waiters))
                }
                RequestState::InFlight { .. }
                | RequestState::Complete { .. }
                | RequestState::Suspended { .. }
                | RequestState::Choked { .. } => None,
                RequestState::Cancelled | RequestState::Committed => unreachable!(),
            };

            let client_ids = iter::once(client_id)
                .chain(waiters.as_ref().into_iter().flatten().copied())
                .collect();
            let decrement_pending = waiters.is_some();

            // If the request was `InFlight` from this client, switch it to `Complete`. Otherwise
            // keep it as is.
            if let Some(waiters) = waiters {
                *node.value_mut() = RequestState::Complete {
                    sender_client_id: client_id,
                    waiters,
                };
            }

            (client_ids, decrement_pending)
        } else {
            (vec![client_id], false)
        };

        // Register the followup (child) requests with this client but also with all the clients
        // that were waiting for the original request.
        for child_request in requests {
            for (client_id, child_request) in
                client_ids.iter().copied().zip(iter::repeat(child_request))
            {
                self.insert_request(client_id, child_request, node_key);
            }

            // Round-robin the requests among the clients.
            client_ids.rotate_left(1);
        }

        if decrement_pending {
            if let Some(client_state) = self.clients.get_mut(&client_id) {
                client_state.decrement_pending();
            }
        }
    }

    #[instrument(skip(self))]
    fn handle_failure(
        &mut self,
        client_id: ClientId,
        request_key: MessageKey,
        reason: FailureReason,
    ) {
        let client_state = self.clients.get_mut(&client_id).expect(CLIENT_NOT_FOUND);

        let Some(node_key) = client_state.requests.remove(&request_key) else {
            return;
        };

        self.cancel_request(client_id, node_key, Some(reason));
    }

    #[instrument(skip(self))]
    fn resume(&mut self, request_key: MessageKey, variant: RequestVariant) {
        let Some(node) = self.requests.lookup_mut(request_key, variant) else {
            return;
        };

        match node.value_mut() {
            RequestState::Suspended { waiters } => {
                *node.value_mut() = promote_waiter(
                    mem::take(waiters),
                    node.request(),
                    &self.clients,
                    &mut self.flight,
                );
            }
            RequestState::InFlight { .. }
            | RequestState::Complete { .. }
            | RequestState::Choked { .. }
            | RequestState::Committed
            | RequestState::Cancelled => (),
        }
    }

    #[instrument(skip(self))]
    fn commit(&mut self, client_id: ClientId) {
        // Collect all requests completed by this client.
        //
        // Note: `commit` is invoked via `RequestTrackerCommitter` possibly from another
        // task/thread. This means it might be invoked out of order with the other commands and so
        // it's possible the client might have been already removed. We need to handle that case
        // gracefully.
        let requests: Vec<_> = self
            .clients
            .get_mut(&client_id)
            .into_iter()
            .flat_map(|client_state| client_state.requests.iter())
            .filter(|(_, node_key)| {
                match self
                    .requests
                    .get(**node_key)
                    .expect(BROKEN_INVARIANT)
                    .value()
                {
                    RequestState::Complete {
                        sender_client_id, ..
                    } if *sender_client_id == client_id => true,
                    RequestState::Complete { .. }
                    | RequestState::InFlight { .. }
                    | RequestState::Suspended { .. }
                    | RequestState::Choked { .. } => false,
                    RequestState::Committed | RequestState::Cancelled => unreachable!(),
                }
            })
            .map(|(request_key, node_key)| (*request_key, *node_key))
            .collect();

        for (request_key, node_key) in requests {
            // We need to handle the `None` case gracefully here because the request might have been
            // removed already if it was a child of a request removed in a previous iteration.
            let Some(node) = self.requests.get_mut(node_key) else {
                continue;
            };

            // The sender's pending count already got decremented when the request transitioned to
            // `Complete` so we only need to decrement the waiters here.
            match node.value() {
                RequestState::Complete { waiters, .. } => {
                    for client_id in waiters {
                        if let Some(client_state) = self.clients.get_mut(client_id) {
                            client_state.decrement_pending();
                        }
                    }
                }
                RequestState::InFlight { .. }
                | RequestState::Suspended { .. }
                | RequestState::Choked { .. }
                | RequestState::Committed
                | RequestState::Cancelled => unreachable!(),
            }

            // If the node has no children, remove it, otherwise mark is as committed.
            if node.children().len() == 0 {
                self.remove_request(node_key);
            } else {
                remove_request_from_clients(&mut self.clients, request_key, node_key, node.value());
                *node.value_mut() = RequestState::Committed;
            }
        }
    }

    #[instrument(skip(self))]
    fn set_choked(&mut self, client_id: ClientId, choked: bool) {
        let client_state = self.clients.get_mut(&client_id).expect(CLIENT_NOT_FOUND);

        if client_state.choked == choked {
            return;
        }

        client_state.choked = choked;

        // Reborrow immutably so we can access other entries.
        let client_state = self.clients.get(&client_id).unwrap();

        for (request_key, node_key) in &client_state.requests {
            let node = self.requests.get_mut(*node_key).expect(BROKEN_INVARIANT);

            match node.value_mut() {
                RequestState::InFlight {
                    sender_client_id,
                    sender_timer_key,
                    sent_at,
                    waiters,
                } => {
                    if !choked {
                        continue;
                    }

                    if *sender_client_id != client_id {
                        continue;
                    }

                    // The newly choked client has an `InFlight` request. If that request has at
                    // least one unchoked waiter, we promote that waiter to a sender.
                    // Otherwise we transition the request to `Choked`. In any case, we demote the
                    // current sender (who's just been choked) to a waiter.

                    let mut waiters = mem::take(waiters);

                    // FIXME: remove this assert (or change to `debug_assert`) when the invariant
                    // breaking bug is fixed.
                    assert!(!waiters.contains(&client_id));

                    waiters.push_back(client_id);

                    self.flight
                        .cancel(*request_key, *sender_timer_key, *sent_at, None);

                    *node.value_mut() =
                        promote_waiter(waiters, node.request(), &self.clients, &mut self.flight);
                }
                RequestState::Choked { waiters } => {
                    if choked {
                        continue;
                    }

                    remove_waiter(waiters, client_id);

                    *node.value_mut() = RequestState::InFlight {
                        sender_client_id: client_id,
                        sender_timer_key: self.flight.send(client_id, *request_key),
                        sent_at: Instant::now(),
                        waiters: mem::take(waiters),
                    };

                    client_state.send(node.request().clone());
                }
                RequestState::Complete { .. } | RequestState::Suspended { .. } => (),
                RequestState::Committed | RequestState::Cancelled => unreachable!(),
            }
        }
    }

    #[instrument(skip(self))]
    fn insert_request(
        &mut self,
        client_id: ClientId,
        request: CandidateRequest,
        parent_key: Option<GraphKey>,
    ) {
        let node_key = self.requests.get_or_insert(
            PendingRequest {
                payload: request.payload,
                variant: request.variant,
            },
            parent_key,
            RequestState::Cancelled,
        );

        self.update_request(client_id, node_key, request.state);
    }

    #[instrument(skip(self))]
    fn update_request(
        &mut self,
        client_id: ClientId,
        node_key: GraphKey,
        initial_state: InitialRequestState,
    ) {
        //
        // # Transitions
        //
        // Old request state | Client choked | initial_state | Action
        // ------------------+---------------+---------------+-----------------------
        // InFlight          | -             | -             | add client to waiters
        // Complete          | -             | -             | add client to waiters
        // Suspended         | -             | -             | add client to waiters
        // Choked            | false         | -             | insert InFlight
        // Choked            | true          | -             | add client to waiters
        // Cancelled         | false         | InFlight      | insert InFlight
        // Cancelled         | true          | InFlight      | insert Choked
        // Cancelled         | -             | Suspended     | insert Suspended
        // Committed         | -             | -             | nothing
        //
        // # Existing request
        //
        // If the client is already tracking a request with the same message key:
        //
        // - If the new request has the same variant as the old one, do nothing
        // - If the new request has different variant than the old one, cancel the old request and
        //   replace it with the new one.
        //

        let client_state = self.clients.get_mut(&client_id).expect(CLIENT_NOT_FOUND);
        let node = self.requests.get_mut(node_key).expect(BROKEN_INVARIANT);
        let request_key = MessageKey::from(&node.request().payload);
        let add = match node.value() {
            RequestState::InFlight { .. }
            | RequestState::Complete { .. }
            | RequestState::Suspended { .. }
            | RequestState::Choked { .. }
            | RequestState::Cancelled => true,
            RequestState::Committed => false,
        };

        let (old_key, send) = match client_state.requests.entry(request_key) {
            // The request is not yet tracked with this client: start tracking it.
            Entry::Vacant(entry) => {
                if add {
                    entry.insert(node_key);
                }

                (None, true)
            }
            // The request with the same variant is already tracked with this client: do nothing.
            Entry::Occupied(entry) if *entry.get() == node_key => (None, false),
            // The request with a different variant is already tracked with this client: cancel the
            // existing request and start trackig the new one.
            Entry::Occupied(mut entry) => {
                let old_key = *entry.get();

                if add {
                    entry.insert(node_key);
                } else {
                    entry.remove();
                }

                (Some(old_key), true)
            }
        };

        let children: Vec<_> = node.children().collect();

        if send {
            let in_flight_waiters = match node.value_mut() {
                RequestState::Suspended { waiters } | RequestState::InFlight { waiters, .. } => {
                    // FIXME: remove this assert (or change to `debug_assert`) when the invariant
                    // breaking bug is fixed.
                    assert!(!waiters.contains(&client_id));

                    client_state.increment_pending();
                    waiters.push_back(client_id);
                    None
                }
                RequestState::Complete { waiters, .. } => {
                    // FIXME: remove this assert (or change to `debug_assert`) when the invariant
                    // breaking bug is fixed.
                    assert!(!waiters.contains(&client_id));

                    client_state.increment_pending();
                    waiters.push_back(client_id);
                    None
                }
                RequestState::Committed => None,
                RequestState::Cancelled => {
                    client_state.increment_pending();

                    match (initial_state, client_state.choked) {
                        (InitialRequestState::InFlight, true) => {
                            *node.value_mut() = RequestState::Choked {
                                waiters: [client_id].into(),
                            };

                            None
                        }
                        (InitialRequestState::InFlight, false) => Some(VecDeque::new()),
                        (InitialRequestState::Suspended, _) => {
                            *node.value_mut() = RequestState::Suspended {
                                waiters: [client_id].into(),
                            };
                            None
                        }
                    }
                }
                RequestState::Choked { waiters } => {
                    client_state.increment_pending();

                    if client_state.choked {
                        // FIXME: remove this assert (or change to `debug_assert`) when the
                        // invariant breaking bug is fixed.
                        assert!(!waiters.contains(&client_id));

                        waiters.push_back(client_id);
                        None
                    } else {
                        Some(mem::take(waiters))
                    }
                }
            };

            if let Some(waiters) = in_flight_waiters {
                // FIXME: remove this assert (or change to `debug_assert`) when the invariant
                // breaking bug is fixed.
                assert!(!waiters.contains(&client_id));

                *node.value_mut() = RequestState::InFlight {
                    sender_client_id: client_id,
                    sender_timer_key: self.flight.send(client_id, request_key),
                    sent_at: Instant::now(),
                    waiters,
                };

                client_state.send(node.request().clone());
            }
        }

        if let Some(old_key) = old_key {
            self.cancel_request(client_id, old_key, None);
        }

        // Note: we are using recursion, but the graph is only a few layers deep (currently 5) so
        // there is no danger of stack overflow.
        for child_key in children {
            self.update_request(client_id, child_key, initial_state);
        }
    }

    // Cancels a tracked request. This is a helper function that gets called after the request's
    // been already removed from the client.
    #[instrument(skip(self))]
    fn cancel_request(
        &mut self,
        client_id: ClientId,
        node_key: GraphKey,
        reason: Option<FailureReason>,
    ) {
        let Some(node) = self.requests.get_mut(node_key) else {
            // If this request is being cancelled because its client is being removed, it's possible
            // that it's already been removed due to being ancestor of another cancelled request of
            // the same client.
            return;
        };

        let request_key = MessageKey::from(&node.request().payload);

        // The client state might've been already removed at this point.
        let client_state = self.clients.get_mut(&client_id);
        let try_decrement_pending = || {
            if let Some(client_state) = client_state {
                client_state.decrement_pending();
            }
        };

        let waiters = match node.value_mut() {
            RequestState::Suspended { waiters } | RequestState::Choked { waiters } => {
                try_decrement_pending();
                remove_waiter(waiters, client_id);

                if !waiters.is_empty() {
                    return;
                }

                None
            }
            RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                sent_at,
                waiters,
            } => {
                try_decrement_pending();

                if *sender_client_id == client_id {
                    self.flight
                        .cancel(request_key, *sender_timer_key, *sent_at, reason);

                    Some(waiters)
                } else {
                    remove_waiter(waiters, client_id);
                    return;
                }
            }
            RequestState::Complete {
                sender_client_id,
                waiters,
            } => {
                if *sender_client_id == client_id {
                    Some(waiters)
                } else {
                    try_decrement_pending();
                    remove_waiter(waiters, client_id);
                    return;
                }
            }
            RequestState::Committed | RequestState::Cancelled => unreachable!(),
        };

        // Find next waiting client:
        //
        // - if non-choked waiter exists, promote them to sender and transition to `InFlight`
        // - if only choked waiters exist, transition to `Choked`
        // - if no waiters exist, transition to `Cancelled`
        if let Some(waiters) = waiters.filter(|waiters| !waiters.is_empty()) {
            *node.value_mut() = promote_waiter(
                mem::take(waiters),
                node.request(),
                &self.clients,
                &mut self.flight,
            );
            return;
        }

        *node.value_mut() = RequestState::Cancelled;

        // No waiter found. If this request has no children, we can remove it, otherwise we mark it
        // as cancelled.
        if node.children().len() > 0 {
            remove_request_from_clients(&mut self.clients, request_key, node_key, node.value());
        } else {
            self.remove_request(node_key);
        }
    }

    fn remove_request(&mut self, node_key: GraphKey) {
        let Some(node) = self.requests.remove(node_key) else {
            return;
        };

        // FIXME: remove this assert (or change to `debug_assert`) when the invariant
        // breaking bug is fixed.
        assert!(!matches!(node.value(), RequestState::InFlight { .. }));

        let request_key = MessageKey::from(&node.request().payload);
        remove_request_from_clients(&mut self.clients, request_key, node_key, node.value());

        for parent_key in node.parents() {
            let parent_node = self.requests.get(parent_key).expect(BROKEN_INVARIANT);

            if parent_node.children().len() > 0 {
                continue;
            }

            self.remove_request(parent_key);
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct ClientId(usize);

impl ClientId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
struct CommandSender(mpsc::UnboundedSender<SpannedCommand>);

impl CommandSender {
    fn send(&self, command: Command) {
        self.0
            .send(SpannedCommand {
                command,
                span: Span::current(),
            })
            .ok();
    }
}

impl From<mpsc::UnboundedSender<SpannedCommand>> for CommandSender {
    fn from(tx: mpsc::UnboundedSender<SpannedCommand>) -> Self {
        Self(tx)
    }
}

struct SpannedCommand {
    command: Command,
    span: Span,
}

enum Command {
    InsertClient {
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<PendingRequest>,
    },
    RemoveClient {
        client_id: ClientId,
    },
    SetTimeout {
        timeout: Duration,
    },
    HandleInitial {
        client_id: ClientId,
        request: CandidateRequest,
    },
    HandleSuccess {
        client_id: ClientId,
        request_key: MessageKey,
        requests: Vec<CandidateRequest>,
    },
    HandleFailure {
        client_id: ClientId,
        request_key: MessageKey,
    },
    Resume {
        request_key: MessageKey,
        variant: RequestVariant,
    },
    Commit {
        client_id: ClientId,
    },
    Choke {
        client_id: ClientId,
    },
    Unchoke {
        client_id: ClientId,
    },
}

struct ClientState {
    request_tx: mpsc::UnboundedSender<PendingRequest>,
    requests: HashMap<MessageKey, GraphKey>,
    // Number of pending requests tracked for this client. A request is considered "pending" if it
    // is:
    //
    // - `InFlight`, `Choked` or `Suspended`
    // - `Complete` and the client is a waiter, not sender.
    //
    // When this number drops to zero we send a `Request::Idle` message to notify the that we've
    // processed all their responses and have no more requests to send. This is used as a hint for
    // the choker.
    pending: usize,
    choked: bool,
}

impl ClientState {
    fn new(request_tx: mpsc::UnboundedSender<PendingRequest>) -> Self {
        Self {
            request_tx,
            requests: HashMap::default(),
            pending: 0,
            choked: false,
        }
    }

    fn send(&self, request: PendingRequest) {
        self.request_tx.send(request).ok();
    }

    fn increment_pending(&mut self) {
        self.pending = self
            .pending
            .checked_add(1)
            .expect("pending requests counter overflow");
    }

    fn decrement_pending(&mut self) {
        self.pending = self
            .pending
            .checked_sub(1)
            .expect("pending requests counter underflow");

        if self.pending == 0 {
            self.send(PendingRequest::new(Request::Idle));
        }
    }
}

#[derive(Debug)]
enum RequestState {
    /// This request is currently in flight
    InFlight {
        /// Client who's sending this request
        sender_client_id: ClientId,
        /// Timeout key for the request
        sender_timer_key: delay_queue::Key,
        /// When was the request sent.
        sent_at: Instant,
        /// Other clients interested in sending this request. If the current client fails or
        /// timeouts, a new one will be picked from this list.
        waiters: VecDeque<ClientId>,
    },
    /// The response to this request has already been received but the request hasn't been committed
    /// because the response hasn't been fully processed yet. If the sender client is dropped
    /// before this request gets committed, a new sender is picked and the request is switched back
    /// to `InFlight`.
    Complete {
        sender_client_id: ClientId,
        waiters: VecDeque<ClientId>,
    },
    /// This request is suspended. It will be transitioned to `InFlight` when resumed.
    Suspended { waiters: VecDeque<ClientId> },
    /// This request is choked. It (together with all the other choked requests of the same client)
    /// will be transitioned to `InFlight` when at least one of the waiting clients is unchoked.
    Choked { waiters: VecDeque<ClientId> },
    /// The response to this request has been received and fully processed. This request won't be
    /// retried even when the sender client gets dropped.
    Committed,
    /// The response for the current client failed and there are no more clients waiting.
    Cancelled,
}

impl RequestState {
    fn clients(&self) -> impl Iterator<Item = &ClientId> {
        match self {
            Self::InFlight {
                sender_client_id,
                waiters,
                ..
            }
            | Self::Complete {
                sender_client_id,
                waiters,
            } => Some((Some(sender_client_id), waiters)),
            Self::Suspended { waiters } | Self::Choked { waiters } => Some((None, waiters)),
            Self::Committed | Self::Cancelled => None,
        }
        .into_iter()
        .flat_map(|(sender_client_id, waiters)| sender_client_id.into_iter().chain(waiters))
    }
}

#[derive(Debug)]
enum FailureReason {
    Response,
    Timeout,
}

// Helper for tracking in-flight requests.
struct Flight {
    timer: DelayQueue<(ClientId, MessageKey)>,
    timeout: Duration,
    monitor: TrafficMonitor,
}

impl Flight {
    fn new(monitor: TrafficMonitor) -> Self {
        Self {
            timer: DelayQueue::new(),
            timeout: DEFAULT_TIMEOUT,
            monitor,
        }
    }

    fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn send(&mut self, client_id: ClientId, request_key: MessageKey) -> delay_queue::Key {
        let timer_key = self.timer.insert((client_id, request_key), self.timeout);
        self.monitor.record(RequestEvent::Send, request_key.kind());
        timer_key
    }

    fn complete(&mut self, request_key: MessageKey, timer_key: delay_queue::Key, sent_at: Instant) {
        self.timer.try_remove(&timer_key);
        self.monitor.record(
            RequestEvent::Success {
                rtt: sent_at.elapsed(),
            },
            request_key.kind(),
        );
    }

    fn cancel(
        &mut self,
        request_key: MessageKey,
        timer_key: delay_queue::Key,
        sent_at: Instant,
        reason: Option<FailureReason>,
    ) {
        self.timer.try_remove(&timer_key);
        self.monitor.record(
            match reason {
                Some(FailureReason::Response) => RequestEvent::Failure {
                    rtt: sent_at.elapsed(),
                },
                Some(FailureReason::Timeout) => RequestEvent::Timeout,
                None => RequestEvent::Cancel,
            },
            request_key.kind(),
        );
    }

    async fn next_timeout(&mut self) -> Option<(ClientId, MessageKey)> {
        Some(self.timer.next().await?.into_inner())
    }
}

/// Panics if `client_id` is not in `waiters`.
fn remove_waiter(waiters: &mut VecDeque<ClientId>, client_id: ClientId) {
    let index = waiters
        .iter()
        .position(|waiter_id| *waiter_id == client_id)
        .unwrap();
    waiters.remove(index).unwrap();

    // FIXME: remove this assert (or change to `debug_assert`) when the invariant breaking bug
    // is fixed.
    assert!(!waiters.contains(&client_id));
}

fn remove_request_from_clients(
    clients: &mut HashMap<ClientId, ClientState>,
    request_key: MessageKey,
    node_key: GraphKey,
    state: &RequestState,
) {
    for client_id in state.clients() {
        if let Some(client_state) = clients.get_mut(client_id) {
            match client_state.requests.entry(request_key) {
                Entry::Occupied(entry) => {
                    if *entry.get() == node_key {
                        entry.remove();
                    }
                }
                Entry::Vacant(_) => (),
            }
        }
    }
}

// If there is at least one un-choked waiter, promote it to sender and return the corresponding
// `InFlight` state. Otherwise return `Choked` state.
fn promote_waiter(
    mut waiters: VecDeque<ClientId>,
    request: &PendingRequest,
    clients: &HashMap<ClientId, ClientState>,
    flight: &mut Flight,
) -> RequestState {
    // Find the first unchoked waiter, if any.
    let unchoked_client = waiters
        .iter()
        .enumerate()
        .map(|(index, id)| (index, *id, clients.get(id).expect(BROKEN_INVARIANT)))
        .find(|(_, _, state)| !state.choked);

    if let Some((waiter_index, client_id, client_state)) = unchoked_client {
        waiters.remove(waiter_index).unwrap();

        client_state.send(request.clone());

        // FIXME: remove this assert (or change to `debug_assert`) when the invariant breaking bug
        // is fixed.
        assert!(!waiters.contains(&client_id));

        RequestState::InFlight {
            sender_client_id: client_id,
            sender_timer_key: flight.send(client_id, MessageKey::from(&request.payload)),
            sent_at: Instant::now(),
            waiters,
        }
    } else {
        RequestState::Choked { waiters }
    }
}
