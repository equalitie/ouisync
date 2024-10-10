use super::{
    choke::Choker,
    client::Client,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, Role},
    message::{Message, Request, Response},
    message_dispatcher::{MessageDispatcher, MessageSink, MessageStream},
    peer_exchange::{PexPeer, PexReceiver, PexRepository, PexSender},
    request_tracker::RequestTracker,
    runtime_id::PublicRuntimeId,
    server::Server,
    stats::ByteCounters,
};
use crate::{
    collections::HashMap,
    crypto::Hashable,
    network::constants::{REQUEST_BUFFER_SIZE, RESPONSE_BUFFER_SIZE},
    protocol::RepositoryId,
    repository::Vault,
};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use bytes::{BufMut, BytesMut};
use futures_util::{SinkExt, StreamExt};
use net::{bus::TopicId, unified::Connection};
use state_monitor::StateMonitor;
use std::{collections::hash_map::Entry, sync::Arc, time::Instant};
use tokio::{
    select,
    sync::{
        mpsc::{self, error::TryRecvError},
        oneshot,
    },
    task,
    time::Duration,
};
use tracing::{instrument::Instrument, Span};

/// Handler for communication with one peer.
pub(super) struct MessageBroker {
    this_runtime_id: PublicRuntimeId,
    that_runtime_id: PublicRuntimeId,
    dispatcher: MessageDispatcher,
    links: HashMap<RepositoryId, oneshot::Sender<()>>,
    pex_peer: PexPeer,
    monitor: StateMonitor,
    span: SpanGuard,
}

impl MessageBroker {
    pub fn new(
        this_runtime_id: PublicRuntimeId,
        that_runtime_id: PublicRuntimeId,
        connection: Connection,
        pex_peer: PexPeer,
        monitor: StateMonitor,
        total_counters: Arc<ByteCounters>,
        peer_counters: Arc<ByteCounters>,
    ) -> Self {
        let span = SpanGuard::new(&that_runtime_id);

        Self {
            this_runtime_id,
            that_runtime_id,
            dispatcher: MessageDispatcher::builder(connection)
                .with_total_counters(total_counters)
                .with_peer_counters(peer_counters)
                .build(),
            links: HashMap::default(),
            pex_peer,
            monitor,
            span,
        }
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching repository id for the link to actually be
    /// created.
    pub fn create_link(
        &mut self,
        vault: Vault,
        pex_repo: &PexRepository,
        request_tracker: RequestTracker,
        choker: Choker,
        repo_counters: Arc<ByteCounters>,
    ) {
        let monitor = self.monitor.make_child(vault.monitor.name());
        let span = tracing::info_span!(
            parent: &self.span.0,
            "link",
            message = vault.monitor.name(),
        );

        let span_enter = span.enter();

        let (abort_tx, abort_rx) = oneshot::channel();

        match self.links.entry(*vault.repository_id()) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_closed() {
                    entry.insert(abort_tx);
                } else {
                    tracing::warn!("Link not created - already exists");
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(abort_tx);
            }
        }

        let role = Role::determine(
            vault.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
        );

        let topic_id = make_topic_id(
            vault.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
        );

        let (pex_tx, pex_rx) = self.pex_peer.new_link(pex_repo);

        let mut link = Link {
            role,
            topic_id,
            dispatcher: self.dispatcher.clone(),
            vault,
            request_tracker,
            choker,
            pex_tx,
            pex_rx,
            monitor,
            repo_counters,
        };

        drop(span_enter);

        let task = async move {
            select! {
                _ = link.maintain() => (),
                _ = abort_rx => (),
            }
        };
        let task = task.instrument(span);

        task::spawn(task);
    }

    /// Destroy the link between a local repository with the specified id hash and its remote
    /// counterpart (if one exists).
    pub fn destroy_link(&mut self, id: &RepositoryId) {
        self.links.remove(id);
    }

    pub async fn shutdown(self) {
        self.dispatcher.shutdown().await;
    }
}

struct SpanGuard(Span);

impl SpanGuard {
    fn new(that_runtime_id: &PublicRuntimeId) -> Self {
        let span = tracing::info_span!(
            "peer",
            message = ?that_runtime_id.as_public_key(),
        );

        tracing::info!(parent: &span, "Message broker created");

        Self(span)
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        tracing::info!(parent: &self.0, "Message broker destroyed");
    }
}

fn make_topic_id(
    repo_id: &RepositoryId,
    this_runtime_id: &PublicRuntimeId,
    that_runtime_id: &PublicRuntimeId,
) -> TopicId {
    let (id1, id2) = if this_runtime_id > that_runtime_id {
        (this_runtime_id, that_runtime_id)
    } else {
        (that_runtime_id, this_runtime_id)
    };

    let bytes: [_; TopicId::SIZE] = (repo_id, id1, id2, b"ouisync message topic id")
        .hash()
        .into();

    TopicId::from(bytes)
}

struct Link {
    role: Role,
    topic_id: TopicId,
    dispatcher: MessageDispatcher,
    vault: Vault,
    request_tracker: RequestTracker,
    choker: Choker,
    pex_tx: PexSender,
    pex_rx: PexReceiver,
    monitor: StateMonitor,
    repo_counters: Arc<ByteCounters>,
}

