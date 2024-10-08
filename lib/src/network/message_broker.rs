use super::{
    barrier::{Barrier, BarrierError},
    client::Client,
    connection::ConnectionPermit,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, RecvError, Role, SendError},
    message::{Content, MessageChannelId, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    peer_exchange::{PexPeer, PexReceiver, PexRepository, PexSender},
    raw,
    runtime_id::PublicRuntimeId,
    server::Server,
    stats::{ByteCounters, Instrumented},
};
use crate::{
    collections::{hash_map::Entry, HashMap},
    network::constants::{REQUEST_BUFFER_SIZE, RESPONSE_BUFFER_SIZE},
    protocol::RepositoryId,
    repository::Vault,
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

/// Maintains one or more connections to a single peer, listening on all of them at the same time.
/// Note that at the present all the connections are UDP/QUIC based and so dropping some of them
/// would make sense. However, in the future we may also have other transports (e.g. TCP,
/// Bluetooth) and thus keeping all may make sence because even if one is dropped, the others may
/// still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
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
        pex_peer: PexPeer,
        monitor: StateMonitor,
    ) -> Self {
        let span = SpanGuard::new(&that_runtime_id);

        Self {
            this_runtime_id,
            that_runtime_id,
            dispatcher: MessageDispatcher::new(),
            links: HashMap::default(),
            pex_peer,
            monitor,
            span,
        }
    }

    pub fn add_connection(&self, stream: Instrumented<raw::Stream>, permit: ConnectionPermit) {
        self.pex_peer
            .handle_connection(permit.addr(), permit.source(), permit.released());
        self.dispatcher.bind(stream, permit)
    }

    /// Has this broker at least one live connection?
    pub fn has_connections(&self) -> bool {
        self.dispatcher.is_bound()
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching repository id for the link to actually be
    /// created.
    pub fn create_link(
        &mut self,
        vault: Vault,
        pex_repo: &PexRepository,
        response_limiter: Arc<Semaphore>,
        byte_counters: Arc<ByteCounters>,
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

        let channel_id = MessageChannelId::new(
            vault.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
            role,
        );

        let (pex_tx, pex_rx) = self.pex_peer.new_link(pex_repo);

        let stream =
            Instrumented::new(self.dispatcher.open_recv(channel_id), byte_counters.clone());
        let sink = Instrumented::new(self.dispatcher.open_send(channel_id), byte_counters);

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

struct Link {
    role: Role,
    stream: Instrumented<ContentStream>,
    sink: Instrumented<ContentSink>,
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
        let state = self.monitor.make_value("state", State::AwaitingBarrier);

        loop {
            if let Some(sleep) = next_sleep {
                *state.get() = State::Sleeping(sleep);
                tokio::time::sleep(sleep).await;
            }

            next_sleep = backoff.next_backoff();

            *state.get() = State::AwaitingBarrier;

            match Barrier::new(self.stream.as_mut(), self.sink.as_ref(), &self.monitor)
                .run()
                .await
            {
                Ok(()) => (),
                Err(BarrierError::Failure) => continue,
                Err(BarrierError::ChannelClosed) => break,
                Err(BarrierError::TransportChanged) => continue,
            }

            *state.get() = State::EstablishingChannel;

            let (crypto_stream, crypto_sink) =
                match establish_channel(self.role, &mut self.stream, &mut self.sink, &self.vault)
                    .await
                {
                    Ok(io) => io,
                    Err(EstablishError::Crypto) => continue,
                    Err(EstablishError::Closed) => break,
                    Err(EstablishError::TransportChanged) => continue,
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
    stream: &'a mut Instrumented<ContentStream>,
    sink: &'a mut Instrumented<ContentSink>,
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
            Content::Pex(payload) => pex_rx.handle_message(payload).await,
        }
    }
}

// Handle outgoing messages
async fn send_messages(
    mut content_rx: mpsc::UnboundedReceiver<Content>,
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
