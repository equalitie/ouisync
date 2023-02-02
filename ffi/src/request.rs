use crate::{
    directory,
    file::{self, FileHolder},
    network,
    protocol::Value,
    registry::Handle,
    repository::{self, RepositoryHolder},
    session::{self, SubscriptionHandle},
    share_token,
    state::{ClientState, ServerState},
    state_monitor,
};
use camino::Utf8PathBuf;
use ouisync_lib::{Result, ShareToken};
use serde::{Deserialize, Deserializer};
use serde_bytes::ByteBuf;
use std::{
    fmt,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    str::FromStr,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "args")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    RepositoryCreate {
        path: Utf8PathBuf,
        read_password: Option<String>,
        write_password: Option<String>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
        share_token: Option<ShareToken>,
    },
    RepositoryOpen {
        path: Utf8PathBuf,
        password: Option<String>,
    },
    RepositoryClose(Handle<RepositoryHolder>),
    RepositorySubscribe(Handle<RepositoryHolder>),
    RepositorySetReadAccess {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
        share_token: Option<ShareToken>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        old_password: Option<String>,
        new_password: Option<String>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
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
        access_mode: u8,
        name: Option<String>,
    },
    RepositoryAccessMode(Handle<RepositoryHolder>),
    RepositorySyncProgress(Handle<RepositoryHolder>),
    ShareTokenMode(DeserializeAsStr<ShareToken>),
    ShareTokenInfoHash(DeserializeAsStr<ShareToken>),
    ShareTokenSuggestedName(DeserializeAsStr<ShareToken>),
    ShareTokenNormalize(DeserializeAsStr<ShareToken>),
    ShareTokenEncode(DeserializeAsStr<ShareToken>),
    ShareTokenDecode(ByteBuf),
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
        data: ByteBuf,
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
        #[serde(deserialize_with = "deserialize_as_option_str")]
        quic_v4: Option<SocketAddrV4>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
        quic_v6: Option<SocketAddrV6>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
        tcp_v4: Option<SocketAddrV4>,
        #[serde(deserialize_with = "deserialize_as_option_str")]
        tcp_v6: Option<SocketAddrV6>,
    },
    NetworkTcpListenerLocalAddrV4,
    NetworkTcpListenerLocalAddrV6,
    NetworkQuicListenerLocalAddrV4,
    NetworkQuicListenerLocalAddrV6,
    NetworkAddUserProvidedQuicPeer(#[serde(deserialize_with = "deserialize_as_str")] SocketAddr),
    NetworkRemoveUserProvidedQuicPeer(#[serde(deserialize_with = "deserialize_as_str")] SocketAddr),
    NetworkKnownPeers,
    NetworkThisRuntimeId,
    NetworkCurrentProtocolVersion,
    NetworkHighestSeenProtocolVersion,
    NetworkIsPortForwardingEnabled,
    NetworkSetPortForwardingEnabled(bool),
    NetworkIsLocalDiscoveryEnabled,
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkShutdown,
    StateMonitorGet(String),
    StateMonitorSubscribe(String),
    Unsubscribe(SubscriptionHandle),
}

pub(crate) async fn dispatch(
    server_state: &ServerState,
    client_state: &ClientState,
    request: Request,
) -> Result<Value> {
    // TODO: This sometimes creates huge log messages (mostly on FileWrite?)
    // tracing::debug!(?request);

    let response = match request {
        Request::RepositoryCreate {
            path,
            read_password,
            write_password,
            share_token,
        } => repository::create(
            server_state,
            path,
            read_password,
            write_password,
            share_token,
        )
        .await?
        .into(),
        Request::RepositoryOpen { path, password } => {
            repository::open(server_state, path, password).await?.into()
        }
        Request::RepositoryClose(handle) => repository::close(server_state, handle).await?.into(),
        Request::RepositorySubscribe(handle) => {
            repository::subscribe(server_state, client_state, handle).into()
        }
        Request::RepositorySetReadAccess {
            repository,
            password,
            share_token,
        } => repository::set_read_access(server_state, repository, password, share_token)
            .await?
            .into(),
        Request::RepositorySetReadAndWriteAccess {
            repository,
            old_password,
            new_password,
            share_token,
        } => repository::set_read_and_write_access(
            server_state,
            repository,
            old_password,
            new_password,
            share_token,
        )
        .await?
        .into(),
        Request::RepositoryRemoveReadKey(handle) => {
            repository::remove_read_key(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRemoveWriteKey(handle) => {
            repository::remove_write_key(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRequiresLocalPasswordForReading(handle) => {
            repository::requires_local_password_for_reading(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryRequiresLocalPasswordForWriting(handle) => {
            repository::requires_local_password_for_writing(server_state, handle)
                .await?
                .into()
        }
        Request::RepositoryInfoHash(handle) => repository::info_hash(server_state, handle).into(),
        Request::RepositoryDatabaseId(handle) => {
            repository::database_id(server_state, handle).await?.into()
        }
        Request::RepositoryEntryType { repository, path } => {
            repository::entry_type(server_state, repository, path)
                .await?
                .into()
        }
        Request::RepositoryMoveEntry {
            repository,
            src,
            dst,
        } => repository::move_entry(server_state, repository, src, dst)
            .await?
            .into(),
        Request::RepositoryIsDhtEnabled(repository) => {
            repository::is_dht_enabled(server_state, repository).into()
        }
        Request::RepositorySetDhtEnabled {
            repository,
            enabled,
        } => {
            repository::set_dht_enabled(server_state, repository, enabled);
            ().into()
        }
        Request::RepositoryIsPexEnabled(repository) => {
            repository::is_pex_enabled(server_state, repository).into()
        }
        Request::RepositorySetPexEnabled {
            repository,
            enabled,
        } => {
            repository::set_pex_enabled(server_state, repository, enabled);
            ().into()
        }
        Request::RepositoryCreateShareToken {
            repository,
            password,
            access_mode,
            name,
        } => repository::create_share_token(server_state, repository, password, access_mode, name)
            .await?
            .into(),
        Request::ShareTokenMode(token) => share_token::mode(token.into_value()).into(),
        Request::ShareTokenInfoHash(token) => share_token::info_hash(token.into_value()).into(),
        Request::ShareTokenSuggestedName(token) => {
            share_token::suggested_name(token.into_value()).into()
        }
        Request::ShareTokenNormalize(token) => token.into_value().to_string().into(),
        Request::ShareTokenEncode(token) => share_token::encode(token.into_value()).into(),
        Request::ShareTokenDecode(bytes) => share_token::decode(bytes.into_vec())
            .map(|token| token.to_string())
            .into(),
        Request::RepositoryAccessMode(repository) => {
            repository::access_mode(server_state, repository).into()
        }
        Request::RepositorySyncProgress(repository) => {
            repository::sync_progress(server_state, repository)
                .await?
                .into()
        }
        Request::DirectoryCreate { repository, path } => {
            directory::create(server_state, repository, path)
                .await?
                .into()
        }
        Request::DirectoryOpen { repository, path } => {
            directory::open(server_state, repository, path)
                .await?
                .into()
        }
        Request::DirectoryRemove {
            repository,
            path,
            recursive,
        } => directory::remove(server_state, repository, path, recursive)
            .await?
            .into(),
        Request::FileOpen { repository, path } => {
            file::open(server_state, repository, path).await?.into()
        }
        Request::FileCreate { repository, path } => {
            file::create(server_state, repository, path).await?.into()
        }
        Request::FileRemove { repository, path } => {
            file::remove(server_state, repository, path).await?.into()
        }
        Request::FileRead { file, offset, len } => {
            file::read(server_state, file, offset, len).await?.into()
        }
        Request::FileWrite { file, offset, data } => {
            file::write(server_state, file, offset, data.into_vec())
                .await?
                .into()
        }
        Request::FileTruncate { file, len } => {
            file::truncate(server_state, file, len).await?.into()
        }
        Request::FileLen(file) => file::len(server_state, file).await.into(),
        Request::FileFlush(file) => file::flush(server_state, file).await?.into(),
        Request::FileClose(file) => file::close(server_state, file).await?.into(),
        Request::NetworkSubscribe => network::subscribe(server_state, client_state).into(),
        Request::NetworkBind {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
        } => {
            network::bind(server_state, quic_v4, quic_v6, tcp_v4, tcp_v6).await;
            ().into()
        }
        Request::NetworkTcpListenerLocalAddrV4 => {
            network::tcp_listener_local_addr_v4(server_state).into()
        }
        Request::NetworkTcpListenerLocalAddrV6 => {
            network::tcp_listener_local_addr_v6(server_state).into()
        }
        Request::NetworkQuicListenerLocalAddrV4 => {
            network::quic_listener_local_addr_v4(server_state).into()
        }
        Request::NetworkQuicListenerLocalAddrV6 => {
            network::quic_listener_local_addr_v6(server_state).into()
        }
        Request::NetworkAddUserProvidedQuicPeer(addr) => {
            network::add_user_provided_quic_peer(server_state, addr);
            ().into()
        }
        Request::NetworkRemoveUserProvidedQuicPeer(addr) => {
            network::remove_user_provided_quic_peer(server_state, addr);
            ().into()
        }
        Request::NetworkKnownPeers => network::known_peers(server_state).into(),
        Request::NetworkThisRuntimeId => network::this_runtime_id(server_state).into(),
        Request::NetworkCurrentProtocolVersion => {
            network::current_protocol_version(server_state).into()
        }
        Request::NetworkHighestSeenProtocolVersion => {
            network::highest_seen_protocol_version(server_state).into()
        }
        Request::NetworkIsPortForwardingEnabled => {
            network::is_port_forwarding_enabled(server_state).into()
        }
        Request::NetworkSetPortForwardingEnabled(enabled) => {
            network::set_port_forwarding_enabled(server_state, enabled);
            ().into()
        }
        Request::NetworkIsLocalDiscoveryEnabled => {
            network::is_local_discovery_enabled(server_state).into()
        }
        Request::NetworkSetLocalDiscoveryEnabled(enabled) => {
            network::set_local_discovery_enabled(server_state, enabled);
            ().into()
        }
        Request::NetworkShutdown => {
            network::shutdown(server_state).await;
            ().into()
        }
        Request::StateMonitorGet(path) => state_monitor::get(server_state, path)?.into(),
        Request::StateMonitorSubscribe(path) => {
            state_monitor::subscribe(server_state, client_state, path)?.into()
        }
        Request::Unsubscribe(handle) => {
            session::unsubscribe(server_state, handle);
            ().into()
        }
    };

    Ok(response)
}

fn deserialize_as_str<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let s = <&str>::deserialize(de)?;
    let v = s.parse().map_err(serde::de::Error::custom)?;
    Ok(v)
}

fn deserialize_as_option_str<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let s = Option::<&str>::deserialize(de)?;
    if let Some(s) = s {
        Ok(Some(s.parse().map_err(serde::de::Error::custom)?))
    } else {
        Ok(None)
    }
}

// HACK: sometimes `#[serde(deserialize_with = "deserialize_as_str")]` doesn't work for some
// reason, but this wrapper does.
#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct DeserializeAsStr<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    #[serde(deserialize_with = "deserialize_as_str")]
    value: T,
}

impl<T> DeserializeAsStr<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    pub fn into_value(self) -> T {
        self.value
    }
}

impl<T> fmt::Debug for DeserializeAsStr<T>
where
    T: fmt::Debug + FromStr,
    T::Err: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.value.fmt(f)
    }
}
