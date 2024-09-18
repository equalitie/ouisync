#[cfg(test)]
mod tests;

use super::{constants::REQUEST_TIMEOUT, message::Request};
use crate::{
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    protocol::BlockId,
};
use futures_util::Stream;
use std::{
    collections::hash_map::Entry,
    iter,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
    time::Duration,
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
        let (this, worker) = build(DelayQueue::new());
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
    pub fn initial(&self, request: Request) {
        self.command_tx
            .send(Command::HandleInitial {
                client_id: self.client_id,
                request,
            })
            .ok();
    }

    /// Handle sending requests that follow from a received success response.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn success(&self, response_key: MessageKey, requests: Vec<Request>) {
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

fn build<T: Timer>(timer: T) -> (RequestTracker, Worker<T>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    (
        RequestTracker { command_tx },
        Worker::new(timer, command_rx),
    )
}

struct Worker<T: Timer> {
    clients: HashMap<ClientId, ClientState>,
    requests: HashMap<MessageKey, RequestState<T>>,
    timer: TimerStream<T>,
    command_rx: mpsc::UnboundedReceiver<Command>,
}

impl<T: Timer> Worker<T> {
    fn new(timer: T, command_rx: mpsc::UnboundedReceiver<Command>) -> Self {
        Self {
            clients: HashMap::default(),
            requests: HashMap::default(),
            timer: TimerStream(timer),
            command_rx,
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
                Some((client_id, request_key)) = self.timer.next() => {
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
            Command::HandleInitial { client_id, request } => {
                self.handle_initial(client_id, request);
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

    fn insert_client(&mut self, client_id: ClientId, request_tx: mpsc::UnboundedSender<Request>) {
        self.clients.insert(
            client_id,
            ClientState {
                request_tx,
                requests: HashSet::default(),
            },
        );
    }

    #[instrument(skip(self))]
    fn remove_client(&mut self, client_id: ClientId) {
        let Some(client_state) = self.clients.remove(&client_id) else {
            return;
        };

        for message_key in client_state.requests {
            let Entry::Occupied(mut entry) = self.requests.entry(message_key) else {
                continue;
            };

            let Some(interest) = entry.get_mut().interests.remove(&client_id) else {
                continue;
            };

            if let Some(timeout_key) = interest.timer_key {
                self.timer.remove(&timeout_key);
            }

            if entry.get().interests.is_empty() {
                entry.remove();
                continue;
            }

            // TODO: fallback to another client, if any
        }
    }

    #[instrument(skip(self))]
    fn handle_initial(&mut self, client_id: ClientId, request: Request) {
        let Some(client_state) = self.clients.get_mut(&client_id) else {
            // client not inserted
            return;
        };

        let request_key = MessageKey::from(&request);

        client_state.requests.insert(request_key);

        let request_state = self
            .requests
            .entry(request_key)
            .or_insert_with(|| RequestState {
                request,
                interests: HashMap::default(),
            });

        let timer_key = if request_state
            .interests
            .values()
            .all(|state| state.timer_key.is_none())
        {
            client_state
                .request_tx
                .send(request_state.request.clone())
                .ok();

            Some(self.timer.insert(client_id, request_key, REQUEST_TIMEOUT))
        } else {
            None
        };

        request_state
            .interests
            .insert(client_id, Interest { timer_key });
    }

    #[instrument(skip(self))]
    fn handle_success(
        &mut self,
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<Request>,
    ) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.requests.remove(&response_key);
        }

        let mut followup_client_ids = if requests.is_empty() {
            vec![]
        } else {
            vec![client_id]
        };

        if let Entry::Occupied(mut entry) = self.requests.entry(response_key) {
            entry
                .get_mut()
                .interests
                .retain(|other_client_id, interest| {
                    // TODO: remove only those with the same or worse block presence.

                    if let Some(timer_key) = interest.timer_key {
                        self.timer.remove(&timer_key);
                    }

                    if !requests.is_empty() && *other_client_id != client_id {
                        followup_client_ids.push(*other_client_id);
                    }

                    false
                });

            if entry.get().interests.is_empty() {
                entry.remove();
            }
        }

        for request in requests {
            for (client_id, request) in followup_client_ids
                .iter()
                .copied()
                // TODO: use `repeat_n` once it gets stabilized.
                .zip(iter::repeat(request))
            {
                self.handle_initial(client_id, request);
            }

            // round-robin the requests across the clients
            followup_client_ids.rotate_right(1);
        }
    }

    #[instrument(skip(self))]
    fn handle_failure(&mut self, client_id: ClientId, response_key: MessageKey) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.requests.remove(&response_key);
        }

        let Entry::Occupied(mut entry) = self.requests.entry(response_key) else {
            return;
        };

        if let Some(interest) = entry.get_mut().interests.remove(&client_id) {
            if let Some(timer_key) = interest.timer_key {
                self.timer.remove(&timer_key);
            }
        }

        // TODO: prefer one with the same or better block presence as `client_id`.
        if let Some((fallback_client_id, interest)) = entry
            .get_mut()
            .interests
            .iter_mut()
            .find(|(_, interest)| interest.timer_key.is_none())
        {
            interest.timer_key = Some(self.timer.insert(
                *fallback_client_id,
                response_key,
                REQUEST_TIMEOUT,
            ));

            // TODO: send the request
        }

        if entry.get().interests.is_empty() {
            entry.remove();
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
    },
    HandleSuccess {
        client_id: ClientId,
        response_key: MessageKey,
        requests: Vec<Request>,
    },
    HandleFailure {
        client_id: ClientId,
        response_key: MessageKey,
    },
}

struct ClientState {
    request_tx: mpsc::UnboundedSender<Request>,
    requests: HashSet<MessageKey>,
}

struct RequestState<T: Timer> {
    request: Request,
    interests: HashMap<ClientId, Interest<T>>,
}

struct Interest<T: Timer> {
    // disambiguator: ResponseDisambiguator,
    timer_key: Option<T::Key>,
}

/// Trait for timer to to track request timeouts.
trait Timer: Unpin {
    type Key: Copy;

    fn insert(
        &mut self,
        client_id: ClientId,
        message_key: MessageKey,
        timeout: Duration,
    ) -> Self::Key;

    fn remove(&mut self, key: &Self::Key);

    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<(ClientId, MessageKey)>>;
}

struct TimerStream<T>(T);

impl<T> Deref for TimerStream<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for TimerStream<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Timer> Stream for TimerStream<T> {
    type Item = (ClientId, MessageKey);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().0.poll_expired(cx)
    }
}

impl Timer for DelayQueue<(ClientId, MessageKey)> {
    type Key = delay_queue::Key;

    fn insert(
        &mut self,
        client_id: ClientId,
        message_key: MessageKey,
        timeout: Duration,
    ) -> Self::Key {
        DelayQueue::insert(self, (client_id, message_key), timeout)
    }

    fn remove(&mut self, key: &Self::Key) {
        DelayQueue::try_remove(self, key);
    }

    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<(ClientId, MessageKey)>> {
        self.poll_expired(cx)
            .map(|expired| expired.map(|expired| expired.into_inner()))
    }
}
