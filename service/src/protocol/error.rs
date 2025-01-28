use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, ops::Deref};

use super::error_code::{ErrorCode, ToErrorCode};

/// Error response from the server
#[derive(Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolError(Inner);

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(Inner {
            code,
            message: message.into(),
            sources: Vec::new(),
        })
    }

    pub fn code(&self) -> ErrorCode {
        self.0.code
    }

    pub fn message(&self) -> &str {
        &self.0.message
    }

    pub fn sources(&self) -> impl ExactSizeIterator<Item = &str> {
        self.0.sources.iter().map(|s| s.as_str())
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<E> From<E> for ProtocolError
where
    E: Error + ToErrorCode,
{
    fn from(error: E) -> Self {
        let code = error.to_error_code();
        let message = error.to_string();

        // Would preffer to use `iter::successors` but there were lifetime issues.
        let mut sources = Vec::new();
        let mut source = error.source();
        while let Some(error) = source {
            sources.push(error.to_string());
            source = error.source();
        }

        Self(Inner {
            code,
            message,
            sources,
        })
    }
}

// This allows using `ProtocolError` as `source` for errors that implement `std::error::Error`.
impl Deref for ProtocolError {
    type Target = dyn Error;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Eq, PartialEq, Serialize, Deserialize)]
struct Inner {
    code: ErrorCode,
    message: String,
    sources: Vec<String>,
}

impl Error for Inner {}

impl fmt::Display for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: {}", u16::from(self.code), self.message)?;

        if !self.sources.is_empty() {
            for source in &self.sources {
                write!(f, " → {}", source)?;
            }
        }

        Ok(())
    }
}

impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("sources", &self.sources)
            .finish()
    }
}
