use crate::{
    directory,
    error::Result,
    file::{self, FileHolder},
    network,
    protocol::Response,
    registry::Handle,
    repository::{self, RepositoryHolder},
    share_token,
    state::{self, State, SubscriptionHandle},
    state_monitor,
    transport::NotificationSender,
};
use camino::Utf8PathBuf;
use ouisync_lib::{AccessMode, MonitorId, PeerAddr, ShareToken};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddrV4, SocketAddrV6};
use tracing::Instrument;

#[derive(Eq, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Request {
    RepositoryCreate {
        path: Utf8PathBuf,
        read_password: Option<String>,
        write_password: Option<String>,
        #[serde(with = "as_option_str")]
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
        #[serde(with = "as_option_str")]
        share_token: Option<ShareToken>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        old_password: Option<String>,
        new_password: Option<String>,
        #[serde(with = "as_option_str")]
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
    RepositoryMount {
        repository: Handle<RepositoryHolder>,
        path: Utf8PathBuf,
    },
    RepositoryUnmount(Handle<RepositoryHolder>),
    RepositoryFind {
        path: Utf8PathBuf,
    },
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
    FileFlush(Handle<FileHolder>),
    FileClose(Handle<FileHolder>),
    NetworkSubscribe,
    NetworkBind {
        #[serde(with = "as_option_str")]
        quic_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str")]
        quic_v6: Option<SocketAddrV6>,
        #[serde(with = "as_option_str")]
        tcp_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str")]
        tcp_v6: Option<SocketAddrV6>,
    },
    NetworkTcpListenerLocalAddrV4,
    NetworkTcpListenerLocalAddrV6,
    NetworkQuicListenerLocalAddrV4,
    NetworkQuicListenerLocalAddrV6,
    NetworkAddUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkRemoveUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkKnownPeers,
    NetworkThisRuntimeId,
    NetworkCurrentProtocolVersion,
    NetworkHighestSeenProtocolVersion,
    NetworkIsPortForwardingEnabled,
    NetworkSetPortForwardingEnabled(bool),
    NetworkIsLocalDiscoveryEnabled,
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkShutdown,
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
    Unsubscribe(SubscriptionHandle),
}

