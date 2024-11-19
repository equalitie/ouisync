pub mod protocol;
pub mod transport;

mod error;
mod metrics;
mod state;

pub use error::ServiceError;
use metrics::MetricsServer;
pub use state::StateError;

use futures_util::SinkExt;
use protocol::{DecodeError, Message, ProtocolError, Request, Response, ServerPayload};
use slab::Slab;
use state::State;
use std::path::PathBuf;
use tokio::select;
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use tracing::instrument;
use transport::{LocalServer, LocalServerReader, LocalServerWriter, ReadError};

pub struct Service {
    state: State,
    local_server: LocalServer,
    local_readers: StreamMap<ConnectionId, StreamNotifyClose<LocalServerReader>>,
    local_writers: Slab<LocalServerWriter>,
    metrics_server: MetricsServer,
}

impl Service {
    pub async fn init(
        local_socket_path: PathBuf,
        config_dir: PathBuf,
    ) -> Result<Self, ServiceError> {
        let state = State::init(&config_dir).await?;
        let local_server = LocalServer::bind(&local_socket_path).await?;

        let metrics_server = MetricsServer::init(&state).await?;

        Ok(Self {
            state,
            local_server,
            local_readers: StreamMap::new(),
            local_writers: Slab::new(),
            metrics_server,
        })
    }

    pub async fn run(&mut self) -> Result<(), ServiceError> {
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

    pub async fn close(&mut self) -> Result<(), ServiceError> {
        self.metrics_server.close();
        self.state.close().await?;

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
    ) -> Result<Response, ProtocolError> {
        match message.payload {
            Request::RemoteControlBind { addrs: _ } => todo!(),
            Request::MetricsBind { addr } => {
                Ok(self.metrics_server.bind(&self.state, addr).await?.into())
            }
            Request::RepositoryCreate {
                name,
                read_secret,
                write_secret,
                share_token,
            } => {
                todo!()
            }
        }
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
