use crate::repository::SecretRepositoryId;
use std::{fmt, str::FromStr};
use thiserror::Error;
use url::Url;

pub const SCHEME: &str = "ouisync";

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Eq, PartialEq, Debug)]
pub struct ShareToken {
    pub(crate) id: SecretRepositoryId,
    pub(crate) name: String,
}

impl ShareToken {
    pub fn new(id: SecretRepositoryId) -> Self {
        Self {
            id,
            name: "".to_owned(),
        }
    }

    pub fn with_name(self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..self
        }
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
        let name = url
            .query_pairs()
            .find(|(name, _)| name == "name")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();

        Ok(Self { id, name })
    }
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut url = Url::parse(&format!("{}:{:x}", SCHEME, self.id)).unwrap();

        if !self.name.is_empty() {
            url.query_pairs_mut().append_pair("name", &self.name);
        }

        write!(f, "{}", url)
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
        let id = SecretRepositoryId::try_from(id_bytes.as_ref()).unwrap();

        let token = ShareToken::new(id);
        assert_eq!(token.to_string(), format!("ouisync:{}", id_hex));

        let token = token.with_name("foo");
        assert_eq!(token.to_string(), format!("ouisync:{}?name=foo", id_hex))
    }

    #[test]
    fn encode_and_decode() {
        let id: SecretRepositoryId = rand::random();

        let token = ShareToken::new(id);
        let string = token.to_string();
        let decoded: ShareToken = string.parse().unwrap();
        assert_eq!(decoded, token);

        let token = token.with_name("foo");
        let string = token.to_string();
        let decoded: ShareToken = string.parse().unwrap();
        assert_eq!(decoded, token);
    }
}
