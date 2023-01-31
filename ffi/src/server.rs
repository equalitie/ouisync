use crate::{
    protocol::{ClientEnvelope, ServerEnvelope, Value},
    request, socket,
    state::{ClientState, ServerState},
};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt, TryStreamExt};
use ouisync_lib::{Error, Result};
use std::{net::SocketAddr, sync::Arc};
use tokio::{select, sync::mpsc, task::JoinSet};

pub(crate) struct Server {
    listener: socket::ws::Listener,
}

impl Server {
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let listener = socket::ws::Listener::bind(addr)
            .await
            .map_err(Error::Interface)?;
        Ok(Self { listener })
    }

    pub(crate) async fn run(self, state: Arc<ServerState>) {
        let mut clients = JoinSet::new();

        loop {
            match self.listener.accept().await {
                Ok(stream) => {
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

async fn run_client(mut stream: socket::ws::Stream, server_state: Arc<ServerState>) {
    let (notification_tx, mut notification_rx) = mpsc::channel(1);
    let client_state = ClientState { notification_tx };

    let mut request_handlers = FuturesUnordered::new();

    loop {
        select! {
            client_envelope = receive(&mut stream) => {
                let Some(client_envelope) = client_envelope else {
                    break;
                };

                request_handlers.push(handle_request(&server_state, &client_state, client_envelope));
            }
            notification = notification_rx.recv() => {
                // unwrap is OK because the sender exists at this point.
                let (id, notification) = notification.unwrap();
                let envelope = ServerEnvelope::notification(id,notification);
                send(&mut stream, envelope).await;
            }
            Some((id, result)) = request_handlers.next() => {
                let envelope = ServerEnvelope::response(id, result);
                send(&mut stream, envelope).await;
            }
        }
    }
}

async fn receive(stream: &mut socket::ws::Stream) -> Option<ClientEnvelope> {
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

async fn send(stream: &mut socket::ws::Stream, envelope: ServerEnvelope) {
    let buffer = match rmp_serde::to_vec_named(&envelope) {
        Ok(buffer) => buffer,
        Err(error) => {
            tracing::error!(?error, "failed to encode server message");
            return;
        }
    };

    if let Err(error) = stream.send(buffer).await {
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
