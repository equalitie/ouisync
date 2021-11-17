use super::{
    client::Client,
    connection::{ConnectionPermit, MultiReader, MultiWriter},
    message::{Message, Request, Response},
    object_stream::TcpObjectStream,
    server::Server,
};
use crate::{error::Result, index::Index, replica_id::ReplicaId, scoped_task::ScopedJoinHandle};
use btdht::InfoHash;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    fmt,
    future::Future,
};
use tokio::{
    select,
    sync::{
        mpsc::{self, error::SendError},
        RwLock,
    },
    task,
};

/// A stream for receiving Requests and sending Responses
pub(crate) struct ServerStream {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Request>,
    id: InfoHash,
}

impl ServerStream {
    pub(super) fn new(
        tx: mpsc::Sender<Command>,
        rx: mpsc::Receiver<Request>,
        id: InfoHash,
    ) -> Self {
        Self { tx, rx, id }
    }

    pub async fn recv(&mut self) -> Option<Request> {
        let rq = self.rx.recv().await?;
        log::trace!("server: recv {:?}", rq);
        Some(rq)
    }

    pub async fn send(&self, response: Response) -> Result<(), SendError<Response>> {
        log::trace!("server: send {:?}", response);
        self.tx
            .send(Command::SendMessage(Message::Response {
                id: self.id,
                response,
            }))
            .await
            .map_err(|e| SendError(into_message(e.0)))
    }
}

/// A stream for sending Requests and receiving Responses
pub(crate) struct ClientStream {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Response>,
    id: InfoHash,
}

impl ClientStream {
    pub(super) fn new(
        tx: mpsc::Sender<Command>,
        rx: mpsc::Receiver<Response>,
        id: InfoHash,
    ) -> Self {
        Self { tx, rx, id }
    }

    pub async fn recv(&mut self) -> Option<Response> {
        let rs = self.rx.recv().await?;
        log::trace!("client: recv {:?}", rs);
        Some(rs)
    }

    pub async fn send(&self, request: Request) -> Result<(), SendError<Request>> {
        log::trace!("client: send {:?}", request);
        self.tx
            .send(Command::SendMessage(Message::Request {
                id: self.id,
                request,
            }))
            .await
            .map_err(|e| SendError(into_message(e.0)))
    }
}

