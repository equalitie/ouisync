use super::Response;
use ouisync_bridge::protocol::NetworkEvent;
use serde::{Deserialize, Serialize};
use std::{fmt, iter};

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
