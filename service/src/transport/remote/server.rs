use std::{
    io, iter,
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
use tokio_rustls::{rustls, server::TlsStream, TlsAcceptor};
use tokio_tungstenite::{tungstenite as ws, WebSocketStream};

use crate::{
    protocol::{Message, MessageId, Request, ResponseResult},
    transport::{ReadError, ValidateError, WriteError},
};

use super::{extract_session_cookie, protocol, RemoteSocket};

pub(crate) struct RemoteServer {
    tcp_listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    local_addr: SocketAddr,
}

impl RemoteServer {
    pub async fn bind(addr: SocketAddr, config: Arc<rustls::ServerConfig>) -> io::Result<Self> {
        let tcp_listener = TcpListener::bind(addr).await?;
        let local_addr = tcp_listener.local_addr()?;

        let tls_acceptor = TlsAcceptor::from(config);

        tracing::info!("remote server listening on {}", local_addr);

        Ok(Self {
            tcp_listener,
            tls_acceptor,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept the next remote client connection. The returned value needs to be finalized (
    /// [AcceptedRemoteConnection::finalize]) before use.
    pub async fn accept(&self) -> io::Result<AcceptedRemoteConnection> {
        let (socket, addr) = self.tcp_listener.accept().await?;

        Ok(AcceptedRemoteConnection {
            socket,
            addr,
            tls_acceptor: self.tls_acceptor.clone(),
        })
    }
}

pub(crate) struct AcceptedRemoteConnection {
    socket: TcpStream,
    addr: SocketAddr,
    tls_acceptor: TlsAcceptor,
}

impl AcceptedRemoteConnection {
    /// Finalize accepting the connection by upgrading it to websockets+tls
    ///
    /// # Cancel safety
    ///
    /// This function is *not* cancel safe.
    pub async fn finalize(self) -> Option<(RemoteServerReader, RemoteServerWriter)> {
        // Upgrade to TLS
        let socket = self
            .tls_acceptor
            .accept(self.socket)
            .await
            .inspect_err(|error| {
                tracing::warn!(?error, addr = ?self.addr, "failed to upgrade the connection to TLS")
            })
            .ok()?;

        let session_cookie = extract_session_cookie(socket.get_ref().1);

        // Upgrade to websocket
        let socket = tokio_tungstenite::accept_async(socket)
            .await
            .inspect_err(|error| {
                tracing::warn!(?error, addr = ?self.addr, "failed to upgrade the connection to websocket")
            })
            .ok()?;

        tracing::info!(addr = ?self.addr, "accepted remote client");

        let (writer, reader) = socket.split();
        let reader = RemoteServerReader::new(reader, session_cookie);
        let writer = RemoteServerWriter::new(writer);

        Some((reader, writer))
    }
}

pub(crate) struct RemoteServerReader {
    socket: RemoteSocket<protocol::Request, SplitStream<WebSocketStream<TlsStream<TcpStream>>>>,

    // Opaque, non-sensitive value unique to a particular client session and accessible to both the
    // client and the server. It's useful for constructing zero-knowledge proofs: the client can
    // sign this cookie with a private key and send the signature to the server in order to prove
    // the possession of the private key without revealing it. The cookie is unique per session
    // which makes this proof resistant to replay attacks.
    session_cookie: [u8; 32],
}

impl RemoteServerReader {
    fn new(
        socket: SplitStream<WebSocketStream<TlsStream<TcpStream>>>,
        session_cookie: [u8; 32],
    ) -> Self {
        Self {
            socket: RemoteSocket::new(socket),
            session_cookie,
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
                    Request::RepositoryDeleteByName {
                        name: make_repository_name(&repository_id),
                    }
                }
                protocol::v1::Request::Exists { repository_id } => Request::RepositoryFind {
                    name: make_repository_name(&repository_id),
                },
                protocol::v1::Request::GetListenerAddrs => Request::NetworkGetLocalListenerAddrs,
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

        let message = match ready!(this.socket.poll_next_unpin(cx)) {
            Some(Ok(message)) => message,
            Some(Err(error)) => return Poll::Ready(Some(Err(error))),
            None => return Poll::Ready(None),
        };

        Poll::Ready(Some(this.preprocess_message(message)))
    }
}

pub(crate) struct RemoteServerWriter {
    socket:
        RemoteSocket<ResponseResult, SplitSink<WebSocketStream<TlsStream<TcpStream>>, ws::Message>>,
}

impl RemoteServerWriter {
    fn new(socket: SplitSink<WebSocketStream<TlsStream<TcpStream>>, ws::Message>) -> Self {
        Self {
            socket: RemoteSocket::new(socket),
        }
    }
}

impl Sink<Message<ResponseResult>> for RemoteServerWriter {
    type Error = WriteError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().socket.poll_ready_unpin(cx)))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().socket.poll_flush_unpin(cx)))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(ready!(self.get_mut().socket.poll_close_unpin(cx)))
    }

    fn start_send(
        self: Pin<&mut Self>,
        message: Message<ResponseResult>,
    ) -> Result<(), Self::Error> {
        self.get_mut().socket.start_send_unpin(message)
    }
}

fn make_repository_name(id: &RepositoryId) -> String {
    insert_separators(&id.salted_hash(b"ouisync repository name").to_string())
}

fn make_repository_create_request(id: RepositoryId) -> Request {
    Request::RepositoryCreate {
        path: make_repository_name(&id).into(),
        read_secret: None,
        write_secret: None,
        token: Some(ShareToken::from(AccessSecrets::Blind { id })),
        sync_enabled: true,
        // NOTE: DHT is disabled to prevent spamming the DHT when there is a lot of repos.
        // This is fine because the clients add the cache servers as user-provided peers.
        // TODO: After we address https://github.com/equalitie/ouisync/issues/128 we should
        // consider enabling it again.
        dht_enabled: false,
        pex_enabled: true,
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
