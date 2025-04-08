use crate::{file::FileHandle, repository::RepositoryHandle};
use ouisync::{
    crypto::{cipher::SecretKey, PasswordSalt},
    AccessMode, EntryType, NatBehavior, NetworkEvent, PeerAddr, PeerInfo, Progress,
    PublicRuntimeId, ShareToken, Stats, StorageSize,
};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use state_monitor::StateMonitor;
use std::{
    collections::BTreeMap,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

use super::{
    helpers::{self, Bytes},
    ProtocolError,
};

// The `Response` enum is auto-generated in `build.rs` from the `#[api]` annotated methods in `impl
// State` and `impl Service`.
include!(concat!(env!("OUT_DIR"), "/response.rs"));

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum ResponseResult {
    Success(Response),
    Failure(ProtocolError),
}

impl From<Result<Response, ProtocolError>> for ResponseResult {
    fn from(result: Result<Response, ProtocolError>) -> Self {
        match result {
            Ok(response) => Self::Success(response),
            Err(error) => Self::Failure(error),
        }
    }
}

impl From<ResponseResult> for Result<Response, ProtocolError> {
    fn from(payload: ResponseResult) -> Self {
        match payload {
            ResponseResult::Success(response) => Ok(response),
            ResponseResult::Failure(error) => Err(error),
        }
    }
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl TryFrom<Response> for () {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::None => Ok(()),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl<T> From<Option<T>> for Response
where
    Self: From<T>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::from(value),
            None => Self::None,
        }
    }
}

impl<'a> From<&'a Path> for Response {
    fn from(value: &'a Path) -> Self {
        Self::Path(value.to_owned())
    }
}

impl From<Vec<u8>> for Response {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value.into())
    }
}

impl<'a> From<&'a str> for Response {
    fn from(value: &'a str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<SocketAddrV4> for Response {
    fn from(value: SocketAddrV4) -> Self {
        Self::SocketAddr(value.into())
    }
}

impl From<SocketAddrV6> for Response {
    fn from(value: SocketAddrV6) -> Self {
        Self::SocketAddr(value.into())
    }
}

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[api]
pub struct DirectoryEntry {
    pub name: String,
    pub entry_type: EntryType,
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[api]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use ouisync::{AccessSecrets, PeerSource, PeerState, Stats, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;

    #[test]
    fn serialize_deserialize_msgpack() {
        use rmp::encode::*;

        let mut rng = StdRng::seed_from_u64(0);
        let token = ShareToken::from(AccessSecrets::Write(WriteSecrets::generate(&mut rng)));

        let test_vectors = [
            (Response::None, {
                let mut out = Vec::new();
                write_str(&mut out, "None").unwrap();
                out
            }),
            (Response::Bool(true), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "Bool").unwrap();
                write_bool(&mut out, true).unwrap();
                out
            }),
            (Response::Bool(false), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "Bool").unwrap();
                write_bool(&mut out, false).unwrap();
                out
            }),
            (Response::Duration(Duration::from_secs(1)), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "Duration").unwrap();
                write_uint(&mut out, 1000).unwrap();
                out
            }),
            (
                Response::NetworkEvent(NetworkEvent::ProtocolVersionMismatch),
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "NetworkEvent").unwrap();
                    write_uint(&mut out, 0).unwrap();
                    out
                },
            ),
            (Response::NetworkEvent(NetworkEvent::PeerSetChange), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "NetworkEvent").unwrap();
                write_uint(&mut out, 1).unwrap();
                out
            }),
            (Response::RepositoryEvent, {
                let mut out = Vec::new();
                write_str(&mut out, "RepositoryEvent").unwrap();
                out
            }),
            (Response::Path(PathBuf::from("/home/alice/ouisync")), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "Path").unwrap();
                write_str(&mut out, "/home/alice/ouisync").unwrap();
                out
            }),
            (Response::Repository(RepositoryHandle::from_raw(1)), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "Repository").unwrap();
                write_uint(&mut out, 1).unwrap();
                out
            }),
            (
                Response::Repositories(
                    [
                        ("one".into(), RepositoryHandle::from_raw(1)),
                        ("two".into(), RepositoryHandle::from_raw(2)),
                    ]
                    .into(),
                ),
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "Repositories").unwrap();
                    write_map_len(&mut out, 2).unwrap();
                    write_str(&mut out, "one").unwrap();
                    write_uint(&mut out, 1).unwrap();
                    write_str(&mut out, "two").unwrap();
                    write_uint(&mut out, 2).unwrap();
                    out
                },
            ),
            (
                Response::PeerInfos(vec![PeerInfo {
                    addr: PeerAddr::Quic((Ipv4Addr::LOCALHOST, 12345).into()),
                    source: PeerSource::Listener,
                    state: PeerState::Connecting,
                    stats: Stats {
                        bytes_tx: 0,
                        bytes_rx: 0,
                        throughput_tx: 0,
                        throughput_rx: 0,
                    },
                }]),
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "PeerInfos").unwrap();
                    write_array_len(&mut out, 1).unwrap();
                    write_array_len(&mut out, 4).unwrap();
                    write_str(&mut out, "quic/127.0.0.1:12345").unwrap();
                    write_uint(&mut out, 1).unwrap();
                    write_str(&mut out, "Connecting").unwrap();
                    write_array_len(&mut out, 4).unwrap();
                    write_uint(&mut out, 0).unwrap();
                    write_uint(&mut out, 0).unwrap();
                    write_uint(&mut out, 0).unwrap();
                    write_uint(&mut out, 0).unwrap();
                    out
                },
            ),
            (
                Response::QuotaInfo(QuotaInfo {
                    quota: Some(StorageSize::from_bytes(10 * 1024 * 1024)),
                    size: StorageSize::from_bytes(1024 * 1024),
                }),
                {
                    let mut out = Vec::new();
                    write_map_len(&mut out, 1).unwrap();
                    write_str(&mut out, "QuotaInfo").unwrap();
                    write_array_len(&mut out, 2).unwrap();
                    write_uint(&mut out, 10 * 1024 * 1024).unwrap();
                    write_uint(&mut out, 1024 * 1024).unwrap();
                    out
                },
            ),
            (Response::ShareToken(token.clone()), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "ShareToken").unwrap();
                write_str(&mut out, &token.to_string()).unwrap();
                out
            }),
            (Response::SocketAddr((Ipv4Addr::LOCALHOST, 24816).into()), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "SocketAddr").unwrap();
                write_str(&mut out, "127.0.0.1:24816").unwrap();
                out
            }),
            (Response::StorageSize(StorageSize::from_bytes(1024)), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "StorageSize").unwrap();
                write_uint(&mut out, 1024).unwrap();
                out
            }),
            (Response::U16(3), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "U16").unwrap();
                write_uint(&mut out, 3).unwrap();
                out
            }),
            (Response::U64(12345678), {
                let mut out = Vec::new();
                write_map_len(&mut out, 1).unwrap();
                write_str(&mut out, "U64").unwrap();
                write_uint(&mut out, 12345678).unwrap();
                out
            }),
        ];

        for (input, expected) in test_vectors {
            let s = rmp_serde::to_vec(&input).unwrap();
            assert_eq!(s, expected);

            let d: Response = rmp_serde::from_slice(&s).unwrap();
            assert_eq!(d, input);
        }
    }
}
