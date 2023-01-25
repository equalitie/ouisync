use super::session::State;
use futures_util::{SinkExt, TryStreamExt};
use ouisync_lib::Result;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinSet,
};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;

pub(crate) enum ListenerStatus {
    Starting,
    Running(SocketAddr),
    Failed(io::Error),
}

pub(crate) async fn run(state: Arc<State>, status_tx: watch::Sender<ListenerStatus>) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => {
            status_tx.send(ListenerStatus::Failed(error)).ok();
            return;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => {
            status_tx.send(ListenerStatus::Running(addr)).ok();
            addr
        }
        Err(error) => {
            status_tx.send(ListenerStatus::Failed(error)).ok();
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
                status_tx.send(ListenerStatus::Failed(error)).ok();
                break;
            }
        }
    }
}

#[instrument(skip(stream, state))]
async fn client(stream: TcpStream, addr: SocketAddr, state: Arc<State>) {
    // Convert to websocket
    let mut stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(?error, "failed to accept");
            return;
        }
    };

    tracing::debug!("accepted");

    loop {
        match stream.try_next().await {
            Ok(Some(message)) => match message {
                Message::Binary(mut payload) => {
                    let request: Request = match rmp_serde::from_slice(&payload) {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::error!(?error, "failed to decode request");
                            continue;
                        }
                    };

                    let response = match handle_request(&state, request).await {
                        Ok(response) => response,
                        Err(error) => {
                            tracing::error!(?error, "failed to handle request");
                            continue;
                        }
                    };

                    // Reuse the buffer
                    payload.clear();

                    if let Err(error) = rmp_serde::encode::write(&mut payload, &response) {
                        tracing::error!(?error, "failed to encode response");
                        continue;
                    }

                    if let Err(error) = stream.send(Message::Binary(payload)).await {
                        tracing::error!(?error, "failed to send response");
                    }
                }
                Message::Text(_)
                | Message::Ping(_)
                | Message::Pong(_)
                | Message::Close(_)
                | Message::Frame(_) => {
                    tracing::debug!(?message, "unexpected request message type");
                }
            },
            Ok(None) => {
                tracing::debug!("disconnected");
                break;
            }
            Err(error) => {
                tracing::error!(?error, "failed to receive request");
                break;
            }
        }
    }
}

async fn handle_request(state: &State, request: Request) -> Result<Response> {
    todo!()
}

#[derive(Deserialize)]
enum Request {}

#[derive(Serialize)]
enum Response {}
