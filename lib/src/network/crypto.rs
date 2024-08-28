//! Encryption protocol for syncing repositories.
//!
//! Using the "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s" protocol from
//! [Noise Protocol Framework](https://noiseprotocol.org/noise.html).
//!
//! Using the salted hash of the secret repository id as the pre-shared key. This way only the
//! replicas that posses the secret repository id are able to communicate and no authentication
//! based on the identity of the replicas is needed.

use super::{
    message_dispatcher::{ChannelClosed, ContentSink, ContentStream, ContentStreamError},
    runtime_id::PublicRuntimeId,
};
use crate::protocol::RepositoryId;
use noise_protocol::Cipher as _;
use noise_rust_crypto::{Blake2s, ChaCha20Poly1305, X25519};
use std::mem;
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

/// Wrapper for [`ContentStream`] that decrypts incoming messages.
pub(super) struct DecryptingStream<'a> {
    inner: &'a mut ContentStream,
    cipher: CipherState,
    buffer: Vec<u8>,
}

impl DecryptingStream<'_> {
    pub async fn recv(&mut self) -> Result<Vec<u8>, RecvError> {
        if self.cipher.get_next_n() >= MAX_NONCE {
            return Err(RecvError::Exhausted);
        }

        let mut content = self.inner.recv().await?;

        let plain_len = content
            .len()
            .checked_sub(Cipher::tag_len())
            .ok_or(RecvError::Crypto)?;
        self.buffer.resize(plain_len, 0);
        self.cipher
            .decrypt_ad(self.inner.channel().as_ref(), &content, &mut self.buffer)
            .map_err(|_| RecvError::Crypto)?;

        mem::swap(&mut content, &mut self.buffer);

        Ok(content)
    }
}

/// Wrapper for [`ContentSink`] that encrypts outgoing messages.
pub(super) struct EncryptingSink<'a> {
    inner: &'a mut ContentSink,
    cipher: CipherState,
    buffer: Vec<u8>,
}

impl EncryptingSink<'_> {
    pub async fn send(&mut self, mut content: Vec<u8>) -> Result<(), SendError> {
        if self.cipher.get_next_n() >= MAX_NONCE {
            return Err(SendError::Exhausted);
        }

        self.buffer.resize(content.len() + Cipher::tag_len(), 0);
        self.cipher
            .encrypt_ad(self.inner.channel().as_ref(), &content, &mut self.buffer);

        mem::swap(&mut content, &mut self.buffer);

        Ok(self.inner.send(content).await?)
    }
}

/// Establish encrypted communication channel for the purpose of syncing the given
/// repository.
pub(super) async fn establish_channel<'a>(
    role: Role,
    repo_id: &RepositoryId,
    stream: &'a mut ContentStream,
    sink: &'a mut ContentSink,
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
        buffer: vec![],
    };

    let sink = EncryptingSink {
        inner: sink,
        cipher: send_cipher,
        buffer: vec![],
    };

    Ok((stream, sink))
}

#[derive(Debug, Error)]
pub(super) enum SendError {
    #[error("channel closed")]
    Closed,
    #[error("nonce counter exhausted")]
    Exhausted,
}

impl From<ChannelClosed> for SendError {
    fn from(_: ChannelClosed) -> Self {
        Self::Closed
    }
}

#[derive(Debug, Error)]
pub(super) enum RecvError {
    #[error("decryption failed")]
    Crypto,
    #[error("channel closed")]
    Closed,
    #[error("nonce counter exhausted")]
    Exhausted,
    #[error("network transport changed")]
    TransportChanged,
}

impl From<ContentStreamError> for RecvError {
    fn from(error: ContentStreamError) -> Self {
        match error {
            ContentStreamError::ChannelClosed => Self::Closed,
            ContentStreamError::TransportChanged => Self::TransportChanged,
        }
    }
}

#[derive(Debug, Error)]
pub(super) enum EstablishError {
    #[error("encryption / decryption failed")]
    Crypto,
    #[error("channel closed")]
    Closed,
    #[error("network transport changed")]
    TransportChanged,
}

impl From<noise_protocol::Error> for EstablishError {
    fn from(_: noise_protocol::Error) -> Self {
        Self::Crypto
    }
}

impl From<ChannelClosed> for EstablishError {
    fn from(_: ChannelClosed) -> Self {
        Self::Closed
    }
}

impl From<ContentStreamError> for EstablishError {
    fn from(error: ContentStreamError) -> Self {
        match error {
            ContentStreamError::ChannelClosed => Self::Closed,
            ContentStreamError::TransportChanged => Self::TransportChanged,
        }
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
    sink: &mut ContentSink,
    msg: &[u8],
) -> Result<(), EstablishError> {
    let content = state.write_message_vec(msg)?;
    Ok(sink.send(content).await?)
}

async fn handshake_recv(
    state: &mut HandshakeState,
    stream: &mut ContentStream,
) -> Result<Vec<u8>, EstablishError> {
    let content = stream.recv().await?;
    Ok(state.read_message_vec(&content)?)
}
