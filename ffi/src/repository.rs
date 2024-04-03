use crate::{
    error::Error,
    registry::{Handle, InvalidHandle, Registry},
    state::{State, TaskHandle},
};
use camino::Utf8PathBuf;
use ouisync_bridge::{protocol::Notification, repository, transport::NotificationSender};
use ouisync_lib::{
    network::{self, Registration},
    path, AccessMode, Credentials, Event, LocalSecret, Payload, Progress, Repository,
    SetLocalSecret, ShareToken,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::Entry, HashMap},
    mem,
    path::PathBuf,
    sync::{Arc, RwLock as BlockingRwLock},
};
use thiserror::Error;
use tokio::sync::{broadcast::error::RecvError, Notify, RwLock as AsyncRwLock};

pub(crate) struct RepositoryHolder {
    pub store_path: PathBuf,
    pub repository: Arc<Repository>,
    pub registration: AsyncRwLock<Option<Registration>>,
}

pub(crate) type RepositoryHandle = Handle<Arc<RepositoryHolder>>;

#[derive(Debug, Error)]
#[error("operation requires network registration")]
pub(crate) struct RegistrationRequired;

#[derive(Debug, Error)]
#[error("entry has been changed")]
pub(crate) struct EntryChanged;

pub(crate) async fn create(
    state: &State,
    store_path: PathBuf,
    local_read_secret: Option<SetLocalSecret>,
    local_write_secret: Option<SetLocalSecret>,
    share_token: Option<ShareToken>,
) -> Result<RepositoryHandle, Error> {
    let entry = ensure_vacant_entry(state, store_path.clone()).await?;

    let repository = repository::create(
        store_path.clone(),
        local_read_secret,
        local_write_secret,
        share_token,
        &state.config,
        &state.repos_monitor,
    )
    .await?;

    let holder = RepositoryHolder {
        store_path,
        repository: Arc::new(repository),
        registration: AsyncRwLock::new(None),
    };

    state
        .mounter
        .mount(&holder.store_path, &holder.repository)?;

    let handle = entry.insert(holder);

    Ok(handle)
}

/// Opens an existing repository.
pub(crate) async fn open(
    state: &State,
    store_path: PathBuf,
    local_secret: Option<LocalSecret>,
) -> Result<RepositoryHandle, Error> {
    let entry = match state.repositories.entry(store_path.clone()).await {
        RepositoryEntry::Occupied(handle) => {
            // If `local_secret` provides higher access mode than what the repo currently has,
            // increase it. If not, the access mode remains unchanged.
            // See `Repository::set_access_mode` for details.
            let holder = state.repositories.get(handle)?;
            holder
                .repository
                .set_access_mode(AccessMode::Write, local_secret.clone())
                .await?;

            return Ok(handle);
        }
        RepositoryEntry::Vacant(entry) => entry,
    };

    let repository = repository::open(
        store_path.clone(),
        local_secret,
        &state.config,
        &state.repos_monitor,
    )
    .await?;

    let holder = RepositoryHolder {
        store_path,
        repository: Arc::new(repository),
        registration: AsyncRwLock::new(None),
    };

    state
        .mounter
        .mount(&holder.store_path, &holder.repository)?;

    let handle = entry.insert(holder);

    Ok(handle)
}

async fn ensure_vacant_entry(
    state: &State,
    store_path: PathBuf,
) -> Result<RepositoryVacantEntry<'_>, ouisync_lib::Error> {
    loop {
        match state.repositories.entry(store_path.clone()).await {
            RepositoryEntry::Occupied(handle) => {
                if let Some(holder) = state.repositories.remove(handle) {
                    holder.repository.close().await?;
                }
            }
            RepositoryEntry::Vacant(entry) => return Ok(entry),
        }
    }
}

/// Closes a repository.
pub(crate) async fn close(state: &State, handle: RepositoryHandle) -> Result<(), Error> {
    if let Some(holder) = state.repositories.remove(handle) {
        holder.repository.close().await?;
        state.mounter.unmount(&holder.store_path)?;
    }

    Ok(())
}

/// Called when the session is closed and the user has not closed some or all the open
/// repositories.
pub async fn close_all_repositories(state: &State) {
    // Best effort: if some operation fails, continue with the rest.
    for holder in state.repositories.remove_all() {
        if let Err(error) = holder.repository.close().await {
            tracing::warn!(
                "Failed to close repository \"{:?}\": {error:?}",
                holder.store_path
            );
        }
        if let Err(error) = state.mounter.unmount(&holder.store_path) {
            tracing::warn!(
                "Failed to unmount repository \"{:?}\": {error:?}",
                holder.store_path
            );
        }
    }
}