fn into_message<T: From<Message>>(command: Command) -> T {
    command.into_send_message().into()
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
    pub async fn new(
        their_replica_id: ReplicaId,
        stream: TcpObjectStream,
        permit: ConnectionPermit,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);

        let inner = Inner {
            their_replica_id,
            command_tx: command_tx.clone(),
            reader: MultiReader::new(),
            writer: MultiWriter::new(),
            links: RwLock::new(Links::new()),
        };

        inner.add_connection(stream, permit);

        let handle = task::spawn(inner.run(command_rx));

        Self {
            command_tx,
            _join_handle: ScopedJoinHandle(handle),
        }
    }

    pub async fn add_connection(&self, stream: TcpObjectStream, permit: ConnectionPermit) {
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
    pub async fn create_link(&self, id: InfoHash, index: Index) {
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
    pub async fn destroy_link(&self, id: InfoHash) {
        // We can safely ignore the error here because it only means the message broker is shutting
        // down and so all existing links are going to be destroyed anyway.
        self.command_tx
            .send(Command::DestroyLink { id })
            .await
            .unwrap_or(())
    }
}

struct Inner {
    their_replica_id: ReplicaId,
    command_tx: mpsc::Sender<Command>,
    reader: MultiReader,
    writer: MultiWriter,
    links: RwLock<Links>,
}

impl Inner {
    async fn run(self, mut command_rx: mpsc::Receiver<Command>) {
        // NOTE: it might be tempting to rewrite this code to something like:
        //
        //     loop {
        //         select! {
        //             command = command_rx.recv() => self.handle_command(command),
        //             message = self.reader.read() => self.handle_message(message),
        //         }
        //     }
        //
        // to avoid all the synchronization machinery. This, however could result in a deadlock.
        // The deadlock could happen this way:
        //
        // * We receive a Request from a peer and we send it to the Server.
        // * We receive another Request, but because the queue to the server has size 1*, we block.
        // * Server processes the Request and sends the Response back to us.
        // * We're unable to process the Response because we're waiting for the second Request to
        //   go through.
        //
        // *) In general, the problem happens when the number of received messages is higher than
        //    the capacity of the channel, so just increasing the capacity won't help.

        let command_task = async {
            while let Some(command) = command_rx.recv().await {
                if !self.handle_command(command).await {
                    break;
                }
            }
        };

        let message_task = async {
            while let Some(message) = self.reader.read().await {
                self.handle_message(message).await;
            }
        };

        // Wait for either to finish.
        select! {
            _ = command_task => (),
            _ = message_task => (),
        }
    }

    async fn handle_command(&self, command: Command) -> bool {
        match command {
            Command::AddConnection(stream, permit) => {
                self.add_connection(stream, permit);
                true
            }
            Command::SendMessage(message) => self.send_message(message).await,
            Command::CreateLink { id, index } => self.create_outgoing_link(id, index).await,
            Command::DestroyLink { id } => {
                self.links.write().await.destroy_one(&id, None);
                true
            }
        }
    }

    async fn handle_message(&self, message: Message) {
        match message {
            Message::Request { id, request } => self.handle_request(&id, request).await,
            Message::Response { id, response } => self.handle_response(&id, response).await,
            Message::CreateLink { id } => self.create_incoming_link(id).await,
        }
    }

    fn add_connection(&self, stream: TcpObjectStream, permit: ConnectionPermit) {
        let (reader, writer) = stream.into_split();
        let (reader_permit, writer_permit) = permit.split();

        self.reader.add(reader, reader_permit);
        self.writer.add(writer, writer_permit);
    }

    async fn send_message(&self, message: Message) -> bool {
        self.writer.write(&message).await
    }

    async fn create_outgoing_link(&self, id: InfoHash, index: Index) -> bool {
        let mut links = self.links.write().await;

        if links.active.contains_key(&id) {
            log::warn!("not creating link for {:?} - already exists", id);
            return true;
        }

        if links.pending_outgoing.contains_key(&id) {
            log::warn!("not creating link for {:?} - already pending", id);
            return true;
        }

        if !self.writer.write(&Message::CreateLink { id }).await {
            log::warn!(
                "not creating link for {:?} - \
                 failed to send CreateLink message - all writers closed",
                id,
            );
            return false;
        }

        if links.pending_incoming.remove(&id) {
            self.create_link(&mut *links, id, index)
        } else {
            links.pending_outgoing.insert(id, index);
        }

        true
    }

    async fn create_incoming_link(&self, id: InfoHash) {
        let mut links = self.links.write().await;

        if let Some(index) = links.pending_outgoing.remove(&id) {
            self.create_link(&mut *links, id, index)
        } else {
            links.pending_incoming.insert(id);
        }
    }

    fn create_link(&self, links: &mut Links, id: InfoHash, index: Index) {
        log::debug!("creating link for {:?}", id);

        let (request_tx, request_rx) = mpsc::channel(1);
        let (response_tx, response_rx) = mpsc::channel(1);

        links.insert_active(id, request_tx, response_tx);

        // NOTE: we just fire-and-forget the tasks which should be OK because when this
        // `MessageBroker` instance is dropped, the associated senders (`request_tx`, `response_tx`)
        // are dropped as well which closes the corresponding receivers which then terminates the
        // tasks.

        let client_stream = ClientStream::new(self.command_tx.clone(), response_rx, id);
        let mut client = Client::new(index.clone(), self.their_replica_id, client_stream);
        task::spawn(async move { log_error(client.run(), "client failed: ").await });

        let server_stream = ServerStream::new(self.command_tx.clone(), request_rx, id);
        let mut server = Server::new(index, server_stream);
        task::spawn(async move { log_error(server.run(), "server failed: ").await });
    }

    async fn handle_request(&self, id: &InfoHash, request: Request) {
        if let Some((link_id, request_tx)) = self.links.read().await.get_request_link(id) {
            if request_tx.send(request).await.is_err() {
                log::warn!("server unexpectedly terminated - destroying the link");
                self.links.write().await.destroy_one(id, Some(link_id));
            }
        } else {
            log::warn!(
                "received request {:?} for unlinked repository {:?}",
                request,
                id
            );
        }
    }

    async fn handle_response(&self, id: &InfoHash, response: Response) {
        if let Some((link_id, response_tx)) = self.links.read().await.get_response_link(id) {
            if response_tx.send(response).await.is_err() {
                log::warn!("client unexpectedly terminated - destroying the link");
                self.links.write().await.destroy_one(id, Some(link_id));
            }
        } else {
            log::warn!(
                "received response {:?} for unlinked repository {:?}",
                response,
                id
            );
        }
    }
}

async fn log_error<F>(fut: F, prefix: &'static str)
where
    F: Future<Output = Result<()>>,
{
    if let Err(error) = fut.await {
        log::error!("{}{}", prefix, error.verbose())
    }
}

pub(super) enum Command {
    AddConnection(TcpObjectStream, ConnectionPermit),
    SendMessage(Message),
    CreateLink { id: InfoHash, index: Index },
    DestroyLink { id: InfoHash },
}

impl Command {
    pub(super) fn into_send_message(self) -> Message {
        match self {
            Self::SendMessage(message) => message,
            _ => panic!("Command is not SendMessage"),
        }
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::AddConnection(..) => f
                .debug_tuple("AddConnection")
                .field(&format_args!("_"))
                .finish(),
            Self::SendMessage(message) => f.debug_tuple("SendMessage").field(message).finish(),
            Self::CreateLink { id, .. } => f
                .debug_struct("CreateLink")
                .field("id", id)
                .finish_non_exhaustive(),
            Self::DestroyLink { id } => f.debug_struct("DestroyLink").field("id", id).finish(),
        }
    }
}

// LinkId is used for when we want to remove a particular link from Links::active, but keep it if
// the link has been replaced with a new one in the mean time. For example, this could happen:
//
// 1. User clones a request_tx from one of the links in Links::active
// 2. User attempts to send to a message using the above request_tx
// 3. In the mean time, the original Link is replaced Links::active with a new one
// 4. The step #2 from above fails and we attempt to remove the link where the request_tx is from,
//    but instead we remove the newly replace link from step #3.
type LinkId = u64;

// Established link between local and remote repositories.
struct Link {
    id: LinkId,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
}

struct Links {
    active: HashMap<InfoHash, Link>,
    pending_incoming: HashSet<InfoHash>,
    pending_outgoing: HashMap<InfoHash, Index>,
    next_link_id: LinkId,
}

impl Links {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            pending_incoming: HashSet::new(),
            pending_outgoing: HashMap::new(),
            next_link_id: 0,
        }
    }

    pub fn insert_active(
        &mut self,
        repository_id: InfoHash,
        request_tx: mpsc::Sender<Request>,
        response_tx: mpsc::Sender<Response>,
    ) {
        let link_id = self.generate_link_id();

        self.active.insert(
            repository_id,
            Link {
                id: link_id,
                request_tx,
                response_tx,
            },
        );
    }

    pub fn get_request_link(
        &self,
        repository_id: &InfoHash,
    ) -> Option<(LinkId, mpsc::Sender<Request>)> {
        self.active
            .get(repository_id)
            .map(|link| (link.id, link.request_tx.clone()))
    }

    pub fn get_response_link(
        &self,
        repository_id: &InfoHash,
    ) -> Option<(LinkId, mpsc::Sender<Response>)> {
        self.active
            .get(repository_id)
            .map(|link| (link.id, link.response_tx.clone()))
    }

    fn destroy_one(&mut self, repository_id: &InfoHash, link_id: Option<LinkId>) {
        // NOTE: this drops the `request_tx` / `response_tx` senders which causes the
        // corresponding receivers to be closed which terminates the client/server tasks.
        if let Entry::Occupied(entry) = self.active.entry(*repository_id) {
            if link_id.is_none() || link_id == Some(entry.get().id) {
                entry.remove();
            }
        }
    }

    fn generate_link_id(&mut self) -> LinkId {
        let id = self.next_link_id;
        self.next_link_id += 1;
        id
    }
}
