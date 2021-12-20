use super::{AccessSecrets, WriteSecrets};
use crate::{crypto::sign, error::Error, repository::RepositoryId};
use std::{borrow::Cow, fmt, str::FromStr, string::FromUtf8Error};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

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
    /// Create share token with the given access secrets.
    pub(crate) fn new(secrets: AccessSecrets) -> Self {
        Self {
            secrets,
            name: "".to_owned(),
        }
    }

    /// Attach a suggested repository name to the token.
    pub(crate) fn with_name(self, name: impl Into<String>) -> Self {
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
}

#[repr(u8)]
enum AccessMode {
    Blind = 0,
    Read = 1,
    Write = 2,
}

impl FromStr for ShareToken {
    type Err = DecodeError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim_start();
        let input = input.strip_prefix(SCHEME).ok_or(DecodeError)?;
        let input = input.strip_prefix(':').ok_or(DecodeError)?;

        let (input, params) = input.split_once('?').unwrap_or((input, ""));

        let input = Zeroizing::new(base64::decode_config(input, base64::URL_SAFE_NO_PAD)?);
        let mut input = &input[..];

        let version = read_byte(&mut input)?;
        if version > VERSION {
            return Err(DecodeError);
        }

        let mode = read_mode(&mut input)?;
        let secrets = match mode {
            AccessMode::Blind => {
                let id = read_key(&mut input)?;
                AccessSecrets::Blind { id }
            }
            AccessMode::Read => {
                let id = read_key(&mut input)?;
                let read_key = read_key(&mut input)?;
                AccessSecrets::Read { id, read_key }
            }
            AccessMode::Write => {
                let write_key = read_key(&mut input)?;
                let secrets = WriteSecrets::new(write_key);
                AccessSecrets::Write(secrets)
            }
        };

        let name = parse_name(params)?;

        Ok(Self { secrets, name })
    }
}

fn read_mode(input: &mut &[u8]) -> Result<AccessMode, DecodeError> {
    match read_byte(input)? {
        b if b == AccessMode::Blind as u8 => Ok(AccessMode::Blind),
        b if b == AccessMode::Read as u8 => Ok(AccessMode::Read),
        b if b == AccessMode::Write as u8 => Ok(AccessMode::Write),
        _ => Err(DecodeError),
    }
}

fn read_byte(input: &mut &[u8]) -> Result<u8, DecodeError> {
    if let Some(b) = input.get(0).copied() {
        *input = &input[1..];
        Ok(b)
    } else {
        Err(DecodeError)
    }
}

fn read_key<T, const N: usize>(input: &mut &[u8]) -> Result<T, DecodeError>
where
    T: From<[u8; N]>,
{
    if N <= input.len() {
        let (output_slice, new_input) = input.split_at(N);

        let mut output_array: [u8; N] = output_slice.try_into().unwrap();
        let output_key = T::from(output_array);
        output_array.zeroize();

        *input = new_input;

        Ok(output_key)
    } else {
        Err(DecodeError)
    }
}

fn parse_name(query: &str) -> Result<String, DecodeError> {
    let value = query
        .split('&')
        .find_map(|param| param.strip_prefix("name="))
        .unwrap_or("");

    Ok(urlencoding::decode(value)?.into_owned())
}

impl fmt::Display for ShareToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:", SCHEME)?;

        let mut buffer = vec![VERSION];

        match &self.secrets {
            AccessSecrets::Blind { id } => {
                buffer.push(AccessMode::Blind as u8);
                buffer.extend_from_slice(id.as_ref());
            }
            AccessSecrets::Read { id, read_key } => {
                buffer.push(AccessMode::Read as u8);
                buffer.extend_from_slice(id.as_ref());
                buffer.extend_from_slice(read_key.as_array().as_ref());
            }
            AccessSecrets::Write(secrets) => {
                buffer.push(AccessMode::Write as u8);
                buffer.extend_from_slice(secrets.write_key.as_ref());
            }
        }

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

#[derive(Debug, Error)]
#[error("failed to decode share token")]
pub struct DecodeError;

impl From<FromUtf8Error> for DecodeError {
    fn from(_: FromUtf8Error) -> Self {
        Self
    }
}

impl From<base64::DecodeError> for DecodeError {
    fn from(_: base64::DecodeError) -> Self {
        Self
    }
}

impl From<sign::SignatureError> for DecodeError {
    fn from(_: sign::SignatureError) -> Self {
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
    use crate::crypto::SecretKey;
    use assert_matches::assert_matches;

    #[test]
    fn encode_and_decode_blind() {
        let token_id: RepositoryId = rand::random();
        let token = ShareToken::new(AccessSecrets::Blind { id: token_id });

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, "");
        assert_matches!(decoded.secrets, AccessSecrets::Blind { id } => {
            assert_eq!(id, token_id)
        });
    }

    #[test]
    fn encode_and_decode_blind_with_name() {
        let token_id: RepositoryId = rand::random();
        let token = ShareToken::new(AccessSecrets::Blind { id: token_id }).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Blind { id } => assert_eq!(id, token_id));
    }

    #[test]
    fn encode_and_decode_reader() {
        let token_id: RepositoryId = rand::random();
        let token_read_key = SecretKey::random();
        let token = ShareToken::new(AccessSecrets::Read {
            id: token_id,
            read_key: token_read_key.clone(),
        })
        .with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Read { id, read_key } => {
            assert_eq!(id,token_id);
            assert_eq!(read_key.as_array(), token_read_key.as_array());
        });
    }

    #[test]
    fn encode_and_decode_writer() {
        let token_write_key: sign::SecretKey = rand::random();
        let token_id = RepositoryId::from(sign::PublicKey::from(&token_write_key));

        let token = ShareToken::new(AccessSecrets::Write(WriteSecrets::new(token_write_key)))
            .with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.secrets, AccessSecrets::Write(access) => {
            assert_eq!(access.id, token_id);
        });
    }
}
