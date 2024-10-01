mod graph;

#[cfg(test)]
mod simulation;
#[cfg(test)]
mod tests;

use self::graph::{Graph, Key as GraphKey};
use super::{constants::REQUEST_TIMEOUT, message::Request};
use crate::{
    collections::HashMap,
    crypto::{sign::PublicKey, Hash},
    protocol::{BlockId, MultiBlockPresence},
};
use std::{
    collections::VecDeque,
    iter, mem,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{select, sync::mpsc, task};
use tokio_stream::StreamExt;
use tokio_util::time::{delay_queue, DelayQueue};
use tracing::instrument;

/// Keeps track of in-flight requests. Falls back on another peer in case the request failed (due to
/// error response, timeout or disconnection). Evenly distributes the requests between the peers
/// and ensures every request is only sent to one peer at a time.
#[derive(Clone)]
pub(super) struct RequestTracker {
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTracker {
    pub fn new() -> Self {
        let (this, worker) = build();
        task::spawn(worker.run());
        this
    }

    pub fn new_client(
        &self,
    ) -> (
        RequestTrackerClient,
        mpsc::UnboundedReceiver<PendingRequest>,
    ) {
        let client_id = ClientId::next();
        let (request_tx, request_rx) = mpsc::unbounded_channel();

        self.command_tx
            .send(Command::InsertClient {
                client_id,
                request_tx,
            })
            .ok();

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
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTrackerClient {
    /// Handle sending a request that does not follow from any previously received response.
    pub fn initial(&self, request: Request) {
        self.command_tx
            .send(Command::HandleInitial {
                client_id: self.client_id,
                request,
            })
            .ok();
    }

    /// Handle sending requests that follow from a received success response.
    pub fn success(&self, request_key: MessageKey, requests: Vec<PendingRequest>) {
        self.command_tx
            .send(Command::HandleSuccess {
                client_id: self.client_id,
                request_key,
                requests,
            })
            .ok();
    }

    /// Handle failure response.
    pub fn failure(&self, request_key: MessageKey) {
        self.command_tx
            .send(Command::HandleFailure {
                client_id: self.client_id,
                request_key,
            })
            .ok();
    }

    /// Obtain a handle to commit all the requests that were successfully completed by this client.
    /// The handle can be sent to other tasks/threads before invoking the commit.
    pub fn new_committer(&self) -> RequestTrackerCommitter {
        RequestTrackerCommitter {
            client_id: self.client_id,
            command_tx: self.command_tx.clone(),
        }
    }
}

impl Drop for RequestTrackerClient {
    fn drop(&mut self) {
        self.command_tx
            .send(Command::RemoveClient {
                client_id: self.client_id,
            })
            .ok();
    }
}

pub(crate) struct RequestTrackerCommitter {
    client_id: ClientId,
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTrackerCommitter {
    /// Commit all successfully completed requests.
    ///
    /// If the client associated with this committer is dropped before this is called, all requests
    /// successfully completed by the client will be considered failed and will be made available
    /// for retry by other clients.
    pub fn commit(self) {
        self.command_tx
            .send(Command::Commit {
                client_id: self.client_id,
            })
            .ok();
    }
}

/// Request to be sent to the peer.
///
/// It also contains the block presence from the response that triggered this request. This is
/// mostly useful for diagnostics and testing.
#[derive(Clone, Debug)]
pub(super) struct PendingRequest {
    pub request: Request,
    pub block_presence: MultiBlockPresence,
}

/// Key identifying a request and its corresponding response.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Debug)]
pub(super) enum MessageKey {
    RootNode(PublicKey),
    ChildNodes(Hash),
    Block(BlockId),
}

impl<'a> From<&'a Request> for MessageKey {
    fn from(request: &'a Request) -> Self {
        match request {
            Request::RootNode(writer_id, _) => MessageKey::RootNode(*writer_id),
            Request::ChildNodes(hash, _, _) => MessageKey::ChildNodes(*hash),
            Request::Block(block_id, _) => MessageKey::Block(*block_id),
        }
    }
}

fn build() -> (RequestTracker, Worker) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    (RequestTracker { command_tx }, Worker::new(command_rx))
}

struct Worker {
    command_rx: mpsc::UnboundedReceiver<Command>,
    clients: HashMap<ClientId, ClientState>,
    requests: Graph<RequestState>,
    timer: DelayQueue<(ClientId, MessageKey)>,
}

impl Worker {
    fn new(command_rx: mpsc::UnboundedReceiver<Command>) -> Self {
        Self {
            command_rx,
            clients: HashMap::default(),
            requests: Graph::new(),
            timer: DelayQueue::new(),
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
                Some(expired) = self.timer.next() => {
                    let (client_id, request_key) = expired.into_inner();
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

    fn handle_command(&mut self, command: Command) {
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
            Command::Commit { client_id } => {
                self.commit(client_id);
            }
        }
    }

    #[instrument(skip(self, request_tx))]
    fn insert_client(
        &mut self,
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<PendingRequest>,
    ) {
        tracing::trace!("insert_client");

        self.clients.insert(client_id, ClientState::new(request_tx));
    }

    #[instrument(skip(self))]
    fn remove_client(&mut self, client_id: ClientId) {
        tracing::trace!("remove_client");

        let Some(client_state) = self.clients.remove(&client_id) else {
            return;
        };

        for (_, node_key) in client_state.requests {
            self.cancel_request(client_id, node_key);
        }
    }

    #[instrument(skip(self))]
    fn handle_initial(&mut self, client_id: ClientId, request: Request) {
        tracing::trace!("handle_initial");

        self.insert_request(
            client_id,
            PendingRequest {
                request,
                block_presence: MultiBlockPresence::None,
            },
            None,
        )
    }

    #[instrument(skip(self))]
    fn handle_success(
        &mut self,
        client_id: ClientId,
        request_key: MessageKey,
        requests: Vec<PendingRequest>,
    ) {
        tracing::trace!("handle_success");

        let node_key = self
            .clients
            .get(&client_id)
            .and_then(|state| state.requests.get(&request_key))
            .copied();

        let mut client_ids = if let Some(node_key) = node_key {
            let Some(node) = self.requests.get_mut(node_key) else {
                return;
            };

            let waiters = match node.value_mut() {
                RequestState::InFlight {
                    sender_client_id,
                    sender_timer_key,
                    waiters,
                } if *sender_client_id == client_id => {
                    self.timer.try_remove(sender_timer_key);
                    Some(mem::take(waiters))
                }
                RequestState::InFlight { .. }
                | RequestState::Complete { .. }
                | RequestState::Committed
                | RequestState::Cancelled => None,
            };

            let client_ids = if requests.is_empty() {
                Vec::new()
            } else {
                iter::once(client_id)
                    .chain(waiters.as_ref().into_iter().flatten().copied())
                    .collect()
            };

            // If the request was `InFlight` from this client, switch it to `Complete`. Otherwise
            // keep it as is.
            if let Some(waiters) = waiters {
                *node.value_mut() = RequestState::Complete {
                    sender_client_id: client_id,
                    waiters,
                };
            }

            client_ids
        } else {
            vec![client_id]
        };

        // Register the followup (child) requests with this client but also with all the clients
        // that were waiting for the original request.
        for child_request in requests {
            for (client_id, child_request) in
                // TODO: use `repeat_n` once it gets stabilized.
                client_ids.iter().copied().zip(iter::repeat(child_request))
            {
                self.insert_request(client_id, child_request, node_key);
            }

            // Round-robin the requests among the clients.
            client_ids.rotate_left(1);
        }
    }

    #[instrument(skip(self))]
    fn handle_failure(
        &mut self,
        client_id: ClientId,
        request_key: MessageKey,
        reason: FailureReason,
    ) {
        tracing::trace!("handle_failure");

        let Some(client_state) = self.clients.get_mut(&client_id) else {
            return;
        };

        let Some(node_key) = client_state.requests.remove(&request_key) else {
            return;
        };

        self.cancel_request(client_id, node_key);
    }

    #[instrument(skip(self))]
    fn commit(&mut self, client_id: ClientId) {
        tracing::trace!("commit");

        // Collect all requests completed by this client.
        let requests: Vec<_> = self
            .clients
            .get_mut(&client_id)
            .into_iter()
            .flat_map(|client_state| client_state.requests.iter())
            .filter(|(_, node_key)| {
                self.requests
                    .get(**node_key)
                    .map(|node| match node.value() {
                        RequestState::Complete {
                            sender_client_id, ..
                        } if *sender_client_id == client_id => true,
                        RequestState::Complete { .. }
                        | RequestState::InFlight { .. }
                        | RequestState::Committed
                        | RequestState::Cancelled => false,
                    })
                    .unwrap_or(false)
            })
            .map(|(request_key, node_key)| (*request_key, *node_key))
            .collect();

        for (request_key, node_key) in requests {
            let Some(node) = self.requests.get_mut(node_key) else {
                continue;
            };

            let waiters = match node.value_mut() {
                RequestState::Complete { waiters, .. } => mem::take(waiters),
                RequestState::InFlight { .. }
                | RequestState::Committed
                | RequestState::Cancelled => unreachable!(),
            };

            // Remove the requests from this client and all the waiters
            for client_id in iter::once(client_id).chain(waiters) {
                if let Some(client_state) = self.clients.get_mut(&client_id) {
                    client_state.requests.remove(&request_key);
                }
            }

            // If the node has no children, remove it, otherwise mark is as committed.
            if node.children().len() == 0 {
                self.remove_request(node_key);
            } else {
                *node.value_mut() = RequestState::Committed;
            }
        }
    }

    fn insert_request(
        &mut self,
        client_id: ClientId,
        request: PendingRequest,
        parent_key: Option<GraphKey>,
    ) {
        let node_key = self
            .requests
            .get_or_insert(request, parent_key, RequestState::Cancelled);

        self.update_request(client_id, node_key);
    }

    fn update_request(&mut self, client_id: ClientId, node_key: GraphKey) {
        let Some(node) = self.requests.get_mut(node_key) else {
            return;
        };

        let Some(client_state) = self.clients.get_mut(&client_id) else {
            return;
        };

        let request_key = MessageKey::from(&node.request().request);

        match node.value_mut() {
            RequestState::InFlight { waiters, .. } | RequestState::Complete { waiters, .. } => {
                waiters.push_back(client_id);
                client_state.requests.insert(request_key, node_key);
            }
            RequestState::Committed => (),
            RequestState::Cancelled => {
                let timer_key = self.timer.insert((client_id, request_key), REQUEST_TIMEOUT);

                *node.value_mut() = RequestState::InFlight {
                    sender_client_id: client_id,
                    sender_timer_key: timer_key,
                    waiters: VecDeque::new(),
                };

                client_state.requests.insert(request_key, node_key);
                client_state.request_tx.send(node.request().clone()).ok();
            }
        }

        // Note: we are using recursion, but the graph is only a few layers deep (currently 5) so
        // there is no danger of stack overflow.
        let children: Vec<_> = node.children().collect();

        for child_key in children {
            self.update_request(client_id, child_key);
        }
    }

    fn cancel_request(&mut self, client_id: ClientId, node_key: GraphKey) {
        let Some(node) = self.requests.get_mut(node_key) else {
            return;
        };

        let (request, state) = node.parts_mut();

        let (sender_client_id, sender_timer_key, waiters) = match state {
            RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                waiters,
            } => (sender_client_id, Some(sender_timer_key), waiters),
            RequestState::Complete {
                sender_client_id,
                waiters,
            } => (sender_client_id, None, waiters),
            RequestState::Committed | RequestState::Cancelled => return,
        };

        if *sender_client_id != client_id {
            remove_from_queue(waiters, &client_id);
            return;
        }

        if let Some(timer_key) = sender_timer_key {
            self.timer.try_remove(timer_key);
        }

        // Find next waiting client
        let next_client = iter::from_fn(|| waiters.pop_front())
            .find_map(|client_id| self.clients.get_key_value(&client_id));

        if let Some((&next_client_id, next_client_state)) = next_client {
            // Next waiting client found. Promote it to the sender.
            let sender_client_id = next_client_id;
            let sender_timer_key = self.timer.insert(
                (next_client_id, MessageKey::from(&request.request)),
                REQUEST_TIMEOUT,
            );
            let waiters = mem::take(waiters);

            *state = RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                waiters,
            };

            next_client_state.request_tx.send(request.clone()).ok();
        } else {
            // No waiting client found. If this request has no children, we can remove
            // it, otherwise we mark it as cancelled.
            if node.children().len() > 0 {
                *node.value_mut() = RequestState::Cancelled;
            } else {
                self.remove_request(node_key);
            }
        }
    }

    fn remove_request(&mut self, node_key: GraphKey) {
        let Some(node) = self.requests.remove(node_key) else {
            return;
        };

        for parent_key in node.parents() {
            let Some(parent_node) = self.requests.get(parent_key) else {
                continue;
            };

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

enum Command {
    InsertClient {
        client_id: ClientId,
        request_tx: mpsc::UnboundedSender<PendingRequest>,
    },
    RemoveClient {
        client_id: ClientId,
    },
    HandleInitial {
        client_id: ClientId,
        request: Request,
    },
    HandleSuccess {
        client_id: ClientId,
        request_key: MessageKey,
        requests: Vec<PendingRequest>,
    },
    HandleFailure {
        client_id: ClientId,
        request_key: MessageKey,
    },
    Commit {
        client_id: ClientId,
    },
}

struct ClientState {
    request_tx: mpsc::UnboundedSender<PendingRequest>,
    requests: HashMap<MessageKey, GraphKey>,
}

impl ClientState {
    fn new(request_tx: mpsc::UnboundedSender<PendingRequest>) -> Self {
        Self {
            request_tx,
            requests: HashMap::default(),
        }
    }
}

enum RequestState {
    /// This request is currently in flight
    InFlight {
        /// Client who's sending this request
        sender_client_id: ClientId,
        /// Timeout key for the request
        sender_timer_key: delay_queue::Key,
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
    /// The response to this request has been received and fully processed. This request won't be
    /// retried even when the sender client gets dropped.
    Committed,
    /// The response for the current client failed and there are no more clients waiting.
    Cancelled,
}

#[derive(Debug)]
enum FailureReason {
    Response,
    Timeout,
}

fn remove_from_queue<T: Eq>(queue: &mut VecDeque<T>, item: &T) {
    if let Some(index) = queue.iter().position(|other| other == item) {
        queue.remove(index);
    }
}
