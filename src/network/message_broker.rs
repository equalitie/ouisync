use super::{
    client::Client,
    message::{Message, Request, Response},
    object_stream::{TcpObjectReader, TcpObjectStream, TcpObjectWriter},
    server::Server,
};
use crate::{
    error::Result,
    index::Index,
    replica_id::ReplicaId,
    repository::RepositoryId,
    scoped_task::ScopedJoinHandle,
    tagged::{Local, Remote},
};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    future::Future,
    pin::Pin,
};
use tokio::{
    select,
    sync::mpsc::{self, error::SendError},
    task,
};

/// A stream for receiving Requests and sending Responses
pub(crate) struct ServerStream {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Request>,
    remote_id: Remote<RepositoryId>,
}

impl ServerStream {
    pub(super) fn new(
        tx: mpsc::Sender<Command>,
        rx: mpsc::Receiver<Request>,
        remote_id: Remote<RepositoryId>,
    ) -> Self {
        Self { tx, rx, remote_id }
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
                dst_id: self.remote_id.into_inner(),
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
    remote_id: Remote<RepositoryId>,
}

impl ClientStream {
    pub(super) fn new(
        tx: mpsc::Sender<Command>,
        rx: mpsc::Receiver<Response>,
        remote_id: Remote<RepositoryId>,
    ) -> Self {
        Self { tx, rx, remote_id }
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
                dst_id: self.remote_id.into_inner(),
                request,
            }))
            .await
            .map_err(|e| SendError(into_message(e.0)))
    }
}

fn into_message<T: From<Message>>(command: Command) -> T {
    command.into_send_message().into()
}

type OnFinish = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Maintains one or more connections to a peer, listening on all of them at the same time. Note
/// that at the present all the connections are TCP based and so dropping some of them would make
/// sense. However, in the future we may also have other transports (e.g. Bluetooth) and thus
/// keeping all may make sence because even if one is dropped, the others may still function.
///
/// Once a message is received, it is determined whether it is a request or a response. Based on
/// that it either goes to the ClientStream or ServerStream for processing by the Client and Server
/// structures respectively.
pub(crate) struct MessageBroker {
    command_tx: mpsc::Sender<Command>,
    _join_handle: ScopedJoinHandle<()>,
}

impl MessageBroker {
    pub async fn new(
        their_replica_id: ReplicaId,
        stream: TcpObjectStream,
        on_finish: OnFinish,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1);

        let mut inner = Inner {
            their_replica_id,
            command_tx: command_tx.clone(),
            reader: MultiReader::new(),
            writer: MultiWriter::new(),
            links: HashMap::new(),
            pending_outgoing_links: HashMap::new(),
            pending_incoming_links: HashMap::new(),
        };

        inner.add_connection(stream);

        let handle = task::spawn(inner.run(command_rx, on_finish));

        Self {
            command_tx,
            _join_handle: ScopedJoinHandle(handle),
        }
    }

    pub async fn add_connection(&self, stream: TcpObjectStream) {
        self.send_command(Command::AddConnection(stream)).await
    }

    /// Try to establish a link between a local repository and a remote repository. The remote
    /// counterpart needs to call this too with matching `local_name` and `remote_name` for the link
    /// to actually be created.
    pub async fn create_link(
        &self,
        index: Index,
        local_id: Local<RepositoryId>,
        local_name: Local<String>,
        remote_name: Remote<String>,
    ) {
        self.send_command(Command::CreateLink {
            index,
            local_id,
            local_name,
            remote_name,
        })
        .await
    }

    /// Destroy the link between a local repository with the specified id and its remote
    /// counterpart (if one exists).
    pub async fn destroy_link(&self, local_id: Local<RepositoryId>) {
        self.send_command(Command::DestroyLink { local_id }).await
    }

    async fn send_command(&self, command: Command) {
        if let Err(command) = self.command_tx.send(command).await {
            log::error!(
                "failed to send command {:?} - broker already finished",
                command
            );
        }
    }
}

struct Inner {
    their_replica_id: ReplicaId,
    command_tx: mpsc::Sender<Command>,
    reader: MultiReader,
    writer: MultiWriter,
    links: HashMap<Local<RepositoryId>, Link>,

    // TODO: consider using LruCache instead of HashMap for these, to expire unrequited link
    //       requests.
    pending_outgoing_links: HashMap<Local<String>, PendingOutgoingLink>,
    pending_incoming_links: HashMap<Local<String>, PendingIncomingLink>,
}

