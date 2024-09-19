mod graph;
#[cfg(test)]
mod tests;

use self::graph::{Entry as GraphEntry, Graph, Key as GraphKey};
use super::{constants::REQUEST_TIMEOUT, message::Request};
use crate::{
    collections::{HashMap, HashSet},
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
pub(super) struct RequestTracker {
    command_tx: mpsc::UnboundedSender<Command>,
}

impl RequestTracker {
    #[expect(dead_code)]
    pub fn new() -> Self {
        let (this, worker) = build();
        task::spawn(worker.run());
        this
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn new_client(&self) -> (RequestTrackerClient, mpsc::UnboundedReceiver<Request>) {
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
    #[expect(dead_code)]
    pub fn initial(&self, request: Request, block_presence: MultiBlockPresence) {
        self.command_tx
            .send(Command::HandleInitial {
                client_id: self.client_id,
                request,
                block_presence,
            })
            .ok();
    }

    /// Handle sending requests that follow from a received success response.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn success(&self, response_key: MessageKey, requests: Vec<(Request, MultiBlockPresence)>) {
        self.command_tx
            .send(Command::HandleSuccess {
                client_id: self.client_id,
                response_key,
                requests,
            })
            .ok();
    }

    /// Handle failure response.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn failure(&self, response_key: MessageKey) {
        self.command_tx
            .send(Command::HandleFailure {
                client_id: self.client_id,
                response_key,
            })
            .ok();
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

/// Key identifying a request and its corresponding response.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
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
                    self.handle_failure(client_id, request_key);
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
    pub fn request_count(&self) -> usize {
        self.requests.len()
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
            Command::HandleInitial {
                client_id,
                request,
                block_presence,
            } => {
                self.handle_initial(client_id, request, block_presence);
            }
            Command::HandleSuccess {
                client_id,
                response_key,
                requests,
            } => {
                self.handle_success(client_id, response_key, requests);
            }
            Command::HandleFailure {
                client_id,
                response_key,
            } => {
                self.handle_failure(client_id, response_key);
            }
        }
    }

    #[instrument(skip(self, request_tx))]
    fn insert_client(&mut self, client_id: ClientId, request_tx: mpsc::UnboundedSender<Request>) {
        self.clients.insert(client_id, ClientState::new(request_tx));
    }

    #[instrument(skip(self))]
    fn remove_client(&mut self, client_id: ClientId) {
        let Some(client_state) = self.clients.remove(&client_id) else {
            return;
        };

        for (request_key, block_presence) in client_state.requests {
            self.cancel_request(client_id, GraphKey(request_key, block_presence));
        }
    }

    #[instrument(skip(self))]
    fn handle_initial(
        &mut self,
        client_id: ClientId,
        request: Request,
        block_presence: MultiBlockPresence,
    ) {
        self.insert_request(
            client_id,
            GraphKey(MessageKey::from(&request), block_presence),
            Some(request),
            None,
        )
    }

    #[instrument(skip(self))]
    fn handle_success(
        &mut self,
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<(Request, MultiBlockPresence)>,
    ) {
        let Some(block_presence) = self
            .clients
            .get_mut(&client_id)
            .and_then(|client_state| client_state.requests.remove(&response_key))
        else {
            return;
        };

        let request_key = GraphKey(response_key, block_presence);

        let (parent_keys, mut client_ids) = match self.requests.entry(request_key) {
            GraphEntry::Occupied(mut entry) => match entry.get_mut() {
                RequestState::InFlight {
                    sender_client_id,
                    sender_timer_key,
                    waiting,
                } if *sender_client_id == client_id => {
                    self.timer.try_remove(sender_timer_key);

                    let waiting = mem::take(waiting);

                    // Add child requests to this request.
                    for (request, block_presence) in &requests {
                        entry.insert_child(GraphKey(MessageKey::from(request), *block_presence));
                    }

                    // If this request has children, mark it as complete, otherwise remove it.
                    let parent_key = if !entry.children().is_empty() {
                        *entry.get_mut() = RequestState::Complete;
                        HashSet::default()
                    } else {
                        entry.remove().parents
                    };

                    (parent_key, waiting)
                }
                RequestState::InFlight { .. }
                | RequestState::Complete
                | RequestState::Cancelled => return,
            },
            GraphEntry::Vacant(_) => return,
        };

        // Register the followup (child) requests with this client but also with all the clients that
        // were waiting for the original request.
        client_ids.push_front(client_id);

        for (child_request, child_block_presence) in requests {
            for (client_id, child_request) in
                // TODO: use `repeat_n` once it gets stabilized.
                client_ids.iter().copied().zip(iter::repeat(child_request))
            {
                self.insert_request(
                    client_id,
                    GraphKey(MessageKey::from(&child_request), child_block_presence),
                    Some(child_request.clone()),
                    Some(request_key),
                );
            }

            // Round-robin the requests among the clients.
            client_ids.rotate_left(1);
        }

        for parent_key in parent_keys {
            self.request_removed(request_key, parent_key);
        }
    }

    #[instrument(skip(self))]
    fn handle_failure(&mut self, client_id: ClientId, response_key: MessageKey) {
        let Some(client_state) = self.clients.get_mut(&client_id) else {
            return;
        };

        let Some(block_presence) = client_state.requests.remove(&response_key) else {
            return;
        };

        self.cancel_request(client_id, GraphKey(response_key, block_presence));
    }

    fn insert_request(
        &mut self,
        client_id: ClientId,
        request_key: GraphKey,
        request: Option<Request>,
        parent_key: Option<GraphKey>,
    ) {
        let Some(client_state) = self.clients.get_mut(&client_id) else {
            return;
        };

        let (children, request, request_state) = match self.requests.entry(request_key) {
            GraphEntry::Occupied(mut entry) => {
                if let Some(parent_key) = parent_key {
                    entry.insert_parent(parent_key);
                }

                (
                    entry.children().iter().copied().collect(),
                    entry.request(),
                    entry.into_mut(),
                )
            }
            GraphEntry::Vacant(entry) => {
                if let Some(request) = request {
                    let (request, request_state) =
                        entry.insert(request, parent_key, RequestState::Cancelled);
                    (Vec::new(), request, request_state)
                } else {
                    return;
                }
            }
        };

        match request_state {
            RequestState::InFlight { waiting, .. } => {
                waiting.push_back(client_id);
                client_state.requests.insert(request_key.0, request_key.1);
            }
            RequestState::Complete => (),
            RequestState::Cancelled => {
                let timer_key = self
                    .timer
                    .insert((client_id, request_key.0), REQUEST_TIMEOUT);

                *request_state = RequestState::InFlight {
                    sender_client_id: client_id,
                    sender_timer_key: timer_key,
                    waiting: VecDeque::new(),
                };

                client_state.requests.insert(request_key.0, request_key.1);
                client_state.request_tx.send(request.clone()).ok();
            }
        }

        // NOTE: we are using recursion, but the graph is only a few layers deep (currently 5) so
        // there is no danger of stack overflow.
        for child_key in children {
            self.insert_request(client_id, child_key, None, Some(request_key));
        }
    }

    fn cancel_request(&mut self, client_id: ClientId, request_key: GraphKey) {
        let GraphEntry::Occupied(mut entry) = self.requests.entry(request_key) else {
            return;
        };

        let parent_keys = match entry.get_mut() {
            RequestState::InFlight {
                sender_client_id,
                sender_timer_key,
                waiting,
            } => {
                if *sender_client_id == client_id {
                    // The removed client is the current sender of this request.

                    // Remove the timeout for the previous sender
                    self.timer.try_remove(sender_timer_key);

                    // Find a waiting client
                    let next_client = iter::from_fn(|| waiting.pop_front()).find_map(|client_id| {
                        self.clients
                            .get(&client_id)
                            .map(|client_state| (client_id, client_state))
                    });

                    if let Some((next_client_id, next_client_state)) = next_client {
                        // Next waiting client found. Promote it to a sender.

                        *sender_client_id = next_client_id;
                        *sender_timer_key = self
                            .timer
                            .insert((next_client_id, request_key.0), REQUEST_TIMEOUT);

                        // Send the request to the new sender.
                        next_client_state
                            .request_tx
                            .send(entry.request().clone())
                            .ok();

                        return;
                    } else {
                        // No waiting client found. If this request has no children, we can remove
                        // it, otherwise we mark it as cancelled.
                        if !entry.children().is_empty() {
                            *entry.get_mut() = RequestState::Cancelled;
                            return;
                        } else {
                            entry.remove().parents
                        }
                    }
                } else {
                    // The removed client is one of the waiting clients - remove it from the
                    // waiting queue.
                    if let Some(index) = waiting
                        .iter()
                        .position(|next_client_id| *next_client_id == client_id)
                    {
                        waiting.remove(index);
                    }

                    return;
                }
            }
            RequestState::Complete | RequestState::Cancelled => return,
        };

        for parent_key in parent_keys {
            self.request_removed(request_key, parent_key);
        }
    }

    // Remove the request from its parent request and if it was the last child of the parent, remove
    // the parent as well, recursively.
    fn request_removed(&mut self, request_key: GraphKey, parent_key: GraphKey) {
        let mut entry = match self.requests.entry(parent_key) {
            GraphEntry::Occupied(entry) => entry,
            GraphEntry::Vacant(_) => return,
        };

        entry.remove_child(&request_key);

        if !entry.children().is_empty() {
            return;
        }

        let grandparent_keys: Vec<_> = entry.parents().iter().copied().collect();
        for grandparent_key in grandparent_keys {
            self.request_removed(parent_key, grandparent_key);
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
        request_tx: mpsc::UnboundedSender<Request>,
    },
    RemoveClient {
        client_id: ClientId,
    },
    HandleInitial {
        client_id: ClientId,
        request: Request,
        block_presence: MultiBlockPresence,
    },
    HandleSuccess {
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<(Request, MultiBlockPresence)>,
    },
    HandleFailure {
        client_id: ClientId,
        response_key: MessageKey,
    },
}

struct ClientState {
    request_tx: mpsc::UnboundedSender<Request>,
    requests: HashMap<MessageKey, MultiBlockPresence>,
}

impl ClientState {
    fn new(request_tx: mpsc::UnboundedSender<Request>) -> Self {
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
        waiting: VecDeque<ClientId>,
    },
    /// The response to this request has already been received.
    Complete,
    /// The response for the current client failed and there are no more clients waiting.
    Cancelled,
}
