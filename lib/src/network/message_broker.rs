use super::{
    client::Client,
    connection::ConnectionPermit,
    crypto::{self, DecryptingStream, EncryptingSink, Role},
    message::{Content, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    runtime_id::RuntimeId,
    server::Server,
};
use crate::{
    index::Index,
    repository::{PublicRepositoryId, SecretRepositoryId},
    scoped_task::ScopedJoinHandle,
};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
};
use tokio::{
    net::TcpStream,
    select,
    sync::{mpsc, oneshot},
    task,
};

/// A stream for receiving Requests and sending Responses
pub(crate) struct ServerStream {
    tx: mpsc::Sender<Content>,
    rx: mpsc::Receiver<Request>,
}

impl ServerStream {
    pub(super) fn new(tx: mpsc::Sender<Content>, rx: mpsc::Receiver<Request>) -> Self {
        Self { tx, rx }
    }

    pub async fn recv(&mut self) -> Option<Request> {
        let rq = self.rx.recv().await?;
        log::trace!("server: recv {:?}", rq);
        Some(rq)
    }

    pub async fn send(&self, response: Response) -> bool {
        log::trace!("server: send {:?}", response);
        self.tx.send(Content::Response(response)).await.is_ok()
    }
}

/// A stream for sending Requests and receiving Responses
pub(crate) struct ClientStream {
    tx: mpsc::Sender<Content>,
    rx: mpsc::Receiver<Response>,
}

impl ClientStream {
    pub(super) fn new(tx: mpsc::Sender<Content>, rx: mpsc::Receiver<Response>) -> Self {
        Self { tx, rx }
    }

    pub async fn recv(&mut self) -> Option<Response> {
        let rs = self.rx.recv().await?;
        log::trace!("client: recv {:?}", rs);
        Some(rs)
    }

    pub async fn send(&self, request: Request) -> bool {
        log::trace!("client: send {:?}", request);
        self.tx.send(Content::Request(request)).await.is_ok()
    }
}

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
    command_tx: mpsc::Sender<Command>,
    _join_handle: ScopedJoinHandle<()>,
}

impl MessageBroker {
    pub fn new(
        this_runtime_id: RuntimeId,
        that_runtime_id: RuntimeId,
        stream: TcpStream,
        permit: ConnectionPermit,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);

        let inner = Inner {
            dispatcher: MessageDispatcher::new(),
            links: HashMap::new(),
        };

        inner.add_connection(stream, permit);

        let handle = task::spawn(inner.run(command_rx));

        Self {
            this_runtime_id,
            that_runtime_id,
            command_tx,
            _join_handle: ScopedJoinHandle(handle),
        }
    }

    pub async fn add_connection(&self, stream: TcpStream, permit: ConnectionPermit) {
        // The unwrap is ok here because the receiver gets closed only when this sender closes
        self.command_tx
            .send(Command::AddConnection(stream, permit))
            .await
            .unwrap()
    }

    /// Has this broker at least one live connection?
    pub async fn has_connections(&self) -> bool {
        let (tx, rx) = oneshot::channel();

        // unwrap is ok because the command receiver gets closed only when the sender gets closed.
        self.command_tx
            .send(Command::CheckHasConnections(tx))
            .await
            .unwrap();

        // unwrap is ok because the oneshot sender is never dropped without being sent to.
        rx.await.unwrap()
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching `local_name` and `remote_name` for the link
    /// to actually be created.
    pub async fn create_link(&self, id: SecretRepositoryId, index: Index) {
        let role = Role::determine(&id, &self.this_runtime_id, &self.that_runtime_id);

        self.command_tx
            .send(Command::CreateLink { id, index, role })
            .await
            .unwrap()
    }

    /// Destroy the link between a local repository with the specified id hash and its remote
    /// counterpart (if one exists).
    pub async fn destroy_link(&self, id: PublicRepositoryId) {
        self.command_tx
            .send(Command::DestroyLink { id })
            .await
            .unwrap()
    }
}

struct Inner {
    dispatcher: MessageDispatcher,
    links: HashMap<PublicRepositoryId, oneshot::Sender<()>>,
}

impl Inner {
    async fn run(mut self, mut command_rx: mpsc::Receiver<Command>) {
        while let Some(command) = command_rx.recv().await {
            self.handle_command(command);
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::AddConnection(stream, permit) => self.add_connection(stream, permit),
            Command::CheckHasConnections(tx) => tx.send(!self.dispatcher.is_closed()).unwrap_or(()),
            Command::CreateLink { role, id, index } => {
                self.create_link(role, id, index);
            }
            Command::DestroyLink { id } => {
                self.links.remove(&id);
            }
        }
    }

