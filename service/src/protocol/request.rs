use crate::repository::RepositoryHandle;
use ouisync::{AccessMode, LocalSecret, PeerAddr, SetLocalSecret, ShareToken, StorageSize};
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};
use thiserror::Error;

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum Request {
    MetricsBind(Option<SocketAddr>),
    MetricsGetListenerAddr,
    NetworkAddUserProvidedPeers(#[serde(with = "as_strs")] Vec<PeerAddr>),
    NetworkBind(Vec<PeerAddr>),
    NetworkGetListenerAddrs,
    NetworkGetPeers,
    NetworkGetUserProvidedPeers,
    NetworkIsLocalDiscoveryEnabled,
    NetworkIsPexRecvEnabled,
    NetworkIsPexSendEnabled,
    NetworkIsPortForwardingEnabled,
    NetworkRemoveUserProvidedPeers(#[serde(with = "as_strs")] Vec<PeerAddr>),
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkSetPexRecvEnabled(bool),
    NetworkSetPexSendEnabled(bool),
    NetworkSetPortForwardingEnabled(bool),
    RemoteControlBind(Option<SocketAddr>),
    RemoteControlGetListenerAddr,
    RepositoryCreate {
        name: String,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        token: Option<ShareToken>,
        dht: bool,
        pex: bool,
    },
    RepositoryCreateMirror {
        handle: RepositoryHandle,
        host: String,
    },
    /// Delete a repository
    RepositoryDelete(RepositoryHandle),
    /// Delete a repository with the given name (name matching is the same as in `RepositoryFind).
    RepositoryDeleteByName(String),
    RepositoryDeleteMirror {
        handle: RepositoryHandle,
        host: String,
    },
    /// Export repository to a file
    RepositoryExport {
        handle: RepositoryHandle,
        output: PathBuf,
    },
    /// Find repository by name. Returns the repository that matches the name exactly or
    /// unambiguously by prefix.
    RepositoryFind(String),
    RepositoryGetBlockExpiration(RepositoryHandle),
    RepositoryGetDefaultBlockExpiration,
    RepositoryGetDefaultQuota,
    RepositoryGetDefaultRepositoryExpiration,
    RepositoryGetMountDir,
    RepositoryGetQuota(RepositoryHandle),
    RepositoryGetRepositoryExpiration(RepositoryHandle),
    RepositoryGetStoreDir,
    /// Import a repository from a file
    RepositoryImport {
        input: PathBuf,
        name: Option<String>,
        mode: ImportMode,
        force: bool,
    },
    RepositoryIsDhtEnabled(RepositoryHandle),
    RepositoryIsPexEnabled(RepositoryHandle),
    RepositoryList,
    RepositoryMirrorExists {
        handle: RepositoryHandle,
        host: String,
    },
    RepositoryMount(RepositoryHandle),
    RepositoryResetAccess {
        handle: RepositoryHandle,
        token: ShareToken,
    },
    RepositorySetBlockExpiration {
        handle: RepositoryHandle,
        value: Option<Duration>,
    },
    RepositorySetDefaultBlockExpiration {
        value: Option<Duration>,
    },
    RepositorySetDefaultQuota {
        quota: Option<StorageSize>,
    },
    RepositorySetDefaultRepositoryExpiration {
        value: Option<Duration>,
    },
    RepositorySetDhtEnabled {
        handle: RepositoryHandle,
        enabled: bool,
    },
    RepositorySetMountDir(PathBuf),
    RepositorySetPexEnabled {
        handle: RepositoryHandle,
        enabled: bool,
    },
    RepositorySetQuota {
        handle: RepositoryHandle,
        quota: Option<StorageSize>,
    },
    RepositorySetRepositoryExpiration {
        handle: RepositoryHandle,
        value: Option<Duration>,
    },
    RepositorySetStoreDir(PathBuf),
    RepositoryShare {
        handle: RepositoryHandle,
        secret: Option<LocalSecret>,
        mode: AccessMode,
    },
    RepositoryUnmount(RepositoryHandle),
    /*
    Open {
        name: String,
        password: Option<String>,
    },
    Close {
        name: String,
    },
    */
}

/*

    From ffi:

#[derive(Eq, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    RepositoryOpen {
        path: Utf8PathBuf,
        secret: Option<LocalSecret>,
    },
    RepositoryClose(RepositoryHandle),
    RepositorySubscribe(RepositoryHandle),
    ListRepositoriesSubscribe,
    RepositoryIsSyncEnabled(RepositoryHandle),
    RepositorySetSyncEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositoryRequiresLocalSecretForReading(RepositoryHandle),
    RepositoryRequiresLocalSecretForWriting(RepositoryHandle),
    RepositorySetAccess {
        repository: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    },
    RepositoryCredentials(RepositoryHandle),
    RepositorySetCredentials {
        repository: RepositoryHandle,
        credentials: Bytes,
    },
    RepositoryAccessMode(RepositoryHandle),
    RepositorySetAccessMode {
        repository: RepositoryHandle,
        access_mode: AccessMode,
        secret: Option<LocalSecret>,
    },
    RepositoryName(RepositoryHandle),
    RepositoryInfoHash(RepositoryHandle),
    RepositoryDatabaseId(RepositoryHandle),
    RepositoryEntryType {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    RepositoryEntryVersionHash {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    RepositoryMoveEntry {
        repository: RepositoryHandle,
        src: Utf8PathBuf,
        dst: Utf8PathBuf,
    },
    RepositorySyncProgress(RepositoryHandle),
    RepositoryMountAll(PathBuf),
    RepositoryGetMetadata {
        repository: RepositoryHandle,
        key: String,
    },
    RepositorySetMetadata {
        repository: RepositoryHandle,
        edits: Vec<MetadataEdit>,
    },
    RepositoryStats(RepositoryHandle),
    ShareTokenMode(#[serde(with = "as_str")] ShareToken),
    ShareTokenInfoHash(#[serde(with = "as_str")] ShareToken),
    ShareTokenSuggestedName(#[serde(with = "as_str")] ShareToken),
    ShareTokenNormalize(#[serde(with = "as_str")] ShareToken),
    ShareTokenMirrorExists {
        #[serde(with = "as_str")]
        share_token: ShareToken,
        host: String,
    },
    DirectoryCreate {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryOpen {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryExists {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryRemove {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
        recursive: bool,
    },
    FileOpen {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileExists {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileCreate {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileRemove {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileRead {
        file: FileHandle,
        offset: u64,
        len: u64,
    },
    FileWrite {
        file: FileHandle,
        offset: u64,
        data: Bytes,
    },
    FileTruncate {
        file: FileHandle,
        len: u64,
    },
    FileLen(FileHandle),
    FileProgress(FileHandle),
    FileFlush(FileHandle),
    FileClose(FileHandle),
    NetworkInit(NetworkDefaults),
    NetworkSubscribe,
    NetworkThisRuntimeId,
    NetworkCurrentProtocolVersion,
    NetworkHighestSeenProtocolVersion,
    NetworkExternalAddrV4,
    NetworkExternalAddrV6,
    NetworkNatBehavior,
    NetworkStats,
    NetworkShutdown,
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
    Unsubscribe(TaskHandle),
    GenerateSaltForSecretKey,
    DeriveSecretKey {
        password: String,
        salt: PasswordSalt,
    },
    GetReadPasswordSalt(RepositoryHandle),
    GetWritePasswordSalt(RepositoryHandle),
}
*/

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum ImportMode {
    Copy,
    Move,
    SoftLink,
    HardLink,
}

impl FromStr for ImportMode {
    type Err = InvalidImportMode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim_start().chars().next() {
            Some('c') | Some('C') => Ok(Self::Copy),
            Some('m') | Some('M') => Ok(Self::Move),
            Some('s') | Some('S') => Ok(Self::SoftLink),
            Some('h') | Some('H') => Ok(Self::HardLink),
            _ => Err(InvalidImportMode),
        }
    }
}

impl fmt::Display for ImportMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Copy => write!(f, "copy"),
            Self::Move => write!(f, "move"),
            Self::SoftLink => write!(f, "softlink"),
            Self::HardLink => write!(f, "hardlink"),
        }
    }
}

