use super::{AccessMode, AccessSecrets, DecodeError};
use crate::protocol::RepositoryId;
use bincode::Options;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    str::{self, FromStr},
};
use zeroize::Zeroizing;

pub const PREFIX: &str = "https://ouisync.net/r";
pub const VERSION: u64 = 1;

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Clone, Eq, PartialEq, Debug)]
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

    /// Suggested name of the repository, if provided.
    pub fn suggested_name(&self) -> &str {
        &self.name
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
        let input = input.strip_prefix(PREFIX).ok_or(DecodeError)?;

        // The '/' before '#...' is optional.
        let input = match input.strip_prefix('/') {
            Some(input) => input,
            None => input,
        };

        let input = input.strip_prefix('#').ok_or(DecodeError)?;

        let (input, params) = input.split_once('?').unwrap_or((input, ""));

        let input = Zeroizing::new(base64::decode_config(input, base64::URL_SAFE_NO_PAD)?);
        let input = decode_version(&input)?;

        let secrets: AccessSecrets = bincode::options().deserialize(input)?;
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

fn encode_version(output: &mut Vec<u8>, version: u64) {
    let version = vint64::encode(version);
    output.extend_from_slice(version.as_ref());
}

fn decode_version(mut input: &[u8]) -> Result<&[u8], DecodeError> {
    let version = vint64::decode(&mut input).map_err(|_| DecodeError)?;
    if version == VERSION {
        Ok(input)
    } else {
        Err(DecodeError)
    }
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}#", PREFIX)?;

        let mut buffer = Vec::new();
        encode_version(&mut buffer, VERSION);
        bincode::options()
            .serialize_into(&mut buffer, &self.secrets)
            .map_err(|_| fmt::Error)?;

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

impl Serialize for ShareToken {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_string().serialize(s)
    }
}

impl<'de> Deserialize<'de> for ShareToken {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <&str>::deserialize(d)?;
        let v = s.parse().map_err(serde::de::Error::custom)?;
        Ok(v)
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
        let token_id = RepositoryId::from(token_write_keys.public_key());

        let token =
            ShareToken::from(AccessSecrets::Write(token_write_keys.into())).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Write(access) => {
            assert_eq!(access.id, token_id);
        });
    }
}
