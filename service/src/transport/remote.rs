use std::{
    io::Cursor,
    iter,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

use futures_util::{
    stream::{SplitSink, SplitStream},
    Sink, SinkExt, Stream, StreamExt,
};
use ouisync::{crypto::sign::Signature, AccessSecrets, RepositoryId, ShareToken};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{
    rustls::{self, ConnectionCommon},
    server::TlsStream,
    TlsAcceptor,
};
use tokio_tungstenite::{tungstenite as ws, WebSocketStream};

use crate::protocol::{Message, MessageId, Request, ServerPayload};

use super::{AcceptError, BindError, ReadError, ValidateError, WriteError};

pub(crate) struct RemoteServer {
    tcp_listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    local_addr: SocketAddr,
}

impl RemoteServer {
    pub async fn bind(
        addr: SocketAddr,
        config: Arc<rustls::ServerConfig>,
    ) -> Result<Self, BindError> {
        let tcp_listener = TcpListener::bind(addr).await?;
        let local_addr = tcp_listener.local_addr()?;

        tracing::info!("remote server listening on {}", local_addr);

        Ok(Self {
            tcp_listener,
            tls_acceptor: TlsAcceptor::from(config),
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn accept(&self) -> Result<(RemoteServerReader, RemoteServerWriter), AcceptError> {
        loop {
            let (stream, addr) = self.tcp_listener.accept().await?;

            // Upgrade to TLS
            let stream = match self.tls_acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::error!(?error, "failed to upgrade to tls");
                    continue;
                }
            };

            let session_cookie = extract_session_cookie(stream.get_ref().1);

            // Upgrade to websocket
            let stream = match tokio_tungstenite::accept_async(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::error!(?error, "failed to upgrade to websocket");
                    continue;
                }
            };

            tracing::info!(?addr, "accepted remote client");

            let (writer, reader) = stream.split();
            let reader = RemoteServerReader {
                reader,
                session_cookie,
            };
            let writer = RemoteServerWriter { writer };

            return Ok((reader, writer));
        }
    }
}

pub(crate) struct RemoteServerReader {
    reader: SplitStream<WebSocketStream<TlsStream<TcpStream>>>,

    // Opaque, non-sensitive value unique to a particular client session and accessible to both the
    // client and the server. It's useful for constructing zero-knowledge proofs: the client can
    // sign this cookie with a private key and send the signature to the server in order to prove
    // the possession of the private key without revealing it. The cookie is unique per session
    // which makes this proof resistant to replay attacks.
    session_cookie: [u8; 32],
}

impl RemoteServerReader {
    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Vec<u8>, ReadError>>> {
        loop {
            match ready!(self.reader.poll_next_unpin(cx)) {
                Some(Ok(ws::Message::Binary(payload))) => return Poll::Ready(Some(Ok(payload))),
                Some(Ok(ws::Message::Close(_))) => continue,
                Some(Ok(
                    message @ (ws::Message::Text(_)
                    | ws::Message::Ping(_)
                    | ws::Message::Pong(_)
                    | ws::Message::Frame(_)),
                )) => {
                    tracing::debug!(?message, "unexpected message type");
                    continue;
                }
                Some(Err(error)) => {
                    return Poll::Ready(Some(Err(ReadError::Receive(error.into()))));
                }
                None => return Poll::Ready(None),
            }
        }
    }

    fn preprocess_message(
        &self,
        message: Message<protocol::Request>,
    ) -> Result<Message<Request>, ReadError> {
        let payload = match message.payload {
            // TODO: disable v0 eventually
            protocol::Request::V0(request) => {
                tracing::warn!("deprecated API version: v0");

                match request {
                    protocol::v0::Request::Mirror { share_token } => {
                        make_repository_create_request(*share_token.id())
                    }
                }
            }
            protocol::Request::V1(request) => match request {
                protocol::v1::Request::Create {
                    repository_id,
                    proof,
                } => {
                    self.verify_proof(message.id, &repository_id, &proof)?;
                    make_repository_create_request(repository_id)
                }
                protocol::v1::Request::Delete {
                    repository_id,
                    proof,
                } => {
                    self.verify_proof(message.id, &repository_id, &proof)?;
                    Request::RepositoryDeleteByName(make_repository_name(&repository_id))
                }
                protocol::v1::Request::Exists { repository_id } => {
                    Request::RepositoryExists(make_repository_name(&repository_id))
                }
            },
        };

        Ok(Message {
            id: message.id,
            payload,
        })
    }

    fn verify_proof(
        &self,
        message_id: MessageId,
        repository_id: &RepositoryId,
        proof: &Signature,
    ) -> Result<(), ReadError> {
        if repository_id
            .write_public_key()
            .verify(&self.session_cookie, proof)
        {
            Ok(())
        } else {
            tracing::debug!("invalid proof");
            Err(ReadError::Validate(
                message_id,
                ValidateError::PermissionDenied,
            ))
        }
    }
}

impl Stream for RemoteServerReader {
    type Item = Result<Message<Request>, ReadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        let message = match ready!(this.poll_recv(cx)) {
            Some(Ok(message)) => message,
            Some(Err(error)) => return Poll::Ready(Some(Err(error))),
            None => return Poll::Ready(None),
        };
        let message: Message<protocol::Request> = Message::decode(&mut Cursor::new(message))?;

        Poll::Ready(Some(this.preprocess_message(message)))
    }
}

