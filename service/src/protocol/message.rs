use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    mem,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct MessageId(u64);

impl MessageId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug)]
pub struct Message<T> {
    pub id: MessageId,
    pub payload: T,
}

impl<T> Message<T>
where
    T: Serialize,
{
    pub fn encode(&self, buffer: &mut BytesMut) -> Result<(), EncodeError> {
        buffer.put_u64(self.id.0);
        rmp_serde::encode::write(&mut buffer.writer(), &self.payload)
            .map_err(EncodeError::Payload)?;

        Ok(())
    }
}

impl<T> Message<T>
where
    T: DeserializeOwned,
{
    pub fn decode(buffer: &mut Bytes) -> Result<Self, DecodeError> {
        if buffer.remaining() < mem::size_of::<u64>() {
            return Err(DecodeError::Id);
        }

        let id = MessageId(buffer.get_u64());

        let payload =
            rmp_serde::from_slice(buffer).map_err(|error| DecodeError::Payload(id, error))?;

        Ok(Self { id, payload })
    }
}

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("failed to encode message payload")]
    Payload(#[source] rmp_serde::encode::Error),
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("failed to decode message id")]
    Id,
    #[error("failed to decode message payload")]
    Payload(MessageId, #[source] rmp_serde::decode::Error),
}
