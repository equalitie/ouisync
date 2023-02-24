use crate::{
    client_message::{self, Request},
    error::{Error, Result},
    server_message::{ServerMessage, Value},
    socket,
    state::{ClientState, ServerState},
};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt, TryStreamExt};
use std::{net::SocketAddr, sync::Arc};
use tokio::{select, sync::mpsc, task::JoinSet};

pub struct Server {
    listener: socket::ws::Listener,
}

impl Server {
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let listener = socket::ws::Listener::bind(addr)
            .await
            .map_err(Error::Bind)?;
        Ok(Self { listener })
    }

    pub async fn run(self, state: Arc<ServerState>) {
        let mut clients = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok(stream) => {
                    let state = state.clone();
                    clients.spawn(async move { run_client(stream, &state).await });
                }
                Err(error) => {
                    tracing::error!(?error, "failed to accept client");
                    break;
                }
            }
        }
    }
}

pub async fn run_client(mut stream: impl socket::Stream, server_state: &ServerState) {
    let (notification_tx, mut notification_rx) = mpsc::channel(1);
    let client_state = ClientState { notification_tx };

    let mut request_handlers = FuturesUnordered::new();

    loop {
        select! {
            message = receive(&mut stream) => {
                let Some((id, result)) = message else {
                    break;
                };

                request_handlers.push(handle_request(server_state, &client_state, id, result));
            }
            notification = notification_rx.recv() => {
                // unwrap is OK because the sender exists at this point.
                let (id, notification) = notification.unwrap();
                let message = ServerMessage::notification(notification);
                send(&mut stream, id, message).await;
            }
            Some((id, result)) = request_handlers.next() => {
                let message = ServerMessage::response(result);
                send(&mut stream, id, message).await;
            }
        }
    }
}

async fn receive(stream: &mut impl socket::Stream) -> Option<(u64, Result<Request>)> {
    loop {
        let buffer = match stream.try_next().await {
            Ok(Some(buffer)) => buffer,
            Ok(None) => {
                tracing::debug!("disconnected");
                return None;
            }
            Err(error) => {
                tracing::error!(?error, "failed to receive client message");
                return None;
            }
        };

        // The message id is encoded separately (big endian u64) followed by the message body
        // (messagepack encoded byte string). This allows us to decode the id even if the rest of
        // the message is malformed so that we can send error response back.

        let Some(id) = buffer.get(..8) else {
            tracing::error!("failed to decode client message id");
            continue;
        };
        let id = u64::from_be_bytes(id.try_into().unwrap());

        let body = rmp_serde::from_slice(&buffer[8..]).map_err(|error| {
            tracing::error!(?error, "failed to decode client message body");
            Error::MalformedRequest(error)
        });

        return Some((id, body));
    }
}

async fn send(stream: &mut impl socket::Stream, id: u64, message: ServerMessage) {
    // Here we encode the id separately only for consistency with `receive`.

    let mut buffer = Vec::new();
    buffer.extend(id.to_be_bytes());

    if let Err(error) = rmp_serde::encode::write(&mut buffer, &message) {
        tracing::error!(?error, "failed to encode server message");
        return;
    };

    if let Err(error) = stream.send(buffer).await {
        tracing::error!(?error, "failed to send server message");
    }
}

async fn handle_request(
    server_state: &ServerState,
    client_state: &ClientState,
    request_id: u64,
    request: Result<Request>,
) -> (u64, Result<Value>) {
    let result = match request {
        Ok(request) => client_message::dispatch(server_state, client_state, request).await,
        Err(error) => Err(error),
    };

    if let Err(error) = &result {
        tracing::error!(?error, "failed to handle request");
    }

    (request_id, result)
}
