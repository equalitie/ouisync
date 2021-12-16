use super::writer_request::WriterRequest;
use crate::{
    crypto::{sign, SecretKey},
    error::Error,
    repository::RepositoryId,
};
use std::{
    borrow::Cow,
    fmt,
    io::{self, Cursor, Read, Write},
    slice,
    str::FromStr,
    string::FromUtf8Error,
};
use thiserror::Error;

pub const SCHEME: &str = "ouisync";
pub const VERSION: u8 = 0; // when this reaches 128, switch to variable-lengh encoding.

/// Token to share a repository which can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Debug)]
pub struct ShareToken {
    id: RepositoryId,
    access: Access,
    name: String,
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
            access: Access::Blind,
            name: "".to_owned(),
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

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim_start();
        let input = input.strip_prefix(SCHEME).ok_or(DecodeError)?;
        let input = input.strip_prefix(':').ok_or(DecodeError)?;

        let (input, params) = input.split_once('?').unwrap_or((input, ""));

        let mut cursor = Cursor::new(input);
        let mut decoder = base64::read::DecoderReader::new(&mut cursor, base64::URL_SAFE_NO_PAD);

        let version = read_version(&mut decoder)?;
        if version > VERSION {
            return Err(DecodeError);
        }

        let id = RepositoryId::from(read_array(&mut decoder)?);
        let access = read_access(&mut decoder)?;

        let name = parse_name(params)?;

        Ok(Self { id, access, name })
    }
}

fn read_version<R>(reader: &mut R) -> io::Result<u8>
where
    R: Read,
{
    let mut byte = 0u8;
    reader.read_exact(slice::from_mut(&mut byte))?;
    Ok(byte)
}

fn read_access<R>(reader: &mut R) -> io::Result<Access>
where
    R: io::Read,
{
    let read_key = if let Some(array) = none_on_eof(read_array(reader))? {
        SecretKey::from(array)
    } else {
        return Ok(Access::Blind);
    };

    let giver_pk = if let Some(array) = none_on_eof(read_array(reader))? {
        sign::PublicKey::from(array)
    } else {
        return Ok(Access::Reader { read_key });
    };

    let nonce_sk = sign::SecretKey::from(read_array(reader)?);

    let nonce_pk_signature: [u8; sign::Signature::SIZE] = read_array(reader)?;
    let nonce_pk_signature = sign::Signature::try_from(nonce_pk_signature.as_ref())
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;

    Ok(Access::Writer {
        read_key,
        giver_pk,
        nonce_sk,
        nonce_pk_signature,
    })
}

fn read_array<R, const N: usize>(reader: &mut R) -> io::Result<[u8; N]>
where
    R: io::Read,
{
    let mut buffer = [0; N];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn none_on_eof<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
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

        let mut encoder = base64::write::EncoderStringWriter::new(base64::URL_SAFE_NO_PAD);
        encoder.write_all(VERSION.to_be_bytes().as_ref()).unwrap();
        encoder.write_all(self.id.as_ref()).unwrap();

        if let Some(read_key) = self.read_key() {
            encoder.write_all(read_key.as_array().as_ref()).unwrap();
        }

        if let Access::Writer {
            read_key: _,
            giver_pk,
            nonce_sk,
            nonce_pk_signature,
        } = &self.access
        {
            encoder.write_all(giver_pk.as_ref()).unwrap();
            encoder.write_all(nonce_sk.as_ref()).unwrap();
            encoder.write_all(nonce_pk_signature.as_ref()).unwrap();
        }

        write!(f, "{}", encoder.into_inner())?;

        if !self.name.is_empty() {
            write!(f, "?name={}", urlencoding::encode(&self.name))?
        }

        Ok(())
    }
}

#[derive(Debug, Error)]
#[error("failed to decode share token")]
pub struct DecodeError;

impl From<io::Error> for DecodeError {
    fn from(_: io::Error) -> Self {
        Self
    }
}

impl From<FromUtf8Error> for DecodeError {
    fn from(_: FromUtf8Error) -> Self {
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
    fn encode_and_decode_blind() {
        let id: RepositoryId = rand::random();
        let token = ShareToken::new(id);

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.id, token.id);
        assert_eq!(decoded.name, "");
        assert_matches!(decoded.access, Access::Blind);
    }

    #[test]
    fn encode_and_decode_blind_with_name() {
        let id: RepositoryId = rand::random();
        let token = ShareToken::new(id).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.id, token.id);
        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.access, Access::Blind);
    }

    #[test]
    fn encode_and_decode_reader() {
        let id: RepositoryId = rand::random();
        let read_key = SecretKey::random();
        let token = ShareToken::new(id).for_reader(read_key).with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.id, token.id);
        assert_eq!(decoded.name, token.name);
        assert_matches!(decoded.access, Access::Reader { read_key } => {
            assert_eq!(read_key.as_array(), token.read_key().unwrap().as_array())
        });
    }

    #[test]
    fn encode_and_decode_writer() {
        let id: RepositoryId = rand::random();
        let read_key = SecretKey::random();
        let giver_keys = sign::Keypair::generate();

        let token = ShareToken::new(id)
            .for_writer(read_key, &giver_keys)
            .with_name("foo");

        let encoded = token.to_string();
        let decoded: ShareToken = encoded.parse().unwrap();

        assert_eq!(decoded.id, token.id);
        assert_eq!(decoded.name, token.name);

        match (token.access, decoded.access) {
            (
                Access::Writer {
                    read_key: orig_read_key,
                    giver_pk: orig_giver_pk,
                    nonce_sk: orig_nonce_sk,
                    nonce_pk_signature: orig_nonce_pk_signature,
                },
                Access::Writer {
                    read_key: decoded_read_key,
                    giver_pk: decoded_giver_pk,
                    nonce_sk: decoded_nonce_sk,
                    nonce_pk_signature: decoded_nonce_pk_signature,
                },
            ) => {
                assert_eq!(decoded_read_key.as_array(), orig_read_key.as_array());
                assert_eq!(decoded_giver_pk, orig_giver_pk);
                assert_eq!(decoded_nonce_sk.as_ref(), orig_nonce_sk.as_ref());
                assert_eq!(decoded_nonce_pk_signature, orig_nonce_pk_signature);
            }
            (Access::Writer { .. }, decoded) => panic!("unexpected decoded access {:?}", decoded),
            (_, _) => unreachable!(),
        }
    }
}
