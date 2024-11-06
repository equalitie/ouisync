mod credentials;
mod metadata;
mod monitor;
mod params;
mod vault;
mod worker;

#[cfg(test)]
mod tests;

pub use self::{credentials::Credentials, metadata::Metadata, params::RepositoryParams};

pub(crate) use self::{
    metadata::{data_version, quota},
    monitor::RepositoryMonitor,
    vault::Vault,
};

use crate::{
    access_control::{Access, AccessChange, AccessKeys, AccessMode, AccessSecrets, LocalSecret},
    block_tracker::RequestMode,
    branch::{Branch, BranchShared},
    crypto::{sign::PublicKey, PasswordSalt},
    db::{self, DatabaseId},
    debug::DebugPrinter,
    directory::{Directory, DirectoryFallback, DirectoryLocking, EntryRef, EntryType},
    error::{Error, Result},
    event::{Event, EventSender},
    file::File,
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    path,
    progress::Progress,
    protocol::{RootNodeFilter, StorageSize, BLOCK_SIZE},
    store,
    sync::stream::Throttle,
    version_vector::VersionVector,
};
use camino::Utf8Path;
use deadlock::{BlockingMutex, BlockingRwLock};
use futures_util::{future, TryStreamExt};
use futures_util::{stream, StreamExt};
use metrics::{NoopRecorder, Recorder};
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{borrow::Cow, io, path::Path, pin::pin, sync::Arc, time::SystemTime};
use tokio::{
    fs,
    sync::broadcast::{self, error::RecvError},
    time::Duration,
};
use tracing::instrument::Instrument;

const EVENT_CHANNEL_CAPACITY: usize = 10000;

pub struct Repository {
    shared: Arc<Shared>,
    worker_handle: BlockingMutex<Option<ScopedJoinHandle<()>>>,
    progress_reporter_handle: BlockingMutex<Option<ScopedJoinHandle<()>>>,
}

