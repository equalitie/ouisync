use crate::{
    error::{ErrorCode, ToErrorCode},
    network::{self, NetworkEvent},
    registry::Handle,
    repository::{self, RepositoryHolder},
    session::{self, ServerState, SubscriptionHandle},
    state_monitor,
};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt, TryStreamExt};
use ouisync_lib::{Result, StateMonitor};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    select,
    sync::{mpsc, watch},
    task::JoinSet,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tracing::instrument;

type Socket = WebSocketStream<TcpStream>;

pub(crate) enum ServerStatus {
    Starting,
    Running(SocketAddr),
    Failed(io::Error),
}

pub(crate) async fn run_server(state: Arc<ServerState>, status_tx: watch::Sender<ServerStatus>) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => {
            status_tx.send(ServerStatus::Failed(error)).ok();
            return;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => {
            status_tx.send(ServerStatus::Running(addr)).ok();
            addr
        }
        Err(error) => {
            status_tx.send(ServerStatus::Failed(error)).ok();
            return;
        }
    };

    tracing::debug!(?local_addr, "interface listener started");

    let mut clients = JoinSet::new();

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                clients.spawn(run_client(stream, addr, state.clone()));
            }
            Err(error) => {
                status_tx.send(ServerStatus::Failed(error)).ok();
                break;
            }
        }
    }
}

pub(crate) struct ClientState {
    pub notification_tx: mpsc::Sender<(u64, Notification)>,
}

#[instrument(skip(stream, server_state))]
async fn run_client(stream: TcpStream, addr: SocketAddr, server_state: Arc<ServerState>) {
    // Convert to websocket
    let mut socket = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to accept");
            return;
        }
    };

    tracing::debug!("accepted");

    let (notification_tx, mut notification_rx) = mpsc::channel(1);
    let client_state = ClientState { notification_tx };

    let mut request_handlers = FuturesUnordered::new();

    loop {
        select! {
            client_envelope = receive(&mut socket) => {
                let Some(client_envelope) = client_envelope else {
                    break;
                };

                request_handlers.push(handle_request(&server_state, &client_state, client_envelope));
            }
            notification = notification_rx.recv() => {
                // unwrap is OK because the sender exists at this point.
                let (id, notification) = notification.unwrap();
                send_notification(&mut socket, id, notification).await;
            }
            Some((id, result)) = request_handlers.next() => {
                send_response(&mut socket, id, result).await;
            }
        }
    }
}

async fn receive(socket: &mut Socket) -> Option<ClientEnvelope> {
    loop {
        let message = match socket.try_next().await {
            Ok(Some(message)) => message,
            Ok(None) => {
                tracing::debug!("disconnected");
                return None;
            }
            Err(error) => {
                tracing::error!(?error, "failed to receive client message");
                return None;
            }
        };

        let buffer = match message {
            Message::Binary(buffer) => buffer,
            Message::Text(_)
            | Message::Ping(_)
            | Message::Pong(_)
            | Message::Close(_)
            | Message::Frame(_) => {
                tracing::debug!(?message, "unexpected message type");
                continue;
            }
        };

        let envelope: ClientEnvelope = match rmp_serde::from_slice(&buffer) {
            Ok(envelope) => envelope,
            Err(error) => {
                tracing::error!(?error, "failed to decode client message");
                continue;
            }
        };

        return Some(envelope);
    }
}

async fn send_response(socket: &mut Socket, id: u64, result: Result<Value>) {
    let response = match result {
        Ok(response) => Response::Success(response),
        Err(error) => Response::Failure {
            code: error.to_error_code(),
            message: error.to_string(),
        },
    };

    let envelope = ServerEnvelope {
        id,
        message: ServerMessage::Response(response),
    };

    send(socket, envelope).await;
}

async fn send_notification(socket: &mut Socket, id: u64, notification: Notification) {
    let envelope = ServerEnvelope {
        id,
        message: ServerMessage::Notification(notification),
    };

    send(socket, envelope).await;
}

async fn send(socket: &mut Socket, envelope: ServerEnvelope) {
    let buffer = match rmp_serde::to_vec_named(&envelope) {
        Ok(buffer) => buffer,
        Err(error) => {
            tracing::error!(?error, "failed to encode server message");
            return;
        }
    };

    if let Err(error) = socket.send(Message::Binary(buffer)).await {
        tracing::error!(?error, "failed to send server message");
    }
}

async fn handle_request(
    server_state: &ServerState,
    client_state: &ClientState,
    envelope: ClientEnvelope,
) -> (u64, Result<Value>) {
    let result = dispatch_request(server_state, client_state, envelope.message).await;

    if let Err(error) = &result {
        tracing::error!(?error, "failed to handle request");
    }

    (envelope.id, result)
}

