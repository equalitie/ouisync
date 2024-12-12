use crate::{file::FileHandle, repository::RepositoryHandle};
use ouisync::{
    crypto::PasswordSalt, AccessChange, AccessMode, LocalSecret, PeerAddr, SetLocalSecret,
    ShareToken, StorageSize,
};
use ouisync_bridge::network::NetworkDefaults;
use serde::{Deserialize, Serialize};
use state_monitor::MonitorId;
use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};
use thiserror::Error;

use super::{
    helpers::{self, Bytes},
    MessageId, MetadataEdit,
};

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    DirectoryCreate {
        repository: RepositoryHandle,
        path: String,
    },
    DirectoryRead {
        repository: RepositoryHandle,
        path: String,
    },
    DirectoryRemove {
        repository: RepositoryHandle,
        path: String,
        recursive: bool,
    },
    FileClose(FileHandle),
    FileCreate {
        repository: RepositoryHandle,
        path: String,
    },
    FileExists {
        repository: RepositoryHandle,
        path: String,
    },
    FileFlush(FileHandle),
    FileOpen {
        repository: RepositoryHandle,
        path: String,
    },
    FileLen(FileHandle),
    FileProgress(FileHandle),
    FileRead {
        file: FileHandle,
        offset: u64,
        len: u64,
    },
    FileRemove {
        repository: RepositoryHandle,
        path: String,
    },
    FileTruncate {
        file: FileHandle,
        len: u64,
    },
    FileWrite {
        file: FileHandle,
        offset: u64,
        data: Bytes,
    },
    MetricsBind(Option<SocketAddr>),
    MetricsGetListenerAddr,
    NetworkAddUserProvidedPeers(#[serde(with = "helpers::strs")] Vec<PeerAddr>),
    NetworkBind(#[serde(with = "helpers::strs")] Vec<PeerAddr>),
    NetworkCurrentProtocolVersion,
    NetworkGetListenerAddrs,
    NetworkGetPeers,
    NetworkGetUserProvidedPeers,
    NetworkInit(NetworkDefaults),
    NetworkIsLocalDiscoveryEnabled,
    NetworkIsPexRecvEnabled,
    NetworkIsPexSendEnabled,
    NetworkIsPortForwardingEnabled,
    NetworkRemoveUserProvidedPeers(#[serde(with = "helpers::strs")] Vec<PeerAddr>),
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkSetPexRecvEnabled(bool),
    NetworkSetPexSendEnabled(bool),
    NetworkSetPortForwardingEnabled(bool),
    NetworkStats,
    NetworkSubscribe,
    PasswordGenerateSalt,
    PasswordDeriveSecretKey {
        password: String,
        salt: PasswordSalt,
    },
    RemoteControlBind(Option<SocketAddr>),
    RemoteControlGetListenerAddr,
    RepositoryClose(RepositoryHandle),
    RepositoryCreate {
        path: PathBuf,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        token: Option<ShareToken>,
        sync_enabled: bool,
        dht_enabled: bool,
        pex_enabled: bool,
    },
    RepositoryCreateMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryCredentials(RepositoryHandle),
    /// Delete a repository
    RepositoryDelete(RepositoryHandle),
    /// Delete a repository with the given name.
    RepositoryDeleteByName(String),
    RepositoryDeleteMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryEntryType {
        repository: RepositoryHandle,
        path: String,
    },
    /// Export repository to a file
    RepositoryExport {
        repository: RepositoryHandle,
        output: PathBuf,
    },
    /// Find repository whose path uniquely matches the given string.
    RepositoryFind(String),
    RepositoryGetAccessMode(RepositoryHandle),
    RepositoryGetBlockExpiration(RepositoryHandle),
    RepositoryGetDefaultBlockExpiration,
    RepositoryGetDefaultQuota,
    RepositoryGetDefaultRepositoryExpiration,
    RepositoryGetInfoHash(RepositoryHandle),
    RepositoryGetMetadata {
        repository: RepositoryHandle,
        key: String,
    },
    RepositoryGetMountPoint(RepositoryHandle),
    RepositoryGetMountRoot,
    RepositoryGetPath(RepositoryHandle),
    RepositoryGetQuota(RepositoryHandle),
    RepositoryGetRepositoryExpiration(RepositoryHandle),
    RepositoryGetStoreDir,
    /// Import a repository from a file
    #[deprecated = "use RepositoryOpen or move/copy/link the file manually"]
    RepositoryImport {
        input: PathBuf,
        name: Option<String>,
        mode: ImportMode,
        force: bool,
    },
    RepositoryIsDhtEnabled(RepositoryHandle),
    RepositoryIsPexEnabled(RepositoryHandle),
    RepositoryIsSyncEnabled(RepositoryHandle),
    RepositoryList,
    RepositoryMirrorExists {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryMount(RepositoryHandle),
    RepositoryMove {
        repository: RepositoryHandle,
        to: PathBuf,
    },
    RepositoryMoveEntry {
        repository: RepositoryHandle,
        src: String,
        dst: String,
    },
    RepositoryOpen {
        path: PathBuf,
        secret: Option<LocalSecret>,
    },
    RepositoryResetAccess {
        repository: RepositoryHandle,
        token: ShareToken,
    },
    RepositorySetAccess {
        repository: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    },
    RepositorySetAccessMode {
        repository: RepositoryHandle,
        access_mode: AccessMode,
        secret: Option<LocalSecret>,
    },
    RepositorySetBlockExpiration {
        repository: RepositoryHandle,
        value: Option<Duration>,
    },
    RepositorySetCredentials {
        repository: RepositoryHandle,
        credentials: Bytes,
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
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositorySetMetadata {
        repository: RepositoryHandle,
        edits: Vec<MetadataEdit>,
    },
    RepositorySetMountRoot(Option<PathBuf>),
    RepositorySetPexEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositorySetQuota {
        repository: RepositoryHandle,
        quota: Option<StorageSize>,
    },
    RepositorySetRepositoryExpiration {
        repository: RepositoryHandle,
        value: Option<Duration>,
    },
    RepositorySetStoreDir(PathBuf),
    RepositorySetSyncEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositoryShare {
        repository: RepositoryHandle,
        secret: Option<LocalSecret>,
        mode: AccessMode,
    },
    RepositorySubscribe(RepositoryHandle),
    RepositorySyncProgress(RepositoryHandle),
    RepositoryUnmount(RepositoryHandle),
    ShareTokenGetInfoHash(#[serde(with = "helpers::str")] ShareToken),
    ShareTokenGetAccessMode(#[serde(with = "helpers::str")] ShareToken),
    ShareTokenGetSuggestedName(#[serde(with = "helpers::str")] ShareToken),
    ShareTokenNormalize(#[serde(with = "helpers::str")] ShareToken),
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
    /// Cancel a subscription identified by the given message id. The message id should be the same
    /// that was used for sending the corresponding subscribe request.
    Unsubscribe(MessageId),
}

/*

    From ffi:

#[derive(Eq, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    ListRepositoriesSubscribe,
    RepositoryRequiresLocalSecretForReading(RepositoryHandle),
    RepositoryRequiresLocalSecretForWriting(RepositoryHandle),
    RepositoryName(RepositoryHandle),
    RepositoryDatabaseId(RepositoryHandle),
    RepositoryEntryVersionHash {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    RepositoryMountAll(PathBuf),
    RepositoryStats(RepositoryHandle),
    ShareTokenMirrorExists {
        #[serde(with = "as_str")]
        share_token: ShareToken,
        host: String,
    },
    DirectoryExists {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    NetworkThisRuntimeId,
    NetworkHighestSeenProtocolVersion,
    NetworkExternalAddrV4,
    NetworkExternalAddrV6,
    NetworkNatBehavior,
    NetworkShutdown,
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

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{AccessSecrets, WriteSecrets};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::Ipv4Addr;

    #[test]
    fn serialize() {
        let mut rng = StdRng::seed_from_u64(0);
        let secrets = AccessSecrets::Write(WriteSecrets::generate(&mut rng));

        let test_vectors = [
            (
                Request::NetworkAddUserProvidedPeers(vec![]),
                "81bf6e6574776f726b5f6164645f757365725f70726f76696465645f706565727390",
            ),
            (
                Request::NetworkAddUserProvidedPeers(vec![PeerAddr::Quic(SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    12345,
                )))]),
                "81bf6e6574776f726b5f6164645f757365725f70726f76696465645f706565727391b4717569632f31\
                 32372e302e302e313a3132333435",
            ),
            (
                Request::NetworkBind(vec![PeerAddr::Quic(SocketAddr::from((
                    Ipv4Addr::UNSPECIFIED,
                    12345,
                )))]),
                "81ac6e6574776f726b5f62696e6491b2717569632f302e302e302e303a3132333435",
            ),
            (
                Request::NetworkGetListenerAddrs,
                "ba6e6574776f726b5f6765745f6c697374656e65725f6164647273",
            ),
            (
                Request::RepositoryCreate {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: None,
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                "81b17265706f7369746f72795f63726561746597a3666f6fc0c0c0c3c2c2",
            ),
            (
                Request::RepositoryCreate {
                    path: "foo".into(),
                    read_secret: None,
                    write_secret: None,
                    token: Some(ShareToken::from(secrets)),
                    sync_enabled: true,
                    dht_enabled: false,
                    pex_enabled: false,
                },
                "81b17265706f7369746f72795f63726561746597a3666f6fc0c0d94568747470733a2f2f6f75697379\
                 6e632e6e65742f722341774967663238737a62495f4b7274376153654f6c4877427868594b4d633843\
                 775a30473050626c71783132693555c3c2c2",
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