impl Link {
    // Repeatedly establish and run the link until it's explicitly destroyed by calling `destroy_link()`.
    async fn maintain(&mut self) {
        #[derive(Debug)]
        enum State {
            Sleeping(#[allow(dead_code)] Duration),
            EstablishingChannel,
            Running,
        }

        let min_backoff = Duration::from_millis(100);
        let max_backoff = Duration::from_secs(5);

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(min_backoff)
            .with_max_interval(max_backoff)
            .with_max_elapsed_time(None)
            .build();

        let mut next_sleep = None;
        let state = self.monitor.make_value("state", State::EstablishingChannel);

        loop {
            if let Some(sleep) = next_sleep {
                *state.get() = State::Sleeping(sleep);
                tokio::time::sleep(sleep).await;
            }

            next_sleep = backoff.next_backoff();

            *state.get() = State::EstablishingChannel;

            let (mut sink, mut stream) = self
                .dispatcher
                .open(self.topic_id, self.repo_counters.clone());

            let Ok((crypto_stream, crypto_sink)) =
                establish_channel(self.role, &mut stream, &mut sink, &self.vault).await
            else {
                continue;
            };

            *state.get() = State::Running;

            let start = Instant::now();

            run_link(
                crypto_stream,
                crypto_sink,
                &self.vault,
                &self.request_tracker,
                self.choker.clone(),
                &mut self.pex_tx,
                &mut self.pex_rx,
            )
            .await;

            if start.elapsed() > max_backoff {
                backoff.reset();
            }
        }
    }
}

async fn establish_channel<'a>(
    role: Role,
    stream: &'a mut MessageStream,
    sink: &'a mut MessageSink,
    vault: &Vault,
) -> Result<(DecryptingStream<'a>, EncryptingSink<'a>), EstablishError> {
    match crypto::establish_channel(role, vault.repository_id(), stream, sink).await {
        Ok(io) => {
            tracing::debug!("Established encrypted channel");
            Ok(io)
        }
        Err(error) => {
            tracing::warn!(?error, "Failed to establish encrypted channel");
            Err(error)
        }
    }
}

async fn run_link(
    stream: DecryptingStream<'_>,
    sink: EncryptingSink<'_>,
    vault: &Vault,
    request_tracker: &RequestTracker,
    choker: Choker,
    pex_tx: &mut PexSender,
    pex_rx: &mut PexReceiver,
) {
    // Incoming message channels are bounded to prevent malicious peers from sending us too many
    // messages and exhausting our memory.
    let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
    let (response_tx, response_rx) = mpsc::channel(RESPONSE_BUFFER_SIZE);
    // Outgoing message channel is unbounded because we fully control how much stuff goes into it.
    let (message_tx, message_rx) = mpsc::unbounded_channel();

    let _guard = LinkGuard::new();

    select! {
        _ = run_client(vault.clone(), message_tx.clone(), response_rx, request_tracker) => (),
        _ = run_server(vault.clone(), message_tx.clone(), request_rx, choker) => (),
        _ = recv_messages(stream, request_tx, response_tx, pex_rx) => (),
        _ = send_messages(message_rx, sink) => (),
        _ = pex_tx.run(message_tx) => (),
    };
}

struct LinkGuard;

impl LinkGuard {
    fn new() -> Self {
        tracing::info!("Link opened");
        Self
    }
}

impl Drop for LinkGuard {
    fn drop(&mut self) {
        tracing::info!("Link closed");
    }
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
    pex_rx: &PexReceiver,
) {
    loop {
        let message = match stream.next().await {
            Some(Ok(message)) => message,
            Some(Err(error)) => {
                tracing::warn!(?error, "Failed to receive incoming message");
                break;
            }
            None => {
                tracing::debug!("Message channel closed");
                break;
            }
        };

        let message: Message = match bincode::deserialize(&message) {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(?error, "Failed to deserialize incoming message");
                continue;
            }
        };

        match message {
            Message::Request(request) => request_tx.send(request).await.unwrap_or(()),
            Message::Response(response) => response_tx.send(response).await.unwrap_or(()),
            Message::Pex(payload) => pex_rx.handle_message(payload).await,
        }
    }
}

// Handle outgoing messages
async fn send_messages(
    mut message_rx: mpsc::UnboundedReceiver<Message>,
    mut sink: EncryptingSink<'_>,
) {
    let mut writer = BytesMut::new().writer();

    loop {
        let message = match message_rx.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty) => {
                match sink.flush().await {
                    Ok(()) => (),
                    Err(error) => {
                        tracing::warn!(?error, "Failed to flush outgoing messages");
                        break;
                    }
                }

                message_rx.recv().await
            }
            Err(TryRecvError::Disconnected) => None,
        };

        let Some(message) = message else {
            return;
        };

        // unwrap is OK because serialization into a vec should never fail unless we have a bug
        // somewhere.
        bincode::serialize_into(&mut writer, &message).unwrap();

        match sink.feed(writer.get_mut().split().freeze()).await {
            Ok(()) => (),
            Err(error) => {
                tracing::warn!(?error, "Failed to send outgoing message");
                break;
            }
        }
    }
}

// Create and run client. Returns only on error.
async fn run_client(
    vault: Vault,
    message_tx: mpsc::UnboundedSender<Message>,
    response_rx: mpsc::Receiver<Response>,
    request_tracker: &RequestTracker,
) {
    let mut client = Client::new(vault, message_tx, response_rx, request_tracker);
    let result = client.run().await;

    tracing::debug!("Client stopped running with result {:?}", result);
}

// Create and run server. Returns only on error.
async fn run_server(
    vault: Vault,
    message_tx: mpsc::UnboundedSender<Message>,
    request_rx: mpsc::Receiver<Request>,
    choker: Choker,
) {
    let mut server = Server::new(vault, message_tx, request_rx, choker);

    let result = server.run().await;

    tracing::debug!("Server stopped running with result {:?}", result);
}
