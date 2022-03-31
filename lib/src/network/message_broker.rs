use super::{
    client::Client,
    connection::ConnectionPermit,
    crypto::{self, DecryptingStream, EncryptingSink, EstablishError, RecvError, Role, SendError},
    message::{Content, MessageChannel, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    protocol::RuntimeId,
    server::Server,
};
use crate::{index::Index, repository::RepositoryId};
use std::{
    collections::{hash_map::Entry, HashMap},
    future,
};
use tokio::{
    net::TcpStream,
    select,
    sync::{mpsc, oneshot},
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
        let (abort_tx, abort_rx) = oneshot::channel();

        match self.links.entry(channel) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_closed() {
                    entry.insert(abort_tx);
                } else {
                    log::warn!("not creating link for {:?} - already exists", channel);
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(abort_tx);
            }
        }

        log::debug!("creating link for {:?}", channel);

        let role = Role::determine(
            index.repository_id(),
            &self.this_runtime_id,
            &self.that_runtime_id,
        );

        let stream = self.dispatcher.open_recv(channel);
        let sink = self.dispatcher.open_send(channel);

        task::spawn(async move {
            select! {
                _ = maintain_link(role, channel, stream, sink.clone(), index) => (),
                _ = abort_rx => sink.reset().await,
            }

            log::debug!("link for {:?} destroyed", channel)
        });
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
    channel: MessageChannel,
    mut stream: ContentStream,
    mut sink: ContentSink,
    index: Index,
) {
    loop {
        let (crypto_stream, crypto_sink) =
            match crypto::establish_channel(role, index.repository_id(), &mut stream, &mut sink)
                .await
            {
                Ok(io) => {
                    log::debug!(
                        "established encrypted channel for {:?} as {:?}",
                        channel,
                        role
                    );

                    io
                }
                Err(error @ (EstablishError::Crypto | EstablishError::Reset)) => {
                    log::warn!(
                        "failed to establish encrypted channel for {:?} as {:?}: {}",
                        channel,
                        role,
                        error
                    );

                    continue;
                }
                Err(error @ EstablishError::Closed) => {
                    log::debug!(
                        "failed to establish encrypted channel for {:?} as {:?}: {}",
                        channel,
                        role,
                        error
                    );

                    break;
                }
            };

        let status = run_link(channel, crypto_stream, crypto_sink, &index).await;

        // DEBUG
        log::debug!("link stopped as {:?}: {:?}", role, status);

        match status {
            Status::Failed => sink.reset().await,
            Status::Reset => (),
            Status::Closed => break,
        }
    }
}

async fn run_link(
    channel: MessageChannel,
    stream: DecryptingStream<'_>,
    sink: EncryptingSink<'_>,
    index: &Index,
) -> Status {
    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, content_rx) = mpsc::channel(1);

    // Run everything in parallel:
    select! {
        status = run_client(channel, index.clone(), content_tx.clone(), response_rx) => status,
        status = run_server(channel, index.clone(), content_tx, request_rx ) => status,
        status = recv_messages(stream, request_tx, response_tx) => status,
        status = send_messages(content_rx, sink) => status,
    }
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) -> Status {
    loop {
        let content = match stream.recv().await {
            Ok(content) => content,
            Err(RecvError::Crypto) => {
                log::warn!(
                    "failed to decrypt incoming message for {:?}",
                    stream.channel()
                );
                return Status::Reset;
            }
            Err(RecvError::Reset) => {
                log::debug!("message stream for {:?} reset", stream.channel());
                return Status::Reset;
            }
            Err(RecvError::Closed) => {
                log::debug!("message stream for {:?} closed", stream.channel());
                return Status::Closed;
            }
        };

        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                log::warn!(
                    "failed to deserialize incoming message for {:?}: {}",
                    stream.channel(),
                    error
                );
                continue; // TODO: should we return `Status::Reset` here as well?
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
) -> Status {
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
            Err(SendError::Reset) => {
                log::debug!("message sink for {:?} reset", sink.channel());
                return Status::Reset;
            }
            Err(SendError::Closed) => {
                log::debug!("message sink for {:?} closed", sink.channel());
                return Status::Closed;
            }
        }
    }
}

// Create and run client. Returns only on error.
async fn run_client(
    channel: MessageChannel,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    response_rx: mpsc::Receiver<Response>,
) -> Status {
    let mut client = Client::new(index, content_tx, response_rx);

    match client.run().await {
        Ok(()) => forever().await,
        Err(error) => {
            log::error!("client for {:?} failed: {:?}", channel, error);
            Status::Failed
        }
    }
}

// Create and run server. Returns only on error.
async fn run_server(
    channel: MessageChannel,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    request_rx: mpsc::Receiver<Request>,
) -> Status {
    let mut server = Server::new(index, content_tx, request_rx);

    match server.run().await {
        Ok(()) => forever().await,
        Err(error) => {
            log::error!("server for {:?} failed: {:?}", channel, error);
            Status::Failed
        }
    }
}

async fn forever() -> ! {
    let () = future::pending().await;
    unreachable!()
}

#[derive(Debug)]
enum Status {
    // Failure (e.g. database error, request timeout, ...)
    Failed,
    // Message channel reset
    Reset,
    // Connection closed
    Closed,
}
