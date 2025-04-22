use super::{AccessMode, AccessSecrets, DecodeError};
use crate::protocol::RepositoryId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64, Engine};
use bincode::Options;
use ouisync_macros::api;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    str::{self, FromStr},
};
use zeroize::Zeroizing;

pub const PREFIX: &str = "https://ouisync.net/r";
pub const VERSION: u64 = 1;

/// Token to share a repository. It can be encoded as a URL-formatted string and transmitted to
/// other replicas.
#[derive(Clone, Eq, PartialEq, Debug)]
#[api(repr(String))]
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

        let input = Zeroizing::new(BASE64.decode(input)?);
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

        write!(f, "{}", BASE64.encode(buffer))?;

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
    use super::{super::WriteSecrets, *};
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

    #[test]
    fn snapshot() {
        let test_vectors = [
            (
                "7f6f2ccdb23f2abb7b69278e947c01c6160a31cf02c19d06d0f6e5ab1d768b95",
                "https://ouisync.net/r#AwIgf28szbI_Krt7aSeOlHwBxhYKMc8CwZ0G0Pblqx12i5U",
                "https://ouisync.net/r#AwEg7hqkmkRZ3-gTo89uuIIEEjDHslWEad6B-Hyb8jvxCgMgaaT2LXiRw4hdhJFw84_9uSUJ4ztutD9NnENPVnVU1rY",
                "https://ouisync.net/r#AwAg7hqkmkRZ3-gTo89uuIIEEjDHslWEad6B-Hyb8jvxCgM",
            ),
            (
                "117be1de549d1d4322c4711f11efa0c5137903124f85fc37c761ffc91ace30cb",
                "https://ouisync.net/r#AwIgEXvh3lSdHUMixHEfEe-gxRN5AxJPhfw3x2H_yRrOMMs",
                "https://ouisync.net/r#AwEgDCKeJ6jGnnr-hpAN-86rbvbSB1gsEnoNmf6bP7WiBoogs3QJBS11slqiANc0z-ep6TGMokjJxkxzb-tPahjEjFg",
                "https://ouisync.net/r#AwAgDCKeJ6jGnnr-hpAN-86rbvbSB1gsEnoNmf6bP7WiBoo",
            ),
            (
                "ac7f0d9eaea4d4bf5438b887e34d0cf87e7f98d97da70eff001850487b2cae23",
                "https://ouisync.net/r#AwIgrH8Nnq6k1L9UOLiH400M-H5_mNl9pw7_ABhQSHssriM",
                "https://ouisync.net/r#AwEgKmhZmO5ElTo-sKXTFpN_gQqAvcyVLAqge02Cs_7UWcIgZaQ5F6Mkfsakp1H_AZvkilY9u25vetBkxbVwcdiXkhY",
                "https://ouisync.net/r#AwAgKmhZmO5ElTo-sKXTFpN_gQqAvcyVLAqge02Cs_7UWcI",
            ),
            (
                "bbb7d40b7bb8e41c550696fdef78fff6f013bb34627ba50ca2d63b6e84cffa6c",
                "https://ouisync.net/r#AwIgu7fUC3u45BxVBpb973j_9vATuzRie6UMotY7boTP-mw",
                "https://ouisync.net/r#AwEgf3_dHKjXw-2CBhNxeLR7yv56VNSgtM5b2eJZeBhLSM4g4l_rARpMZdhkhudmQlO4xgvJC9kf7FAhAy-7XmARe6Q",
                "https://ouisync.net/r#AwAgf3_dHKjXw-2CBhNxeLR7yv56VNSgtM5b2eJZeBhLSM4",
            ),
            (
                "9a32e1a6638ce87528a3f0303c7a9cecba4ed5fef0551f3afd1c7865bc66308f",
                "https://ouisync.net/r#AwIgmjLhpmOM6HUoo_AwPHqc7LpO1f7wVR86_Rx4ZbxmMI8",
                "https://ouisync.net/r#AwEgPzwkeuagmeh1RyeOg9K1Gta4-4X_HXJl6EnPeCqHHXAgigK_99pB7Mqq75MYmxnffBzudb0swtGDZXG7IfTe0Xk",
                "https://ouisync.net/r#AwAgPzwkeuagmeh1RyeOg9K1Gta4-4X_HXJl6EnPeCqHHXA",
            ),
            (
                "eb6a97af1f95c72764a092b8794ce3d5b14fef7697095f34f33e5f13a814cd80",
                "https://ouisync.net/r#AwIg62qXrx-VxydkoJK4eUzj1bFP73aXCV808z5fE6gUzYA",
                "https://ouisync.net/r#AwEgQPSvtDyRS7cw7gUOo6MY6Yy7Si5n3vXQ6H1yqkULlzggnNrwTb8H3OrkTNEt2E1LjjdUGSvCqqxknmtbaCJUTGU",
                "https://ouisync.net/r#AwAgQPSvtDyRS7cw7gUOo6MY6Yy7Si5n3vXQ6H1yqkULlzg",
            ),
            (
                "8d4e5ee1d08b43a3d80457bf09e0957a2f922b58e79646e02a2529cb7c99e3de",
                "https://ouisync.net/r#AwIgjU5e4dCLQ6PYBFe_CeCVei-SK1jnlkbgKiUpy3yZ494",
                "https://ouisync.net/r#AwEgVtEZXw7riIBZBZBedrmi3XfzuevmO1No3sbyIv9TomsgEttex_j8LvPYeT6CesaXqTh9XY1JViWmp0FGblnUAOM",
                "https://ouisync.net/r#AwAgVtEZXw7riIBZBZBedrmi3XfzuevmO1No3sbyIv9Toms",
            ),
            (
                "560162bb28f02f1015a3dcec38dca4fc73535b298b0b8037077edc6fe22b20fa",
                "https://ouisync.net/r#AwIgVgFiuyjwLxAVo9zsONyk_HNTWymLC4A3B37cb-IrIPo",
                "https://ouisync.net/r#AwEg5SzdGSsWn9u9YLqjK6991V6Yuh_chZAR27P0CIpAHsggY-m-q0MtHuQ1h9zpJNXpEQOnOiC5nEfPquVyGEqHTKA",
                "https://ouisync.net/r#AwAg5SzdGSsWn9u9YLqjK6991V6Yuh_chZAR27P0CIpAHsg",
            ),
            (
                "72cc0b4cee98ddeaa5a0626311355dad94690e6110aed80397ea92d13a82b811",
                "https://ouisync.net/r#AwIgcswLTO6Y3eqloGJjETVdrZRpDmEQrtgDl-qS0TqCuBE",
                "https://ouisync.net/r#AwEg0qIB6AJ497v8pBn15tTP19jYkxp9ePRSM73lR51ObVwgIynW_5WAjN3jkOy9KgIxCMzDInogwcpVE9JhyY-1fIg",
                "https://ouisync.net/r#AwAg0qIB6AJ497v8pBn15tTP19jYkxp9ePRSM73lR51ObVw",
            ),
            (
                "e3fd1ddb28613f9ead9869b392fa1f9d91bef1ab625605c968c72f5312ac77aa",
                "https://ouisync.net/r#AwIg4_0d2yhhP56tmGmzkvofnZG-8atiVgXJaMcvUxKsd6o",
                "https://ouisync.net/r#AwEg824LDR5VNan2dNT220rBhdojmoD6BxnZ-olCblcMfPcgtGlE43E57F6gLJz3dee8c-8rH0JBpiZ1np-YbGYtW0k",
                "https://ouisync.net/r#AwAg824LDR5VNan2dNT220rBhdojmoD6BxnZ-olCblcMfPc",
            ),
            (
                "8e8958dddba89b1547d9efd010b37e156ebc6bf41a4dca84d67c6bdca88ac0c0",
                "https://ouisync.net/r#AwIgjolY3duomxVH2e_QELN-FW68a_QaTcqE1nxr3KiKwMA",
                "https://ouisync.net/r#AwEgsSu6XyhPQ82lz7XsBN4N4VxDW1JQD3WzQleLsxVPqjggCrXQ5tAbYgwX_0SXtY81S3zIY2Ta6bsIG_t7ySzj2qM",
                "https://ouisync.net/r#AwAgsSu6XyhPQ82lz7XsBN4N4VxDW1JQD3WzQleLsxVPqjg",
            ),
            (
                "c3420141d57426de45356f2a84456d169bc4593c8a23e359b898dbe51b4ef62f",
                "https://ouisync.net/r#AwIgw0IBQdV0Jt5FNW8qhEVtFpvEWTyKI-NZuJjb5RtO9i8",
                "https://ouisync.net/r#AwEgwqmZu-rGbq9Ph2YbCnlWY37fGbri2BHJP43m7dLqgxwgJjbsCKHZG3k7DfrUYNdjnKfL588jkstlJ1mg5-xMehM",
                "https://ouisync.net/r#AwAgwqmZu-rGbq9Ph2YbCnlWY37fGbri2BHJP43m7dLqgxw",
            ),
            (
                "10eb99e218c5be855df2859f267d07fa024a19693b96894336ac426710210a40",
                "https://ouisync.net/r#AwIgEOuZ4hjFvoVd8oWfJn0H-gJKGWk7lolDNqxCZxAhCkA",
                "https://ouisync.net/r#AwEgUgZT1ofDl1Vo6V1OqxBNTlsEpATy8aYgk3E0iFFIMe8gaqaBmsR0qk8h1zBKWsinZk2IjkJtKNSa6to-bbmoavk",
                "https://ouisync.net/r#AwAgUgZT1ofDl1Vo6V1OqxBNTlsEpATy8aYgk3E0iFFIMe8",
            ),
            (
                "ceadb07bb79926fcd95ad7fff5d37faa7dd2df1849940cacd208a79fb3a0b2ca",
                "https://ouisync.net/r#AwIgzq2we7eZJvzZWtf_9dN_qn3S3xhJlAys0ginn7Ogsso",
                "https://ouisync.net/r#AwEgkbc-pQLI2oVLZBF1vUKwCCzqyryexuRJZTfrjLHdG1gg0t9r4gtj7SZuBi63NtCDd-y3nTUp6VkvtErsHgs3af8",
                "https://ouisync.net/r#AwAgkbc-pQLI2oVLZBF1vUKwCCzqyryexuRJZTfrjLHdG1g",
            ),
                        (
                "87cb50b2635ce54aa15782cd2b9c6ca22b2501eabdf3c808fa1ab8ea93176615",
                "https://ouisync.net/r#AwIgh8tQsmNc5UqhV4LNK5xsoislAeq988gI-hq46pMXZhU",
                "https://ouisync.net/r#AwEgXw0gkijsdg0BggUw3N89-tQFzIp3a8nAOp4Gq8z2a3QgJI7N12DueZ4ev7WiJiWUDc20Ugffci3PqG6MJxey0WE",
                "https://ouisync.net/r#AwAgXw0gkijsdg0BggUw3N89-tQFzIp3a8nAOp4Gq8z2a3Q",
            ),
            (
                "27e0fb035ae58c31ffa84dbf181ce7a72ab61bbefb8bda94950b7c0fdcbacf2f",
                "https://ouisync.net/r#AwIgJ-D7A1rljDH_qE2_GBznpyq2G777i9qUlQt8D9y6zy8",
                "https://ouisync.net/r#AwEgqTfkcLMFVLpFuwxEi9dhWtBKDBfkLVgUI0LohEAQ2ycgc-EyPvCft0S-gRCim6DTMjWhJnxvOuuYWyid0Uee6-A",
                "https://ouisync.net/r#AwAgqTfkcLMFVLpFuwxEi9dhWtBKDBfkLVgUI0LohEAQ2yc",
            ),
        ];

        for (secret_key_hex, expected_write, expected_read, expected_blind) in test_vectors {
            let mut secret_key = [0; sign::Keypair::SECRET_KEY_SIZE];
            hex::decode_to_slice(secret_key_hex, &mut secret_key).unwrap();

            let secrets =
                AccessSecrets::Write(WriteSecrets::from(sign::Keypair::from(&secret_key)));

            assert_eq!(
                ShareToken::from(secrets.clone()).to_string(),
                expected_write,
            );

            assert_eq!(
                ShareToken::from(secrets.with_mode(AccessMode::Read)).to_string(),
                expected_read,
            );

            assert_eq!(
                ShareToken::from(secrets.with_mode(AccessMode::Blind)).to_string(),
                expected_blind,
            );
        }
    }
}