impl Inner {
    async fn run(mut self, mut command_rx: mpsc::Receiver<Command>, on_finish: OnFinish) {
        let mut run = true;

        while run {
            run = select! {
                command = command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command).await
                    } else {
                        false
                    }
                }
                message = self.reader.read() => {
                    if let Some(message) = message {
                        self.handle_message(message).await
                    } else {
                        false
                    }
                }
            }
        }

        on_finish.await
    }

    async fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::AddConnection(stream) => {
                self.add_connection(stream);
                true
            }
            Command::SendMessage(message) => self.send_message(message).await,
            Command::CreateLink {
                index,
                local_id,
                local_name,
                remote_name,
            } => {
                self.create_outgoing_link(index, local_id, local_name, remote_name)
                    .await
            }
            Command::DestroyLink { local_id } => {
                self.destroy_link(local_id);
                true
            }
        }
    }

    async fn handle_message(&mut self, message: Message) -> bool {
        match message {
            Message::Request { dst_id, request } => {
                self.handle_request(Local::new(dst_id), request).await
            }
            Message::Response { dst_id, response } => {
                self.handle_response(Local::new(dst_id), response).await
            }
            Message::CreateLink { src_id, dst_name } => {
                self.create_incoming_link(Local::new(dst_name), Remote::new(src_id))
                    .await;
                true
            }
        }
    }

    fn add_connection(&mut self, stream: TcpObjectStream) {
        let (reader, writer) = stream.into_split();
        self.reader.add(reader);
        self.writer.add(writer);
    }

    async fn send_message(&mut self, message: Message) -> bool {
        self.writer.write(&message).await
    }

    async fn create_outgoing_link(
        &mut self,
        index: Index,
        local_id: Local<RepositoryId>,
        local_name: Local<String>,
        remote_name: Remote<String>,
    ) -> bool {
        if self.links.contains_key(&local_id) {
            log::warn!(
                "not creating link from {:?} ({:?}) - already created",
                local_id,
                local_name,
            );
            return true;
        }

        if self.pending_outgoing_links.contains_key(&local_name) {
            log::warn!(
                "not creating link from {:?} ({:?}) - already pending",
                local_id,
                local_name,
            );
            return true;
        }

        if let Some(pending) = self.pending_incoming_links.remove(&local_name) {
            self.create_link(index, local_id, pending.remote_id).await
        } else {
            if !self
                .writer
                .write(&Message::CreateLink {
                    src_id: local_id.into_inner(),
                    dst_name: remote_name.into_inner(),
                })
                .await
            {
                log::warn!(
                    "not creating link from {:?} ({:?}) - \
                     failed to send CreateLink message - all writers closed",
                    local_id,
                    local_name,
                );
                return false;
            }

            self.pending_outgoing_links
                .insert(local_name, PendingOutgoingLink { index, local_id });
        }

        true
    }

    async fn create_incoming_link(
        &mut self,
        local_name: Local<String>,
        remote_id: Remote<RepositoryId>,
    ) {
        if let Some(pending) = self.pending_outgoing_links.remove(&local_name) {
            self.create_link(pending.index, pending.local_id, remote_id)
                .await
        } else {
            self.pending_incoming_links
                .insert(local_name, PendingIncomingLink { remote_id });
        }
    }

    async fn create_link(
        &mut self,
        index: Index,
        local_id: Local<RepositoryId>,
        remote_id: Remote<RepositoryId>,
    ) {
        log::debug!("creating link from {:?} to {:?}", local_id, remote_id);

        let (request_tx, request_rx) = mpsc::channel(1);
        let (response_tx, response_rx) = mpsc::channel(1);

        self.links.insert(
            local_id,
            Link {
                request_tx,
                response_tx,
            },
        );

        // NOTE: we just fire-and-forget the tasks which should be OK because when this
        // `MessageBroker` instance is dropped, the associated senders (`request_tx`, `response_tx`)
        // are dropped as well which closes the corresponding receivers which then teminates the
        // tasks.

        let client_stream = ClientStream::new(self.command_tx.clone(), response_rx, remote_id);
        let mut client = Client::new(index.clone(), self.their_replica_id, client_stream);
        task::spawn(async move { log_error(client.run(), "client failed: ").await });

        let server_stream = ServerStream::new(self.command_tx.clone(), request_rx, remote_id);
        let mut server = Server::new(index, server_stream).await;
        task::spawn(async move { log_error(server.run(), "server failed: ").await });
    }

    fn destroy_link(&mut self, local_id: Local<RepositoryId>) {
        // NOTE: this dropps the `request_tx` / `response_tx` senders which causes the
        // corresponding receivers to be closed which terminates the client/server tasks.
        self.links.remove(&local_id);
    }

    async fn handle_request(&mut self, local_id: Local<RepositoryId>, request: Request) -> bool {
        match self.links.entry(local_id) {
            Entry::Occupied(entry) => {
                if entry.get().request_tx.send(request).await.is_err() {
                    log::warn!("server unexpectedly terminated - destroying the link");
                    entry.remove();
                }
            }
            Entry::Vacant(_) => {
                log::warn!(
                    "received request {:?} for unlinked repository {:?}",
                    request,
                    local_id
                );
            }
        }

        !self.links.is_empty()
    }

    async fn handle_response(&mut self, local_id: Local<RepositoryId>, response: Response) -> bool {
        match self.links.entry(local_id) {
            Entry::Occupied(entry) => {
                if entry.get().response_tx.send(response).await.is_err() {
                    log::warn!("client unexpectedly terminated - destroying the link");
                    entry.remove();
                }
            }
            Entry::Vacant(_) => {
                log::warn!(
                    "received response {:?} for unlinked repository {:?}",
                    response,
                    local_id
                );
            }
        }

        !self.links.is_empty()
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
    AddConnection(TcpObjectStream),
    SendMessage(Message),
    CreateLink {
        index: Index,
        local_id: Local<RepositoryId>,
        local_name: Local<String>,
        remote_name: Remote<String>,
    },
    DestroyLink {
        local_id: Local<RepositoryId>,
    },
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
            Self::AddConnection(_) => f.debug_tuple("AddConnection").field(&"_").finish(),
            Self::SendMessage(message) => f.debug_tuple("SendMessage").field(message).finish(),
            Self::CreateLink {
                local_id,
                local_name,
                remote_name,
                ..
            } => f
                .debug_struct("CreateLink")
                .field("local_id", local_id)
                .field("local_name", local_name)
                .field("remote_name", remote_name)
                .finish_non_exhaustive(),
            Self::DestroyLink { local_id } => f
                .debug_struct("DestroyLink")
                .field("local_id", local_id)
                .finish(),
        }
    }
}

