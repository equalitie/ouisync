use crate::{config::ConfigStore, device_id, error::Result};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync_lib::{
    crypto::Password,
    network::{Network, Registration},
    Access, AccessMode, AccessSecrets, LocalSecret, ReopenToken, Repository, RepositoryDb,
    ShareToken,
};
use std::{borrow::Cow, sync::Arc};
use tracing::{Instrument, Span};

pub struct RepositoryHolder {
    pub repository: Arc<Repository>,
    pub registration: Registration,
}

/// Creates a new repository and set access to it based on the following table:
///
/// local_read_password  |  local_write_password  |  token access  |  result
/// ---------------------+------------------------+----------------+------------------------------
/// None or any          |  None or any           |  blind         |  blind replica
/// None                 |  None or any           |  read          |  read without password
/// read_pwd             |  None or any           |  read          |  read with read_pwd as password
/// None                 |  None                  |  write         |  read and write without password
/// any                  |  None                  |  write         |  read (only!) with password
/// None                 |  any                   |  write         |  read without password, require password for writing
/// any                  |  any                   |  write         |  read with password, write with (same or different) password
pub async fn create(
    store: Utf8PathBuf,
    local_read_password: Option<String>,
    local_write_password: Option<String>,
    share_token: Option<ShareToken>,
    config: &ConfigStore,
    network: &Network,
) -> Result<RepositoryHolder> {
    let local_read_password = local_read_password.as_deref().map(Password::new);
    let local_write_password = local_write_password.as_deref().map(Password::new);

    let access_secrets = if let Some(share_token) = share_token {
        share_token.into_secrets()
    } else {
        AccessSecrets::random_write()
    };

    let span = repo_span(&store);

    async {
        let device_id = device_id::get_or_create(config).await?;

        let db = RepositoryDb::create(store.into_std_path_buf()).await?;

        let local_read_key = if let Some(local_read_password) = local_read_password {
            Some(db.password_to_key(local_read_password).await?)
        } else {
            None
        };

        let local_write_key = if let Some(local_write_password) = local_write_password {
            Some(db.password_to_key(local_write_password).await?)
        } else {
            None
        };

        let access = Access::new(local_read_key, local_write_key, access_secrets);
        let repository = Repository::create(db, device_id, access).await?;
        let repository = Arc::new(repository);

        let registration = network.register(repository.store().clone()).await;

        Ok(RepositoryHolder {
            repository,
            registration,
        })
    }
    .instrument(span)
    .await
}

/// Opens an existing repository.
pub async fn open(
    store: Utf8PathBuf,
    local_password: Option<String>,
    config: &ConfigStore,
    network: &Network,
) -> Result<RepositoryHolder> {
    let local_password = local_password.as_deref().map(Password::new);

    let span = repo_span(&store);

    async {
        let device_id = device_id::get_or_create(config).await?;

        let repository = Repository::open(
            store.into_std_path_buf(),
            device_id,
            local_password.map(LocalSecret::Password),
        )
        .await?;
        let repository = Arc::new(repository);

        let registration = network.register(repository.store().clone()).await;

        Ok(RepositoryHolder {
            repository,
            registration,
        })
    }
    .instrument(span)
    .await
}

pub async fn reopen(
    store: Utf8PathBuf,
    token: Vec<u8>,
    network: &Network,
) -> Result<RepositoryHolder> {
    let token = ReopenToken::decode(&token)?;
    let span = repo_span(&store);

    async {
        let repository = Repository::reopen(store.into_std_path_buf(), token).await?;
        let repository = Arc::new(repository);

        let registration = network.register(repository.store().clone()).await;

        Ok(RepositoryHolder {
            repository,
            registration,
        })
    }
    .instrument(span)
    .await
}

/// If `share_token` is null, the function will try with the currently active access secrets in the
/// repository. Note that passing `share_token` explicitly (as opposed to implicitly using the
/// currently active secrets) may be used to increase access permissions.
///
/// Attempting to change the secret without enough permissions will fail with PermissionDenied
/// error.
///
/// If `local_read_password` is null, the repository will become readable without a password.
/// To remove the read (and write) permission use the `repository_remove_read_access`
/// function.
pub async fn set_read_access(
    repository: &Repository,
    local_read_password: Option<String>,
    share_token: Option<ShareToken>,
) -> Result<()> {
    // If None, repository shall attempt to use the one it's currently using.
    let access_secrets = share_token.map(ShareToken::into_secrets);

    let local_read_secret = local_read_password
        .as_deref()
        .map(Password::new)
        .map(LocalSecret::Password);

    repository
        .set_read_access(local_read_secret.as_ref(), access_secrets.as_ref())
        .await?;

    Ok(())
}

/// If `share_token` is `None`, the function will try with the currently active access secrets in the
/// repository. Note that passing `share_token` explicitly (as opposed to implicitly using the
/// currently active secrets) may be used to increase access permissions.
///
/// Attempting to change the secret without enough permissions will fail with PermissionDenied
/// error.
///
/// If `local_new_rw_password` is None, the repository will become read and writable without a
/// password.  To remove the read and write access use the
/// `repository_remove_read_and_write_access` function.
///
/// The `local_old_rw_password` is optional, if it is set the previously used "writer ID" shall be
/// used, otherwise a new one shall be generated. Note that it is preferred to keep the writer ID
/// as it was, this reduces the number of writers in version vectors for every entry in the
/// repository (files and directories) and thus reduces traffic and CPU usage when calculating
/// causal relationships.
pub async fn set_read_and_write_access(
    repository: &Repository,
    local_old_rw_password: Option<String>,
    local_new_rw_password: Option<String>,
    share_token: Option<ShareToken>,
) -> Result<()> {
    // If None, repository shall attempt to use the one it's currently using.
    let access_secrets = share_token.map(ShareToken::into_secrets);

    let local_old_rw_secret = local_old_rw_password
        .as_deref()
        .map(Password::new)
        .map(LocalSecret::Password);

    let local_new_rw_secret = local_new_rw_password
        .as_deref()
        .map(Password::new)
        .map(LocalSecret::Password);

    repository
        .set_read_and_write_access(
            local_old_rw_secret.as_ref(),
            local_new_rw_secret.as_ref(),
            access_secrets.as_ref(),
        )
        .await?;

    Ok(())
}

/// The `password` parameter is optional, if `None` the current access level of the opened
/// repository is used. If provided, the highest access level that the password can unlock is used.
pub async fn create_share_token(
    repository: &Repository,
    password: Option<String>,
    access_mode: AccessMode,
    name: Option<String>,
) -> Result<String> {
    let password = password.as_deref().map(Password::new);

    let access_secrets = if let Some(password) = password {
        Cow::Owned(
            repository
                .unlock_secrets(LocalSecret::Password(password))
                .await?,
        )
    } else {
        Cow::Borrowed(repository.secrets())
    };

    let share_token = ShareToken::from(access_secrets.with_mode(access_mode));
    let share_token = if let Some(name) = name {
        share_token.with_name(name)
    } else {
        share_token
    };

    Ok(share_token.to_string())
}

fn repo_span(store: &Utf8Path) -> Span {
    tracing::info_span!("repo", ?store)
}
