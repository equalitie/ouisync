use crate::repository::RepositoryHandle;
use ouisync::{
    crypto::Password, AccessMode, LocalSecret, PeerAddr, SetLocalSecret, ShareToken, StorageSize,
};
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Enable/disable metrics collection endpoint
    MetricsBind {
        addr: Option<SocketAddr>,
    },
    NetworkAddUserProvidedPeers(#[serde(with = "as_vec_str")] Vec<PeerAddr>),
    NetworkBind(Vec<PeerAddr>),
    NetworkGetListenerAddrs,
    NetworkGetPeers,
    NetworkGetUserProvidedPeers,
    NetworkIsLocalDiscoveryEnabled,
    NetworkIsPexRecvEnabled,
    NetworkIsPexSendEnabled,
    NetworkIsPortForwardingEnabled,
    NetworkRemoveUserProvidedPeers(#[serde(with = "as_vec_str")] Vec<PeerAddr>),
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkSetPexRecvEnabled(bool),
    NetworkSetPexSendEnabled(bool),
    NetworkSetPortForwardingEnabled(bool),
    /// Enable/disable remote control endpoint
    RemoteControlBind {
        addrs: Vec<SocketAddr>,
    },

    RepositoryCreate {
        name: String,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        token: Option<ShareToken>,
    },
    /// Delete a repository
    RepositoryDelete(RepositoryHandle),
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
    /// Mirror repository
    Mirror {
        /// Name of the repository to mirror
        name: String,

        /// Domain name or network address of the server to host the mirror
        host: String,
    },
    /// Bind the remote API to the specified addresses.
    ///
    /// Overwrites any previously specified addresses.
    RemoteControl {
        /// Addresses to bind to. IP is a IPv4 or IPv6 address and PORT is a port number. If IP is
        /// 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port. If empty
        /// disables the remote API.
        addrs: Vec<SocketAddr>,
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
    RepositoryCreateMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryDeleteMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryMirrorExists {
        repository: RepositoryHandle,
        host: String,
    },
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
