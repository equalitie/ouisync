mod graph;

#[cfg(test)]
mod simulation;
#[cfg(test)]
mod tests;

use self::graph::{Graph, Key as GraphKey};
use super::message::Request;
use crate::{
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    protocol::{BlockId, MultiBlockPresence},
    repository::monitor::TrafficMonitor,
};
use std::{
    collections::hash_map::Entry,
    fmt, iter, mem,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tokio::{
    select,
    sync::{mpsc, oneshot},
    task,
};
use tokio_stream::StreamExt;
use tokio_util::time::{delay_queue, DelayQueue};
use tracing::{instrument, Instrument, Span};
use xxhash_rust::xxh3::Xxh3Default;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

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

    pub fn new_client(&self) -> (RequestTrackerClient, RequestTrackerReceiver) {
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
            RequestTrackerReceiver {
                client_id,
                command_tx: self.command_tx.clone(),
                request_rx,

                #[cfg(test)]
                reply_rx: None,
            },
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

    /// Handle receiving response to a previously sent request.
    pub fn receive(&self, request_key: MessageKey) {
        self.command_tx.send(Command::HandleReceive {
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

/// Receives requests to be sent to peers.
pub(crate) struct RequestTrackerReceiver {
    client_id: ClientId,
    command_tx: CommandSender,
    request_rx: mpsc::UnboundedReceiver<MessageKey>,

    #[cfg(test)]
    reply_rx: Option<oneshot::Receiver<PendingRequest>>,
}

impl RequestTrackerReceiver {
    pub async fn recv(&mut self) -> Option<PendingRequest> {
        loop {
            let request_key = self.request_rx.recv().await?;
            let reply_rx = self.send(request_key);

            if let Ok(request) = reply_rx.await {
                return Some(request);
            }
        }
    }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<PendingRequest, TryRecvError> {
        loop {
            if let Some(rx) = &mut self.reply_rx {
                match rx.try_recv() {
                    Ok(request) => {
                        self.reply_rx = None;
                        return Ok(request);
                    }
                    Err(oneshot::error::TryRecvError::Empty) => {
                        return Err(TryRecvError::InProgress)
                    }
                    Err(oneshot::error::TryRecvError::Closed) => {
                        self.reply_rx = None;
                    }
                }
            }

            let request_key = self.request_rx.try_recv()?;
            self.reply_rx = Some(self.send(request_key));
        }
    }

    fn send(&self, request_key: MessageKey) -> oneshot::Receiver<PendingRequest> {
        let (reply_tx, reply_rx) = oneshot::channel();

        match request_key {
            MessageKey::Idle => {
                // `Request::Idle` is not tracked - send it right away.
                reply_tx.send(PendingRequest::new(Request::Idle)).unwrap();
            }
            MessageKey::Block(_)
            | MessageKey::RootNode(_, _)
            | MessageKey::ChildNodes(_)
            | MessageKey::Other => {
                self.command_tx.send(Command::HandleSend {
                    client_id: self.client_id,
                    request_key,
                    reply_tx,
                });
            }
        }

        reply_rx
    }
}

#[cfg(test)]
#[derive(Eq, PartialEq, Debug)]
pub(crate) enum TryRecvError {
    // The channel is currently empty but request might yet become available
    Empty,
    // The channel has been closed, not request will be received.
    Closed,
    // The request receiving is in progress. Call [Worker::step()] and try again.
    InProgress,
}

#[cfg(test)]
impl From<mpsc::error::TryRecvError> for TryRecvError {
    fn from(error: mpsc::error::TryRecvError) -> Self {
        match error {
            mpsc::error::TryRecvError::Empty => Self::Empty,
            mpsc::error::TryRecvError::Disconnected => Self::Closed,
        }
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
            state: InitialRequestState::Ready,
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
    Ready,
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
    Other,
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

    /// Process all currently queued commands and expired timeouts.
    #[cfg(test)]
    pub fn step(&mut self) {
        use futures_util::FutureExt;

        while let Ok(command) = self.command_rx.try_recv() {
            self.handle_command(command);
        }

        while let Some(Some((client_id, request_key))) = self.flight.next_timeout().now_or_never() {
            self.handle_failure(client_id, request_key, FailureReason::Timeout);
        }
    }

    #[cfg(test)]
    pub fn requests(&self) -> impl ExactSizeIterator<Item = &Request> {
        self.requests.requests()
    }

    #[allow(unused)]
    #[cfg(test)]
    pub fn dump(&self) {
        for (client_id, client_state) in &self.clients {
            println!("{client_id:?}: {:?}", client_state.requests);
        }

        println!();
        println!("{:?}", self.requests);
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
            Command::HandleSend {
                client_id,
                request_key,
                reply_tx,
            } => {
                if let Some(request) = self.handle_send(client_id, request_key) {
                    reply_tx.send(request).ok();
                }
            }
            Command::HandleReceive {
                client_id,
                request_key,
            } => self.handle_receive(client_id, request_key),
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
        request_tx: mpsc::UnboundedSender<MessageKey>,
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
    fn handle_send(
        &mut self,
        client_id: ClientId,
        request_key: MessageKey,
    ) -> Option<PendingRequest> {
        let node_key = self
            .clients
            .get(&client_id)?
            .requests
            .get(&request_key)
            .copied()?;
        let node = self.requests.get_mut(node_key).expect(BROKEN_INVARIANT);

        match node.value_mut() {
            RequestState::Ready { waiters } => {
                assert!(waiters.remove(&client_id), "{BROKEN_INVARIANT}");

                *node.value_mut() = RequestState::InFlight {
                    sender_client_id: client_id,
                    sender_timer_key: self.flight.send(client_id, request_key),
                    sent_at: Instant::now(),
                    waiters: mem::take(waiters),
                };

                Some(node.request().clone())
            }
            RequestState::InFlight { .. }
            | RequestState::Received { .. }
            | RequestState::Complete { .. }
            | RequestState::Suspended { .. }
            | RequestState::Committed
            | RequestState::Cancelled => None,
        }
    }

    #[instrument(skip(self))]
    fn handle_receive(&mut self, client_id: ClientId, request_key: MessageKey) {
        let Some(node_key) = self
            .clients
            .get(&client_id)
            .expect(CLIENT_NOT_FOUND)
            .requests
            .get(&request_key)
            .copied()
        else {
            return;
        };

        let node = self.requests.get_mut(node_key).expect(BROKEN_INVARIANT);

        let waiters = match node.value_mut() {
            RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                sent_at,
                waiters,
            } if *sender_client_id == client_id => {
                self.flight
                    .receive(request_key, *sender_timer_key, *sent_at);
                mem::take(waiters)
            }
            RequestState::Ready { .. }
            | RequestState::InFlight { .. }
            | RequestState::Received { .. }
            | RequestState::Complete { .. }
            | RequestState::Suspended { .. } => return,

            RequestState::Cancelled | RequestState::Committed => unreachable!(),
        };

        *node.value_mut() = RequestState::Received {
            sender_client_id: client_id,
            waiters,
        };
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

        let (client_ids, decrement_pending) = if let Some(node_key) = node_key {
            let node = self.requests.get_mut(node_key).expect(BROKEN_INVARIANT);

            // If the request was `Received` from this client, switch it to `Complete`. Otherwise
            // keep it as is.
            let waiters = match node.value_mut() {
                RequestState::Received {
                    sender_client_id,
                    waiters,
                } if *sender_client_id == client_id => Some(mem::take(waiters)),
                RequestState::Received { .. }
                | RequestState::Ready { .. }
                | RequestState::InFlight { .. }
                | RequestState::Complete { .. }
                | RequestState::Suspended { .. } => None,
                RequestState::Cancelled | RequestState::Committed => unreachable!(),
            };

            let client_ids = iter::once(client_id)
                .chain(waiters.as_ref().into_iter().flatten().copied())
                .collect();
            let decrement_pending = waiters.is_some();

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
            for client_id in &client_ids {
                self.insert_request(*client_id, child_request.clone(), node_key);
            }
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
                send(&self.clients, &*waiters, request_key);
                *node.value_mut() = RequestState::Ready {
                    waiters: mem::take(waiters),
                };
            }
            RequestState::Ready { .. }
            | RequestState::InFlight { .. }
            | RequestState::Received { .. }
            | RequestState::Complete { .. }
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
                    | RequestState::Ready { .. }
                    | RequestState::InFlight { .. }
                    | RequestState::Received { .. }
                    | RequestState::Suspended { .. } => false,
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
                RequestState::Ready { .. }
                | RequestState::InFlight { .. }
                | RequestState::Received { .. }
                | RequestState::Suspended { .. }
                | RequestState::Committed
                | RequestState::Cancelled => unreachable!(),
            }

            remove_request_from_clients(&mut self.clients, request_key, node_key, node.value());
            *node.value_mut() = RequestState::Committed;
            self.remove_request(node_key);
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
                RequestState::Ready { .. } => {
                    if choked {
                        continue;
                    }

                    client_state.send(*request_key);
                }
                RequestState::InFlight {
                    sender_client_id,
                    sender_timer_key,
                    waiters,
                    ..
                } => {
                    if !choked {
                        continue;
                    }

                    if *sender_client_id != client_id {
                        continue;
                    }

                    self.flight.cancel(*request_key, *sender_timer_key, None);

                    let mut waiters = mem::take(waiters);
                    waiters.insert(client_id);

                    send(&self.clients, &waiters, *request_key);

                    *node.value_mut() = RequestState::Ready { waiters };
                }
                RequestState::Received { .. }
                | RequestState::Complete { .. }
                | RequestState::Suspended { .. } => (),
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
        // Ready             | -             | -             | add client to waiters
        // InFlight          | -             | -             | add client to waiters
        // Received          | -             | -             | add client to waiters
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

        let add_to_client = match node.value() {
            RequestState::Ready { .. }
            | RequestState::InFlight { .. }
            | RequestState::Received { .. }
            | RequestState::Complete { .. }
            | RequestState::Suspended { .. }
            | RequestState::Cancelled => true,
            RequestState::Committed => false,
        };

        let (old_key, update) = match client_state.requests.entry(request_key) {
            // The request is not yet tracked with this client: start tracking it.
            Entry::Vacant(entry) => {
                if add_to_client {
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

                if add_to_client {
                    entry.insert(node_key);
                } else {
                    entry.remove();
                }

                (Some(old_key), true)
            }
        };

        let children: Vec<_> = node.children().collect();

        if update {
            match node.value_mut() {
                RequestState::Ready { waiters } => {
                    client_state.increment_pending();
                    send(&self.clients, iter::once(&client_id), request_key);
                    waiters.insert(client_id);
                }
                RequestState::Suspended { waiters }
                | RequestState::InFlight { waiters, .. }
                | RequestState::Received { waiters, .. }
                | RequestState::Complete { waiters, .. } => {
                    client_state.increment_pending();
                    waiters.insert(client_id);
                }
                RequestState::Committed => (),
                RequestState::Cancelled => {
                    client_state.increment_pending();

                    match initial_state {
                        InitialRequestState::Ready => {
                            send(&self.clients, iter::once(&client_id), request_key);

                            *node.value_mut() = RequestState::Ready {
                                waiters: [client_id].into(),
                            };
                        }
                        InitialRequestState::Suspended => {
                            *node.value_mut() = RequestState::Suspended {
                                waiters: [client_id].into(),
                            };
                        }
                    }
                }
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
            RequestState::Suspended { waiters } | RequestState::Ready { waiters } => {
                try_decrement_pending();
                waiters.remove(&client_id);

                if !waiters.is_empty() {
                    return;
                }

                None
            }
            RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                waiters,
                ..
            } => {
                try_decrement_pending();

                if *sender_client_id == client_id {
                    self.flight.cancel(request_key, *sender_timer_key, reason);

                    Some(waiters)
                } else {
                    waiters.remove(&client_id);
                    return;
                }
            }
            RequestState::Received {
                sender_client_id,
                waiters,
            } => {
                try_decrement_pending();

                if *sender_client_id == client_id {
                    Some(waiters)
                } else {
                    waiters.remove(&client_id);
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
                    waiters.remove(&client_id);
                    return;
                }
            }
            RequestState::Committed | RequestState::Cancelled => unreachable!(),
        };

        if let Some(waiters) = waiters.filter(|waiters| !waiters.is_empty()) {
            send(&self.clients, &*waiters, request_key);
            *node.value_mut() = RequestState::Ready {
                waiters: mem::take(waiters),
            };
            return;
        }

        remove_request_from_clients(&mut self.clients, request_key, node_key, node.value());
        *node.value_mut() = RequestState::Cancelled;
        self.remove_request(node_key);
    }

    fn remove_request(&mut self, node_key: GraphKey) {
        let Some(node) = self.requests.get_mut(node_key) else {
            return;
        };

        match node.value() {
            RequestState::Committed | RequestState::Cancelled => (),
            RequestState::Ready { .. }
            | RequestState::InFlight { .. }
            | RequestState::Received { .. }
            | RequestState::Complete { .. }
            | RequestState::Suspended { .. } => return,
        }

        if node.children().len() == 0 {
            let node = self.requests.remove(node_key).unwrap();
            for parent_key in node.parents() {
                self.remove_request(parent_key);
            }
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

#[derive(Debug)]
enum Command {
    InsertClient {
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<MessageKey>,
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
    HandleSend {
        client_id: ClientId,
        request_key: MessageKey,
        reply_tx: oneshot::Sender<PendingRequest>,
    },
    HandleReceive {
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
    request_tx: mpsc::UnboundedSender<MessageKey>,
    requests: HashMap<MessageKey, GraphKey>,
    // Number of pending requests tracked for this client. A request is considered "pending" if it
    // is:
    //
    // - `InFlight`, `Received`, `Choked` or `Suspended`
    // - `Complete` and the client is a waiter, not sender.
    //
    // When this number drops to zero we send a `Request::Idle` message to notify the that we've
    // processed all their responses and have no more requests to send. This is used as a hint for
    // the choker.
    pending: usize,
    choked: bool,
}

impl ClientState {
    fn new(request_tx: mpsc::UnboundedSender<MessageKey>) -> Self {
        Self {
            request_tx,
            requests: HashMap::default(),
            pending: 0,
            choked: false,
        }
    }

    fn send(&self, request_key: MessageKey) {
        self.request_tx.send(request_key).ok();
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
            self.send(MessageKey::Idle);
        }
    }
}

#[derive(Debug)]
enum RequestState {
    /// This request is ready to be sent, waiting for a client to pick it up.
    Ready { waiters: HashSet<ClientId> },
    /// This request is currently in flight
    InFlight {
        /// Client who's sending this request
        sender_client_id: ClientId,
        /// Timeout key for the request
        sender_timer_key: delay_queue::Key,
        /// When was the request sent.
        sent_at: Instant,
        /// Other clients interested in sending this request. If the sender client is dropped or the
        /// request timeouts, a new sender is picked from this list.
        waiters: HashSet<ClientId>,
    },
    /// The response to this request has been received but not yet processed.
    Received {
        /// Client who sent this request
        sender_client_id: ClientId,
        /// Other clients interested in sending this request. If the sender client is dropped, a new
        /// sender is picked from this list.
        waiters: HashSet<ClientId>,
    },
    /// The response to this request has been processed but not commited yet (responses are
    /// processed in batches and commited only at the end of each batch).
    Complete {
        /// Client who sent the request.
        sender_client_id: ClientId,
        /// Other clients interested in sending this request. If the sender client is dropped, a new
        /// sender is picked from this list.
        waiters: HashSet<ClientId>,
    },
    /// This request is suspended. It will be transitioned to `InFlight` when resumed.
    Suspended { waiters: HashSet<ClientId> },
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
            | Self::Received {
                sender_client_id,
                waiters,
                ..
            }
            | Self::Complete {
                sender_client_id,
                waiters,
            } => Some((Some(sender_client_id), waiters)),
            Self::Ready { waiters } | Self::Suspended { waiters } => Some((None, waiters)),
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

        match request_key {
            MessageKey::Block(_) => {
                self.monitor.block_requests_sent.increment(1);
                self.monitor.block_requests_inflight.increment(1);
            }
            MessageKey::RootNode(..) | MessageKey::ChildNodes(_) => {
                self.monitor.index_requests_sent.increment(1);
                self.monitor.index_requests_inflight.increment(1);
            }
            MessageKey::Idle | MessageKey::Other => (),
        }

        timer_key
    }

    fn receive(&mut self, request_key: MessageKey, timer_key: delay_queue::Key, sent_at: Instant) {
        self.timer.try_remove(&timer_key);

        match request_key {
            MessageKey::Block(_) => {
                self.monitor.block_requests_inflight.decrement(1);
            }
            MessageKey::RootNode(..) | MessageKey::ChildNodes(_) => {
                self.monitor.index_requests_inflight.decrement(1);
            }
            MessageKey::Idle | MessageKey::Other => (),
        }

        self.monitor.request_rtt.record(sent_at.elapsed());
    }

    fn cancel(
        &mut self,
        request_key: MessageKey,
        timer_key: delay_queue::Key,
        reason: Option<FailureReason>,
    ) {
        self.timer.try_remove(&timer_key);

        match request_key {
            MessageKey::Block(_) => {
                self.monitor.block_requests_inflight.decrement(1);
            }
            MessageKey::RootNode(..) | MessageKey::ChildNodes(_) => {
                self.monitor.index_requests_inflight.decrement(1);
            }
            MessageKey::Idle | MessageKey::Other => (),
        }

        match reason {
            Some(FailureReason::Timeout) => {
                self.monitor.request_timeouts.increment(1);
            }
            Some(FailureReason::Response) | None => (),
        }
    }

    async fn next_timeout(&mut self) -> Option<(ClientId, MessageKey)> {
        Some(self.timer.next().await?.into_inner())
    }
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

fn send<'a>(
    clients: &'_ HashMap<ClientId, ClientState>,
    recipients: impl IntoIterator<Item = &'a ClientId>,
    request_key: MessageKey,
) {
    for client_id in recipients {
        let client_state = clients.get(client_id).expect(BROKEN_INVARIANT);

        if client_state.choked {
            continue;
        }

        client_state.send(request_key);
    }
}