pub(crate) async fn is_sync_enabled(
    state: &State,
    handle: RepositoryHandle,
) -> Result<bool, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .registration
        .read()
        .await
        .is_some())
}

pub(crate) async fn set_sync_enabled(
    state: &State,
    handle: RepositoryHandle,
    enabled: bool,
) -> Result<(), Error> {
    let holder = state.repositories.get(handle)?;

    if enabled {
        let mut registration = holder.registration.write().await;
        if registration.is_none() {
            *registration = Some(state.network.register(holder.repository.handle()).await);
        }
    } else {
        holder.registration.write().await.take();
    }

    Ok(())
}

pub(crate) fn credentials(state: &State, handle: RepositoryHandle) -> Result<Vec<u8>, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .repository
        .credentials()
        .encode())
}

pub(crate) async fn set_credentials(
    state: &State,
    handle: RepositoryHandle,
    credentials: Vec<u8>,
) -> Result<(), Error> {
    state
        .repositories
        .get(handle)?
        .repository
        .set_credentials(Credentials::decode(&credentials)?)
        .await?;

    Ok(())
}

pub(crate) fn access_mode(state: &State, handle: RepositoryHandle) -> Result<u8, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .repository
        .access_mode()
        .into())
}

pub(crate) async fn set_access_mode(
    state: &State,
    handle: RepositoryHandle,
    access_mode: AccessMode,
    local_secret: Option<LocalSecret>,
) -> Result<(), Error> {
    state
        .repositories
        .get(handle)?
        .repository
        .set_access_mode(access_mode, local_secret)
        .await?;

    Ok(())
}

/// Return the info-hash of the repository formatted as hex string. This can be used as a globally
/// unique, non-secret identifier of the repository.
/// User is responsible for deallocating the returned string.
pub(crate) fn info_hash(state: &State, handle: RepositoryHandle) -> Result<String, Error> {
    let holder = state.repositories.get(handle)?;
    let info_hash = network::repository_info_hash(holder.repository.secrets().id());

    Ok(hex::encode(info_hash))
}

/// Returns an ID that is randomly generated once per repository. Can be used to store local user
/// data per repository (e.g. passwords behind biometric storage).
pub(crate) async fn database_id(state: &State, handle: RepositoryHandle) -> Result<Vec<u8>, Error> {
    let holder = state.repositories.get(handle)?;
    Ok(holder.repository.database_id().await?.as_ref().to_vec())
}

/// Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
/// exist.
pub(crate) async fn entry_type(
    state: &State,
    handle: RepositoryHandle,
    path: Utf8PathBuf,
) -> Result<Option<u8>, Error> {
    let holder = state.repositories.get(handle)?;

    match holder.repository.lookup_type(path).await {
        Ok(entry_type) => Ok(Some(entry_type.into())),
        Err(ouisync_lib::Error::EntryNotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Move/rename entry from src to dst.
pub(crate) async fn move_entry(
    state: &State,
    handle: RepositoryHandle,
    src: Utf8PathBuf,
    dst: Utf8PathBuf,
) -> Result<(), Error> {
    let holder = state.repositories.get(handle)?;
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
    state: &State,
    notification_tx: &NotificationSender,
    repository_handle: RepositoryHandle,
) -> Result<TaskHandle, Error> {
    let holder = state.repositories.get(repository_handle)?;

    let mut notification_rx = holder.repository.subscribe();
    let notification_tx = notification_tx.clone();

    let handle = state.spawn_task(|id| async move {
        loop {
            match notification_rx.recv().await {
                Ok(Event {
                    payload: Payload::BranchChanged(_) | Payload::BlockReceived { .. },
                    ..
                }) => (),
                Ok(Event { .. }) => continue,
                Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => break,
            }

            notification_tx
                .send((id, Notification::Repository))
                .await
                .ok();
        }
    });

    Ok(handle)
}

pub(crate) async fn is_dht_enabled(state: &State, handle: RepositoryHandle) -> Result<bool, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .registration
        .read()
        .await
        .as_ref()
        .ok_or(RegistrationRequired)?
        .is_dht_enabled())
}

pub(crate) async fn set_dht_enabled(
    state: &State,
    handle: RepositoryHandle,
    enabled: bool,
) -> Result<(), Error> {
    state
        .repositories
        .get(handle)?
        .registration
        .read()
        .await
        .as_ref()
        .ok_or(RegistrationRequired)?
        .set_dht_enabled(enabled)
        .await;
    Ok(())
}

pub(crate) async fn is_pex_enabled(state: &State, handle: RepositoryHandle) -> Result<bool, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .registration
        .read()
        .await
        .as_ref()
        .ok_or(RegistrationRequired)?
        .is_pex_enabled())
}

