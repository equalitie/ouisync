use ouisync_bridge::protocol::NetworkEvent;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt, io, iter};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerPayload {
    Success(Response),
    Failure(ServerError),
    Notification(Notification),
}

impl From<Result<Response, ServerError>> for ServerPayload {
    fn from(result: Result<Response, ServerError>) -> Self {
        match result {
            Ok(response) => Self::Success(response),
            Err(error) => Self::Failure(error),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MessageId(u64);

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

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerError {
    // TODO: error code
    message: String,
    sources: Vec<String>,
}

impl ServerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            sources: Vec::new(),
        }
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "Error: {}", self.message)?;

            if !self.sources.is_empty() {
                writeln!(f)?;
                writeln!(f)?;
                write!(f, "Caused by:")?;
            }

            for (index, source) in self.sources.iter().enumerate() {
                writeln!(f)?;
                write!(f, "{index:>4}: {source}")?;
            }

            Ok(())
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl<E> From<E> for ServerError
where
    E: std::error::Error,
{
    fn from(src: E) -> Self {
        let message = src.to_string();
        let sources = iter::successors(src.source(), |error| error.source())
            .map(|error| error.to_string())
            .collect();

        Self { message, sources }
    }
}
