use crate::{
    protocol::{ClientEnvelope, ServerEnvelope, Value},
    request,
    state::{ClientState, ServerState},
};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt, TryStreamExt};
use ouisync_lib::{Error, Result};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc,
    task::JoinSet,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

pub(crate) struct Server {
    listener: TcpListener,
}

impl Server {
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let listener = TcpListener::bind(addr).await.map_err(Error::Interface)?;

        match listener.local_addr() {
            Ok(addr) => tracing::debug!("server bound to {:?}", addr),
            Err(error) => {
                tracing::error!(?error, "failed to retrieve server local address")
            }
        }

        Ok(Self { listener })
    }

    pub(crate) async fn run(self, state: Arc<ServerState>) {
        let mut clients = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    tracing::debug!("client accepted at {:?}", addr);
                    clients.spawn(run_client(stream, state.clone()));
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept client");
                    break;
                }
            }
        }
    }
}

async fn run_client(stream: TcpStream, server_state: Arc<ServerState>) {
    // Convert to websocket
    let mut socket = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to upgrade tcp socket to websocket");
            return;
        }
    };

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
                let envelope = ServerEnvelope::notification(id,notification);
                send(&mut socket, envelope).await;
            }
            Some((id, result)) = request_handlers.next() => {
                let envelope = ServerEnvelope::response(id, result);
                send(&mut socket, envelope).await;
            }
        }
    }
}

type Socket = WebSocketStream<TcpStream>;

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
    let result = request::dispatch(server_state, client_state, envelope.message).await;

    if let Err(error) = &result {
        tracing::error!(?error, "failed to handle request");
    }

    (envelope.id, result)
}