pub async fn dispatch(
    request: Request,
    notification_tx: &NotificationSender,
    state: &State,
) -> Result<Response> {
    // tracing::debug!(?request);

    let response = match request {
        Request::RepositoryCreate {
            path,
            read_password,
            write_password,
            share_token,
        } => {
            let holder = repository::create(
                path,
                read_password,
                write_password,
                share_token,
                &state.config,
                &state.network,
            )
            .instrument(state.repos_span.clone())
            .await?;
            state.repositories.insert(holder).into()
        }
        Request::RepositoryOpen { path, password } => {
            repository::open(state, path, password).await?.into()
        }
        Request::RepositoryClose(handle) => repository::close(state, handle).await?.into(),
        Request::RepositoryCreateReopenToken(handle) => {
            repository::create_reopen_token(state, handle)?.into()
        }
        Request::RepositoryReopen { path, token } => {
            repository::reopen(state, path, token).await?.into()
        }
        Request::RepositorySubscribe(handle) => {
            repository::subscribe(state, notification_tx, handle).into()
        }
        Request::RepositorySetReadAccess {
            repository,
            password,
            share_token,
        } => repository::set_read_access(state, repository, password, share_token)
            .await?
            .into(),
        Request::RepositorySetReadAndWriteAccess {
            repository,
            old_password,
            new_password,
            share_token,
        } => repository::set_read_and_write_access(
            state,
            repository,
            old_password,
            new_password,
            share_token,
        )
        .await?
        .into(),
        Request::RepositoryRemoveReadKey(handle) => {
            repository::remove_read_key(state, handle).await?.into()
        }
        Request::RepositoryRemoveWriteKey(handle) => {
            repository::remove_write_key(state, handle).await?.into()
        }
        Request::RepositoryRequiresLocalPasswordForReading(handle) => {
            repository::requires_local_password_for_reading(state, handle)
                .await?
                .into()
        }
        Request::RepositoryRequiresLocalPasswordForWriting(handle) => {
            repository::requires_local_password_for_writing(state, handle)
                .await?
                .into()
        }
        Request::RepositoryInfoHash(handle) => repository::info_hash(state, handle).into(),
        Request::RepositoryDatabaseId(handle) => {
            repository::database_id(state, handle).await?.into()
        }
        Request::RepositoryEntryType { repository, path } => {
            repository::entry_type(state, repository, path)
                .await?
                .into()
        }
        Request::RepositoryMoveEntry {
            repository,
            src,
            dst,
        } => repository::move_entry(state, repository, src, dst)
            .await?
            .into(),
        Request::RepositoryIsDhtEnabled(repository) => {
            repository::is_dht_enabled(state, repository).into()
        }
        Request::RepositorySetDhtEnabled {
            repository,
            enabled,
        } => {
            repository::set_dht_enabled(state, repository, enabled);
            ().into()
        }
        Request::RepositoryIsPexEnabled(repository) => {
            repository::is_pex_enabled(state, repository).into()
        }
        Request::RepositorySetPexEnabled {
            repository,
            enabled,
        } => {
            repository::set_pex_enabled(state, repository, enabled);
            ().into()
        }
        Request::RepositoryCreateShareToken {
            repository,
            password,
            access_mode,
            name,
        } => {
            let holder = state.repositories.get(repository);
            repository::create_share_token(&holder.repository, password, access_mode, name)
                .await?
                .into()
        }
        Request::ShareTokenMode(token) => share_token::mode(token).into(),
        Request::ShareTokenInfoHash(token) => share_token::info_hash(token).into(),
        Request::ShareTokenSuggestedName(token) => share_token::suggested_name(token).into(),
        Request::ShareTokenNormalize(token) => token.to_string().into(),
        Request::RepositoryAccessMode(repository) => {
            repository::access_mode(state, repository).into()
        }
        Request::RepositorySyncProgress(repository) => {
            repository::sync_progress(state, repository).await?.into()
        }
        Request::DirectoryCreate { repository, path } => {
            directory::create(state, repository, path).await?.into()
        }
        Request::DirectoryOpen { repository, path } => {
            directory::open(state, repository, path).await?.into()
        }
        Request::DirectoryRemove {
            repository,
            path,
            recursive,
        } => directory::remove(state, repository, path, recursive)
            .await?
            .into(),
        Request::FileOpen { repository, path } => file::open(state, repository, path).await?.into(),
        Request::FileCreate { repository, path } => {
            file::create(state, repository, path).await?.into()
        }
        Request::FileRemove { repository, path } => {
            file::remove(state, repository, path).await?.into()
        }
        Request::FileRead { file, offset, len } => {
            file::read(state, file, offset, len).await?.into()
        }
        Request::FileWrite { file, offset, data } => {
            file::write(state, file, offset, data).await?.into()
        }
        Request::FileTruncate { file, len } => file::truncate(state, file, len).await?.into(),
        Request::FileLen(file) => file::len(state, file).await.into(),
        Request::FileFlush(file) => file::flush(state, file).await?.into(),
        Request::FileClose(file) => file::close(state, file).await?.into(),
        Request::NetworkSubscribe => network::subscribe(state, notification_tx).into(),
        Request::NetworkBind {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        } => {
            network::bind(&state.network, quic_v4, quic_v6, tcp_v4, tcp_v6).await;
            ().into()
        }
        Request::NetworkTcpListenerLocalAddrV4 => network::tcp_listener_local_addr_v4(state).into(),
        Request::NetworkTcpListenerLocalAddrV6 => network::tcp_listener_local_addr_v6(state).into(),
        Request::NetworkQuicListenerLocalAddrV4 => {
            network::quic_listener_local_addr_v4(state).into()
        }
        Request::NetworkQuicListenerLocalAddrV6 => {
            network::quic_listener_local_addr_v6(state).into()
        }
        Request::NetworkAddUserProvidedPeer(addr) => {
            state.network.add_user_provided_peer(&addr);
            ().into()
        }
        Request::NetworkRemoveUserProvidedPeer(addr) => {
            state.network.remove_user_provided_peer(&addr);
            ().into()
        }
        Request::NetworkKnownPeers => state.network.collect_peer_info().into(),
        Request::NetworkThisRuntimeId => network::this_runtime_id(state).into(),
        Request::NetworkCurrentProtocolVersion => network::current_protocol_version(state).into(),
        Request::NetworkHighestSeenProtocolVersion => {
            network::highest_seen_protocol_version(state).into()
        }
        Request::NetworkIsPortForwardingEnabled => {
            state.network.is_port_forwarding_enabled().into()
        }
        Request::NetworkSetPortForwardingEnabled(enabled) => {
            network::set_port_forwarding_enabled(&state.network, enabled);
            ().into()
        }
        Request::NetworkIsLocalDiscoveryEnabled => {
            state.network.is_local_discovery_enabled().into()
        }
        Request::NetworkSetLocalDiscoveryEnabled(enabled) => {
            network::set_local_discovery_enabled(&state.network, enabled);
            ().into()
        }
        Request::NetworkShutdown => {
            network::shutdown(state).await;
            ().into()
        }
        Request::StateMonitorGet(path) => state_monitor::get(state, path)?.into(),
        Request::StateMonitorSubscribe(path) => {
            state_monitor::subscribe(state, notification_tx, path)?.into()
        }
        Request::Unsubscribe(handle) => {
            state::unsubscribe(state, handle);
            ().into()
        }
        // These should be implemented by the upper layer
        Request::RepositoryMount { .. }
        | Request::RepositoryUnmount(_)
        | Request::RepositoryFind { .. } => Err(ouisync_lib::Error::OperationNotSupported)?,
    };

    Ok(response)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_deserialize() {
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
}