async fn dispatch_request(
    server_state: &ServerState,
    client_state: &ClientState,
    request: Request,
) -> Result<Value> {
    tracing::debug!(?request);

    let response = match request {
        Request::RepositoryCreate {
            path,
            read_password,
            write_password,
            share_token,
        } => repository::create(
            server_state,
            path,
            read_password,
            write_password,
            share_token,
        )
        .await?
        .into(),
        Request::RepositoryOpen { path, password } => {
            repository::open(server_state, path, password).await?.into()
        }
        Request::RepositoryClose(handle) => repository::close(server_state, handle).await?.into(),
        Request::RepositorySubscribe(handle) => {
            repository::subscribe(server_state, client_state, handle).into()
        }
        Request::RepositorySetReadAccess {
            repository,
            read_password,
            share_token,
        } => repository::set_read_access(server_state, repository, read_password, share_token)
            .await?
            .into(),
        Request::RepositorySetReadAndWriteAccess {
            repository,
            old_password,
            new_password,
            share_token,
        } => repository::set_read_and_write_access(
            server_state,
            repository,
            old_password,
            new_password,
            share_token,
        )
        .await?
        .into(),
        Request::RepositoryRemoveReadKey(handle) => {
            repository::remove_read_key(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRemoveWriteKey(handle) => {
            repository::remove_write_key(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRequiresLocalPasswordForReading(handle) => {
            repository::requires_local_password_for_reading(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRequiresLocalPasswordForWriting(handle) => {
            repository::requires_local_password_for_writing(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryInfoHash(handle) => repository::info_hash(server_state, handle).into(),
        Request::RepositoryDatabaseId(handle) => {
            repository::database_id(server_state, handle).await?.into()
        }
        Request::RepositoryEntryType { repository, path } => {
            repository::entry_type(server_state, repository, path)
                .await?
                .into()
        }
        Request::RepositoryMoveEntry {
            repository,
            src,
            dst,
        } => repository::move_entry(server_state, repository, src, dst)
            .await?
            .into(),
        Request::NetworkSubscribe => network::subscribe(server_state, client_state).into(),
        Request::NetworkBind {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        } => network::bind(server_state, quic_v4, quic_v6, tcp_v4, tcp_v6)
            .await?
            .into(),
        Request::StateMonitorGet(path) => state_monitor::get(server_state, path)?.into(),
        Request::StateMonitorSubscribe(path) => {
            state_monitor::subscribe(server_state, client_state, path)?.into()
        }
        Request::Unsubscribe(handle) => {
            session::unsubscribe(server_state, handle);
            ().into()
        }
    };

    Ok(response)
}

#[derive(Deserialize)]
struct ClientEnvelope {
    id: u64,
    #[serde(flatten)]
    message: Request,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "args")]
enum Request {
    RepositoryCreate {
        path: String,
        read_password: Option<String>,
        write_password: Option<String>,
        share_token: Option<String>,
    },
    RepositoryOpen {
        path: String,
        password: Option<String>,
    },
    RepositoryClose(Handle<RepositoryHolder>),
    RepositorySubscribe(Handle<RepositoryHolder>),
    RepositorySetReadAccess {
        repository: Handle<RepositoryHolder>,
        read_password: Option<String>,
        share_token: Option<String>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        old_password: Option<String>,
        new_password: Option<String>,
        share_token: Option<String>,
    },
    RepositoryRemoveReadKey(Handle<RepositoryHolder>),
    RepositoryRemoveWriteKey(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForReading(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForWriting(Handle<RepositoryHolder>),
    RepositoryInfoHash(Handle<RepositoryHolder>),
    RepositoryDatabaseId(Handle<RepositoryHolder>),
    RepositoryEntryType {
        repository: Handle<RepositoryHolder>,
        path: String,
    },
    RepositoryMoveEntry {
        repository: Handle<RepositoryHolder>,
        src: String,
        dst: String,
    },
    NetworkSubscribe,
    NetworkBind {
        quic_v4: Option<String>,
        quic_v6: Option<String>,
        tcp_v4: Option<String>,
        tcp_v6: Option<String>,
    },
    StateMonitorGet(String),
    StateMonitorSubscribe(String),
    Unsubscribe(SubscriptionHandle),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ServerEnvelope {
    id: u64,
    #[serde(flatten)]
    message: ServerMessage,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
enum ServerMessage {
    Response(Response),
    Notification(Notification),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Response {
    Success(Value),
    Failure { code: ErrorCode, message: String },
}

#[derive(Serialize)]
#[serde(untagged)]
enum Value {
    Unit,
    Bool(bool),
    U8(u8),
    Bytes(Vec<u8>),
    String(String),
    Repository(Handle<RepositoryHolder>),
    Subscription(SubscriptionHandle),
    StateMonitor(StateMonitor),
}

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Self::Unit
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Self::U8(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Handle<RepositoryHolder>> for Value {
    fn from(value: Handle<RepositoryHolder>) -> Self {
        Self::Repository(value)
    }
}

impl From<SubscriptionHandle> for Value {
    fn from(value: SubscriptionHandle) -> Self {
        Self::Subscription(value)
    }
}

impl From<StateMonitor> for Value {
    fn from(value: StateMonitor) -> Self {
        Self::StateMonitor(value)
    }
}

#[derive(Serialize)]
pub(crate) enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}
