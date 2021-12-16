use super::writer_request::WriterRequest;
use crate::{
    crypto::{sign, SecretKey},
    error::Error,
    repository::RepositoryId,
};
use std::{borrow::Cow, fmt, str::FromStr};
use thiserror::Error;
use url::Url;

pub const SCHEME: &str = "ouisync";

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Debug)]
pub struct ShareToken {
    id: RepositoryId,
    name: String,
    access: Access,
}

#[derive(Debug)]
pub(super) enum Access {
    Blind,
    Reader {
        // Key to decrypt the repository content
        read_key: SecretKey,
    },
    Writer {
        // Key to decrypt and encrypt the repository content
        read_key: SecretKey,
        // Public key of the replica that is giving the write access ("giver").
        giver_pk: sign::PublicKey,
        // Secret key created from an unique random nonce to prevent using this share token more
        // than once.
        nonce_sk: sign::SecretKey,
        // Signature of (repository_id, nonce_pk) created by the giver (nonce_pk is the
        // corresponding public key to nonce_sk).
        nonce_pk_signature: sign::Signature,
    },
}

impl ShareToken {
    /// Create share token for blind access to the given repository.
    pub fn new(id: RepositoryId) -> Self {
        Self {
            id,
            name: "".to_owned(),
            access: Access::Blind,
        }
    }

    /// Attach a suggested repository name to the token.
    pub fn with_name(self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..self
        }
    }

    /// Convert the share token to one that gives reader access.
    pub fn for_reader(self, read_key: SecretKey) -> Self {
        Self {
            access: Access::Reader { read_key },
            ..self
        }
    }

    /// Convert the share token to one that gives writer access. `giver_keys` is the signing
    /// keypair of the replica that gives the access.
    pub fn for_writer(self, read_key: SecretKey, giver_keys: &sign::Keypair) -> Self {
        let nonce_keys = sign::Keypair::generate();

        let message = signature_material(&self.id, &nonce_keys.public);
        let nonce_pk_signature = giver_keys.sign(&message);

        Self {
            access: Access::Writer {
                read_key,
                giver_pk: giver_keys.public,
                nonce_sk: nonce_keys.secret,
                nonce_pk_signature,
            },
            ..self
        }
    }

    /// Id of the repository to share.
    pub fn id(&self) -> &RepositoryId {
        &self.id
    }

    /// Suggested name of the repository.
    pub fn suggested_name(&self) -> Cow<str> {
        if self.name.is_empty() {
            Cow::Owned(format!(
                "{:x}",
                self.id.salted_hash(b"ouisync repository name")
            ))
        } else {
            Cow::Borrowed(&self.name)
        }
    }

    /// Returns the read key if this token is for reader or writer access, `None` if for blind.
    pub fn read_key(&self) -> Option<&SecretKey> {
        match &self.access {
            Access::Reader { read_key } | Access::Writer { read_key, .. } => Some(read_key),
            Access::Blind => None,
        }
    }

    /// Validate this token
    pub fn validate(&self) -> bool {
        match &self.access {
            Access::Blind | Access::Reader { .. } => true,
            Access::Writer {
                giver_pk,
                nonce_sk,
                nonce_pk_signature,
                ..
            } => {
                let message = signature_material(&self.id, &sign::PublicKey::from(nonce_sk));
                giver_pk.verify(&message[..], nonce_pk_signature)
            }
        }
    }

    /// If this token is for a writer access, converts it to a `WriterRequest`, otherwise returns
    /// an error which contains the original token.
    pub fn into_writer_request(self, recipient_pk: sign::PublicKey) -> Result<WriterRequest, Self> {
        match self.access {
            Access::Writer {
                giver_pk,
                nonce_sk,
                nonce_pk_signature,
                ..
            } => {
                let nonce_pk = sign::PublicKey::from(&nonce_sk);
                let message = signature_material(&self.id, &recipient_pk);
                let recipient_pk_signature = nonce_sk.sign(&message, &nonce_pk);

                Ok(WriterRequest {
                    id: self.id,
                    giver_pk,
                    recipient_pk,
                    recipient_pk_signature,
                    nonce_pk,
                    nonce_pk_signature,
                })
            }
            Access::Reader { .. } | Access::Blind => Err(self),
        }
    }
}

pub(super) fn signature_material(
    id: &RepositoryId,
    pk: &sign::PublicKey,
) -> [u8; RepositoryId::SIZE + sign::PublicKey::SIZE] {
    let mut message = [0; RepositoryId::SIZE + sign::PublicKey::SIZE];
    message[..RepositoryId::SIZE].copy_from_slice(id.as_ref());
    message[RepositoryId::SIZE..].copy_from_slice(pk.as_ref());
    message
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

        Ok(Self {
            id,
            name,
            access: Access::Blind,
        })
    }
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut url = Url::parse(&format!("{}:{:x}", SCHEME, self.id)).map_err(|_| fmt::Error)?;

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

impl From<DecodeError> for Error {
    fn from(_: DecodeError) -> Self {
        Self::MalformedData
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn encode() {
        let id_hex = "416d9c3fe32017f7b5c8e406630576ad416d9c3fe32017f7b5c8e406630576ad";
        let id_bytes = hex::decode(id_hex).unwrap();
        let id = RepositoryId::try_from(id_bytes.as_ref()).unwrap();

        let token = ShareToken::new(id);
        assert_eq!(token.to_string(), format!("ouisync:{}", id_hex));

        let token = token.with_name("foo");
        assert_eq!(token.to_string(), format!("ouisync:{}?name=foo", id_hex))
    }

    #[test]
    fn encode_and_decode() {
        let id: RepositoryId = rand::random();

        let token = ShareToken::new(id);
        let string = token.to_string();
        let decoded: ShareToken = string.parse().unwrap();
        assert_eq!(decoded.id, token.id);
        assert_matches!(decoded.access, Access::Blind);

        let token = token.with_name("foo");
        let string = token.to_string();
        let decoded: ShareToken = string.parse().unwrap();
        assert_eq!(decoded.id, token.id);
        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.access, Access::Blind);
    }
}
