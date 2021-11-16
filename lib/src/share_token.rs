use crate::repository::RepositoryId;
use thiserror::Error;
use url::Url;

pub const SCHEME: &str = "ouisync";

/// Token to share a repository. Can be transmitted to other replicas by various means.
#[derive(Eq, PartialEq, Debug)]
pub struct ShareToken {
    pub(crate) id: RepositoryId,
}

impl ShareToken {
    // Encode this share token as an URL.
    pub fn to_url(&self) -> Url {
        Url::parse(&format!("{}:{:x}", SCHEME, self.id)).unwrap()
    }

    // Decode a share token from an URL.
    pub fn from_url(url: &Url) -> Result<Self, DecodeError> {
        if url.scheme() != SCHEME {
            return Err(DecodeError);
        }

        let id = url.path().parse().map_err(|_| DecodeError)?;

        Ok(Self { id })
    }
}

#[derive(Debug, Error)]
#[error("failed to decode share token")]
pub struct DecodeError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_url() {
        let id_hex = "416d9c3fe32017f7b5c8e406630576ad";
        let id_bytes = hex::decode(id_hex).unwrap();
        let id = RepositoryId::try_from(id_bytes.as_ref()).unwrap();

        let token = ShareToken { id };
        let url = token.to_url();

        assert_eq!(url.to_string(), format!("ouisync:{}", id_hex))
    }

    #[test]
    fn to_and_from_url() {
        let id: RepositoryId = rand::random();
        let token = ShareToken { id };

        let url = token.to_url();
        let decoded = ShareToken::from_url(&url).unwrap();
        assert_eq!(decoded, token);
    }
}
