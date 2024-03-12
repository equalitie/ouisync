use crate::{
    config::{ConfigError, ConfigKey, ConfigStore},
    device_id,
    protocol::remote::{v1, Request, ServerError},
    transport::RemoteClient,
};
use ouisync_lib::{
    crypto::sign::Signature, Access, AccessMode, AccessSecrets, LocalSecret, Repository,
    RepositoryId, RepositoryParams, SetLocalSecret, ShareToken, StorageSize, WriteSecrets,
};
use state_monitor::StateMonitor;
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use thiserror::Error;
use tokio_rustls::rustls;
use tracing::instrument;

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
pub enum RemoteError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("failed to connect to server")]
    Connect(#[source] io::Error),
    #[error("server responded with error")]
    Server(#[from] ServerError),
}

/// Creates a new repository and set access to it based on the following table:
///
/// local_read_secret  |  local_write_secret  |  token access  |  result
/// -------------------+----------------------+----------------+------------------------------
/// None or any        |  None or any         |  blind         |  blind replica
/// None               |  None or any         |  read          |  read without secret key
/// read_secret        |  None or any         |  read          |  read with read_secret as secret key
/// None               |  None                |  write         |  read and write without secret key
/// any                |  None                |  write         |  read (only!) with secret key
/// None               |  any                 |  write         |  read without secret, require secret key for writing
/// any                |  any                 |  write         |  read with secret key, write with (same or different) secret key
pub async fn create(
    store: PathBuf,
    local_read_secret: Option<SetLocalSecret>,
    local_write_secret: Option<SetLocalSecret>,
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

    let access = Access::new(local_read_secret, local_write_secret, access_secrets);

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
    local_secret: Option<LocalSecret>,
    config: &ConfigStore,
    repos_monitor: &StateMonitor,
) -> Result<Repository, OpenError> {
    let params = RepositoryParams::new(store)
        .with_device_id(device_id::get_or_create(config).await?)
        .with_parent_monitor(repos_monitor.clone());

    let repository = Repository::open(&params, local_secret, AccessMode::Write).await?;

    Ok(repository)
}

/// The `key` parameter is optional, if `None` the current access level of the opened
/// repository is used. If provided, the highest access level that the key can unlock is used.
pub async fn create_share_token(
    repository: &Repository,
    local_secret: Option<LocalSecret>,
    access_mode: AccessMode,
    name: Option<String>,
) -> Result<String, ouisync_lib::Error> {
    let access_secrets = if let Some(local_secret) = local_secret {
        repository.unlock_secrets(local_secret).await?
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

/// Create mirrored repository on the cache server
#[instrument(skip(repository, client_config))]
pub async fn create_mirror(
    repository: &Repository,
    client_config: Arc<rustls::ClientConfig>,
    host: &str,
) -> Result<(), RemoteError> {
    let secrets = repository
        .secrets()
        .into_write_secrets()
        .ok_or(RemoteError::PermissionDenied)?;

    let client = connect(client_config, host).await?;
    let proof = make_proof(&client, &secrets);

    invoke(
        &client,
        v1::Request::Create {
            repository_id: secrets.id,
            proof,
        },
    )
    .await
}

/// Delete mirrored repository from the cache server
#[instrument(skip(repository, client_config))]
pub async fn delete_mirror(
    repository: &Repository,
    client_config: Arc<rustls::ClientConfig>,
    host: &str,
) -> Result<(), RemoteError> {
    let secrets = repository
        .secrets()
        .into_write_secrets()
        .ok_or(RemoteError::PermissionDenied)?;

    let client = connect(client_config, host).await?;
    let proof = make_proof(&client, &secrets);

    invoke(
        &client,
        v1::Request::Delete {
            repository_id: secrets.id,
            proof,
        },
    )
    .await
}

/// Check if the repository is mirrored on the cache server.
#[instrument(skip(repository_id, client_config))]
pub async fn mirror_exists(
    repository_id: &RepositoryId,
    client_config: Arc<rustls::ClientConfig>,
    host: &str,
) -> Result<bool, RemoteError> {
    let client = connect(client_config, host).await?;

    match invoke(
        &client,
        v1::Request::Exists {
            repository_id: *repository_id,
        },
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(RemoteError::Server(ServerError::NotFound)) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn connect(
    client_config: Arc<rustls::ClientConfig>,
    host: &str,
) -> Result<RemoteClient, RemoteError> {
    RemoteClient::connect(host, client_config)
        .await
        .map_err(|error| {
            tracing::debug!(?error, "connection failed");
            error
        })
        .map_err(RemoteError::Connect)
}

async fn invoke(client: &RemoteClient, request: v1::Request) -> Result<(), RemoteError> {
    let result = client.invoke(Request::V1(request)).await;
    tracing::debug!("response: {:?}", result);
    result.map_err(RemoteError::Server)
}

fn make_proof(client: &RemoteClient, secrets: &WriteSecrets) -> Signature {
    let cookie = client.session_cookie();
    secrets.write_keys.sign(cookie.as_ref())
}
