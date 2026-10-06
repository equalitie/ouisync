use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use serde::{Deserialize, Deserializer, Serialize, de};

use super::auth::AuthKey;

/// Address of a service running on the same machine as the client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalAddr {
    /// TCP on loopback. Authentication is done with explicit handshake using the provided key.
    Tcp { addr: SocketAddr, auth_key: AuthKey },
    /// UNIX domain socket. Authentication is done via file permissions.
    Unix(PathBuf),
}

impl fmt::Display for LocalAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp { addr, auth_key } => write!(f, "tcp://{addr}?auth_key={auth_key:x}"),
            Self::Unix(path) => write!(f, "unix://{}", path.display()),
        }
    }
}

impl Serialize for LocalAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

// Support both the new format:
//
// ```json
// "tcp://127.0.0.1:8765?auth_key=...."
// ```
//
// ```json
// "unix:///var/run/ouisync.sock"
// ```
//
// and the legacy format:
//
// ```json
// {
//     port: 8765,
//     auth_key: "..."
// }
// ```
//
// ```json
// {
//     addr: 192.168.1.22,
//     port: 9999,
//     auth_key: "..."
// }
// ```
impl<'de> Deserialize<'de> for LocalAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = LocalAddr;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "string or map")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if let Some(v) = v.strip_prefix("tcp://") {
                    let (raw_addr, raw_auth_key) = v.split_once("?auth_key=").ok_or(
                        E::invalid_value(de::Unexpected::Str(v), &"'auth_key' query param"),
                    )?;

                    let addr = if let Ok(addr) = raw_addr.parse::<SocketAddr>() {
                        addr
                    } else if let Ok(addr) = raw_addr.parse::<IpAddr>() {
                        SocketAddr::from((addr, 0))
                    } else {
                        return Err(E::invalid_value(
                            de::Unexpected::Str(raw_addr),
                            &"valid socket address",
                        ));
                    };

                    let auth_key: AuthKey = raw_auth_key
                        .parse()
                        .map_err(|_| auth_key_parse_error(raw_auth_key))?;

                    return Ok(Self::Value::Tcp { addr, auth_key });
                }

                if let Some(v) = v.strip_prefix("unix://") {
                    let path = PathBuf::from(v);
                    return Ok(Self::Value::Unix(path));
                }

                Err(E::invalid_value(
                    de::Unexpected::Str(v),
                    &"string prefixed with 'tcp://' or 'unix://'",
                ))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut addr = Ipv4Addr::LOCALHOST;
                let mut port: Option<u16> = None;
                let mut auth_key: Option<AuthKey> = None;

                loop {
                    match map.next_key()? {
                        Some("addr") => {
                            addr = map.next_value()?;
                        }
                        Some("port") => {
                            port = Some(map.next_value()?);
                        }
                        Some("auth_key") => {
                            let raw: &str = map.next_value()?;
                            auth_key = Some(raw.parse().map_err(|_| auth_key_parse_error(raw))?);
                        }
                        Some(key) => {
                            return Err(de::Error::unknown_field(
                                key,
                                &["addr", "port", "auth_key"],
                            ));
                        }
                        None => break,
                    }
                }

                let port = port.ok_or_else(|| de::Error::missing_field("port"))?;
                let auth_key = auth_key.ok_or_else(|| de::Error::missing_field("auth_key"))?;

                Ok(Self::Value::Tcp {
                    addr: SocketAddr::from((addr, port)),
                    auth_key,
                })
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

fn auth_key_parse_error<E: de::Error>(input: &str) -> E {
    E::invalid_value(
        de::Unexpected::Str(input),
        &format!("hex string of {} characters", 2 * AuthKey::SIZE).as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{SeedableRng, rngs::StdRng};
    use std::net::Ipv4Addr;

    #[test]
    fn serialization() {
        let mut rng = StdRng::seed_from_u64(0);
        let auth_key = AuthKey::generate(&mut rng);
        let auth_key_hex = hex::encode(auth_key.as_bytes());

        let tcp_old_without_addr = (
            format!("{{\"port\":12345,\"auth_key\":\"{auth_key_hex}\"}}"),
            LocalAddr::Tcp {
                addr: (Ipv4Addr::LOCALHOST, 12345).into(),
                auth_key,
            },
        );
        let tcp_old_with_addr = (
            format!("{{\"addr\":\"192.168.1.42\",\"port\":12345,\"auth_key\":\"{auth_key_hex}\"}}"),
            LocalAddr::Tcp {
                addr: (Ipv4Addr::new(192, 168, 1, 42), 12345).into(),
                auth_key,
            },
        );
        let tcp_new = (
            format!("\"tcp://127.0.0.1:12345?auth_key={auth_key_hex}\""),
            LocalAddr::Tcp {
                addr: (Ipv4Addr::LOCALHOST, 12345).into(),
                auth_key,
            },
        );
        let tcp_new_without_port = (
            format!("\"tcp://127.0.0.1?auth_key={auth_key_hex}\""),
            LocalAddr::Tcp {
                addr: (Ipv4Addr::LOCALHOST, 0).into(),
                auth_key,
            },
        );
        let unix = (
            "\"unix:///var/run/ouisync.sock\"".to_owned(),
            LocalAddr::Unix(PathBuf::from("/var/run/ouisync.sock")),
        );

        for (serialized, expected) in [
            &tcp_old_without_addr,
            &tcp_old_with_addr,
            &tcp_new,
            &tcp_new_without_port,
            &unix,
        ] {
            let actual: LocalAddr = match serde_json::from_str(serialized) {
                Ok(value) => value,
                Err(error) => {
                    panic!("error deserializing {serialized:?}: {error:?}");
                }
            };

            assert_eq!(
                actual, *expected,
                "expected {serialized:?} to deserialize to {expected:?} but was {actual:?}"
            )
        }

        for (expected, value) in [&tcp_new, &unix] {
            let actual = serde_json::to_string(value).unwrap();
            assert_eq!(
                actual, *expected,
                "expected {value:?} to serialize to {expected:?} but was {actual:?}"
            );
        }
    }
}