/// Delete the repository database
pub async fn delete(store: impl AsRef<Path>) -> io::Result<()> {
    // Sqlite database consists of up to three files: main db (always present), WAL and WAL-index.
    // Try to delete all of them even if any of them fail then return the first error (if any)
    future::join_all(["", "-wal", "-shm"].into_iter().map(|suffix| {
        let mut path = store.as_ref().as_os_str().to_owned();
        path.push(suffix);

        async move {
            match fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }))
    .await
    .into_iter()
    .find_map(Result::err)
    .map(Err)
    .unwrap_or(Ok(()))
}

impl Repository {
    /// Creates a new repository.
    pub async fn create(params: &RepositoryParams<impl Recorder>, access: Access) -> Result<Self> {
        let pool = params.create().await?;
        let device_id = params.device_id();
        let monitor = params.monitor();

        let mut tx = pool.begin_write().await?;

        let local_keys = metadata::initialize_access_secrets(&mut tx, &access).await?;
        let writer_id =
            metadata::get_or_generate_writer_id(&mut tx, local_keys.write.as_deref()).await?;
        metadata::set_device_id(&mut tx, &device_id).await?;

        tx.commit().await?;

        let credentials = Credentials {
            secrets: access.secrets(),
            writer_id,
        };

        Self::new(pool, credentials, monitor).init().await
    }

    /// Opens an existing repository.
    pub async fn open(
        params: &RepositoryParams<impl Recorder>,
        local_secret: Option<LocalSecret>,
        access_mode: AccessMode,
    ) -> Result<Self> {
        let pool = params.open().await?;
        let monitor = params.monitor();
        let device_id = params.device_id();

        let mut tx = pool.begin_write().await?;

        let (secrets, local_key) =
            metadata::get_access_secrets(&mut tx, local_secret.as_ref()).await?;

        let secrets = secrets.with_mode(access_mode);

        let writer_id = if metadata::check_device_id(&mut tx, &device_id).await? {
            if secrets.can_write() {
                metadata::get_or_generate_writer_id(&mut tx, local_key.as_deref()).await?
            } else {
                metadata::generate_writer_id()
            }
        } else {
            // Device id changed, likely because the repo database has been transferred to a
            // different device. We need to generate a new writer id.
            //
            // Note we need to do this even when not currently opening the repo in write mode. This
            // is so that when the access mode is subsequently switched to write
            // (with [set_access_mode]) we don't end up using the wrong writer_id.
            let writer_id = metadata::generate_writer_id();

            metadata::set_device_id(&mut tx, &device_id).await?;
            metadata::set_writer_id(&mut tx, &writer_id, local_key.as_deref()).await?;

            writer_id
        };

        tx.commit().await?;

        let credentials = Credentials { secrets, writer_id };

        Self::new(pool, credentials, monitor).init().await
    }

    fn new(pool: db::Pool, credentials: Credentials, monitor: RepositoryMonitor) -> Self {
        Self {
            shared: Arc::new(Shared::new(pool, credentials, monitor)),
            worker_handle: BlockingMutex::new(None),
            progress_reporter_handle: BlockingMutex::new(None),
        }
    }

    async fn init(self) -> Result<Self> {
        let credentials = self.credentials();

        if let Some(keys) = credentials
            .secrets
            .write_secrets()
            .map(|secrets| &secrets.write_keys)
        {
            self.shared
                .vault
                .store()
                .migrate_data(credentials.writer_id, keys)
                .await?;
        }

        {
            let mut conn = self.shared.vault.store().db().acquire().await?;
            if let Some(block_expiration) = metadata::block_expiration::get(&mut conn).await? {
                self.shared
                    .vault
                    .set_block_expiration(Some(block_expiration))?;
            }
        }

        tracing::debug!(
            parent: self.shared.vault.monitor.span(),
            access = ?credentials.secrets.access_mode(),
            writer_id = ?credentials.writer_id,
            "Repository opened"
        );

        *self.worker_handle.lock().unwrap() = Some(spawn_worker(self.shared.clone()));

        *self.progress_reporter_handle.lock().unwrap() = Some(scoped_task::spawn(
            report_sync_progress(self.shared.vault.clone())
                .instrument(self.shared.vault.monitor.span().clone()),
        ));

        Ok(self)
    }

    pub async fn database_id(&self) -> Result<DatabaseId> {
        Ok(metadata::get_or_generate_database_id(self.db()).await?)
    }

    pub async fn requires_local_secret_for_reading(&self) -> Result<bool> {
        let mut conn = self.db().acquire().await?;
        Ok(metadata::requires_local_secret_for_reading(&mut conn).await?)
    }

    pub async fn requires_local_secret_for_writing(&self) -> Result<bool> {
        let mut conn = self.db().acquire().await?;
        Ok(metadata::requires_local_secret_for_writing(&mut conn).await?)
    }

    /// Sets, unsets or changes local secrets for accessing the repository or disables the given
    /// access mode.
    ///
    /// In order to enable or change a given access mode the repository must currently be in at
    /// least that access mode. Disabling an access mode doesn't have this restriction.
    ///
    /// Disabling a given access mode makes the repo impossible to be opened in that mode anymore.
    /// However, when the repo is currently in that mode it still remains in it until the repo is
    /// closed.
    ///
    /// To restore a disabled mode the repo must first be put into that mode using
    /// [Self::set_credentials()] where the `Credentials` must be obtained from `AccessSecrets` with
    /// at least the mode one wants to restore.
    ///
    /// If `read` or `write` is `None` then no change is made to that mode. If both are `None` then
    /// this function is a no-op.
    ///
    /// Disabling the read mode while keeping write mode enabled is allowed but not very useful as
    /// write mode also grants read access.
    pub async fn set_access(
        &self,
        read_change: Option<AccessChange>,
        write_change: Option<AccessChange>,
    ) -> Result<()> {
        let mut tx = self.db().begin_write().await?;

        if let Some(change) = read_change {
            self.set_read_access(&mut tx, change).await?;
        }

        if let Some(change) = write_change {
            self.set_write_access(&mut tx, change).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn set_read_access(
        &self,
        tx: &mut db::WriteTransaction,
        change: AccessChange,
    ) -> Result<()> {
        let local = match &change {
            AccessChange::Enable(Some(local_secret)) => {
                Some(metadata::secret_to_key_and_salt(local_secret))
            }
            AccessChange::Enable(None) => None,
            AccessChange::Disable => {
                metadata::remove_read_key(tx).await?;
                return Ok(());
            }
        };

        let (id, read_key) = {
            let cred = self.shared.credentials.read().unwrap();
            (
                *cred.secrets.id(),
                cred.secrets
                    .read_key()
                    .ok_or(Error::PermissionDenied)?
                    .clone(),
            )
        };

        metadata::set_read_key(tx, &id, &read_key, local.as_deref()).await?;

        Ok(())
    }

    async fn set_write_access(
        &self,
        tx: &mut db::WriteTransaction,
        change: AccessChange,
    ) -> Result<()> {
        let local = match &change {
            AccessChange::Enable(Some(local_secret)) => {
                Some(metadata::secret_to_key_and_salt(local_secret))
            }
            AccessChange::Enable(None) => None,
            AccessChange::Disable => {
                metadata::remove_write_key(tx).await?;
                return Ok(());
            }
        };

        let (write_secrets, writer_id) = {
            let cred = self.shared.credentials.read().unwrap();
            (
                cred.secrets
                    .write_secrets()
                    .ok_or(Error::PermissionDenied)?
                    .clone(),
                cred.writer_id,
            )
        };

        metadata::set_write_key(tx, &write_secrets, local.as_deref()).await?;
        metadata::set_writer_id(tx, &writer_id, local.as_deref().map(|ks| &ks.key)).await?;

        Ok(())
    }

    /// Gets the current credentials of this repository.
    ///
    /// See also [Self::set_credentials()].
    pub fn credentials(&self) -> Credentials {
        self.shared.credentials.read().unwrap().clone()
    }

    pub fn secrets(&self) -> AccessSecrets {
        self.shared.credentials.read().unwrap().secrets.clone()
    }

    /// Gets the current access mode of this repository.
    pub fn access_mode(&self) -> AccessMode {
        self.shared
            .credentials
            .read()
            .unwrap()
            .secrets
            .access_mode()
    }

    /// Switches the repository to the given mode.
    ///
    /// The actual mode the repository gets switched to is the higher of the current access mode
    /// and the mode provided by `local_key` but at most the mode specified in `access_mode`
    /// ("higher" means according to `AccessMode`'s `Ord` impl, that is: Write > Read > Blind).
    pub async fn set_access_mode(
        &self,
        access_mode: AccessMode,
        local_secret: Option<LocalSecret>,
    ) -> Result<()> {
        // Try to use the current secrets but fall back to the secrets stored in the metadata if the
        // current ones are insufficient and the stored ones have higher access mode.
        let old_secrets = {
            let creds = self.shared.credentials.read().unwrap();

            if creds.secrets.access_mode() == access_mode {
                return Ok(());
            }

            creds.secrets.clone()
        };

        let mut tx = self.db().begin_write().await?;

        let (secrets, local_key) = if old_secrets.access_mode() >= access_mode {
            (old_secrets, None)
        } else {
            let (new_secrets, local_key) =
                metadata::get_access_secrets(&mut tx, local_secret.as_ref()).await?;

            if new_secrets.access_mode() > old_secrets.access_mode() {
                (new_secrets, local_key)
            } else {
                (old_secrets, None)
            }
        };

        let secrets = secrets.with_mode(access_mode);

        let writer_id = if secrets.can_write() {
            metadata::get_or_generate_writer_id(&mut tx, local_key.as_deref()).await?
        } else {
            metadata::generate_writer_id()
        };

        tx.commit().await?;

        // Data migration can only be applied in write mode so apply any pending ones if we just
        // switched to write mode.
        if let Some(write_keys) = secrets.write_secrets().map(|secrets| &secrets.write_keys) {
            self.shared
                .vault
                .store()
                .migrate_data(writer_id, write_keys)
                .await?;
        }

        self.update_credentials(Credentials { secrets, writer_id });

        Ok(())
    }

    /// Overrides the current credentials of this repository.
    ///
    /// This is useful for moving/renaming the repo database or to restore access which has been
    /// either disabled or it's local secret lost.
    ///
    /// # Move/rename the repo db
    ///
    /// 1. Obtain the current credentials with [Self::credentials()] and keep them locally.
    /// 2. Close the repo.
    /// 3. Rename the repo database files(s).
    /// 4. Open the repo from its new location in blind mode.
    /// 5. Restore the credentials from step 1 with [Self::set_credentials()].
    ///
    /// # Restore access
    ///
    /// 1. Get the `AccessSecrets` the repository was originally created from (e.g., by extracting
    ///    them from the original `ShareToken`).
    /// 2. Construct `Credentials` using this access secrets and a random writer id.
    /// 3. Restore the credentials with [Self::set_credentials()].
    /// 4. Enable/change the access with [Self::set_access()].
    pub async fn set_credentials(&self, credentials: Credentials) -> Result<()> {
        // Check the credentials are actually for this repository
        let expected_id = {
            let mut conn = self.db().acquire().await?;
            metadata::get_repository_id(&mut conn).await?
        };

        if credentials.secrets.id() != &expected_id {
            return Err(Error::PermissionDenied);
        }

        if let Some(write_secrets) = credentials.secrets.write_secrets() {
            self.shared
                .vault
                .store()
                .migrate_data(credentials.writer_id, &write_secrets.write_keys)
                .await?;
        }

        self.update_credentials(credentials);

        Ok(())
    }

    pub async fn unlock_secrets(&self, local_secret: LocalSecret) -> Result<AccessSecrets> {
        let mut tx = self.db().begin_write().await?;
        Ok(metadata::get_access_secrets(&mut tx, Some(&local_secret))
            .await?
            .0)
    }

    /// Get accessor for repository metadata. The metadata are arbitrary key-value entries that are
    /// stored inside the repository but not synced to other replicas.
    pub fn metadata(&self) -> Metadata {
        self.shared.vault.metadata()
    }

    /// Set the storage quota in bytes. Use `None` to disable quota. Default is `None`.
    pub async fn set_quota(&self, quota: Option<StorageSize>) -> Result<()> {
        self.shared.vault.set_quota(quota).await
    }

    /// Get the storage quota in bytes or `None` if no quota is set.
    pub async fn quota(&self) -> Result<Option<StorageSize>> {
        self.shared.vault.quota().await
    }

    /// Set the duration after which blocks start to expire (are deleted) when not used. Use `None`
    /// to disable expiration. Default is `None`.
    pub async fn set_block_expiration(&self, block_expiration: Option<Duration>) -> Result<()> {
        let mut tx = self.db().begin_write().await?;
        metadata::block_expiration::set(&mut tx, block_expiration).await?;
        tx.commit().await?;

        self.shared.vault.set_block_expiration(block_expiration)
    }

    /// Get the block expiration duration. `None` means block expiration is not set.
    pub fn block_expiration(&self) -> Option<Duration> {
        self.shared.vault.block_expiration()
    }

    /// Get the time when the last block expired or `None` if there are still some unexpired blocks.
    /// If block expiration is not enabled, always return `None`.
    pub fn last_block_expiration_time(&self) -> Option<SystemTime> {
        self.shared.vault.last_block_expiration_time()
    }

    /// Get the total size of the data stored in this repository.
    pub async fn size(&self) -> Result<StorageSize> {
        self.shared.vault.size().await
    }

    pub fn handle(&self) -> RepositoryHandle {
        RepositoryHandle {
            vault: self.shared.vault.clone(),
        }
    }

    pub async fn get_read_password_salt(&self) -> Result<PasswordSalt> {
        let mut tx = self.db().begin_write().await?;
        Ok(metadata::get_password_salt(&mut tx, metadata::KeyType::Read).await?)
    }

    pub async fn get_write_password_salt(&self) -> Result<PasswordSalt> {
        let mut tx = self.db().begin_write().await?;
        Ok(metadata::get_password_salt(&mut tx, metadata::KeyType::Write).await?)
    }

    /// Get the state monitor node of this repository.
    pub fn monitor(&self) -> &StateMonitor {
        self.shared.vault.monitor.node()
    }

    /// Export the repository to the given file.
    ///
    /// The repository is currently exported as read-only with no password. In the future other
    /// modes might be added.
    pub async fn export(&self, dst: &Path) -> Result<()> {
        /// RAII to delete the exported repo in case the process fails or is interupted to avoid
        /// exporting the repo with a wrong access mode.
        struct Cleanup<'a> {
            path: &'a Path,
            armed: bool,
        }

        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                if !self.armed {
                    return;
                }

                if let Err(error) = std::fs::remove_file(self.path) {
                    tracing::error!(
                        path = ?self.path,
                        ?error,
                        "failed to delete partially exported repository",
                    );
                }
            }
        }

        let mut cleanup = Cleanup {
            path: dst,
            armed: true,
        };

        // Export the repo to `dst`
        self.shared.vault.store().export(dst).await?;

        // Open it and strip write access and read password (if any).
        let pool = db::open(dst).await?;
        let credentials = self.credentials().with_mode(AccessMode::Read);
        let access_mode = credentials.secrets.access_mode();
        let monitor = RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder);
        let repo = Self::new(pool, credentials, monitor);

        match access_mode {
            AccessMode::Blind => {
                repo.set_access(Some(AccessChange::Disable), Some(AccessChange::Disable))
                    .await?
            }
            AccessMode::Read => {
                repo.set_access(
                    Some(AccessChange::Enable(None)),
                    Some(AccessChange::Disable),
                )
                .await?
            }
            AccessMode::Write => unreachable!(),
        }

        repo.close().await?;

        cleanup.armed = false;

        Ok(())
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `JointEntryType`, otherwise returns `EntryNotFound`.
    pub async fn lookup_type<P: AsRef<Utf8Path>>(&self, path: P) -> Result<EntryType> {
        match path::decompose(path.as_ref()) {
            Some((parent, name)) => {
                let parent = self.open_directory(parent).await?;
                Ok(parent.lookup_unique(name)?.entry_type())
            }
            None => Ok(EntryType::Directory),
        }
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        self.cd(parent)
            .await?
            .lookup_unique(name)?
            .file()?
            .open()
            .await
    }

    /// Open a specific version of the file at the given path.
    pub async fn open_file_version<P: AsRef<Utf8Path>>(
        &self,
        path: P,
        branch_id: &PublicKey,
    ) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        self.cd(parent)
            .await?
            .lookup_version(name, branch_id)?
            .open()
            .await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.cd(path).await
    }

    /// Creates a new file at the given path.
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let file = self
            .local_branch()?
            .ensure_file_exists(path.as_ref())
            .await?;

        Ok(file)
    }

    /// Creates a new directory at the given path.
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let dir = self
            .local_branch()?
            .ensure_directory_exists(path.as_ref())
            .await?;

        Ok(dir)
    }

    /// Removes the file or directory (must be empty) and flushes its parent directory.
    pub async fn remove_entry<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.cd(parent).await?;
        parent.remove_entry(name).await?;

        Ok(())
    }

    /// Removes the file or directory (including its content) and flushes its parent directory.
    pub async fn remove_entry_recursively<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.cd(parent).await?;
        parent.remove_entry_recursively(name).await?;

        Ok(())
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src_dir_path: S,
        src_name: &str,
        dst_dir_path: D,
        dst_name: &str,
    ) -> Result<()> {
        let local_branch = self.local_branch()?;
        let src_joint_dir = self.cd(src_dir_path).await?;

        // If the src is in a remote branch, need to merge it into the local one first:
        let (mut src_dir, src_name, src_type) = match src_joint_dir.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();

                let mut file = entry.open().await?;
                file.fork(local_branch.clone()).await?;

                (file.parent().await?, Cow::Owned(src_name), EntryType::File)
            }
            JointEntryRef::Directory(entry) => {
                let mut dir_to_move = entry
                    .open_with(MissingVersionStrategy::Skip, DirectoryFallback::Disabled)
                    .await?;
                let dir_to_move = dir_to_move.merge().await?;

                let src_dir = dir_to_move
                    .parent()
                    .await?
                    .ok_or(Error::OperationNotSupported /* can't move root */)?;

                (src_dir, Cow::Borrowed(src_name), EntryType::Directory)
            }
        };

        let src_entry = src_dir.lookup(&src_name)?.clone_data();

        let mut dst_joint_dir = self.cd(&dst_dir_path).await?;
        let dst_dir = dst_joint_dir
            .local_version_mut()
            .ok_or(Error::PermissionDenied)?;

        let dst_old_entry = dst_dir.lookup(dst_name);

        // Emulating the behaviour of the libc's `rename` function
        // (https://www.man7.org/linux/man-pages/man2/rename.2.html)
        let dst_old_vv = match (src_type, dst_old_entry) {
            (EntryType::File | EntryType::Directory, Ok(EntryRef::Tombstone(old_entry))) => {
                old_entry.version_vector().clone()
            }
            (EntryType::File | EntryType::Directory, Err(Error::EntryNotFound)) => {
                VersionVector::new()
            }
            (EntryType::File | EntryType::Directory, Err(error)) => return Err(error),
            (EntryType::File, Ok(EntryRef::File(old_entry))) => old_entry.version_vector().clone(),
            (EntryType::Directory, Ok(EntryRef::Directory(old_entry))) => {
                if old_entry
                    .open(DirectoryFallback::Disabled)
                    .await?
                    .entries()
                    .all(|entry| entry.is_tombstone())
                {
                    old_entry.version_vector().clone()
                } else {
                    return Err(Error::DirectoryNotEmpty);
                }
            }
            (EntryType::File, Ok(EntryRef::Directory(_))) => return Err(Error::EntryIsDirectory),
            (EntryType::Directory, Ok(EntryRef::File(_))) => return Err(Error::EntryIsFile),
        };

        let dst_vv = dst_old_vv
            .merged(src_entry.version_vector())
            .incremented(*local_branch.id());

        src_dir
            .move_entry(&src_name, src_entry, dst_dir, dst_name, dst_vv)
            .await?;

        Ok(())
    }

    /// Returns the local branch or `Error::PermissionDenied` if this repo doesn't have at least
    /// read access.
    pub fn local_branch(&self) -> Result<Branch> {
        self.shared.local_branch()
    }

    /// Returns the branch corresponding to the given id or `Error::PermissionDenied. if this repo
    /// doesn't have at least read access.
    #[cfg(test)]
    pub fn get_branch(&self, id: PublicKey) -> Result<Branch> {
        self.shared.get_branch(id)
    }

    pub async fn load_branches(&self) -> Result<Vec<Branch>> {
        self.shared.load_branches().await
    }

    /// Returns version vector of the given branch. Works in all access moded.
    pub async fn get_branch_version_vector(&self, writer_id: &PublicKey) -> Result<VersionVector> {
        Ok(self
            .shared
            .vault
            .store()
            .acquire_read()
            .await?
            .load_latest_approved_root_node(writer_id, RootNodeFilter::Any)
            .await?
            .proof
            .into_version_vector())
    }

    /// Returns the version vector calculated by merging the version vectors of all branches.
    pub async fn get_merged_version_vector(&self) -> Result<VersionVector> {
        Ok(self
            .shared
            .vault
            .store()
            .acquire_read()
            .await?
            .load_latest_approved_root_nodes()
            .try_fold(VersionVector::default(), |mut merged, node| {
                merged.merge(&node.proof.version_vector);
                future::ready(Ok(merged))
            })
            .await?)
    }

    /// Subscribe to event notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.vault.event_tx.subscribe()
    }

    /// Gets the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    pub async fn sync_progress(&self) -> Result<Progress> {
        Ok(self.shared.vault.store().sync_progress().await?)
    }

    /// Check integrity of the stored data.
    // TODO: Return more detailed info about any integrity violation.
    pub async fn check_integrity(&self) -> Result<bool> {
        Ok(self.shared.vault.store().check_integrity().await?)
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn root(&self) -> Result<JointDirectory> {
        let local_branch = self.local_branch()?;
        let branches = self.shared.load_branches().await?;

        // If we are writer and the local branch doesn't exist yet in the db we include it anyway.
        // This fixes a race condition when the local branch doesn't exist yet at the moment we
        // load the branches but is subsequently created by merging a remote branch and the remote
        // branch is then pruned.
        let branches = if local_branch.keys().write().is_some()
            && branches
                .iter()
                .all(|branch| branch.id() != local_branch.id())
        {
            let mut branches = branches;
            branches.push(local_branch.clone());
            branches
        } else {
            branches
        };

        let mut dirs = Vec::new();

        for branch in branches {
            let dir = match branch
                .open_root(DirectoryLocking::Enabled, DirectoryFallback::Enabled)
                .await
            {
                Ok(dir) => dir,
                Err(error @ Error::Store(store::Error::BranchNotFound)) => {
                    tracing::trace!(
                        branch_id = ?branch.id(),
                        ?error,
                        "Failed to open root directory"
                    );
                    // Either this is the local branch which doesn't exist yet in the store or a
                    // remote branch which has been pruned in the meantime. This is safe to ignore.
                    continue;
                }
                Err(error @ Error::Store(store::Error::BlockNotFound)) => {
                    tracing::trace!(
                        branch_id = ?branch.id(),
                        ?error,
                        "Failed to open root directory"
                    );
                    // Some branch root blocks may not have been loaded across the network yet.
                    // This is safe to ignore.
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        branch_id = ?branch.id(),
                        ?error,
                        "Failed to open root directory"
                    );
                    return Err(error);
                }
            };

            dirs.push(dir);
        }

        Ok(JointDirectory::new(Some(local_branch), dirs))
    }

    pub async fn cd<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.root().await?.cd(path).await
    }

    /// Close all db connections held by this repository. After this function returns, any
    /// subsequent operation on this repository that requires to access the db returns an error.
    pub async fn close(&self) -> Result<()> {
        // Abort and *await* the tasks to make sure that the state they are holding is definitely
        // dropped before we return from this function.
        for task in [&self.worker_handle, &self.progress_reporter_handle] {
            let task = task.lock().unwrap().take();
            if let Some(task) = task {
                task.abort();
                task.await.ok();
            }
        }

        self.shared.vault.store().close().await?;

        Ok(())
    }

    pub async fn debug_print_root(&self) {
        self.debug_print(DebugPrinter::new()).await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"Repository");

        let branches = match self.shared.load_branches().await {
            Ok(branches) => branches,
            Err(error) => {
                print.display(&format_args!("failed to load branches: {:?}", error));
                return;
            }
        };

        let writer_id = self.shared.credentials.read().unwrap().writer_id;

        for branch in branches {
            let print = print.indent();
            let local = if branch.id() == &writer_id {
                " (local)"
            } else {
                ""
            };
            print.display(&format_args!(
                "Branch ID: {:?}{}, root block ID:{:?}",
                branch.id(),
                local,
                branch.root_block_id().await
            ));
            let print = print.indent();
            print.display(&format_args!(
                "/, vv: {:?}",
                branch.version_vector().await.unwrap_or_default()
            ));
            branch.debug_print(print.indent()).await;
        }

        print.display(&"Index");
        let print = print.indent();
        self.shared.vault.debug_print(print).await;
    }

    /// Returns the total number of blocks in this repository. This is useful for diagnostics and
    /// tests.
    pub async fn count_blocks(&self) -> Result<u64> {
        Ok(self.shared.vault.store().count_blocks().await?)
    }

    fn db(&self) -> &db::Pool {
        self.shared.vault.store().db()
    }

    fn update_credentials(&self, credentials: Credentials) {
        tracing::debug!(
            parent: self.shared.vault.monitor.span(),
            access = ?credentials.secrets.access_mode(),
            writer_id = ?credentials.writer_id,
            "Repository access mode changed"
        );

        self.shared
            .vault
            .block_tracker
            .set_request_mode(request_mode(&credentials.secrets));

        *self.shared.credentials.write().unwrap() = credentials;
        *self.worker_handle.lock().unwrap() = Some(spawn_worker(self.shared.clone()));
    }
}

