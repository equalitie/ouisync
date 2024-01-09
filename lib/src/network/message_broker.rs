use super::{
    barrier::{Barrier, BarrierError},
    choke,
    client::Client,
    connection::ConnectionPermit,
    constants::MAX_REQUESTS_IN_FLIGHT,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, RecvError, Role, SendError},
    message::{Content, MessageChannelId, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    peer_exchange::{PexAnnouncer, PexController, PexDiscoverySender},
    raw,
    runtime_id::PublicRuntimeId,
    server::Server,
};
use crate::{
    collections::{hash_map::Entry, HashMap},
    repository::{LocalId, Vault},
};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use state_monitor::StateMonitor;
use std::{future, sync::Arc};
use tokio::{
    select,
    sync::{mpsc, oneshot, Semaphore},
    task,
    time::Duration,
};
use tracing::{instrument::Instrument, Span};

/// Maintains one or more connections to a peer, listening on all of them at the same time. Note
/// that at the present all the connections are TCP based and so dropping some of them would make
/// sense. However, in the future we may also have other transports (e.g. Bluetooth) and thus
/// keeping all may make sence because even if one is dropped, the others may still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
pub(super) struct MessageBroker {
    this_runtime_id: PublicRuntimeId,
    that_runtime_id: PublicRuntimeId,
    dispatcher: MessageDispatcher,
    links: HashMap<LocalId, oneshot::Sender<()>>,
    request_limiter: Arc<Semaphore>,
    monitor: StateMonitor,
    span: Span,
}

impl MessageBroker {
    pub fn new(
        this_runtime_id: PublicRuntimeId,
        that_runtime_id: PublicRuntimeId,
        stream: raw::Stream,
        permit: ConnectionPermit,
        monitor: StateMonitor,
    ) -> Self {
        let span = tracing::info_span!(
            "message_broker",
            message = ?that_runtime_id.as_public_key(),
        );

        tracing::info!(parent: &span, "Message broker created");

        let this = Self {
            this_runtime_id,
            that_runtime_id,
            dispatcher: MessageDispatcher::new(),
            links: HashMap::default(),
            request_limiter: Arc::new(Semaphore::new(MAX_REQUESTS_IN_FLIGHT)),
            monitor,
            span,
        };

        this.add_connection(stream, permit);
        this
    }

    pub fn add_connection(&self, stream: raw::Stream, permit: ConnectionPermit) {
        self.dispatcher.bind(stream, permit)
    }

