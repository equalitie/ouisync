use crate::{
    config::{ConfigError, ConfigKey, ConfigStore},
    device_id,
    protocol::remote::{Request, Response, ServerError},
    transport::RemoteClient,
};
use futures_util::future;
use ouisync_lib::{
    crypto::cipher::SecretKey, Access, AccessMode, AccessSecrets, KeyAndSalt, Repository,
    RepositoryParams, ShareToken, StorageSize,
};
use state_monitor::StateMonitor;
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use thiserror::Error;
use tokio_rustls::rustls;

const DEFAULT_QUOTA_KEY: ConfigKey<u64> = ConfigKey::new("default_quota", "Default storage quota");
const DEFAULT_BLOCK_EXPIRATION_MILLIS: ConfigKey<u64> = ConfigKey::new(
    "default_block_expiration",
    "Default time in seconds when blocks start to expire if not used",
);

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("config error")]
    Config(#[from] ConfigError),
    #[error("repository error")]
    Repository(#[from] ouisync_lib::Error),
}

#[derive(Debug, Error)]
pub enum MirrorError {
    #[error("failed to connect to server")]
    Connect(#[source] io::Error),
    #[error("server responded with error")]
    Server(#[source] ServerError),
}

/// Creates a new repository and set access to it based on the following table:
///
/// local_read_key  |  local_write_key  |  token access  |  result
/// ----------------+-------------------+----------------+------------------------------
/// None or any     |  None or any      |  blind         |  blind replica
/// None            |  None or any      |  read          |  read without secret key
/// read_secret     |  None or any      |  read          |  read with read_secret as secret key
/// None            |  None             |  write         |  read and write without secret key
/// any             |  None             |  write         |  read (only!) with secret key
/// None            |  any              |  write         |  read without secret, require secret key for writing
/// any             |  any              |  write         |  read with secret key, write with (same or different) secret key
pub async fn create(
    store: PathBuf,
    local_read_key: Option<KeyAndSalt>,
    local_write_key: Option<KeyAndSalt>,
    share_token: Option<ShareToken>,
    config: &ConfigStore,
    repos_monitor: &StateMonitor,
) -> Result<Repository, OpenError> {
    let params = RepositoryParams::new(store)
        .with_device_id(device_id::get_or_create(config).await?)
        .with_parent_monitor(repos_monitor.clone());

    let access_secrets = if let Some(share_token) = share_token {
        share_token.into_secrets()
    } else {
        AccessSecrets::random_write()
    };

    let access = Access::new(local_read_key, local_write_key, access_secrets);

    let repository = Repository::create(&params, access).await?;

    let quota = get_default_quota(config).await?;
    repository.set_quota(quota).await?;

    let block_expiration = get_default_block_expiration(config).await?;
    repository.set_block_expiration(block_expiration).await?;

    Ok(repository)
}

/// Opens an existing repository.
pub async fn open(
    store: PathBuf,
    local_key: Option<SecretKey>,
    config: &ConfigStore,
    repos_monitor: &StateMonitor,
) -> Result<Repository, OpenError> {
    let params = RepositoryParams::new(store)
        .with_device_id(device_id::get_or_create(config).await?)
        .with_parent_monitor(repos_monitor.clone());

    let repository = Repository::open(&params, local_key, AccessMode::Write).await?;

    Ok(repository)
}

/// The `key` parameter is optional, if `None` the current access level of the opened
/// repository is used. If provided, the highest access level that the key can unlock is used.
pub async fn create_share_token(
    repository: &Repository,
    key: Option<SecretKey>,
    access_mode: AccessMode,
    name: Option<String>,
) -> Result<String, ouisync_lib::Error> {
    let access_secrets = if let Some(key) = key {
        repository.unlock_secrets(key).await?
    } else {
        repository.secrets()
    };

    let share_token = ShareToken::from(access_secrets.with_mode(access_mode));
    let share_token = if let Some(name) = name {
        share_token.with_name(name)
    } else {
        share_token
    };

    Ok(share_token.to_string())
}

pub async fn set_default_quota(
    config: &ConfigStore,
    value: Option<StorageSize>,
) -> Result<(), ConfigError> {
    let entry = config.entry(DEFAULT_QUOTA_KEY);

    if let Some(value) = value {
        entry.set(&value.to_bytes()).await?;
    } else {
        entry.remove().await?;
    }

    Ok(())
}

pub async fn get_default_quota(config: &ConfigStore) -> Result<Option<StorageSize>, ConfigError> {
    let entry = config.entry(DEFAULT_QUOTA_KEY);

    match entry.get().await {
        Ok(quota) => Ok(Some(StorageSize::from_bytes(quota))),
        Err(ConfigError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn set_default_block_expiration(
    config: &ConfigStore,
    value: Option<Duration>,
) -> Result<(), ConfigError> {
    let entry = config.entry(DEFAULT_BLOCK_EXPIRATION_MILLIS);

    if let Some(value) = value {
        entry
            .set(&value.as_millis().try_into().unwrap_or(u64::MAX))
            .await?;
    } else {
        entry.remove().await?;
    }

    Ok(())
}

pub async fn get_default_block_expiration(
    config: &ConfigStore,
) -> Result<Option<Duration>, ConfigError> {
    let entry = config.entry::<u64>(DEFAULT_BLOCK_EXPIRATION_MILLIS);

    match entry.get().await {
        Ok(millis) => Ok(Some(Duration::from_millis(millis))),
        Err(ConfigError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Mirror the repository to the storage servers
pub async fn mirror(
    repository: &Repository,
    client_config: Arc<rustls::ClientConfig>,
    hosts: &[String],
) -> Result<(), MirrorError> {
    let share_token = repository.secrets().with_mode(AccessMode::Blind);

    let tasks = hosts.iter().map(|host| {
        let client_config = client_config.clone();
        let share_token = share_token.clone();

        // Strip port, if any.
        let host = strip_port(host);

        async move {
            let client = RemoteClient::connect(host, client_config)
                .await
                .map_err(MirrorError::Connect)
                .map_err(|error| {
                    tracing::error!(host, ?error, "mirror request failed");
                    error
                })?;

            let request = Request::Mirror {
                share_token: share_token.into(),
            };

            match client.invoke(request).await.map_err(MirrorError::Server) {
                Ok(Response::None) => {
                    tracing::info!(host, "mirror request successfull");
                    Ok(())
                }
                Err(error) => {
                    tracing::error!(host, ?error, "mirror request failed");
                    Err(error)
                }
            }
        }
    });

    let results = future::join_all(tasks).await;

    if results.iter().any(|result| result.is_ok()) {
        Ok(())
    } else {
        results.into_iter().next().unwrap_or(Ok(()))
    }
}

fn strip_port(s: &str) -> &str {
    if let Some(index) = s.rfind(':') {
        &s[..index]
    } else {
        s
    }
}
