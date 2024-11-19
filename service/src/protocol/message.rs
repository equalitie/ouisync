use serde::{de::DeserializeOwned, Serialize};
use std::{
    io,
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
    pub fn encode<W>(&self, writer: &mut W) -> Result<(), EncodeError>
    where
        W: io::Write,
    {
        writer
            .write_all(&self.id.0.to_be_bytes())
            .map_err(EncodeError::Id)?;

        rmp_serde::encode::write(writer, &self.payload).map_err(EncodeError::Payload)?;

        Ok(())
    }
}

impl<T> Message<T>
where
    T: DeserializeOwned,
{
    pub fn decode<R>(reader: &mut R) -> Result<Self, DecodeError>
    where
        R: io::Read,
    {
        let mut buffer = [0; 8];
        reader.read_exact(&mut buffer).map_err(DecodeError::Id)?;
        let id = MessageId(u64::from_be_bytes(buffer));

        let payload =
            rmp_serde::from_read(reader).map_err(|error| DecodeError::Payload(id, error))?;

        Ok(Self { id, payload })
    }
}

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("failed to encode message id")]
    Id(#[source] io::Error),
    #[error("failed to encode message payload")]
    Payload(#[source] rmp_serde::encode::Error),
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("failed to decode message id")]
    Id(#[source] io::Error),
    #[error("failed to decode message payload")]
    Payload(MessageId, #[source] rmp_serde::decode::Error),
}
