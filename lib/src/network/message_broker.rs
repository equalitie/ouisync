use super::{
    client::Client,
    connection::ConnectionPermit,
    crypto::{self, DecryptingStream, EncryptingSink, Role},
    message::{Content, MessageChannel, Request, Response},
    message_dispatcher::{ContentSink, ContentStream, MessageDispatcher},
    protocol::RuntimeId,
    server::Server,
};
use crate::{index::Index, repository::RepositoryId};
use std::collections::{hash_map::Entry, HashMap};
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
                _ = maintain_link(role, channel, stream, sink, index) => (),
                _ = abort_rx => (),
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
        match crypto::establish_channel(role, index.repository_id(), &mut stream, &mut sink).await {
            Ok((stream, sink)) => {
                run_link(channel, stream, sink, &index).await;
            }
            Err(error) => {
                log::warn!(
                    "failed to establish encrypted channel for {:?}: {}",
                    channel,
                    error
                );
            }
        };

        // HACK: `establish_channel` sometimes returns immediatelly, so we need to yield explicitly
        // here otherwise we could starve the runtime.
        task::yield_now().await;
    }
}

async fn run_link(
    channel: MessageChannel,
    stream: DecryptingStream<'_>,
    sink: EncryptingSink<'_>,
    index: &Index,
) {
    let (request_tx, request_rx) = mpsc::channel(1);
    let (response_tx, response_rx) = mpsc::channel(1);
    let (content_tx, content_rx) = mpsc::channel(1);

    // Run everything in parallel:
    select! {
        _ = run_client(channel, index.clone(), content_tx.clone(), response_rx) => (),
        _ = run_server(channel, index.clone(), content_tx, request_rx ) => (),
        _ = recv_messages(stream, request_tx, response_tx) => (),
        _ = send_messages(content_rx, sink) => (),
    }
}

// Handle incoming messages
async fn recv_messages(
    mut stream: DecryptingStream<'_>,
    request_tx: mpsc::Sender<Request>,
    response_tx: mpsc::Sender<Response>,
) {
    while let Ok(content) = stream.recv().await {
        let content: Content = match bincode::deserialize(&content) {
            Ok(content) => content,
            Err(error) => {
                log::warn!(
                    "failed to deserialize message for {:?}: {}",
                    stream.channel(),
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

    log::debug!("message stream for {:?} closed", stream.channel())
}

// Handle outgoing messages
async fn send_messages(mut content_rx: mpsc::Receiver<Content>, mut sink: EncryptingSink<'_>) {
    while let Some(content) = content_rx.recv().await {
        // unwrap is OK because serialization into a vec should never fail unless we have a bug
        // somewhere.
        let content = bincode::serialize(&content).unwrap();

        if sink.send(content).await.is_err() {
            break;
        }
    }

    log::debug!("message sink for {:?} closed", sink.channel())
}

// Create and run client
async fn run_client(
    channel: MessageChannel,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    response_rx: mpsc::Receiver<Response>,
) {
    let client_stream = ClientStream::new(content_tx, response_rx);
    let mut client = Client::new(index, client_stream);

    match client.run().await {
        Ok(()) => log::debug!("client for {:?} terminated", channel),
        Err(error) => log::error!("client for {:?} failed: {:?}", channel, error),
    }
}

// Create and run server
async fn run_server(
    channel: MessageChannel,
    index: Index,
    content_tx: mpsc::Sender<Content>,
    request_rx: mpsc::Receiver<Request>,
) {
    let server_stream = ServerStream::new(content_tx, request_rx);
    let mut server = Server::new(index, server_stream);

    match server.run().await {
        Ok(()) => log::debug!("server for {:?} terminated", channel),
        Err(error) => log::error!("server for {:?} failed: {:?}", channel, error),
    }
}