pub(crate) async fn set_pex_enabled(
    state: &State,
    handle: RepositoryHandle,
    enabled: bool,
) -> Result<(), Error> {
    state
        .repositories
        .get(handle)?
        .registration
        .read()
        .await
        .as_ref()
        .ok_or(RegistrationRequired)?
        .set_pex_enabled(enabled)
        .await;
    Ok(())
}

/// The `local_secret` parameter is optional, if `None` the current access level of the opened
/// repository is used. If provided, the highest access level that the local_secret can unlock is
/// used.
pub(crate) async fn create_share_token(
    state: &State,
    repository: RepositoryHandle,
    local_secret: Option<LocalSecret>,
    access_mode: AccessMode,
    name: Option<String>,
) -> Result<String, Error> {
    let holder = state.repositories.get(repository)?;
    let token =
        repository::create_share_token(&holder.repository, local_secret, access_mode, name).await?;
    Ok(token)
}

/// Returns the syncing progress.
pub(crate) async fn sync_progress(
    state: &State,
    handle: RepositoryHandle,
) -> Result<Progress, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .repository
        .sync_progress()
        .await?)
}

/// Create mirrored repository on the given server
pub(crate) async fn create_mirror(
    state: &State,
    handle: RepositoryHandle,
    host: &str,
) -> Result<(), Error> {
    let holder = state.repositories.get(handle)?;
    let config = state.get_remote_client_config().await?;

    ouisync_bridge::repository::create_mirror(&holder.repository, config, host).await?;

    Ok(())
}

/// Delete mirrored repository from the given server
pub(crate) async fn delete_mirror(
    state: &State,
    handle: RepositoryHandle,
    host: &str,
) -> Result<(), Error> {
    let holder = state.repositories.get(handle)?;
    let config = state.get_remote_client_config().await?;

    ouisync_bridge::repository::delete_mirror(&holder.repository, config, host).await?;

    Ok(())
}

/// Check if the repository is mirrored on the given server.
pub(crate) async fn mirror_exists(
    state: &State,
    handle: RepositoryHandle,
    host: &str,
) -> Result<bool, Error> {
    let holder = state.repositories.get(handle)?;
    let config = state.get_remote_client_config().await?;

    Ok(
        ouisync_bridge::repository::mirror_exists(holder.repository.secrets().id(), config, host)
            .await?,
    )
}

/// Mount all opened repositories
pub(crate) async fn mount_all(state: &State, mount_point: PathBuf) -> Result<(), Error> {
    let repos = state.repositories.collect();
    state
        .mounter
        .mount_all(
            mount_point,
            repos
                .iter()
                .map(|holder| (holder.store_path.as_ref(), &holder.repository)),
        )
        .await?;

    Ok(())
}

/// Reads a metadata entry
pub(crate) async fn metadata_get(
    state: &State,
    handle: RepositoryHandle,
    key: String,
) -> Result<Option<String>, Error> {
    Ok(state
        .repositories
        .get(handle)?
        .repository
        .metadata()
        .get(&key)
        .await?)
}

/// Atomically updates multiple metadata entries
pub(crate) async fn metadata_set(
    state: &State,
    handle: RepositoryHandle,
    edits: Vec<MetadataEdit>,
) -> Result<(), Error> {
    let mut tx = state
        .repositories
        .get(handle)?
        .repository
        .metadata()
        .write()
        .await?;

    for edit in edits {
        if tx.get(&edit.key).await? != edit.old {
            return Err(EntryChanged.into());
        }

        if let Some(new) = edit.new {
            tx.set(&edit.key, new).await?;
        } else {
            tx.remove(&edit.key).await?;
        }
    }

    tx.commit().await?;

    Ok(())
}

