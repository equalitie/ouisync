use std::{borrow::Cow, sync::Arc};

use ouisync::{crypto::sign::Signature, RepositoryId, WriteSecrets};

use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt, TryStreamExt,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::rustls;
use tokio_tungstenite::{tungstenite as ws, Connector, MaybeTlsStream, WebSocketStream};

use crate::{
    protocol::{
        Message, MessageId, ProtocolError, RepositoryHandle, Response, ServerPayload,
        UnexpectedResponse,
    },
    transport::{ReadError, WriteError},
};

use super::{extract_session_cookie, protocol, RemoteSocket};

pub struct RemoteClient {
    reader: RemoteSocket<ServerPayload, SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
    writer: RemoteSocket<
        protocol::Request,
        SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, ws::Message>,
    >,
    session_cookie: [u8; 32],
}

impl RemoteClient {
    pub async fn connect(
        host: &str,
        config: Arc<rustls::ClientConfig>,
    ) -> Result<Self, RemoteClientError> {
        let host = if host.contains("://") {
            Cow::Borrowed(host)
        } else {
            Cow::Owned(format!("wss://{host}"))
        };

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            host.as_ref(),
            None,
            false,
            Some(Connector::Rustls(config)),
        )
        .await?;

        let session_cookie = match stream.get_ref() {
            MaybeTlsStream::Rustls(stream) => extract_session_cookie(stream.get_ref().1),
            _ => {
                // We created the stream with a rustls connector so the stream should be rustls as
                // well.
                unreachable!()
            }
        };

        let (writer, reader) = stream.split();
        let reader = RemoteSocket::new(reader);
        let writer = RemoteSocket::new(writer);

        Ok(Self {
            reader,
            writer,
            session_cookie,
        })
    }

    pub async fn create_mirror(&mut self, secrets: &WriteSecrets) -> Result<(), RemoteClientError> {
        let proof = self.make_proof(secrets);
        match self
            .invoke(protocol::v1::Request::Create {
                repository_id: secrets.id,
                proof,
            })
            .await
        {
            Ok(response) => {
                let _: RepositoryHandle = response.try_into()?;
                Ok(())
            }
            Err(RemoteClientError::Response(error))
                if error.message() == "repository already exists" =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub async fn delete_mirror(&mut self, secrets: &WriteSecrets) -> Result<(), RemoteClientError> {
        let proof = self.make_proof(secrets);
        match self
            .invoke(protocol::v1::Request::Delete {
                repository_id: secrets.id,
                proof,
            })
            .await
        {
            Ok(response) => {
                let () = response.try_into()?;
                Ok(())
            }
            // TODO: check error code, not message
            Err(RemoteClientError::Response(error))
                if error.message() == "repository not found" =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub async fn mirror_exists(&mut self, id: &RepositoryId) -> Result<bool, RemoteClientError> {
        match self
            .invoke(protocol::v1::Request::Exists { repository_id: *id })
            .await
        {
            Ok(response) => {
                let _: RepositoryHandle = response.try_into()?;
                Ok(true)
            }
            // TODO: check error code, not message
            Err(RemoteClientError::Response(error))
                if error.message() == "repository not found" =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn close(&mut self) {
        if let Err(error) = self.writer.close().await {
            tracing::warn!(?error, "failed to gracefully close remote connection");
        }
    }

    fn make_proof(&self, secrets: &WriteSecrets) -> Signature {
        secrets.write_keys.sign(&self.session_cookie)
    }

    async fn invoke(
        &mut self,
        request: protocol::v1::Request,
    ) -> Result<Response, RemoteClientError> {
        let message_id = MessageId::next();

        self.writer
            .send(Message {
                id: message_id,
                payload: request.into(),
            })
            .await?;

        let message = self
            .reader
            .try_next()
            .await?
            .ok_or(RemoteClientError::Disconnected)?;

        if message.id != message_id {
            return Err(RemoteClientError::UnexpectedResponse);
        }

        let response = match message.payload {
            ServerPayload::Success(response) => response,
            ServerPayload::Failure(error) => return Err(RemoteClientError::Response(error)),
            ServerPayload::Notification(_) => return Err(RemoteClientError::UnexpectedResponse),
        };

        Ok(response)
    }
}

#[derive(Error, Debug)]
pub enum RemoteClientError {
    #[error("connection closed by server")]
    Disconnected,
    #[error("failed to receive response")]
    Read(#[from] ReadError),
    #[error("server responded with error")]
    Response(ProtocolError),
    #[error("unexpected response")]
    UnexpectedResponse,
    #[error("websocket error")]
    WebSocket(#[from] ws::Error),
    #[error("failed to send request")]
    Write(#[from] WriteError),
}

impl From<ProtocolError> for RemoteClientError {
    fn from(src: ProtocolError) -> Self {
        Self::Response(src)
    }
}

impl From<UnexpectedResponse> for RemoteClientError {
    fn from(_: UnexpectedResponse) -> Self {
        Self::UnexpectedResponse
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::setup_service;
    use super::*;
    use crate::test_utils::ServiceRunner;
    use assert_matches::assert_matches;

    #[tokio::test]
    async fn proof_replay_attack() {
        let (_temp_dir, service, server_addr, client_config) = setup_service().await;

        let client0 = RemoteClient::connect(&server_addr, client_config.clone())
            .await
            .unwrap();
        let mut client1 = RemoteClient::connect(&server_addr, client_config)
            .await
            .unwrap();

        let secrets = WriteSecrets::random();
        let proof = secrets.write_keys.sign(&client0.session_cookie);

        // Attempt to invoke the request using a proof leaked from another client.
        let runner = ServiceRunner::start(service);
        let error = assert_matches!(
            client1
                .invoke(protocol::v1::Request::Create {
                    repository_id: secrets.id,
                    proof
                })
                .await,
            Err(RemoteClientError::Response(error)) => error
        );

        // TODO: check code, not message
        assert_eq!(error.message(), "permission denied");

        runner.stop().await.close().await;
    }
}
