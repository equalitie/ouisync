use crate::{
    network,
    protocol::Value,
    registry::Handle,
    repository::{self, RepositoryHolder},
    session::{self, SubscriptionHandle},
    state::{ClientState, ServerState},
    state_monitor,
};
use ouisync_lib::Result;
use serde::{Deserialize, Deserializer};
use std::{
    fmt,
    net::{SocketAddrV4, SocketAddrV6},
    str::FromStr,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "args")]
pub(crate) enum Request {
    RepositoryCreate {
        path: String,
        read_password: Option<String>,
        write_password: Option<String>,
        share_token: Option<String>,
    },
    RepositoryOpen {
        path: String,
        password: Option<String>,
    },
    RepositoryClose(Handle<RepositoryHolder>),
    RepositorySubscribe(Handle<RepositoryHolder>),
    RepositorySetReadAccess {
        repository: Handle<RepositoryHolder>,
        read_password: Option<String>,
        share_token: Option<String>,
    },
    RepositorySetReadAndWriteAccess {
        repository: Handle<RepositoryHolder>,
        password: Option<String>,
        share_token: Option<String>,
    },
    RepositoryRemoveReadKey(Handle<RepositoryHolder>),
    RepositoryRemoveWriteKey(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForReading(Handle<RepositoryHolder>),
    RepositoryRequiresLocalPasswordForWriting(Handle<RepositoryHolder>),
    RepositoryInfoHash(Handle<RepositoryHolder>),
    RepositoryDatabaseId(Handle<RepositoryHolder>),
    RepositoryEntryType {
        repository: Handle<RepositoryHolder>,
        path: String,
    },
    RepositoryMoveEntry {
        repository: Handle<RepositoryHolder>,
        src: String,
        dst: String,
    },
    RepositoryIsDhtEnabled(Handle<RepositoryHolder>),
    RepositorySetDhtEnabled {
        repository: Handle<RepositoryHolder>,
        enabled: bool,
    },
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
    tracing::debug!(?request);

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
            read_password,
            share_token,
        } => repository::set_read_access(server_state, repository, read_password, share_token)
            .await?
            .into(),
        Request::RepositorySetReadAndWriteAccess {
            repository,
            password,
            share_token,
        } => repository::set_read_and_write_access(server_state, repository, password, share_token)
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
