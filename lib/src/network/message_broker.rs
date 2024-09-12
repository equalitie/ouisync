use super::{
    client::Client,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, RecvError, Role, SendError},
    message::{Content, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    peer_exchange::{PexPeer, PexReceiver, PexRepository, PexSender},
    runtime_id::PublicRuntimeId,
    server::Server,
    stats::ByteCounters,
};
use crate::{
    collections::{hash_map::Entry, HashMap},
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
use std::{future, sync::Arc};
use tokio::{
    select,
    sync::{
        mpsc::{self, error::TryRecvError},
        oneshot, Semaphore,
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
    #[allow(clippy::too_many_arguments)]
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
        response_limiter: Arc<Semaphore>,
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

        let (sink, stream) = self.dispatcher.open(topic_id, repo_counters);

        let (pex_tx, pex_rx) = self.pex_peer.new_link(pex_repo);

        let mut link = Link {
            role,
            stream,
            sink,
            vault,
            response_limiter,
            pex_tx,
            pex_rx,
            monitor,
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
            "message_broker",
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
    stream: ContentStream,
    sink: ContentSink,
    vault: Vault,
    response_limiter: Arc<Semaphore>,
    pex_tx: PexSender,
    pex_rx: PexReceiver,
    monitor: StateMonitor,
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

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(100))
            .with_max_interval(Duration::from_secs(5))
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

            let (crypto_stream, crypto_sink) =
                match establish_channel(self.role, &mut self.stream, &mut self.sink, &self.vault)
                    .await
                {
                    Ok(io) => io,
                    Err(EstablishError::Crypto) => continue,
                    Err(EstablishError::Io(_)) => break,
                };

            *state.get() = State::Running;

            match run_link(
                crypto_stream,
                crypto_sink,
                &self.vault,
                self.response_limiter.clone(),
                &mut self.pex_tx,
                &mut self.pex_rx,
            )
            .await
            {
                ControlFlow::Continue => continue,
                ControlFlow::Break => break,
            }
        }
    }
}

async fn establish_channel<'a>(
    role: Role,
    stream: &'a mut ContentStream,
    sink: &'a mut ContentSink,
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
    repo: &Vault,
    response_limiter: Arc<Semaphore>,
    pex_tx: &mut PexSender,
    pex_rx: &mut PexReceiver,
) -> ControlFlow {
    // Incoming message channels are bounded to prevent malicious peers from sending us too many
    // messages and exhausting our memory.
    let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
    let (response_tx, response_rx) = mpsc::channel(RESPONSE_BUFFER_SIZE);
    // Outgoing message channel is unbounded because we fully control how much stuff goes into it.
    let (content_tx, content_rx) = mpsc::unbounded_channel();

    tracing::info!("Link opened");

    // Run everything in parallel:
    let flow = select! {
        flow = run_client(repo.clone(), content_tx.clone(), response_rx) => flow,
        flow = run_server(repo.clone(), content_tx.clone(), request_rx, response_limiter) => flow,
        flow = recv_messages(stream, request_tx, response_tx, pex_rx) => flow,
        flow = send_messages(content_rx, sink) => flow,
        _ = pex_tx.run(content_tx) => ControlFlow::Continue,
    };

    tracing::info!("Link closed");

    flow
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
    pex_rx: &PexReceiver,
) -> ControlFlow {
    loop {
        let content = match stream.next().await {
            Some(Ok(content)) => content,
            Some(Err(RecvError::Crypto)) => {
                tracing::warn!("Failed to decrypt incoming message",);
                return ControlFlow::Continue;
            }
            Some(Err(RecvError::Exhausted)) => {
                tracing::debug!("Incoming message nonce counter exhausted",);
                return ControlFlow::Continue;
            }
            Some(Err(RecvError::Io(error))) => {
                tracing::warn!(?error, "Failed to receive incoming message");
                return ControlFlow::Break;
            }
            None => {
                tracing::debug!("Message channel closed");
                return ControlFlow::Break;
            }
        };

        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                tracing::warn!(?error, "Failed to deserialize incoming message");
                continue;
            }
        };

        match content {
            Content::Request(request) => request_tx.send(request).await.unwrap_or(()),
            Content::Response(response) => response_tx.send(response).await.unwrap_or(()),
            Content::Pex(payload) => pex_rx.handle_message(payload).await,
        }
    }
}

// Handle outgoing messages
async fn send_messages(
    mut content_rx: mpsc::UnboundedReceiver<Content>,
    mut sink: EncryptingSink<'_>,
) -> ControlFlow {
    let mut writer = BytesMut::new().writer();

    loop {
        let content = match content_rx.try_recv() {
            Ok(content) => Some(content),
            Err(TryRecvError::Empty) => {
                match sink.flush().await {
                    Ok(()) => (),
                    Err(error) => {
                        tracing::warn!(?error, "Failed to flush outgoing messages");
                        return ControlFlow::Break;
                    }
                }

                content_rx.recv().await
            }
            Err(TryRecvError::Disconnected) => None,
        };

        let Some(content) = content else {
            forever().await
        };

        // unwrap is OK because serialization into a vec should never fail unless we have a bug
        // somewhere.
        bincode::serialize_into(&mut writer, &content).unwrap();

        match sink.feed(writer.get_mut().split().freeze()).await {
            Ok(()) => (),
            Err(SendError::Exhausted) => {
                tracing::debug!("Outgoing message nonce counter exhausted");
                return ControlFlow::Continue;
            }
            Err(SendError::Io(error)) => {
                tracing::warn!(?error, "Failed to send outgoing message");
                return ControlFlow::Break;
            }
        }
    }
}

// Create and run client. Returns only on error.
async fn run_client(
    repo: Vault,
    content_tx: mpsc::UnboundedSender<Content>,
    response_rx: mpsc::Receiver<Response>,
) -> ControlFlow {
    let mut client = Client::new(repo, content_tx, response_rx);
    let result = client.run().await;

    tracing::debug!("Client stopped running with result {:?}", result);

    match result {
        Ok(()) => forever().await,
        Err(_) => ControlFlow::Continue,
    }
}

// Create and run server. Returns only on error.
async fn run_server(
    repo: Vault,
    content_tx: mpsc::UnboundedSender<Content>,
    request_rx: mpsc::Receiver<Request>,
    response_limiter: Arc<Semaphore>,
) -> ControlFlow {
    let mut server = Server::new(repo, content_tx, request_rx, response_limiter);

    let result = server.run().await;

    tracing::debug!("Server stopped running with result {:?}", result);

    match result {
        Ok(()) => forever().await,
        Err(_) => ControlFlow::Continue,
    }
}

async fn forever() -> ! {
    future::pending::<()>().await;
    unreachable!()
}

enum ControlFlow {
    Continue,
    Break,
}
