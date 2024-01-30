use crate::{
    directory::Directory, file::FileHolder, registry::Handle, repository::RepositoryHolder,
    state::SubscriptionHandle,
};
use camino::Utf8PathBuf;
use ouisync_bridge::network::NetworkDefaults;
use ouisync_lib::{
    network::{NatBehavior, TrafficStats},
    AccessMode, PeerAddr, PeerInfo, Progress, ShareToken,
};
use serde::{Deserialize, Serialize};
use state_monitor::{MonitorId, StateMonitor};
use std::{
    fmt,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    path::PathBuf,
};
use thiserror::Error;

#[derive(Eq, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    RepositoryCreate {
        path: Utf8PathBuf,
        read_password: Option<String>,
        write_password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositoryOpen {
        path: Utf8PathBuf,
        password: Option<String>,
    },
    RepositoryClose(Handle<RepositoryHolder>),
    RepositoryCreateReopenToken(Handle<RepositoryHolder>),
    RepositoryReopen {
        path: Utf8PathBuf,
        #[serde(with = "serde_bytes")]
        token: Vec<u8>,
    },
    RepositorySubscribe(Handle<RepositoryHolder>),
    RepositorySetReadAccess {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        old_password: Option<String>,
        new_password: Option<String>,
        share_token: Option<ShareToken>,
    },
    RepositoryRemoveReadKey(Handle<RepositoryHolder>),
    RepositoryRemoveWriteKey(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForReading(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForWriting(Handle<RepositoryHolder>),
    RepositoryInfoHash(Handle<RepositoryHolder>),
    RepositoryDatabaseId(Handle<RepositoryHolder>),
    RepositoryEntryType {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    RepositoryMoveEntry {
        repository: Handle<RepositoryHolder>,
        src: Utf8PathBuf,
        dst: Utf8PathBuf,
    },
    RepositoryIsDhtEnabled(Handle<RepositoryHolder>),
    RepositorySetDhtEnabled {
        repository: Handle<RepositoryHolder>,
        enabled: bool,
    },
    RepositoryIsPexEnabled(Handle<RepositoryHolder>),
    RepositorySetPexEnabled {
        repository: Handle<RepositoryHolder>,
        enabled: bool,
    },
    RepositoryCreateShareToken {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        access_mode: AccessMode,
        name: Option<String>,
    },
    RepositoryAccessMode(Handle<RepositoryHolder>),
    RepositorySyncProgress(Handle<RepositoryHolder>),
    RepositoryMirror {
        repository: Handle<RepositoryHolder>,
    },
    RepositoryMountAll(PathBuf),
    ShareTokenMode(#[serde(with = "as_str")] ShareToken),
    ShareTokenInfoHash(#[serde(with = "as_str")] ShareToken),
    ShareTokenSuggestedName(#[serde(with = "as_str")] ShareToken),
    ShareTokenNormalize(#[serde(with = "as_str")] ShareToken),
    DirectoryCreate {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    DirectoryOpen {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    DirectoryRemove {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
        recursive: bool,
    },
    FileOpen {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileCreate {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileRemove {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    FileRead {
        file: Handle<FileHolder>,
        offset: u64,
        len: u64,
    },
    FileWrite {
        file: Handle<FileHolder>,
        offset: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    FileTruncate {
        file: Handle<FileHolder>,
        len: u64,
    },
    FileLen(Handle<FileHolder>),
    FileProgress(Handle<FileHolder>),
    FileFlush(Handle<FileHolder>),
    FileClose(Handle<FileHolder>),
    NetworkInit(NetworkDefaults),
    NetworkSubscribe,
    NetworkBind {
        #[serde(with = "as_option_str", default)]
        quic_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str", default)]
        quic_v6: Option<SocketAddrV6>,
        #[serde(with = "as_option_str", default)]
        tcp_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str", default)]
        tcp_v6: Option<SocketAddrV6>,
    },
    NetworkTcpListenerLocalAddrV4,
    NetworkTcpListenerLocalAddrV6,
    NetworkQuicListenerLocalAddrV4,
    NetworkQuicListenerLocalAddrV6,
    NetworkAddUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkRemoveUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkUserProvidedPeers,
    NetworkKnownPeers,
    NetworkThisRuntimeId,
    NetworkCurrentProtocolVersion,
    NetworkHighestSeenProtocolVersion,
    NetworkIsPortForwardingEnabled,
    NetworkSetPortForwardingEnabled(bool),
    NetworkIsLocalDiscoveryEnabled,
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkAddCacheServer(String),
    NetworkExternalAddrV4,
    NetworkExternalAddrV6,
    NetworkNatBehavior,
    NetworkTrafficStats,
    NetworkShutdown,
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
    Unsubscribe(SubscriptionHandle),
}

#[derive(Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Response {
    None,
    Bool(bool),
    U8(u8),
    U32(u32),
    U64(u64),
    Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
    String(String),
    Handle(u64),
    Directory(Directory),
    StateMonitor(StateMonitor),
    Progress(Progress),
    PeerInfos(Vec<PeerInfo>),
    PeerAddrs(#[serde(with = "as_vec_str")] Vec<PeerAddr>),
    TrafficStats(TrafficStats),
}

impl<T> From<Option<T>> for Response
where
    Response: From<T>,
{
    fn from(value: Option<T>) -> Self {
        if let Some(value) = value {
            Self::from(value)
        } else {
            Self::None
        }
    }
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl From<bool> for Response {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl TryFrom<Response> for bool {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::Bool(value) => Ok(value),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<u8> for Response {
    fn from(value: u8) -> Self {
        Self::U8(value)
    }
}

impl From<u32> for Response {
    fn from(value: u32) -> Self {
        Self::U32(value)
    }
}

impl From<u64> for Response {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<Vec<u8>> for Response {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<String> for Response {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl TryFrom<Response> for String {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::String(value) => Ok(value),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<StateMonitor> for Response {
    fn from(value: StateMonitor) -> Self {
        Self::StateMonitor(value)
    }
}

impl From<Directory> for Response {
    fn from(value: Directory) -> Self {
        Self::Directory(value)
    }
}

impl<T> From<Handle<T>> for Response {
    fn from(value: Handle<T>) -> Self {
        Self::Handle(value.id())
    }
}

impl<T> TryFrom<Response> for Handle<T> {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::Handle(value) => Ok(Self::from_id(value)),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<SocketAddr> for Response {
    fn from(value: SocketAddr) -> Self {
        Self::String(value.to_string())
    }
}

impl From<SocketAddrV4> for Response {
    fn from(value: SocketAddrV4) -> Self {
        Self::String(value.to_string())
    }
}

impl From<SocketAddrV6> for Response {
    fn from(value: SocketAddrV6) -> Self {
        Self::String(value.to_string())
    }
}

impl From<Progress> for Response {
    fn from(value: Progress) -> Self {
        Self::Progress(value)
    }
}

impl From<Vec<PeerInfo>> for Response {
    fn from(value: Vec<PeerInfo>) -> Self {
        Self::PeerInfos(value)
    }
}

impl TryFrom<Response> for Vec<PeerInfo> {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::PeerInfos(value) => Ok(value),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<Vec<PeerAddr>> for Response {
    fn from(value: Vec<PeerAddr>) -> Self {
        Self::PeerAddrs(value)
    }
}

impl TryFrom<Response> for Vec<PeerAddr> {
    type Error = UnexpectedResponse;

    fn try_from(response: Response) -> Result<Self, Self::Error> {
        match response {
            Response::PeerAddrs(value) => Ok(value),
            _ => Err(UnexpectedResponse),
        }
    }
}

impl From<Option<NatBehavior>> for Response {
    fn from(value: Option<NatBehavior>) -> Self {
        match value {
            Some(NatBehavior::EndpointIndependent) => Self::String("endpoint independent".into()),
            Some(NatBehavior::AddressDependent) => Self::String("address dependent".into()),
            Some(NatBehavior::AddressAndPortDependent) => {
                Self::String("address and port dependent".into())
            }
            None => Self::None,
        }
    }
}

impl From<TrafficStats> for Response {
    fn from(value: TrafficStats) -> Self {
        Self::TrafficStats(value)
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Bool(value) => f.debug_tuple("Bool").field(value).finish(),
            Self::U8(value) => f.debug_tuple("U8").field(value).finish(),
            Self::U32(value) => f.debug_tuple("U32").field(value).finish(),
            Self::U64(value) => f.debug_tuple("U64").field(value).finish(),
            Self::Bytes(_) => write!(f, "Bytes(_)"),
            Self::String(value) => f.debug_tuple("String").field(value).finish(),
            Self::Handle(value) => f.debug_tuple("Handle").field(value).finish(),
            Self::Directory(_) => write!(f, "Directory(_)"),
            Self::StateMonitor(_) => write!(f, "StateMonitor(_)"),
            Self::Progress(value) => f.debug_tuple("Progress").field(value).finish(),
            Self::PeerInfos(value) => f
                .debug_struct("PeerInfos")
                .field("len", &value.len())
                .finish(),
            Self::PeerAddrs(value) => f.debug_tuple("PeerAddrs").field(value).finish(),
            Self::TrafficStats(value) => f.debug_tuple("TrafficStats").field(value).finish(),
        }
    }
}

#[derive(Error, Debug)]
#[error("unexpected response")]
pub struct UnexpectedResponse;

pub mod as_str {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{fmt, str::FromStr};

    pub fn deserialize<'de, D, T>(d: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: fmt::Display,
    {
        let s = <&str>::deserialize(d)?;
        let v = s.parse().map_err(serde::de::Error::custom)?;
        Ok(v)
    }

    pub fn serialize<T, S>(value: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: fmt::Display,
        S: Serializer,
    {
        value.to_string().serialize(s)
    }
}

pub mod as_option_str {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{fmt, str::FromStr};

    pub fn deserialize<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: fmt::Display,
    {
        let s = Option::<&str>::deserialize(d)?;
        if let Some(s) = s {
            Ok(Some(s.parse().map_err(serde::de::Error::custom)?))
        } else {
            Ok(None)
        }
    }

    pub fn serialize<T, S>(value: &Option<T>, s: S) -> Result<S::Ok, S::Error>
    where
        T: fmt::Display,
        S: Serializer,
    {
        value.as_ref().map(|value| value.to_string()).serialize(s)
    }
}

pub mod as_vec_str {
    use serde::{de, ser::SerializeSeq, Deserializer, Serializer};
    use std::{fmt, marker::PhantomData, str::FromStr};

    pub fn deserialize<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: fmt::Display,
    {
        struct Visitor<T>(PhantomData<T>);

        impl<'de, T> de::Visitor<'de> for Visitor<T>
        where
            T: FromStr,
            T::Err: fmt::Display,
        {
            type Value = Vec<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "sequence of strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));

                while let Some(item) = seq.next_element::<&str>()? {
                    out.push(item.parse().map_err(<A::Error as de::Error>::custom)?);
                }

                Ok(out)
            }
        }

        d.deserialize_seq(Visitor(PhantomData))
    }

    pub fn serialize<T, S>(value: &[T], s: S) -> Result<S::Ok, S::Error>
    where
        T: fmt::Display,
        S: Serializer,
    {
        let mut s = s.serialize_seq(Some(value.len()))?;
        for item in value {
            s.serialize_element(&item.to_string())?;
        }
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync_lib::{
        network::{PeerSource, PeerState},
        PeerInfo, SecretRuntimeId,
    };

    #[test]
    fn request_serialize_deserialize() {
        let origs = [
            Request::RepositoryCreate {
                path: Utf8PathBuf::from("/tmp/repo.db"),
                read_password: None,
                write_password: None,
                share_token: None,
            },
            Request::RepositoryClose(Handle::from_id(1)),
        ];

        for orig in origs {
            let encoded = rmp_serde::to_vec(&orig).unwrap();
            let decoded: Request = rmp_serde::from_slice(&encoded).unwrap();
            assert_eq!(decoded, orig);
        }
    }

    #[test]
    fn response_serialize_deserialize() {
        let origs = [
            Response::None,
            Response::Bool(true),
            Response::Bool(false),
            Response::U8(0),
            Response::U8(1),
            Response::U8(2),
            Response::U8(u8::MAX),
            Response::U32(0),
            Response::U32(1),
            Response::U32(2),
            Response::U32(u32::MAX),
            Response::U64(0),
            Response::U64(1),
            Response::U64(2),
            Response::U64(u64::MAX),
            Response::Bytes(b"hello world".to_vec()),
            Response::Handle(1),
            Response::PeerInfos(vec![
                PeerInfo {
                    addr: PeerAddr::Quic(([192, 168, 1, 204], 65535).into()),
                    source: PeerSource::LocalDiscovery,
                    state: PeerState::Connecting,
                },
                PeerInfo {
                    addr: PeerAddr::Quic(
                        ([0x2001, 0xdb8, 0x0, 0x0, 0x0, 0x8a2e, 0x370, 0x7334], 12345).into(),
                    ),
                    source: PeerSource::Dht,
                    state: PeerState::Active(SecretRuntimeId::random().public()),
                },
            ]),
            Response::PeerAddrs(vec![PeerAddr::Tcp(([192, 168, 1, 234], 45678).into())]),
        ];

        for orig in origs {
            let encoded = rmp_serde::to_vec(&orig).unwrap();
            let decoded: Response = rmp_serde::from_slice(&encoded).unwrap();
            assert_eq!(decoded, orig);
        }
    }
}