    fn create_link(&mut self, role: Role, sid: SecretRepositoryId, index: Index) {
        let (abort_tx, abort_rx) = oneshot::channel();

        let pid = sid.public();

        match self.links.entry(pid) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_closed() {
                    entry.insert(abort_tx);
                } else {
                    log::warn!("not creating link for {:?} - already exists", pid);
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(abort_tx);
            }
        }

        log::debug!("creating link for {:?}", pid);

        let stream = self.dispatcher.open_recv(pid);
        let sink = self.dispatcher.open_send(pid);

        task::spawn(async move {
            select! {
                _ = run_link(role, &sid, stream, sink, index) => (),
                _ = abort_rx => (),
            }
        });
    }

    fn add_connection(&self, stream: TcpStream, permit: ConnectionPermit) {
        self.dispatcher.bind(stream, permit)
    }
}

enum Command {
    AddConnection(TcpStream, ConnectionPermit),
    CheckHasConnections(oneshot::Sender<bool>),
    CreateLink {
        role: Role,
        id: SecretRepositoryId,
        index: Index,
    },
    DestroyLink {
        id: PublicRepositoryId,
    },
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AddConnection(..) => f
                .debug_tuple("AddConnection")
                .field(&format_args!("_"))
                .finish(),
            Self::CheckHasConnections(_) => f
                .debug_tuple("CheckHasConnections")
                .field(&format_args!("_"))
                .finish(),
            Self::CreateLink { id, .. } => f
                .debug_struct("CreateLink")
                .field("id", id)
                .finish_non_exhaustive(),
            Self::DestroyLink { id } => f.debug_struct("DestroyLink").field("id", id).finish(),
        }
    }
}

async fn run_link(
    role: Role,
    repo_id: &SecretRepositoryId,
    stream: ContentStream,
    sink: ContentSink,
    index: Index,
) {
    let id = *stream.id();

    let (stream, sink) = match crypto::establish_channel(role, repo_id, stream, sink).await {
        Ok(channel) => channel,
        Err(error) => {
            log::warn!(
                "failed to establish encrypted channel for {:?}: {}",
                id,
                error
            );
            return;
        }
    };

    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, content_rx) = mpsc::channel(1);

    // Run everything in parallel:
    // TODO: restart when nonce exhausted
    select! {
        _ = run_client(id, index.clone(), content_tx.clone(), response_rx) => (),
        _ = run_server(id, index, content_tx, request_rx ) => (),
        _ = recv_messages(stream, request_tx, response_tx) => (),
        _ = send_messages(content_rx, sink) => (),
    }

    log::debug!("link for {:?} terminated", id)
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) {
    while let Ok(content) = stream.recv().await {
        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                log::warn!(
                    "failed to deserialize message for {:?}: {}",
                    stream.id(),
                    error
                );
                continue;
            }
        };

        match content {
            Content::Request(request) => {
                if request_tx.send(request).await.is_err() {
                    break;
                }
            }
            Content::Response(response) => {
                if response_tx.send(response).await.is_err() {
                    break;
                }
            }
        }
    }

    log::debug!("message stream for {:?} closed", stream.id())
}

// Handle outgoing messages
async fn send_messages(mut content_rx: mpsc::Receiver<Content>, mut sink: EncryptingSink) {
    while let Some(content) = content_rx.recv().await {
        // unwrap is OK because serialization into a vec should never fail unless we have a bug
        // somewhere.
        let content = bincode::serialize(&content).unwrap();

        if sink.send(content).await.is_err() {
            break;
        }
    }

    log::debug!("message sink for {:?} closed", sink.id())
}

// Create and run client
async fn run_client(
    id: PublicRepositoryId,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    response_rx: mpsc::Receiver<Response>,
) {
    let client_stream = ClientStream::new(content_tx, response_rx);
    let mut client = Client::new(index, client_stream);

    match client.run().await {
        Ok(()) => log::debug!("client for {:?} terminated", id),
        Err(error) => log::error!("client for {:?} failed: {}", id, error),
    }
}

// Create and run server
async fn run_server(
    id: PublicRepositoryId,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    request_rx: mpsc::Receiver<Request>,
) {
    let server_stream = ServerStream::new(content_tx, request_rx);
    let mut server = Server::new(index, server_stream);

    match server.run().await {
        Ok(()) => log::debug!("server for {:?} terminated", id),
        Err(error) => log::error!("server for {:?} failed: {}", id, error),
    }
}
