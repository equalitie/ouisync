use crate::{file::FileHandle, repository::RepositoryHandle};
use ouisync::{
    AccessMode, EntryType, NetworkEvent, PeerAddr, PeerInfo, Progress, ShareToken, Stats,
    StorageSize,
};
use serde::{Deserialize, Serialize};
use state_monitor::StateMonitor;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

use super::{
    helpers::{self, Bytes},
    ProtocolError,
};

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[expect(clippy::large_enum_variant)]
pub enum Response {
    None,
    AccessMode(AccessMode),
    Bool(bool),
    Bytes(Bytes),
    Directory(Vec<DirectoryEntry>),
    Duration(Duration),
    EntryType(EntryType),
    File(FileHandle),
    NetworkEvent(NetworkEvent),
    NetworkStats(Stats),
    Path(PathBuf),
    PeerAddrs(#[serde(with = "helpers::strs")] Vec<PeerAddr>),
    PeerInfo(Vec<PeerInfo>),
    Progress(Progress),
    QuotaInfo(QuotaInfo),
    Repository(RepositoryHandle),
    RepositoryEvent,
    Repositories(BTreeMap<PathBuf, RepositoryHandle>),
    ShareToken(ShareToken),
    SocketAddr(SocketAddr),
    SocketAddrs(Vec<SocketAddr>),
    StateMonitor(StateMonitor),
    StateMonitorEvent,
    StorageSize(StorageSize),
    String(String),
    U32(u32),
    U64(u64),
}

macro_rules! impl_response_conversion {
    ($variant:ident ( $ty:ty )) => {
        impl From<$ty> for Response {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }

        impl TryFrom<Response> for $ty {
            type Error = UnexpectedResponse;

            fn try_from(response: Response) -> Result<Self, Self::Error> {
                match response {
                    Response::$variant(value) => Ok(value),
                    _ => Err(UnexpectedResponse),
                }
            }
        }

        impl TryFrom<Response> for Option<$ty> {
            type Error = UnexpectedResponse;

            fn try_from(value: Response) -> Result<Self, Self::Error> {
                match value {
                    Response::$variant(value) => Ok(Some(value)),
                    Response::None => Ok(None),
                    _ => Err(UnexpectedResponse),
                }
            }
        }
    };
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

impl_response_conversion!(AccessMode(AccessMode));
impl_response_conversion!(Bool(bool));
impl_response_conversion!(Directory(Vec<DirectoryEntry>));
impl_response_conversion!(Duration(Duration));
impl_response_conversion!(EntryType(EntryType));
impl_response_conversion!(File(FileHandle));
impl_response_conversion!(NetworkEvent(NetworkEvent));
impl_response_conversion!(NetworkStats(Stats));
impl_response_conversion!(Path(PathBuf));
impl_response_conversion!(Progress(Progress));
impl_response_conversion!(Repository(RepositoryHandle));
impl_response_conversion!(Repositories(BTreeMap<PathBuf, RepositoryHandle>));
impl_response_conversion!(PeerInfo(Vec<PeerInfo>));
impl_response_conversion!(PeerAddrs(Vec<PeerAddr>));
impl_response_conversion!(ShareToken(ShareToken));
impl_response_conversion!(SocketAddr(SocketAddr));
impl_response_conversion!(SocketAddrs(Vec<SocketAddr>));
impl_response_conversion!(StateMonitor(StateMonitor));
impl_response_conversion!(StorageSize(StorageSize));
impl_response_conversion!(String(String));
impl_response_conversion!(QuotaInfo(QuotaInfo));
impl_response_conversion!(U32(u32));
impl_response_conversion!(U64(u64));

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub entry_type: EntryType,
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use ouisync::{AccessSecrets, PeerSource, PeerState, Stats, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;

    #[test]
    fn serialize() {
        let mut rng = StdRng::seed_from_u64(0);
        let token = ShareToken::from(AccessSecrets::Write(WriteSecrets::generate(&mut rng)));

        let test_vectors = [
            (Response::None, "a46e6f6e65"),
            (Response::Bool(true), "81a4626f6f6cc3"),
            (Response::Bool(false), "81a4626f6f6cc2"),
            (
                Response::Duration(Duration::from_secs(1)),
                "81a86475726174696f6e920100",
            ),
            (
                Response::NetworkEvent(NetworkEvent::ProtocolVersionMismatch),
                "81ad6e6574776f726b5f6576656e7400",
            ),
            (
                Response::NetworkEvent(NetworkEvent::PeerSetChange),
                "81ad6e6574776f726b5f6576656e7401",
            ),
            (
                Response::RepositoryEvent,
                "b07265706f7369746f72795f6576656e74",
            ),
            (
                Response::Path(PathBuf::from("/home/alice/ouisync")),
                "81a470617468b32f686f6d652f616c6963652f6f756973796e63",
            ),
            (
                Response::Repository(RepositoryHandle::from_raw(1)),
                "81aa7265706f7369746f727901",
            ),
            (
                Response::Repositories(
                    [
                        ("one".into(), RepositoryHandle::from_raw(1)),
                        ("two".into(), RepositoryHandle::from_raw(2)),
                    ]
                    .into(),
                ),
                "81ac7265706f7369746f7269657382a36f6e6501a374776f02",
            ),
            (
                Response::PeerInfo(vec![PeerInfo {
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
                "81a9706565725f696e666f9194b4717569632f3132372e302e302e313a3132333435010194000000\
                 00",
            ),
            (
                Response::QuotaInfo(QuotaInfo {
                    quota: Some(StorageSize::from_bytes(10 * 1024 * 1024)),
                    size: StorageSize::from_bytes(1024 * 1024),
                }),
                "81aa71756f74615f696e666f92ce00a00000ce00100000",
            ),
            (
                Response::ShareToken(token),
                "81ab73686172655f746f6b656ed94568747470733a2f2f6f756973796e632e6e65742f7223417749\
                 67663238737a62495f4b7274376153654f6c4877427868594b4d633843775a30473050626c717831\
                 32693555",
            ),
            (
                Response::SocketAddr((Ipv4Addr::LOCALHOST, 24816).into()),
                "81ab736f636b65745f6164647281a2563492947f000001cd60f0",
            ),
            (
                Response::SocketAddrs(vec![
                    (Ipv4Addr::LOCALHOST, 24816).into(),
                    (Ipv6Addr::LOCALHOST, 36912).into(),
                ]),
                "81ac736f636b65745f61646472739281a2563492947f000001cd60f081a2563692dc001000000000\
                 000000000000000000000001cd9030",
            ),
            (
                Response::StorageSize(StorageSize::from_bytes(1024)),
                "81ac73746f726167655f73697a65cd0400",
            ),
            (Response::U32(3), "81a375333203"),
        ];

        for (response, expected_encoded) in test_vectors {
            let encoded = rmp_serde::to_vec(&response).unwrap();
            assert_eq!(hex::encode(&encoded), expected_encoded, "{:?}", response);

            let decoded: Response = rmp_serde::from_slice(&encoded).unwrap();

            assert_eq!(decoded, response);
        }
    }
}
