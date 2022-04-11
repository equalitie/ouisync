use super::{
    client::Client,
    connection::ConnectionPermit,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, RecvError, Role, SendError},
    message::{Content, MessageChannel, Request, Response},
    message_dispatcher::{ChannelClosed, ContentSink, ContentStream, MessageDispatcher},
    protocol::RuntimeId,
    request::MAX_PENDING_REQUESTS,
    server::Server,
};
use crate::{index::Index, network::channel_info::ChannelInfo, repository::RepositoryId};
use std::{
    collections::{hash_map::Entry, HashMap},
    future,
    sync::Arc,
};
use tokio::{
    net::TcpStream,
    select,
    sync::{mpsc, oneshot, Semaphore},
    task,
};

/// Maintains one or more connections to a peer, listening on all of them at the same time. Note
/// that at the present all the connections are TCP based and so dropping some of them would make
/// sense. However, in the future we may also have other transports (e.g. Bluetooth) and thus
/// keeping all may make sence because even if one is dropped, the others may still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
pub(super) struct MessageBroker {
    this_runtime_id: RuntimeId,
    that_runtime_id: RuntimeId,
    dispatcher: MessageDispatcher,
    links: HashMap<MessageChannel, oneshot::Sender<()>>,
    request_limiter: Arc<Semaphore>,
}

impl MessageBroker {
    pub fn new(
        this_runtime_id: RuntimeId,
        that_runtime_id: RuntimeId,
        stream: TcpStream,
        permit: ConnectionPermit,
    ) -> Self {
        let this = Self {
            this_runtime_id,
            that_runtime_id,
            dispatcher: MessageDispatcher::new(),
            links: HashMap::new(),
            request_limiter: Arc::new(Semaphore::new(MAX_PENDING_REQUESTS)),
        };

        this.add_connection(stream, permit);
        this
    }

    pub fn add_connection(&self, stream: TcpStream, permit: ConnectionPermit) {
        self.dispatcher.bind(stream, permit)
    }

    /// Has this broker at least one live connection?
    pub fn has_connections(&self) -> bool {
        !self.dispatcher.is_closed()
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching `local_name` and `remote_name` for the link
    /// to actually be created.
    pub fn create_link(&mut self, index: Index) {
        let channel = MessageChannel::from(index.repository_id());
        let channel_info = ChannelInfo::new(channel, self.this_runtime_id, self.that_runtime_id);
        let (abort_tx, abort_rx) = oneshot::channel();

        match self.links.entry(channel) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_closed() {
                    entry.insert(abort_tx);
                } else {
                    log::warn!("{} not creating link - already exists", channel_info);
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(abort_tx);
            }
        }

        log::debug!("{} creating link", channel_info);

        let role = Role::determine(
            index.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
        );

        let stream = self.dispatcher.open_recv(channel);
        let sink = self.dispatcher.open_send(channel);
        let request_limiter = self.request_limiter.clone();

        let task = async move {
            select! {
                _ = maintain_link(role, stream, sink.clone(), index, request_limiter) => (),
                _ = abort_rx => (),
            }

            log::debug!("{} link destroyed", channel_info)
        };

        task::spawn(channel_info.apply(task));
    }

    /// Destroy the link between a local repository with the specified id hash and its remote
    /// counterpart (if one exists).
    pub fn destroy_link(&mut self, id: &RepositoryId) {
        self.links.remove(&MessageChannel::from(id));
    }
}

// Repeatedly establish and run the link until it's explicitly destroyed by calling `destroy_link()`.
async fn maintain_link(
    role: Role,
    mut stream: ContentStream,
    mut sink: ContentSink,
    index: Index,
    request_limiter: Arc<Semaphore>,
) {
    loop {
        match barrier(&mut stream, &mut sink).await {
            Ok(()) => (),
            Err(ChannelClosed) => break,
        }

        let (crypto_stream, crypto_sink) =
            match establish_channel(role, &mut stream, &mut sink, &index).await {
                Ok(io) => io,
                Err(EstablishError::Crypto) => continue,
                Err(EstablishError::Closed) => break,
            };

        match run_link(crypto_stream, crypto_sink, &index, request_limiter.clone()).await {
            ControlFlow::Continue => continue,
            ControlFlow::Break => break,
        }
    }
}