/// Edit of a single metadata entry.
#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct MetadataEdit {
    /// The key of the entry.
    pub key: String,
    /// The current value of the entry or `None` if the entry does not exist yet. This is used for
    /// concurrency control - if the current value is different from this it's assumed it has been
    /// modified by some other task and the whole `RepositorySetMetadata` operation is rolled back.
    /// If that happens, the user should read the current value again, adjust the new value if
    /// needed and retry the operation.
    pub old: Option<String>,
    /// The value to set the entry to or `None` to remove the entry.
    pub new: Option<String>,
}

/// Registry of opened repositories.
pub(crate) struct Repositories {
    inner: BlockingRwLock<Inner>,
}

impl Repositories {
    pub fn new() -> Self {
        Self {
            inner: BlockingRwLock::new(Inner {
                registry: Registry::new(),
                index: HashMap::new(),
            }),
        }
    }

    /// Gets or inserts a repository.
    pub async fn entry(&self, store_path: PathBuf) -> RepositoryEntry {
        loop {
            let notify = {
                let mut inner = self.inner.write().unwrap();

                match inner.index.entry(store_path.clone()) {
                    Entry::Occupied(entry) => match entry.get() {
                        IndexEntry::Reserved(notify) => {
                            // The repo doesn't exists yet but someone is already inserting it.
                            notify.clone()
                        }
                        IndexEntry::Existing(handle) => {
                            // The repo already exists.
                            return RepositoryEntry::Occupied(*handle);
                        }
                    },
                    Entry::Vacant(entry) => {
                        entry.insert(IndexEntry::Reserved(Arc::new(Notify::new())));

                        // The repo doesn't exist yet and we are the first one to insert it.
                        return RepositoryEntry::Vacant(RepositoryVacantEntry {
                            inner: &self.inner,
                            store_path,
                            inserted: false,
                        });
                    }
                }
            };

            notify.notified().await;
        }
    }

    /// Removes the repository regardless of how many handles it has. All outstanding handles
    /// become invalid.
    pub fn remove(&self, handle: RepositoryHandle) -> Option<Arc<RepositoryHolder>> {
        let mut inner = self.inner.write().unwrap();

        let holder = inner.registry.remove(handle)?;
        inner.index.remove(&holder.store_path);

        Some(holder)
    }

    pub fn remove_all(&self) -> Vec<Arc<RepositoryHolder>> {
        self.inner.write().unwrap().registry.remove_all()
    }

    pub fn get(&self, handle: RepositoryHandle) -> Result<Arc<RepositoryHolder>, InvalidHandle> {
        self.inner.read().unwrap().registry.get(handle).cloned()
    }

    pub fn collect(&self) -> Vec<Arc<RepositoryHolder>> {
        self.inner
            .read()
            .unwrap()
            .registry
            .values()
            .cloned()
            .collect()
    }
}

pub(crate) enum RepositoryEntry<'a> {
    Occupied(RepositoryHandle),
    Vacant(RepositoryVacantEntry<'a>),
}

pub(crate) struct RepositoryVacantEntry<'a> {
    inner: &'a BlockingRwLock<Inner>,
    store_path: PathBuf,
    inserted: bool,
}

impl RepositoryVacantEntry<'_> {
    pub fn insert(mut self, holder: RepositoryHolder) -> RepositoryHandle {
        let mut inner = self.inner.write().unwrap();

        let handle = inner.registry.insert(Arc::new(holder));

        let Some(entry) = inner.index.get_mut(&self.store_path) else {
            unreachable!()
        };

        let IndexEntry::Reserved(notify) = mem::replace(entry, IndexEntry::Existing(handle)) else {
            unreachable!()
        };

        self.inserted = true;

        notify.notify_waiters();

        handle
    }
}

impl Drop for RepositoryVacantEntry<'_> {
    fn drop(&mut self) {
        if self.inserted {
            return;
        }

        let mut inner = self.inner.write().unwrap();

        let Some(IndexEntry::Reserved(notify)) = inner.index.remove(&self.store_path) else {
            unreachable!()
        };

        notify.notify_waiters();
    }
}

struct Inner {
    // Registry of the repos
    registry: Registry<Arc<RepositoryHolder>>,
    // Index for looking up repos by their store paths.
    index: HashMap<PathBuf, IndexEntry>,
}

enum IndexEntry {
    Reserved(Arc<Notify>),
    Existing(RepositoryHandle),
}