#[derive(Error, Debug)]
#[error("invalid import mode")]
pub struct InvalidImportMode;

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

pub mod as_strs {
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
        use std::fmt::Write;

        let mut buffer = String::new();
        let mut s = s.serialize_seq(Some(value.len()))?;

        for item in value {
            write!(&mut buffer, "{}", item).expect("failed to format item");
            s.serialize_element(&buffer)?;
            buffer.clear();
        }
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{AccessSecrets, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::Ipv4Addr;

    #[test]
    fn serialize_roundtrip() {
        let mut rng = StdRng::seed_from_u64(0);
        let secrets = AccessSecrets::Write(WriteSecrets::generate(&mut rng));

        let test_vectors = [
            (
                Request::NetworkAddUserProvidedPeers(vec![]),
                "81bb4e6574776f726b4164645573657250726f7669646564506565727390",
            ),
            (
                Request::NetworkAddUserProvidedPeers(vec![PeerAddr::Quic(SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    12345,
                )))]),
                "81bb4e6574776f726b4164645573657250726f7669646564506565727391b4717569632f3132372e\
                 302e302e313a3132333435",
            ),
            (
                Request::NetworkBind(vec![PeerAddr::Quic(SocketAddr::from((
                    Ipv4Addr::UNSPECIFIED,
                    12345,
                )))]),
                "81ab4e6574776f726b42696e649181a47175696381a25634929400000000cd3039",
            ),
            (
                Request::NetworkGetListenerAddrs,
                "b74e6574776f726b4765744c697374656e65724164647273",
            ),
            (
                Request::RepositoryCreate {
                    name: "foo".to_owned(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    dht: false,
                    pex: false,
                },
                "81b05265706f7369746f727943726561746596a3666f6fc0c0c0c2c2",
            ),
            (
                Request::RepositoryCreate {
                    name: "foo".to_owned(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(ShareToken::from(secrets)),
                    dht: false,
                    pex: false,
                },
                "81b05265706f7369746f727943726561746596a3666f6fc0c0d94568747470733a2f2f6f75697379\
                 6e632e6e65742f722341774967663238737a62495f4b7274376153654f6c4877427868594b4d6338\
                 43775a30473050626c71783132693555c2c2",
            ),
        ];

        for (request, expected_encoded) in test_vectors {
            let encoded = rmp_serde::to_vec(&request).unwrap();
            assert_eq!(hex::encode(&encoded), expected_encoded, "{:?}", request);

            let decoded: Request = rmp_serde::from_slice(&encoded).unwrap();

            assert_eq!(decoded, request);
        }
    }
}
