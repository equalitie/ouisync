use crate::{
    constants::{
        ACCESS_MODE_BLIND, ACCESS_MODE_READ, ACCESS_MODE_WRITE, ENTRY_TYPE_DIRECTORY,
        ENTRY_TYPE_FILE,
    },
    error::{Error, Result},
    protocol::Notification,
    registry::Handle,
    state::{ClientState, ServerState, SubscriptionHandle},
};
use camino::Utf8PathBuf;
use ouisync_lib::{
    crypto::Password,
    device_id,
    network::{self, Registration},
    path, Access, AccessMode, AccessSecrets, EntryType, Event, LocalSecret, Payload, Progress,
    ReopenToken, Repository, RepositoryDb, ShareToken,
};
use std::borrow::Cow;
use tokio::sync::broadcast::error::RecvError;
use tracing::Instrument;

pub struct RepositoryHolder {
    pub(super) repository: Repository,
    registration: Registration,
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
pub(crate) async fn create(
    state: &ServerState,
    store: Utf8PathBuf,
    local_read_password: Option<String>,
    local_write_password: Option<String>,
    share_token: Option<ShareToken>,
) -> Result<Handle<RepositoryHolder>> {
    let local_read_password = local_read_password.as_deref().map(Password::new);
    let local_write_password = local_write_password.as_deref().map(Password::new);

    let access_secrets = if let Some(share_token) = share_token {
        share_token.into_secrets()
    } else {
        AccessSecrets::random_write()
    };

    let span = state.repo_span(&store);

    async {
        let device_id = device_id::get_or_create(&state.config).await?;

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

        let registration = state.network.handle().register(repository.store().clone());
        init(&registration);

        let holder = RepositoryHolder {
            repository,
            registration,
        };

        let handle = state.repositories.insert(holder);

        Ok(handle)
    }
    .instrument(span)
    .await
}

/// Opens an existing repository.
pub(crate) async fn open(
    state: &ServerState,
    store: Utf8PathBuf,
    local_password: Option<String>,
) -> Result<Handle<RepositoryHolder>> {
    let local_password = local_password.as_deref().map(Password::new);

    let span = state.repo_span(&store);

    async {
        let device_id = device_id::get_or_create(&state.config).await?;

        let repository = Repository::open(
            store.into_std_path_buf(),
            device_id,
            local_password.map(LocalSecret::Password),
        )
        .await?;

        let registration = state.network.handle().register(repository.store().clone());
        init(&registration);

        let holder = RepositoryHolder {
            repository,
            registration,
        };

        let handle = state.repositories.insert(holder);

        Ok(handle)
    }
    .instrument(span)
    .await
}

/// Closes a repository.
pub(crate) async fn close(state: &ServerState, handle: Handle<RepositoryHolder>) -> Result<()> {
    let holder = state.repositories.remove(handle);

    if let Some(holder) = holder {
        holder.repository.close().await?
    }

    Ok(())
}

pub(crate) fn create_reopen_token(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<Vec<u8>> {
    let holder = state.repositories.get(handle);
    let token = holder.repository.reopen_token().encode();

    Ok(token)
}

pub(crate) async fn reopen(
    state: &ServerState,
    store: Utf8PathBuf,
    token: Vec<u8>,
) -> Result<Handle<RepositoryHolder>> {
    let token = ReopenToken::decode(&token)?;
    let span = state.repo_span(&store);

    async {
        let repository = Repository::reopen(store.into_std_path_buf(), token).await?;

        let registration = state.network.handle().register(repository.store().clone());
        init(&registration);

        let holder = RepositoryHolder {
            repository,
            registration,
        };

        Ok(state.repositories.insert(holder))
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
pub(crate) async fn set_read_access(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    local_read_password: Option<String>,
    share_token: Option<ShareToken>,
) -> Result<()> {
    let holder = state.repositories.get(handle);

    // If None, repository shall attempt to use the one it's currently using.
    let access_secrets = share_token.map(ShareToken::into_secrets);

    let local_read_secret = local_read_password
        .as_deref()
        .map(Password::new)
        .map(LocalSecret::Password);

    holder
        .repository
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
pub(crate) async fn set_read_and_write_access(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    local_old_rw_password: Option<String>,
    local_new_rw_password: Option<String>,
    share_token: Option<ShareToken>,
) -> Result<()> {
    let holder = state.repositories.get(handle);

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

    holder
        .repository
        .set_read_and_write_access(
            local_old_rw_secret.as_ref(),
            local_new_rw_secret.as_ref(),
            access_secrets.as_ref(),
        )
        .await?;

    Ok(())
}

/// Note that after removing read key the user may still read the repository if they previously had
/// write key set up.
pub(crate) async fn remove_read_key(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<()> {
    state
        .repositories
        .get(handle)
        .repository
        .remove_read_key()
        .await?;
    Ok(())
}

/// Note that removing the write key will leave read key intact.
pub(crate) async fn remove_write_key(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<()> {
    state
        .repositories
        .get(handle)
        .repository
        .remove_write_key()
        .await?;
    Ok(())
}

/// Returns true if the repository requires a local password to be opened for reading.
pub(crate) async fn requires_local_password_for_reading(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<bool> {
    Ok(state
        .repositories
        .get(handle)
        .repository
        .requires_local_password_for_reading()
        .await?)
}

/// Returns true if the repository requires a local password to be opened for writing.
pub(crate) async fn requires_local_password_for_writing(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<bool> {
    Ok(state
        .repositories
        .get(handle)
        .repository
        .requires_local_password_for_writing()
        .await?)
}

/// Return the info-hash of the repository formatted as hex string. This can be used as a globally
/// unique, non-secret identifier of the repository.
/// User is responsible for deallocating the returned string.
pub(crate) fn info_hash(state: &ServerState, handle: Handle<RepositoryHolder>) -> String {
    let holder = state.repositories.get(handle);
    let info_hash = network::repository_info_hash(holder.repository.secrets().id());

    hex::encode(info_hash)
}

/// Returns an ID that is randomly generated once per repository. Can be used to store local user
/// data per repository (e.g. passwords behind biometric storage).
pub(crate) async fn database_id(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<Vec<u8>> {
    let holder = state.repositories.get(handle);
    Ok(holder.repository.database_id().await?.as_ref().to_vec())
}

/// Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
/// exist.
pub(crate) async fn entry_type(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    path: Utf8PathBuf,
) -> Result<Option<u8>> {
    let holder = state.repositories.get(handle);

    match holder.repository.lookup_type(path).await {
        Ok(entry_type) => Ok(Some(entry_type_to_num(entry_type))),
        Err(ouisync_lib::Error::EntryNotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Move/rename entry from src to dst.
pub(crate) async fn move_entry(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    src: Utf8PathBuf,
    dst: Utf8PathBuf,
) -> Result<()> {
    let holder = state.repositories.get(handle);
    let (src_dir, src_name) = path::decompose(&src).ok_or(ouisync_lib::Error::EntryNotFound)?;
    let (dst_dir, dst_name) = path::decompose(&dst).ok_or(ouisync_lib::Error::EntryNotFound)?;

    holder
        .repository
        .move_entry(src_dir, src_name, dst_dir, dst_name)
        .await?;

    Ok(())
}

/// Subscribe to change notifications from the repository.
pub(crate) fn subscribe(
    server_state: &ServerState,
    client_state: &ClientState,
    repository_handle: Handle<RepositoryHolder>,
) -> SubscriptionHandle {
    let holder = server_state.repositories.get(repository_handle);

    let mut notification_rx = holder.repository.subscribe();
    let notification_tx = client_state.notification_tx.clone();

    let entry = server_state.tasks.vacant_entry();
    let subscription_id = entry.handle().id();

    let subscription_task = scoped_task::spawn(async move {
        loop {
            match notification_rx.recv().await {
                // Only `BlockReceived` events cause user-observable changes
                Ok(Event {
                    payload: Payload::BlockReceived { .. },
                    ..
                }) => (),
                Ok(Event {
                    payload: Payload::BranchChanged(_) | Payload::FileClosed,
                    ..
                }) => continue,
                Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => break,
            }

            notification_tx
                .send((subscription_id, Notification::Repository))
                .await
                .ok();
        }
    });

    entry.insert(subscription_task)
}

pub(crate) fn is_dht_enabled(state: &ServerState, handle: Handle<RepositoryHolder>) -> bool {
    state.repositories.get(handle).registration.is_dht_enabled()
}

pub(crate) fn set_dht_enabled(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    enabled: bool,
) {
    let reg = &state.repositories.get(handle).registration;

    if enabled {
        reg.enable_dht()
    } else {
        reg.disable_dht()
    }
}

pub(crate) fn is_pex_enabled(state: &ServerState, handle: Handle<RepositoryHolder>) -> bool {
    state.repositories.get(handle).registration.is_pex_enabled()
}

pub(crate) fn set_pex_enabled(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    enabled: bool,
) {
    let reg = &state.repositories.get(handle).registration;

    if enabled {
        reg.enable_pex()
    } else {
        reg.disable_pex()
    }
}

/// The `password` parameter is optional, if `None` the current access level of the opened
/// repository is used. If provided, the highest access level that the password can unlock is used.
pub(crate) async fn create_share_token(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
    password: Option<String>,
    access_mode: u8,
    name: Option<String>,
) -> Result<String> {
    let holder = state.repositories.get(handle);
    let access_mode = access_mode_from_num(access_mode)?;
    let password = password.as_deref().map(Password::new);

    let access_secrets = if let Some(password) = password {
        Cow::Owned(
            holder
                .repository
                .unlock_secrets(LocalSecret::Password(password))
                .await?,
        )
    } else {
        Cow::Borrowed(holder.repository.secrets())
    };

    let share_token = ShareToken::from(access_secrets.with_mode(access_mode));
    let share_token = if let Some(name) = name {
        share_token.with_name(name)
    } else {
        share_token
    };

    Ok(share_token.to_string())
}

pub(crate) fn access_mode(state: &ServerState, handle: Handle<RepositoryHolder>) -> u8 {
    let holder = state.repositories.get(handle);
    access_mode_to_num(holder.repository.access_mode())
}

/// Returns the syncing progress.
pub(crate) async fn sync_progress(
    state: &ServerState,
    handle: Handle<RepositoryHolder>,
) -> Result<Progress> {
    Ok(state
        .repositories
        .get(handle)
        .repository
        .sync_progress()
        .await?)
}

pub(super) fn entry_type_to_num(entry_type: EntryType) -> u8 {
    match entry_type {
        EntryType::File => ENTRY_TYPE_FILE,
        EntryType::Directory => ENTRY_TYPE_DIRECTORY,
    }
}

fn access_mode_from_num(num: u8) -> Result<AccessMode, Error> {
    // Note: we could've used `AccessMode::try_from` instead but then we would need a separate
    // check (ideally a compile-time one) that the `ACCESS_MODE_*` constants match the
    // corresponding `AccessMode` variants.

    match num {
        ACCESS_MODE_BLIND => Ok(AccessMode::Blind),
        ACCESS_MODE_READ => Ok(AccessMode::Read),
        ACCESS_MODE_WRITE => Ok(AccessMode::Write),
        _ => Err(Error::InvalidArgument),
    }
}

pub(crate) fn access_mode_to_num(mode: AccessMode) -> u8 {
    match mode {
        AccessMode::Blind => ACCESS_MODE_BLIND,
        AccessMode::Read => ACCESS_MODE_READ,
        AccessMode::Write => ACCESS_MODE_WRITE,
    }
}

fn init(registration: &Registration) {
    // TODO: consider leaving the decision to enable DHT, PEX to the app.
    registration.enable_dht();
    registration.enable_pex();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_constants() {
        for mode in [AccessMode::Blind, AccessMode::Read, AccessMode::Write] {
            assert_eq!(
                access_mode_from_num(access_mode_to_num(mode)).unwrap(),
                mode
            );
        }
    }
}
