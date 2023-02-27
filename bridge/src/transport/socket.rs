//! Low-level Client and Server than wrap Stream/Sink of bytes. Used to implement some higher-level
//! clients/servers

use super::Client;
use crate::{
    error::{Error, ErrorCode, Result},
    protocol::{self, Request, Response, ServerMessage},
    state::{ClientState, ServerState},
};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures_util::{stream::FuturesUnordered, Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use serde::{de::DeserializeOwned, Serialize};
use std::{collections::HashMap, io, marker::PhantomData, sync::Arc};
use tokio::{
    select,
    sync::{mpsc, oneshot},
    task,
};

pub mod server_connection {
    use super::*;

    pub async fn run<T>(mut socket: T, server_state: Arc<ServerState>)
    where
        T: Stream<Item = io::Result<BytesMut>> + Sink<Bytes, Error = io::Error> + Unpin,
    {
        let (notification_tx, mut notification_rx) = mpsc::channel(1);
        let client_state = ClientState { notification_tx };

        let mut request_handlers = FuturesUnordered::new();

        loop {
            select! {
                message = receive(&mut socket) => {
                    let Some((id, result)) = message else {
                        break;
                    };

                    request_handlers.push(handle_request(&server_state, &client_state, id, result));
                }
                notification = notification_rx.recv() => {
                    // unwrap is OK because the sender exists at this point.
                    let (id, notification) = notification.unwrap();
                    let message = ServerMessage::notification(notification);
                    send(&mut socket, id, message).await;
                }
                Some((id, result)) = request_handlers.next() => {
                    let message = ServerMessage::response(result);
                    send(&mut socket, id, message).await;
                }
            }
        }
    }

    async fn handle_request(
        server_state: &ServerState,
        client_state: &ClientState,
        request_id: u64,
        request: Result<Request>,
    ) -> (u64, Result<Response>) {
        let result = match request {
            Ok(request) => protocol::dispatch(server_state, client_state, request).await,
            Err(error) => Err(error),
        };

        if let Err(error) = &result {
            tracing::error!(?error, "failed to handle request");
        }

        (request_id, result)
    }
}

pub struct SocketClient<R, W> {
    request_tx: mpsc::Sender<(Request, oneshot::Sender<Result<Response>>)>,
    _reader: PhantomData<R>,
    _writer: PhantomData<W>,
}

impl<R, W> SocketClient<R, W>
where
    R: Stream<Item = io::Result<BytesMut>> + Unpin + Send + 'static,
    W: Sink<Bytes, Error = io::Error> + Unpin + Send + 'static,
{
    pub fn new(reader: R, writer: W) -> Self {
        let (request_tx, request_rx) = mpsc::channel(1);

        task::spawn(Worker::new(request_rx, reader, writer).run());

        Self {
            request_tx,
            _reader: PhantomData,
            _writer: PhantomData,
        }
    }
}

#[async_trait(?Send)]
impl<R, W> Client for SocketClient<R, W>
where
    R: Stream<Item = io::Result<BytesMut>> + Unpin + Send + 'static,
    W: Sink<Bytes, Error = io::Error> + Unpin + Send + 'static,
{
    async fn invoke(&self, request: Request) -> Result<Response> {
        let (response_tx, response_rx) = oneshot::channel();

        self.request_tx
            .send((request, response_tx))
            .await
            .map_err(|_| Error::ConnectionLost)?;

        match response_rx.await.map_err(|_| Error::ConnectionLost) {
            Ok(result) => result,
            Err(error) => Err(error),
        }
    }
}

struct Worker<R, W> {
    running: bool,
    request_rx: mpsc::Receiver<(Request, oneshot::Sender<Result<Response>>)>,
    reader: R,
    writer: W,
    pending_requests: HashMap<u64, oneshot::Sender<Result<Response>>>,
    next_message_id: u64,
}

impl<R, W> Worker<R, W>
where
    R: Stream<Item = io::Result<BytesMut>> + Unpin + Send,
    W: Sink<Bytes, Error = io::Error> + Unpin + Send,
{
    fn new(
        request_rx: mpsc::Receiver<(Request, oneshot::Sender<Result<Response>>)>,
        reader: R,
        writer: W,
    ) -> Self {
        Self {
            running: true,
            request_rx,
            reader,
            writer,
            pending_requests: HashMap::new(),
            next_message_id: 0,
        }
    }

    async fn run(mut self) {
        while self.running {
            select! {
                request = self.request_rx.recv() => self.handle_request(request).await,
                response = receive(&mut self.reader) => self.handle_server_message(response).await,
            }
        }
    }

    async fn handle_request(
        &mut self,
        request: Option<(Request, oneshot::Sender<Result<Response>>)>,
    ) {
        let Some((request, response_tx)) = request else {
            tracing::debug!("client closed");
            self.running = false;
            return;
        };

        let message_id = self.next_message_id;
        self.next_message_id = self.next_message_id.wrapping_add(1);
        self.pending_requests.insert(message_id, response_tx);

        if !send(&mut self.writer, message_id, request).await {
            self.running = false;
        }
    }

    async fn handle_server_message(&mut self, message: Option<(u64, Result<ServerMessage>)>) {
        let Some((message_id, message)) = message else {
            self.running = false;
            return;
        };

        let Some(response_tx) = self.pending_requests.remove(&message_id) else {
            tracing::debug!("unsolicited response");
            return;
        };

        let response = match message {
            Ok(ServerMessage::Success(response)) => Ok(response),
            Ok(ServerMessage::Failure { code, message }) => {
                Err(Error::RequestFailed { code, message })
            }
            Ok(ServerMessage::Notification(_)) => Err(Error::RequestFailed {
                code: ErrorCode::OperationNotSupported,
                message: "notifications not supported yet".to_owned(),
            }),
            Err(error) => Err(error),
        };

        response_tx.send(response).ok();
    }
}

async fn receive<R, M>(reader: &mut R) -> Option<(u64, Result<M>)>
where
    R: Stream<Item = io::Result<BytesMut>> + Unpin,
    M: DeserializeOwned,
{
    loop {
        let buffer = match reader.try_next().await {
            Ok(Some(buffer)) => buffer,
            Ok(None) => {
                tracing::debug!("disconnected");
                return None;
            }
            Err(error) => {
                tracing::error!(?error, "failed to receive message");
                return None;
            }
        };

        // The message id is encoded separately (big endian u64) followed by the message body
        // (messagepack encoded byte string). This allows us to decode the id even if the rest of
        // the message is malformed so that we can send error response back.

        let Some(id) = buffer.get(..8) else {
            tracing::error!("failed to decode message id");
            continue;
        };
        let id = u64::from_be_bytes(id.try_into().unwrap());

        let body = rmp_serde::from_slice(&buffer[8..]).map_err(|error| {
            tracing::error!(?error, "failed to decode message body");
            Error::MalformedRequest(error)
        });

        return Some((id, body));
    }
}

async fn send<W, M>(writer: &mut W, id: u64, message: M) -> bool
where
    W: Sink<Bytes, Error = io::Error> + Unpin,
    M: Serialize,
{
    // Here we encode the id separately only for consistency with `receive`.

    let mut buffer = Vec::new();
    buffer.extend(id.to_be_bytes());

    if let Err(error) = rmp_serde::encode::write(&mut buffer, &message) {
        tracing::error!(?error, "failed to encode message");
        return false;
    };

    if let Err(error) = writer.send(buffer.into()).await {
        tracing::error!(?error, "failed to send message");
        return false;
    }

    true
}