pub struct RepositoryHandle {
    pub(crate) vault: Vault,
}

struct Shared {
    vault: Vault,
    credentials: BlockingRwLock<Credentials>,
    branch_shared: BranchShared,
}

impl Shared {
    fn new(pool: db::Pool, credentials: Credentials, monitor: RepositoryMonitor) -> Self {
        let event_tx = EventSender::new(EVENT_CHANNEL_CAPACITY);
        let vault = Vault::new(*credentials.secrets.id(), event_tx, pool, monitor);

        vault
            .block_tracker
            .set_request_mode(request_mode(&credentials.secrets));

        Self {
            vault,
            credentials: BlockingRwLock::new(credentials),
            branch_shared: BranchShared::new(),
        }
    }

    pub fn local_branch(&self) -> Result<Branch> {
        let credentials = self.credentials.read().unwrap();

        Ok(self.make_branch(
            credentials.writer_id,
            credentials.secrets.keys().ok_or(Error::PermissionDenied)?,
        ))
    }

    pub fn get_branch(&self, id: PublicKey) -> Result<Branch> {
        let credentials = self.credentials.read().unwrap();
        let keys = credentials.secrets.keys().ok_or(Error::PermissionDenied)?;

        // Only the local branch is writable.
        let keys = if id == credentials.writer_id {
            keys
        } else {
            keys.read_only()
        };

        Ok(self.make_branch(id, keys))
    }