/// Ensures there are no more in-flight messages beween us and the peer.
async fn barrier(stream: &mut ContentStream, sink: &mut ContentSink) -> Result<(), ChannelClosed> {
    sink.send(vec![]).await?;
    sink.send(vec![rand::random()]).await?;
    while stream.recv().await?.len() != 1 {}

    Ok(())
}

async fn establish_channel<'a>(
    role: Role,
    stream: &'a mut ContentStream,
    sink: &'a mut ContentSink,
    index: &Index,
) -> Result<(DecryptingStream<'a>, EncryptingSink<'a>), EstablishError> {
    match crypto::establish_channel(role, index.repository_id(), stream, sink).await {
        Ok(io) => {
            log::debug!(
                "{} established encrypted channel as {:?}",
                ChannelInfo::current(),
                role
            );

            Ok(io)
        }
        Err(error) => {
            log::warn!(
                "{} failed to establish encrypted channel as {:?}: {}",
                ChannelInfo::current(),
                role,
                error
            );

            Err(error)
        }
    }
}

async fn run_link(
    stream: DecryptingStream<'_>,
    sink: EncryptingSink<'_>,
    index: &Index,
    request_limiter: Arc<Semaphore>,
) -> ControlFlow {
    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, content_rx) = mpsc::channel(1);

    // Run everything in parallel:
    select! {
        flow = run_client(index.clone(), content_tx.clone(), response_rx, request_limiter) => flow,
        flow = run_server(index.clone(), content_tx, request_rx ) => flow,
        flow = recv_messages(stream, request_tx, response_tx) => flow,
        flow = send_messages(content_rx, sink) => flow,
    }
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) -> ControlFlow {
    loop {
        let content = match stream.recv().await {
            Ok(content) => content,
            Err(RecvError::Crypto) => {
                log::warn!(
                    "{} failed to decrypt incoming message",
                    ChannelInfo::current()
                );
                return ControlFlow::Continue;
            }
            Err(RecvError::Exhausted) => {
                log::debug!(
                    "{} incoming message nonce counter exhausted",
                    ChannelInfo::current()
                );
                return ControlFlow::Continue;
            }
            Err(RecvError::Closed) => {
                log::debug!("{} message stream closed", ChannelInfo::current());
                return ControlFlow::Break;
            }
        };

        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                log::warn!(
                    "{} failed to deserialize incoming message: {}",
                    ChannelInfo::current(),
                    error
                );
                continue; // TODO: should we return `ControlFlow::Continue` here as well?
            }
        };

        match content {
            Content::Request(request) => request_tx.send(request).await.unwrap_or(()),
            Content::Response(response) => response_tx.send(response).await.unwrap_or(()),
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
                log::debug!(
                    "{} outgoing message nonce counter exhausted",
                    ChannelInfo::current()
                );
                return ControlFlow::Continue;
            }
            Err(SendError::Closed) => {
                log::debug!("{} message sink closed", ChannelInfo::current());
                return ControlFlow::Break;
            }
        }
    }
}

// Create and run client. Returns only on error.
async fn run_client(
    index: Index,
    content_tx: mpsc::Sender<Content>,
    response_rx: mpsc::Receiver<Response>,
    request_limiter: Arc<Semaphore>,
) -> ControlFlow {
    let mut client = Client::new(index, content_tx, response_rx, request_limiter);

    match client.run().await {
        Ok(()) => forever().await,
        Err(error) => {
            log::error!("{} client failed: {:?}", ChannelInfo::current(), error);
            ControlFlow::Continue
        }
    }
}

// Create and run server. Returns only on error.
async fn run_server(
    index: Index,
    content_tx: mpsc::Sender<Content>,
    request_rx: mpsc::Receiver<Request>,
) -> ControlFlow {
    let mut server = Server::new(index, content_tx, request_rx);

    match server.run().await {
        Ok(()) => forever().await,
        Err(error) => {
            log::error!("{} server failed: {:?}", ChannelInfo::current(), error);
            ControlFlow::Continue
        }
    }
}

async fn forever() -> ! {
    let () = future::pending().await;
    unreachable!()
}

enum ControlFlow {
    Continue,
    Break,
}
