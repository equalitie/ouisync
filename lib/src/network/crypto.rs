//! Encryption protocol for syncing repositories.
//!
//! Using the "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s" protocol from
//! [Noise Protocol Framework](https://noiseprotocol.org/noise.html).
//!
//! Using the salted hash of the secret repository id as the pre-shared key. This way only the
//! replicas that posses the secret repository id are able to communicate and no authentication
//! based on the identity of the replicas is needed.

use super::{
    message_dispatcher::{MessageSink, MessageStream},
    runtime_id::PublicRuntimeId,
};
use crate::protocol::RepositoryId;
use bytes::{Bytes, BytesMut};
use futures_util::{Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use noise_protocol::Cipher as _;
use noise_rust_crypto::{Blake2s, ChaCha20Poly1305, X25519};
use std::{
    io,
    pin::Pin,
    task::{ready, Context, Poll},
};
use thiserror::Error;

type Cipher = ChaCha20Poly1305;
type CipherState = noise_protocol::CipherState<Cipher>;
type HandshakeState = noise_protocol::HandshakeState<X25519, Cipher, Blake2s>;

/// Role of this replica in the communication protocol.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(super) enum Role {
    /// Initiator sends the first message
    Initiator,
    /// Responder waits for the first message and then responds
    Responder,
}

impl Role {
    /// Determine the role this replica will have in the communication protocol for the given
    /// repository.
    ///
    /// # Panics
    ///
    /// Panics if the runtime ids are equal.
    pub fn determine(
        repo_id: &RepositoryId,
        this_runtime_id: &PublicRuntimeId,
        that_runtime_id: &PublicRuntimeId,
    ) -> Self {
        assert_ne!(this_runtime_id, that_runtime_id);

        let this_hash = repo_id.salted_hash(this_runtime_id.as_ref());
        let that_hash = repo_id.salted_hash(that_runtime_id.as_ref());

        if this_hash > that_hash {
            Role::Initiator
        } else {
            Role::Responder
        }
    }
}

// This also determines the maximum number of messages we can send or receive in a single protocol
// session.
const MAX_NONCE: u64 = u64::MAX - 1;

/// Wrapper for [`MessageStream`] that decrypts incoming messages.
pub(super) struct DecryptingStream<'a> {
    inner: &'a mut MessageStream,
    cipher: CipherState,
}

impl Stream for DecryptingStream<'_> {
    type Item = Result<BytesMut, RecvError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.cipher.get_next_n() >= MAX_NONCE {
            return Poll::Ready(Some(Err(RecvError::Exhausted)));
        }

        let mut item = match ready!(self.inner.poll_next_unpin(cx)) {
            Some(Ok(item)) => item,
            Some(Err(error)) => return Poll::Ready(Some(Err(error.into()))),
            None => return Poll::Ready(None),
        };

        let ciphertext_len = item.len();

        match self.cipher.decrypt_in_place(&mut item, ciphertext_len) {
            Ok(n) => Poll::Ready(Some(Ok(item.split_to(n)))),
            Err(_) => Poll::Ready(Some(Err(RecvError::Crypto))),
        }
    }
}

/// Wrapper for [`MessageSink`] that encrypts outgoing messages.
pub(super) struct EncryptingSink<'a> {
    inner: &'a mut MessageSink,
    cipher: CipherState,
}

impl Sink<Bytes> for EncryptingSink<'_> {
    type Error = SendError;

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        if self.cipher.get_next_n() >= MAX_NONCE {
            return Err(SendError::Exhausted);
        }

        let plaintext_len = item.len();
        let mut item = BytesMut::from(item);

        item.resize(plaintext_len + Cipher::tag_len(), 0);
        let n = self.cipher.encrypt_in_place(&mut item, plaintext_len);

        self.inner.start_send_unpin(item.split_to(n).freeze())?;

        Ok(())
    }

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx).map_err(Into::into)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx).map_err(Into::into)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx).map_err(Into::into)
    }
}