    fn make_branch(&self, id: PublicKey, keys: AccessKeys) -> Branch {
        Branch::new(
            id,
            self.vault.store().clone(),
            keys,
            self.branch_shared.clone(),
            self.vault.event_tx.clone(),
        )
    }

    pub async fn load_branches(&self) -> Result<Vec<Branch>> {
        self.vault
            .store()
            .acquire_read()
            .await?
            .load_latest_approved_root_nodes()
            .err_into()
            .and_then(|root_node| future::ready(self.get_branch(root_node.proof.writer_id)))
            .try_collect()
            .await
    }
}

fn spawn_worker(shared: Arc<Shared>) -> ScopedJoinHandle<()> {
    let span = shared.vault.monitor.span().clone();
    scoped_task::spawn(worker::run(shared).instrument(span))
}

async fn report_sync_progress(vault: Vault) {
    let mut prev_progress = Progress { value: 0, total: 0 };

    let events = stream::unfold(vault.event_tx.subscribe(), |mut rx| async move {
        match rx.recv().await {
            Ok(_) | Err(RecvError::Lagged(_)) => Some(((), rx)),
            Err(RecvError::Closed) => None,
        }
    });
    let events = Throttle::new(events, Duration::from_secs(1));
    let mut events = pin!(events);

    while events.next().await.is_some() {
        let next_progress = match vault.store().sync_progress().await {
            Ok(progress) => progress,
            Err(error) => {
                tracing::error!("Failed to retrieve sync progress: {:?}", error);
                continue;
            }
        };

        if next_progress != prev_progress {
            prev_progress = next_progress;
            tracing::debug!(
                "Sync progress: {} bytes ({:.1})",
                prev_progress * BLOCK_SIZE as u64,
                prev_progress.percent()
            );
        }
    }
}

fn request_mode(secrets: &AccessSecrets) -> RequestMode {
    if secrets.can_read() {
        RequestMode::Lazy
    } else {
        RequestMode::Greedy
    }
}