/// Wrapper for arbitrary number of `TcpObjectReader`s which reads from all of them simultaneously.
struct MultiReader {
    tx: mpsc::Sender<Option<Message>>,
    rx: mpsc::Receiver<Option<Message>>,
    count: usize,
}

impl MultiReader {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self { tx, rx, count: 0 }
    }

    fn add(&mut self, mut reader: TcpObjectReader) {
        let tx = self.tx.clone();
        self.count += 1;

        task::spawn(async move {
            loop {
                select! {
                    result = reader.read() => {
                        if let Ok(message) = result {
                            tx.send(Some(message)).await.unwrap_or(())
                        } else {
                            tx.send(None).await.unwrap_or(());
                            break;
                        }
                    },
                    _ = tx.closed() => break,
                }
            }
        });
    }

    async fn read(&mut self) -> Option<Message> {
        loop {
            if self.count == 0 {
                return None;
            }

            match self.rx.recv().await {
                Some(Some(message)) => return Some(message),
                Some(None) => {
                    self.count -= 1;
                }
                None => {
                    // This would mean that all senders were closed, but that can't happen because
                    // `self.tx` still exists.
                    unreachable!()
                }
            }
        }
    }
}

/// Wrapper for arbitrary number of `TcpObjectWriter`s which writes to the first available one.
struct MultiWriter {
    writers: Vec<TcpObjectWriter>,
}

impl MultiWriter {
    fn new() -> Self {
        Self {
            writers: Vec::new(),
        }
    }

    fn add(&mut self, writer: TcpObjectWriter) {
        self.writers.push(writer)
    }

    async fn write(&mut self, message: &Message) -> bool {
        while let Some(writer) = self.writers.last_mut() {
            if writer.write(message).await.is_ok() {
                return true;
            }

            self.writers.pop();
        }

        false
    }
}

// Established link between local and remote repositories.
struct Link {
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
}

// Pending link initiated by the local repository.
struct PendingOutgoingLink {
    local_id: Local<RepositoryId>,
    index: Index,
}

// Pending link initiated by the remote repository.
struct PendingIncomingLink {
    remote_id: Remote<RepositoryId>,
}