/// Establish encrypted communication channel for the purpose of syncing the given
/// repository.
pub(super) async fn establish_channel<'a>(
    role: Role,
    repo_id: &RepositoryId,
    stream: &'a mut MessageStream,
    sink: &'a mut MessageSink,
) -> Result<(DecryptingStream<'a>, EncryptingSink<'a>), EstablishError> {
    let mut handshake_state = build_handshake_state(role, repo_id);

    let (recv_cipher, send_cipher) = match role {
        Role::Initiator => {
            handshake_send(&mut handshake_state, sink, &[]).await?;
            handshake_recv(&mut handshake_state, stream).await?;

            assert!(handshake_state.completed());

            let (send_cipher, recv_cipher) = handshake_state.get_ciphers();
            (recv_cipher, send_cipher)
        }
        Role::Responder => {
            handshake_recv(&mut handshake_state, stream).await?;
            handshake_send(&mut handshake_state, sink, &[]).await?;

            assert!(handshake_state.completed());

            handshake_state.get_ciphers()
        }
    };

    let stream = DecryptingStream {
        inner: stream,
        cipher: recv_cipher,
    };

    let sink = EncryptingSink {
        inner: sink,
        cipher: send_cipher,
    };

    Ok((stream, sink))
}

#[derive(Debug, Error)]
pub(super) enum SendError {
    #[error("nonce counter exhausted")]
    Exhausted,
    #[error("IO error")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub(super) enum RecvError {
    #[error("decryption failed")]
    Crypto,
    #[error("nonce counter exhausted")]
    Exhausted,
    #[error("IO error")]
    Io(#[from] io::Error),
}

#[derive(Debug, Error)]
pub(super) enum EstablishError {
    #[error("encryption / decryption failed")]
    Crypto,
    #[error("IO error")]
    Io(#[from] io::Error),
}

impl From<noise_protocol::Error> for EstablishError {
    fn from(_: noise_protocol::Error) -> Self {
        Self::Crypto
    }
}

fn build_handshake_state(role: Role, repo_id: &RepositoryId) -> HandshakeState {
    use noise_protocol::patterns;

    let mut state = HandshakeState::new(
        patterns::noise_nn_psk0(),
        role == Role::Initiator,
        [],
        None,
        None,
        None,
        None,
    );
    state.push_psk(repo_id.salted_hash(b"pre-shared-key").as_ref());
    state
}

async fn handshake_send(
    state: &mut HandshakeState,
    sink: &mut MessageSink,
    msg: &[u8],
) -> Result<(), EstablishError> {
    let content = state.write_message_vec(msg)?;
    sink.send(content.into()).await?;
    Ok(())
}

async fn handshake_recv(
    state: &mut HandshakeState,
    stream: &mut MessageStream,
) -> Result<Vec<u8>, EstablishError> {
    let content = stream
        .try_next()
        .await?
        .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;

    Ok(state.read_message_vec(&content)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{
        message_dispatcher::{create_connection_pair, MessageDispatcher},
        stats::ByteCounters,
    };
    use futures_util::future;
    use net::bus::TopicId;
    use std::sync::Arc;

    #[tokio::test]
    async fn sanity_check() {
        let (client, server) = create_connection_pair().await;

        let client = MessageDispatcher::builder(client).build();
        let server = MessageDispatcher::builder(server).build();

        let repo_id = RepositoryId::random();
        let topic_id = TopicId::random();

        let (mut client_sink, mut client_stream) =
            client.open(topic_id, Arc::new(ByteCounters::default()));

        let (mut server_sink, mut server_stream) =
            server.open(topic_id, Arc::new(ByteCounters::default()));

        let ((mut client_stream, mut client_sink), (mut server_stream, mut server_sink)) =
            future::try_join(
                establish_channel(
                    Role::Initiator,
                    &repo_id,
                    &mut client_stream,
                    &mut client_sink,
                ),
                establish_channel(
                    Role::Responder,
                    &repo_id,
                    &mut server_stream,
                    &mut server_sink,
                ),
            )
            .await
            .unwrap();

        client_sink.send(Bytes::from_static(b"ping")).await.unwrap();
        assert_eq!(
            server_stream.try_next().await.unwrap().unwrap().as_ref(),
            b"ping"
        );

        server_sink.send(Bytes::from_static(b"pong")).await.unwrap();
        assert_eq!(
            client_stream.try_next().await.unwrap().unwrap().as_ref(),
            b"pong"
        );
    }
}
