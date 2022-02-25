use super::{AccessMode, AccessSecrets, DecodeError};
use crate::repository::RepositoryId;
use std::{
    borrow::Cow,
    fmt,
    str::{self, FromStr},
};
use zeroize::Zeroizing;

pub const SCHEME: &str = "ouisync";
pub const VERSION: u8 = 0; // when this reaches 128, switch to variable-lengh encoding.

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Debug)]
pub struct ShareToken {
    secrets: AccessSecrets,
    name: String,
}

impl ShareToken {
    /// Attach a suggested repository name to the token.
    pub fn with_name(self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..self
        }
    }

    /// Id of the repository to share.
    pub fn id(&self) -> &RepositoryId {
        self.secrets.id()
    }

    /// Suggested name of the repository.
    pub fn suggested_name(&self) -> Cow<str> {
        if self.name.is_empty() {
            Cow::Owned(format!(
                "{:x}",
                self.secrets.id().salted_hash(b"ouisync repository name")
            ))
        } else {
            Cow::Borrowed(&self.name)
        }
    }

    pub fn secrets(&self) -> &AccessSecrets {
        &self.secrets
    }

    pub fn into_secrets(self) -> AccessSecrets {
        self.secrets
    }

    pub fn access_mode(&self) -> AccessMode {
        self.secrets.access_mode()
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(VERSION);
        self.secrets.encode(out);
        out.extend_from_slice(self.name.as_bytes());
    }

    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let input = decode_version(input)?;
        let (secrets, input) = AccessSecrets::decode(input)?;
        let name = str::from_utf8(input)?.to_owned();

        Ok(Self { secrets, name })
    }
}

impl From<AccessSecrets> for ShareToken {
    fn from(secrets: AccessSecrets) -> Self {
        Self {
            secrets,
            name: String::new(),
        }
    }
}

impl FromStr for ShareToken {
    type Err = DecodeError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        // Trim from the end as well because reading lines from a file includes the `\n` character.
        // Also the user may accidentally include white space if done from the app.
        let input = input.trim();
        let input = input.strip_prefix(SCHEME).ok_or(DecodeError)?;
        let input = input.strip_prefix(':').ok_or(DecodeError)?;

        let (input, params) = input.split_once('?').unwrap_or((input, ""));

        let input = Zeroizing::new(base64::decode_config(input, base64::URL_SAFE_NO_PAD)?);
        let input = decode_version(&input)?;

        let (secrets, _) = AccessSecrets::decode(input)?;
        let name = parse_name(params)?;

        Ok(Self::from(secrets).with_name(name))
    }
}

fn parse_name(query: &str) -> Result<String, DecodeError> {
    let value = query
        .split('&')
        .find_map(|param| param.strip_prefix("name="))
        .unwrap_or("");

    Ok(urlencoding::decode(value)?.into_owned())
}

fn decode_version(input: &[u8]) -> Result<&[u8], DecodeError> {
    let (first, rest) = input.split_first().ok_or(DecodeError)?;
    if *first == VERSION {
        Ok(rest)
    } else {
        Err(DecodeError)
    }
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:", SCHEME)?;

        let mut buffer = vec![VERSION];
        self.secrets.encode(&mut buffer);

        write!(
            f,
            "{}",
            base64::encode_config(buffer, base64::URL_SAFE_NO_PAD)
        )?;

        if !self.name.is_empty() {
            write!(f, "?name={}", urlencoding::encode(&self.name))?
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{cipher, sign};
    use assert_matches::assert_matches;

    #[test]
    fn to_string_from_string_blind() {
        let token_id = RepositoryId::random();
        let token = ShareToken::from(AccessSecrets::Blind { id: token_id });

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, "");
        assert_matches!(decoded.secrets, AccessSecrets::Blind { id } => {
            assert_eq!(id, token_id)
        });
    }

    #[test]
    fn to_string_from_string_blind_with_name() {
        let token_id = RepositoryId::random();
        let token = ShareToken::from(AccessSecrets::Blind { id: token_id }).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Blind { id } => assert_eq!(id, token_id));
    }

    #[test]
    fn to_string_from_string_reader() {
        let token_id = RepositoryId::random();
        let token_read_key = cipher::SecretKey::random();
        let token = ShareToken::from(AccessSecrets::Read {
            id: token_id,
            read_key: token_read_key.clone(),
        })
        .with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Read { id, read_key } => {
            assert_eq!(id, token_id);
            assert_eq!(read_key.as_ref(), token_read_key.as_ref());
        });
    }

    #[test]
    fn to_string_from_string_writer() {
        let token_write_keys = sign::Keypair::random();
        let token_id = RepositoryId::from(token_write_keys.public);

        let token =
            ShareToken::from(AccessSecrets::Write(token_write_keys.into())).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Write(access) => {
            assert_eq!(access.id, token_id);
        });
    }

    #[test]
    fn encode_and_decode() {
        let token_id = RepositoryId::random();
        let token_read_key = cipher::SecretKey::random();
        let token = ShareToken::from(AccessSecrets::Read {
            id: token_id,
            read_key: token_read_key.clone(),
        })
        .with_name("foo");

        let mut buffer = vec![];
        token.encode(&mut buffer);

        let decoded = ShareToken::decode(&buffer).unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Read { id, read_key } => {
            assert_eq!(id, token_id);
            assert_eq!(read_key.as_ref(), token_read_key.as_ref());
        });
    }
}