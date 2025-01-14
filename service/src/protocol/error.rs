use serde::{Deserialize, Serialize};
use std::{fmt, iter};

use super::error_code::{ErrorCode, ToErrorCode};

/// Error response from the server
#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct ProtocolError {
    code: ErrorCode,
    message: String,
    sources: Vec<String>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            sources: Vec::new(),
        }
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = &str> {
        self.sources.iter().map(|s| s.as_str())
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "Error [{}]: {}", u16::from(self.code), self.message)?;

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

impl<E> From<E> for ProtocolError
where
    E: std::error::Error + ToErrorCode,
{
    fn from(src: E) -> Self {
        let code = src.to_error_code();
        let message = src.to_string();
        let sources = iter::successors(src.source(), |error| error.source())
            .map(|error| error.to_string())
            .collect();

        Self {
            code,
            message,
            sources,
        }
    }
}