pub(crate) struct RemoteServerWriter {
    writer: SplitSink<WebSocketStream<TlsStream<TcpStream>>, ws::Message>,
}

impl Sink<Message<ServerPayload>> for RemoteServerWriter {
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_ready_unpin(cx)).map_err(into_send_error))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_flush_unpin(cx)).map_err(into_send_error))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().writer.poll_close_unpin(cx)).map_err(into_send_error))
    }

    fn start_send(
        self: Pin<&mut Self>,
        message: Message<ServerPayload>,
    ) -> Result<(), Self::Error> {
        let this = self.get_mut();

        let mut buffer = Vec::new();
        message.encode(&mut buffer)?;

        this.writer
            .start_send_unpin(ws::Message::Binary(buffer))
            .map_err(into_send_error)
    }
}

fn extract_session_cookie<Data>(connection: &ConnectionCommon<Data>) -> [u8; 32] {
    // unwrap is OK as the function fails only if called before TLS handshake or if the output
    // length is zero, none of which is the case here.
    connection
        .export_keying_material([0; 32], b"ouisync session cookie", None)
        .unwrap()
}

fn into_send_error(src: ws::Error) -> WriteError {
    WriteError::Send(src.into())
}

mod protocol {
    use serde::{Deserialize, Serialize};

    pub(super) mod v0 {
        use super::*;
        use ouisync::ShareToken;

        #[derive(Debug, Serialize, Deserialize)]
        pub enum Request {
            Mirror { share_token: ShareToken },
        }
    }

    pub(super) mod v1 {
        use super::*;
        use ouisync::{crypto::sign::Signature, RepositoryId};

        #[derive(Debug, Serialize, Deserialize)]
        pub enum Request {
            /// Create a blind replica of the repository on the remote server
            Create {
                repository_id: RepositoryId,
                /// Zero-knowledge proof that the client has write access to the repository.
                /// Computed by signing `SessionCookie` with the repo write key.
                proof: Signature,
            },
            /// Delete the repository from the remote server
            Delete {
                repository_id: RepositoryId,
                /// Zero-knowledge proof that the client has write access to the repository.
                /// Computed by signing `SessionCookie` with the repo write key.
                proof: Signature,
            },
            /// Check that the repository exists on the remote server.
            Exists { repository_id: RepositoryId },
        }
    }

    // NOTE: using untagged to support old clients that don't support versioning.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(untagged)]
    pub(super) enum Request {
        V0(v0::Request),
        V1(v1::Request),
    }

    impl From<v0::Request> for Request {
        fn from(v0: v0::Request) -> Self {
            Self::V0(v0)
        }
    }

    impl From<v1::Request> for Request {
        fn from(v1: v1::Request) -> Self {
            Self::V1(v1)
        }
    }
}

fn make_repository_name(id: &RepositoryId) -> String {
    insert_separators(&id.salted_hash(b"ouisync repository name").to_string())
}

fn make_repository_create_request(id: RepositoryId) -> Request {
    Request::RepositoryCreate {
        name: make_repository_name(&id),
        read_secret: None,
        write_secret: None,
        token: Some(ShareToken::from(AccessSecrets::Blind { id })),
        // NOTE: DHT is disabled to prevent spamming the DHT when there is a lot of repos.
        // This is fine because the clients add the storage servers as user-provided peers.
        // TODO: After we address https://github.com/equalitie/ouisync/issues/128 we should
        // consider enabling it again.
        dht: false,
        pex: true,
    }
}

fn insert_separators(input: &str) -> String {
    let chunk_count = 4;
    let chunk_len = 2;
    let sep = '/';

    let (head, tail) = input.split_at(chunk_count * chunk_len);

    head.chars()
        .enumerate()
        .flat_map(|(i, c)| {
            (i > 0 && i < chunk_count * chunk_len && i % chunk_len == 0)
                .then_some(sep)
                .into_iter()
                .chain(iter::once(c))
        })
        .chain(iter::once(sep))
        .chain(tail.chars())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_separators_test() {
        let input = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

        let expected_output = format!(
            "{}/{}/{}/{}/{}",
            &input[0..2],
            &input[2..4],
            &input[4..6],
            &input[6..8],
            &input[8..],
        );
        let actual_output = insert_separators(input);

        assert_eq!(actual_output, expected_output);
    }
}