    /// Has this broker at least one live connection?
    pub fn has_connections(&self) -> bool {
        !self.dispatcher.is_closed()
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching repository id for the link to actually be
    /// created.
    pub fn create_link(
        &mut self,
        vault: Vault,
        pex: &PexController,
        choke_manager: &choke::Manager,
    ) {
        let monitor = self.monitor.make_child(vault.monitor.name());
        let span = tracing::info_span!(
            parent: &self.span,
            "link",
            message = vault.monitor.name(),
        );

        let span_enter = span.enter();

        let (abort_tx, abort_rx) = oneshot::channel();

        match self.links.entry(vault.local_id) {
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

        let channel_id = MessageChannelId::new(
            vault.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
            role,
        );

        let stream = self.dispatcher.open_recv(channel_id);
        let sink = self.dispatcher.open_send(channel_id);
        let request_limiter = self.request_limiter.clone();

        let pex_discovery_tx = pex.discovery_sender();
        let pex_announcer = pex.announcer(self.that_runtime_id, self.dispatcher.connection_infos());

        let choker = choke_manager.new_choker();

        tracing::info!(?role, "Link created");

        drop(span_enter);

        let task = async move {
            select! {
                _ = maintain_link(
                    role,
                    stream,
                    sink,
                    vault,
                    request_limiter,
                    pex_discovery_tx,
                    pex_announcer,
                    monitor,
                    choker,
                ) => (),
                _ = abort_rx => (),
            }

            tracing::info!("Link destroyed")
        };
        let task = task.instrument(span);

        task::spawn(task);
    }

    /// Destroy the link between a local repository with the specified id hash and its remote
    /// counterpart (if one exists).
    pub fn destroy_link(&mut self, id: LocalId) {
        self.links.remove(&id);
    }

    pub async fn shutdown(&self) {
        self.dispatcher.close().await;
    }
}

impl Drop for MessageBroker {
    fn drop(&mut self) {
        tracing::info!(parent: &self.span, "Message broker destroyed");
    }
}

// Repeatedly establish and run the link until it's explicitly destroyed by calling `destroy_link()`.
// TODO: Consider consolidating the arguments somehow
#[allow(clippy::too_many_arguments)]
async fn maintain_link(
    role: Role,
    mut stream: ContentStream,
    mut sink: ContentSink,
    vault: Vault,
    request_limiter: Arc<Semaphore>,
    pex_discovery_tx: PexDiscoverySender,
    mut pex_announcer: PexAnnouncer,
    monitor: StateMonitor,
    choker: choke::Choker,
) {
    #[derive(Debug)]
    enum State {
        Sleeping(Duration),
        AwaitingBarrier,
        EstablishingChannel,
        Running,
    }

    let mut backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_millis(100))
        .with_max_interval(Duration::from_secs(5))
        .with_max_elapsed_time(None)
        .build();

    let mut next_sleep = None;
    let state = monitor.make_value("state", State::AwaitingBarrier);

    loop {
        if let Some(sleep) = next_sleep {
            *state.get() = State::Sleeping(sleep);
            tokio::time::sleep(sleep).await;
        }

        next_sleep = backoff.next_backoff();

        *state.get() = State::AwaitingBarrier;

        match Barrier::new(&mut stream, &sink, &monitor).run().await {
            Ok(()) => (),
            Err(BarrierError::Failure) => continue,
            Err(BarrierError::ChannelClosed) => break,
            Err(BarrierError::TransportChanged) => continue,
        }

        *state.get() = State::EstablishingChannel;

        let (crypto_stream, crypto_sink) =
            match establish_channel(role, &mut stream, &mut sink, &vault).await {
                Ok(io) => io,
                Err(EstablishError::Crypto) => continue,
                Err(EstablishError::Closed) => break,
                Err(EstablishError::TransportChanged) => continue,
            };

        *state.get() = State::Running;

        match run_link(
            crypto_stream,
            crypto_sink,
            &vault,
            request_limiter.clone(),
            pex_discovery_tx.clone(),
            &mut pex_announcer,
            choker.clone(),
        )
        .await
        {
            ControlFlow::Continue => continue,
            ControlFlow::Break => break,
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
    request_limiter: Arc<Semaphore>,
    pex_discovery_tx: PexDiscoverySender,
    pex_announcer: &mut PexAnnouncer,
    choker: choke::Choker,
) -> ControlFlow {
    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, content_rx) = mpsc::channel(1);

    // Run everything in parallel:
    select! {
        flow = run_client(repo.clone(), content_tx.clone(), response_rx, request_limiter) => flow,
        flow = run_server(repo.clone(), content_tx.clone(), request_rx, choker) => flow,
        flow = recv_messages(stream, request_tx, response_tx, pex_discovery_tx) => flow,
        flow = send_messages(content_rx, sink) => flow,
        _ = pex_announcer.run(content_tx) => ControlFlow::Continue,
    }
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
    pex_discovery_tx: PexDiscoverySender,
) -> ControlFlow {
    loop {
        let content = match stream.recv().await {
            Ok(content) => content,
            Err(RecvError::Crypto) => {
                tracing::warn!("Failed to decrypt incoming message",);
                return ControlFlow::Continue;
            }
            Err(RecvError::Exhausted) => {
                tracing::debug!("Incoming message nonce counter exhausted",);
                return ControlFlow::Continue;
            }
            Err(RecvError::Closed) => {
                tracing::debug!("Message stream closed");
                return ControlFlow::Break;
            }
            Err(RecvError::TransportChanged) => {
                tracing::debug!("Transport has changed");
                return ControlFlow::Continue;
            }
        };

        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                tracing::warn!(?error, "Failed to deserialize incoming message");
                continue; // TODO: should we return `ControlFlow::Continue` here as well?
            }
        };

        match content {
            Content::Request(request) => request_tx.send(request).await.unwrap_or(()),
            Content::Response(response) => response_tx.send(response).await.unwrap_or(()),
            Content::Pex(payload) => pex_discovery_tx.send(payload).await.unwrap_or(()),
        }
    }
}

// Handle outgoing messages
async fn send_messages(
    mut content_rx: mpsc::Receiver<Content>,
    mut sink: EncryptingSink<'_>,
) -> ControlFlow {
    loop {
        let content = if let Some(content) = content_rx.recv().await {
            content
        } else {
            forever().await
        };

        // unwrap is OK because serialization into a vec should never fail unless we have a bug
        // somewhere.
        let content = bincode::serialize(&content).unwrap();

        match sink.send(content).await {
            Ok(()) => (),
            Err(SendError::Exhausted) => {
                tracing::debug!("Outgoing message nonce counter exhausted");
                return ControlFlow::Continue;
            }
            Err(SendError::Closed) => {
                tracing::debug!("Message sink closed");
                return ControlFlow::Break;
            }
        }
    }
}

// Create and run client. Returns only on error.
async fn run_client(
    repo: Vault,
    content_tx: mpsc::Sender<Content>,
    response_rx: mpsc::Receiver<Response>,
    request_limiter: Arc<Semaphore>,
) -> ControlFlow {
    let mut client = Client::new(repo, content_tx, response_rx, request_limiter);
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
    content_tx: mpsc::Sender<Content>,
    request_rx: mpsc::Receiver<Request>,
    choker: choke::Choker,
) -> ControlFlow {
    let mut server = Server::new(repo, content_tx, request_rx, choker);

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
