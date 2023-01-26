use crate::{error::ErrorCode, registry::Handle, repository::RepositoryHolder, session::State};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt, TryStreamExt};
use ouisync_lib::Result;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    select,
    sync::watch,
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

pub(crate) async fn run_server(state: Arc<State>, status_tx: watch::Sender<ServerStatus>) {
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
                clients.spawn(client(stream, addr, state.clone()));
            }
            Err(error) => {
                status_tx.send(ServerStatus::Failed(error)).ok();
                break;
            }
        }
    }
}

#[instrument(skip(stream, state))]
async fn client(stream: TcpStream, addr: SocketAddr, state: Arc<State>) {
    // Convert to websocket
    let mut socket = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to accept");
            return;
        }
    };

    tracing::debug!("accepted");

    let mut handlers = FuturesUnordered::new();

    loop {
        select! {
            client_envelope = receive(&mut socket) => {
                let Some(client_envelope) = client_envelope else {
                    break;
                };

                handlers.push(handle(&state, client_envelope));
            }
            Some(server_envelope) = handlers.next() => {
                let Some(server_envelope) = server_envelope else {
                    continue;
                };

                send(&mut socket, server_envelope).await;
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

async fn send(socket: &mut Socket, envelope: ServerEnvelope) {
    let buffer = match rmp_serde::to_vec(&envelope) {
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

async fn handle(state: &State, envelope: ClientEnvelope) -> Option<ServerEnvelope> {
    let response = match handle_request(state, envelope.message).await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(?error, "failed to handle request");
            return None;
        }
    };

    Some(ServerEnvelope {
        id: envelope.id,
        message: ServerMessage::Response(response),
    })
}

async fn handle_request(_state: &State, request: Request) -> Result<Response> {
    tracing::debug!(?request);

    todo!()
}

#[derive(Deserialize)]
struct ClientEnvelope {
    id: u64,
    message: Request,
}

#[derive(Debug, Deserialize)]
enum Request {
    CreateRepository,
}

#[derive(Serialize)]
struct ServerEnvelope {
    id: u64,
    message: ServerMessage,
}

#[derive(Serialize)]
enum ServerMessage {
    Response(Response),
    Notification(Notification),
}

#[derive(Serialize)]
enum Response {
    Success(SuccessResponse),
    Failure { code: ErrorCode, message: String },
}

#[derive(Serialize)]
enum SuccessResponse {
    Repository(Handle<RepositoryHolder>),
}

#[derive(Serialize)]
enum Notification {}
