use crate::repository::RepositoryId;
use std::{fmt, str::FromStr};
use thiserror::Error;
use url::Url;

pub const SCHEME: &str = "ouisync";

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Eq, PartialEq, Debug)]
pub struct ShareToken {
    pub(crate) id: RepositoryId,
}

impl ShareToken {
    pub fn new(id: RepositoryId) -> Self {
        Self { id }
    }
}

impl FromStr for ShareToken {
    type Err = DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(s)?;

        if url.scheme() != SCHEME {
            return Err(DecodeError);
        }

        let id = url.path().parse()?;

        Ok(Self { id })
    }
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{:x}", SCHEME, self.id)
    }
}

#[derive(Debug, Error)]
#[error("failed to decode share token")]
pub struct DecodeError;

impl From<hex::FromHexError> for DecodeError {
    fn from(_: hex::FromHexError) -> Self {
        Self
    }
}

impl From<url::ParseError> for DecodeError {
    fn from(_: url::ParseError) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode() {
        let id_hex = "416d9c3fe32017f7b5c8e406630576ad";
        let id_bytes = hex::decode(id_hex).unwrap();
        let id = RepositoryId::try_from(id_bytes.as_ref()).unwrap();

        let token = ShareToken { id };

        assert_eq!(token.to_string(), format!("ouisync:{}", id_hex))
    }

    #[test]
    fn encode_and_decode() {
        let id: RepositoryId = rand::random();
        let token = ShareToken { id };

        let string = token.to_string();
        let decoded: ShareToken = string.parse().unwrap();
        assert_eq!(decoded, token);
    }
}
