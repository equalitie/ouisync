use super::{
    client::Client,
    connection::ConnectionPermit,
    message::{Content, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    server::Server,
};
use crate::{index::Index, repository::PublicRepositoryId, scoped_task::ScopedJoinHandle};
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
    command_tx: mpsc::Sender<Command>,
    _join_handle: ScopedJoinHandle<()>,
}

impl MessageBroker {
    pub fn new(stream: TcpStream, permit: ConnectionPermit) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);

        let inner = Inner {
            dispatcher: MessageDispatcher::new(),
            links: HashMap::new(),
        };

        inner.add_connection(stream, permit);

        let handle = task::spawn(inner.run(command_rx));

        Self {
            command_tx,
            _join_handle: ScopedJoinHandle(handle),
        }
    }

    pub async fn add_connection(&self, stream: TcpStream, permit: ConnectionPermit) {
        if self
            .command_tx
            .send(Command::AddConnection(stream, permit))
            .await
            .is_err()
        {
            log::error!("failed to add connection - message broker is shutting down")
        }
    }

    /// Has this broker at least one live connection?
    pub fn has_connections(&self) -> bool {
        !self.command_tx.is_closed()
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching `local_name` and `remote_name` for the link
    /// to actually be created.
    pub async fn create_link(&self, id: PublicRepositoryId, index: Index) {
        if self
            .command_tx
            .send(Command::CreateLink { id, index })
            .await
            .is_err()
        {
            log::error!("failed to create link - message broker is shutting down")
        }
    }

    /// Destroy the link between a local repository with the specified id hash and its remote
    /// counterpart (if one exists).
    pub async fn destroy_link(&self, id: PublicRepositoryId) {
        // We can safely ignore the error here because it only means the message broker is shutting
        // down and so all existing links are going to be destroyed anyway.
        self.command_tx
            .send(Command::DestroyLink { id })
            .await
            .unwrap_or(())
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
            Command::CreateLink { id, index } => {
                self.create_link(id, index);
            }
            Command::DestroyLink { id } => {
                self.links.remove(&id);
            }
        }
    }

    fn create_link(&mut self, id: PublicRepositoryId, index: Index) {
        let (abort_tx, abort_rx) = oneshot::channel();

        match self.links.entry(id) {
            Entry::Occupied(mut entry) => {
                if entry.get().is_closed() {
                    entry.insert(abort_tx);
                } else {
                    log::warn!("not creating link for {:?} - already exists", id);
                    return;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(abort_tx);
            }
        }

        log::debug!("creating link for {:?}", id);

        let stream = self.dispatcher.open_recv(id);
        let sink = self.dispatcher.open_send(id);

        task::spawn(async move {
            select! {
                _ = run_link(index, stream, sink) => (),
                _ = abort_rx => (),
            }
        });
    }

    fn add_connection(&self, stream: TcpStream, permit: ConnectionPermit) {
        self.dispatcher.bind(stream, permit)
    }
}

pub(super) enum Command {
    AddConnection(TcpStream, ConnectionPermit),
    CreateLink {
        id: PublicRepositoryId,
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
            Self::CreateLink { id, .. } => f
                .debug_struct("CreateLink")
                .field("id", id)
                .finish_non_exhaustive(),
            Self::DestroyLink { id } => f.debug_struct("DestroyLink").field("id", id).finish(),
        }
    }
}

async fn run_link(index: Index, mut stream: ContentStream, sink: ContentSink) {
    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, mut content_rx) = mpsc::channel(1);

    let id = *stream.id();

    // Create client
    let client_stream = ClientStream::new(content_tx.clone(), response_rx);
    let mut client = Client::new(index.clone(), client_stream);
    let client_task = async move {
        match client.run().await {
            Ok(()) => log::debug!("client for {:?} terminated", id),
            Err(error) => log::error!("client for {:?} failed: {}", id, error),
        }
    };

    // Create server
    let server_stream = ServerStream::new(content_tx.clone(), request_rx);
    let mut server = Server::new(index, server_stream);
    let server_task = async move {
        match server.run().await {
            Ok(()) => log::debug!("server for {:?} terminated", id),
            Err(error) => log::error!("server for {:?} failed: {}", id, error),
        }
    };

    // Handle incoming messages
    let recv_task = async move {
        while let Some(content) = stream.recv().await {
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

        log::debug!("message stream for {:?} closed", id)
    };

    // Handle outgoing messages
    let send_task = async move {
        while let Some(content) = content_rx.recv().await {
            if !sink.send(content).await {
                break;
            }
        }

        log::debug!("message sink for {:?} closed", id)
    };

    // Run everything in parallel:
    select! {
        _ = client_task => (),
        _ = server_task => (),
        _ = recv_task => (),
        _ = send_task => (),
    }

    log::debug!("link for {:?} terminated", id)
}
