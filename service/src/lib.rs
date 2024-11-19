pub mod protocol;
pub mod transport;

use futures_util::SinkExt;
use protocol::{DecodeError, Message, Request, Response, ServerError, ServerPayload};
use slab::Slab;
use std::{io, path::PathBuf};
use tokio::select;
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use tracing::instrument;
use transport::{LocalServer, LocalServerReader, LocalServerWriter, ReadError};

pub struct Service {
    local_server: LocalServer,
    local_readers: StreamMap<ConnectionId, StreamNotifyClose<LocalServerReader>>,
    local_writers: Slab<LocalServerWriter>,
}

impl Service {
    pub async fn init(local_socket_path: PathBuf, config_dir: PathBuf) -> io::Result<Self> {
        let local_server = LocalServer::bind(&local_socket_path).await?;

        Ok(Self {
            local_server,
            local_readers: StreamMap::new(),
            local_writers: Slab::new(),
        })
    }

    pub async fn run(&mut self) -> io::Result<()> {
        loop {
            select! {
                result = self.local_server.accept() => {
                    let (reader, writer) = result?;
                    self.insert_local_connection(reader, writer);
                }
                Some((conn_id, message)) = self.local_readers.next() => {
                    if let Some(message) = message {
                        self.handle_message(conn_id, message).await
                    } else {
                        self.remove_local_connection(conn_id)
                    }
                }
            }
        }
    }

    pub async fn close(&mut self) -> io::Result<()> {
        todo!()
    }

    #[instrument(skip(self))]
    async fn handle_message(
        &mut self,
        conn_id: ConnectionId,
        message: Result<Message<Request>, ReadError>,
    ) {
        match message {
            Ok(message) => {
                let id = message.id;
                let payload = self.dispatch_message(conn_id, message).await.into();
                let message = Message { id, payload };

                self.send_message(conn_id, message).await;
            }
            Err(ReadError::Receive(error)) => {
                tracing::error!(?error, "failed to receive message");
                self.remove_local_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Id(error))) => {
                tracing::error!(?error, "failed to decode message id");
                self.remove_local_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Payload(id, error))) => {
                let message = Message {
                    id,
                    payload: ServerPayload::Failure(error.into()),
                };
                self.send_message(conn_id, message).await;
            }
        }
    }

    async fn dispatch_message(
        &mut self,
        conn_id: ConnectionId,
        message: Message<Request>,
    ) -> Result<Response, ServerError> {
        todo!()
    }

    async fn send_message(&mut self, conn_id: ConnectionId, message: Message<ServerPayload>) {
        let Some(writer) = self.local_writers.get_mut(conn_id) else {
            tracing::error!("connection not found");
            return;
        };

        match writer.send(message).await {
            Ok(()) => (),
            Err(error) => {
                tracing::error!(?error, "failed to send message");
                self.remove_local_connection(conn_id);
            }
        }
    }

    fn insert_local_connection(&mut self, reader: LocalServerReader, writer: LocalServerWriter) {
        let conn_id = self.local_writers.insert(writer);
        self.local_readers
            .insert(conn_id, StreamNotifyClose::new(reader));
    }

    fn remove_local_connection(&mut self, conn_id: ConnectionId) {
        self.local_readers.remove(&conn_id);
        self.local_writers.try_remove(conn_id);
    }
}

type ConnectionId = usize;
