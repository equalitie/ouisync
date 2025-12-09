use std::{path::PathBuf, slice};

use tracing::instrument;

use crate::{
    LOCAL_ENDPOINT_KEY,
    config_keys::STORE_DIRS_KEY,
    config_store::{ConfigError, ConfigKey, ConfigStore},
};

#[instrument(name = "config_migration", skip(config))]
pub(crate) async fn run(config: &ConfigStore) {
    local_control(config).await;
    store_dirs(config).await;
    apply_private(config).await;
}

#[instrument(skip(config))]
async fn local_control(config: &ConfigStore) {
    let keys = [
        ConfigKey::<()>::new("local_control_port", ""),
        ConfigKey::<()>::new("local_control_auth_key", ""),
    ];

    for key in keys {
        let entry = config.entry(key);
        if let Err(error) = entry.remove().await {
            tracing::error!(?error, entry = ?entry.path(), "{ERROR_REMOVE}")
        }
    }
}

#[instrument(skip(config))]
async fn store_dirs(config: &ConfigStore) {
    let old = config.entry(ConfigKey::<PathBuf>::new("store_dir", ""));
    let new = config.entry(STORE_DIRS_KEY);

    let value = match old.get().await {
        Ok(value) => Some(value),
        Err(ConfigError::NotFound) => None,
        Err(error) => {
            tracing::error!(?error, entry = ?old.path(), "{ERROR_GET}");
            None
        }
    };

    let remove_old = if let Some(value) = value {
        match new.get().await {
            Ok(_) => true,
            Err(ConfigError::NotFound) => match new.set(slice::from_ref(&value)).await {
                Ok(()) => true,
                Err(error) => {
                    tracing::error!(?error, entry = ?new.path(), "{ERROR_SET}");
                    false
                }
            },
            Err(error) => {
                tracing::error!(?error, entry = ?new.path(), "{ERROR_GET}");
                false
            }
        }
    } else {
        true
    };

    if remove_old && let Err(error) = old.remove().await {
        tracing::error!(?error, entry = ?old.path(), "{ERROR_REMOVE}");
    }
}

#[cfg(unix)]
#[instrument(skip(config))]
async fn apply_private(config: &ConfigStore) {
    use std::{fs::Permissions, io, os::unix::fs::PermissionsExt};
    use tokio::fs::File;

    let paths = [config.entry(LOCAL_ENDPOINT_KEY).path()];

    for path in paths {
        match File::open(&path).await {
            Ok(file) => match file.set_permissions(Permissions::from_mode(0o660)).await {
                Ok(()) => (),
                Err(error) => {
                    tracing::error!(?error, entry = ?path, "failed to change entry permissionss")
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => tracing::error!(?error, entry = ?path, "{ERROR_GET}"),
        }
    }
}

#[cfg(not(unix))]
async fn apply_private(_: &ConfigStore) {}

const ERROR_SET: &str = "failed to set entry";
const ERROR_GET: &str = "failed to get entry";
const ERROR_REMOVE: &str = "failed to remove entry";
