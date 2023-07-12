mod id;
mod metadata;
mod monitor;
mod params;
mod reopen_token;
#[cfg(test)]
mod tests;
mod vault;
mod worker;

pub use self::{
    id::RepositoryId, metadata::Metadata, params::RepositoryParams, reopen_token::ReopenToken,
};

pub(crate) use self::{
    id::LocalId,
    metadata::quota,
    monitor::RepositoryMonitor,
    vault::{BlockRequestMode, Vault},
};

use crate::{
    access_control::{Access, AccessMode, AccessSecrets, LocalSecret},
    block::{BlockTracker, BLOCK_SIZE},
    branch::{Branch, BranchShared},
    crypto::{
        cipher,
        sign::{self, PublicKey},
    },
    db::{self, DatabaseId},
    deadlock::BlockingMutex,
    debug::DebugPrinter,
    device_id::DeviceId,
    directory::{Directory, DirectoryFallback, DirectoryLocking, EntryType},
    error::{Error, Result},
    event::{Event, EventSender},
    file::File,
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    path,
    progress::Progress,
    state_monitor::StateMonitor,
    storage_size::StorageSize,
    store::{self, Store},
    sync::broadcast::ThrottleReceiver,
};
use camino::Utf8Path;
use futures_util::{future, TryStreamExt};
use scoped_task::ScopedJoinHandle;
use std::{io, path::Path, sync::Arc};
use tokio::{
    fs,
    sync::broadcast::{self, error::RecvError},
    time::Duration,
};
use tracing::instrument::Instrument;

const EVENT_CHANNEL_CAPACITY: usize = 256;

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
    pub async fn create(params: &RepositoryParams, access: Access) -> Result<Self> {
        let pool = params.create().await?;
        let device_id = params.device_id();
        let monitor = params.monitor();

        let mut tx = pool.begin_write().await?;
        let local_keys = metadata::initialize_access_secrets(&mut tx, &access).await?;
        let this_writer_id =
            generate_and_store_writer_id(&mut tx, &device_id, local_keys.write.as_deref()).await?;

        tx.commit().await?;

        Self::new(pool, this_writer_id, access.secrets(), monitor).await
    }

    /// Opens an existing repository.
    ///
    /// # Arguments
    ///
    /// * `local_secret` - A user provided secret to encrypt the access secrets. If not provided,
    ///                    the repository will be opened as a blind replica.
    pub async fn open(
        params: &RepositoryParams,
        local_secret: Option<LocalSecret>,
        max_access_mode: AccessMode,
    ) -> Result<Self> {
        let pool = params.open().await?;
        let device_id = params.device_id();
        let monitor = params.monitor();

        let mut tx = pool.begin_write().await?;
        let local_key = if let Some(local_secret) = local_secret {
            let key = match local_secret {
                LocalSecret::Password(pwd) => metadata::password_to_key(&mut tx, &pwd).await?,
                LocalSecret::SecretKey(key) => key,
            };
            Some(key)
        } else {
            None
        };

        let access_secrets = metadata::get_access_secrets(&mut tx, local_key.as_ref()).await?;

        // If we are writer, load the writer id from the db, otherwise use a dummy random one.
        let this_writer_id = if access_secrets.can_write() {
            if metadata::check_device_id(&mut tx, &device_id).await? {
                metadata::get_writer_id(&mut tx, local_key.as_ref()).await?
            } else {
                // Replica id changed. Must generate new writer id.
                generate_and_store_writer_id(&mut tx, &device_id, local_key.as_ref()).await?
            }
        } else {
            PublicKey::from(&sign::SecretKey::random())
        };

        tx.commit().await?;

        let access_secrets = access_secrets.with_mode(max_access_mode);

        Self::new(pool, this_writer_id, access_secrets, monitor).await
    }

    /// Reopens an existing repository using a reopen token (see [`Self::reopen_token`]).
    pub async fn reopen(params: &RepositoryParams, token: ReopenToken) -> Result<Self> {
        let pool = params.open().await?;
        let monitor = params.monitor();

        Self::new(pool, token.writer_id, token.secrets, monitor).await
    }

    async fn new(
        pool: db::Pool,
        this_writer_id: PublicKey,
        secrets: AccessSecrets,
        monitor: RepositoryMonitor,
    ) -> Result<Self> {
        let event_tx = EventSender::new(EVENT_CHANNEL_CAPACITY);
        let store = Store::new(pool);

        let block_request_mode = if secrets.can_read() {
            BlockRequestMode::Lazy
        } else {
            BlockRequestMode::Greedy
        };

        let vault = Vault {
            repository_id: *secrets.id(),
            store,
            event_tx,
            block_tracker: BlockTracker::new(),
            block_request_mode,
            local_id: LocalId::new(),
            monitor: Arc::new(monitor),
        };

        tracing::debug!(
            parent: vault.monitor.span(),
            access = ?secrets.access_mode(),
            writer_id = ?this_writer_id
        );

        let shared = Arc::new(Shared {
            vault,
            this_writer_id,
            secrets,
            branch_shared: BranchShared::new(),
        });

        let local_branch = if shared.secrets.can_write() {
            shared.local_branch().ok()
        } else {
            None
        };

        let worker_handle = scoped_task::spawn(
            worker::run(shared.clone(), local_branch)
                .instrument(shared.vault.monitor.span().clone()),
        );
        let worker_handle = BlockingMutex::new(Some(worker_handle));

        let progress_reporter_handle = scoped_task::spawn(
            report_sync_progress(shared.vault.clone())
                .instrument(shared.vault.monitor.span().clone()),
        );
        let progress_reporter_handle = BlockingMutex::new(Some(progress_reporter_handle));

        Ok(Self {
            shared,
            worker_handle,
            progress_reporter_handle,
        })
    }

    pub async fn database_id(&self) -> Result<DatabaseId> {
        metadata::get_or_generate_database_id(self.db()).await
    }

    pub async fn requires_local_password_for_reading(&self) -> Result<bool> {
        let mut conn = self.db().acquire().await?;
        metadata::requires_local_password_for_reading(&mut conn).await
    }

    pub async fn requires_local_password_for_writing(&self) -> Result<bool> {
        let mut conn = self.db().acquire().await?;
        metadata::requires_local_password_for_writing(&mut conn).await
    }

    pub async fn set_access(&self, access: &Access) -> Result<()> {
        if access.id() != self.shared.secrets.id() {
            return Err(Error::PermissionDenied);
        }

        let mut tx = self.db().begin_write().await?;
        metadata::set_access(&mut tx, access).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_read_access(
        &self,
        local_read_secret: Option<&LocalSecret>,
        secrets: Option<&AccessSecrets>,
    ) -> Result<()> {
        let mut tx = self.db().begin_write().await?;
        self.set_read_access_in(&mut tx, local_read_secret, secrets)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_read_access_in(
        &self,
        tx: &mut db::WriteTransaction,
        local_read_secret: Option<&LocalSecret>,
        secrets: Option<&AccessSecrets>,
    ) -> Result<()> {
        let secrets = match secrets.as_ref() {
            Some(secrets) => secrets,
            None => self.secrets(),
        };

        if secrets.id() != self.shared.secrets.id() {
            return Err(Error::PermissionDenied);
        }

        let read_key = match secrets.read_key() {
            Some(read_key) => read_key,
            None => return Err(Error::PermissionDenied),
        };

        let local_read_key = if let Some(local_secret) = local_read_secret {
            Some(metadata::secret_to_key(tx, local_secret).await?)
        } else {
            None
        };

        metadata::set_read_key(
            tx,
            secrets.id(),
            read_key,
            // Option<Cow<SecretKey>> -> Option<&SecretKey>
            local_read_key.as_ref().map(|k| k.as_ref()),
        )
        .await?;

        Ok(())
    }

    // Making this function public instead of the `set_read_access_in` and `set_write_access_in`
    // separately to make the setting both (read and write) accesses ACID without having to make
    // the db::WriteTransaction public.
    pub async fn set_read_and_write_access(
        &self,
        local_old_secret: Option<&LocalSecret>,
        local_new_secret: Option<&LocalSecret>,
        secrets: Option<&AccessSecrets>,
    ) -> Result<()> {
        let mut tx = self.db().begin_write().await?;
        self.set_read_access_in(&mut tx, local_new_secret, secrets)
            .await?;
        self.set_write_access_in(&mut tx, local_old_secret, local_new_secret, secrets)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_write_access_in(
        &self,
        tx: &mut db::WriteTransaction,
        local_old_write_secret: Option<&LocalSecret>,
        local_new_write_secret: Option<&LocalSecret>,
        secrets: Option<&AccessSecrets>,
    ) -> Result<()> {
        let secrets = match secrets.as_ref() {
            Some(secrets) => secrets,
            None => self.secrets(),
        };

        if secrets.id() != self.shared.secrets.id() {
            return Err(Error::PermissionDenied);
        }

        let write_secrets = match secrets.write_secrets() {
            Some(write_key) => write_key,
            None => return Err(Error::PermissionDenied),
        };

        let local_new_write_key = if let Some(secret) = local_new_write_secret {
            Some(metadata::secret_to_key(tx, secret).await?)
        } else {
            None
        };

        let writer_id = if let Some(local_old_write_secret) = local_old_write_secret {
            let local_old_write_key = metadata::secret_to_key(tx, local_old_write_secret).await?;
            metadata::get_writer_id(tx, Some(&local_old_write_key)).await?
        } else {
            generate_writer_id()
        };

        metadata::set_write_key(
            tx,
            write_secrets,
            // Option<Cow<SecretKey>> -> Option<&SecretKey>
            local_new_write_key.as_ref().map(|k| k.as_ref()),
        )
        .await?;

        metadata::set_writer_id(
            tx,
            &writer_id,
            // Option<Cow<SecretKey>> -> Option<&SecretKey>
            local_new_write_key.as_ref().map(|k| k.as_ref()),
        )
        .await?;

        Ok(())
    }

    /// After running this command, the user won't be able to obtain read access to the repository
    /// using their local read secret.
    pub async fn remove_read_key(&self) -> Result<()> {
        let mut tx = self.db().begin_write().await?;
        metadata::remove_read_key(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// After running this command, the user won't be able to obtain write access to the repository
    /// using their local write secret.
    pub async fn remove_write_key(&self) -> Result<()> {
        let mut tx = self.db().begin_write().await?;
        metadata::remove_write_key(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub fn secrets(&self) -> &AccessSecrets {
        &self.shared.secrets
    }

    pub async fn unlock_secrets(&self, local_secret: LocalSecret) -> Result<AccessSecrets> {
        // TODO: We don't really want to write here, but the `password_to_key` function requires a
        // transaction. Consider changing it so that it only needs a connection (writing the seed would
        // be done only during the db initialization explicitly).
        let mut tx = self.db().begin_write().await?;

        let local_key = match local_secret {
            LocalSecret::Password(pwd) => metadata::password_to_key(&mut tx, &pwd).await?,
            LocalSecret::SecretKey(key) => key,
        };

        metadata::get_access_secrets(&mut tx, Some(&local_key)).await
    }

    /// Obtain the reopen token for this repository. The token can then be used to reopen this
    /// repository (using [`Self::reopen()`]) in the same access mode without having to provide the
    /// local secret.
    pub fn reopen_token(&self) -> ReopenToken {
        ReopenToken {
            secrets: self.secrets().clone(),
            writer_id: self.shared.this_writer_id,
        }
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

    /// Get the total size of the data stored in this repository.
    pub async fn size(&self) -> Result<StorageSize> {
        self.shared.vault.size().await
    }

    pub fn handle(&self) -> RepositoryHandle {
        RepositoryHandle {
            vault: self.shared.vault.clone(),
        }
    }

    /// Get the state monitor node of this repository.
    pub fn monitor(&self) -> &StateMonitor {
        self.shared.vault.monitor.node()
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
        use std::borrow::Cow;

        let local_branch = self.local_branch()?;
        let src_joint_dir = self.cd(src_dir_path).await?;

        let (mut src_dir, src_name) = match src_joint_dir.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();

                let mut file = entry.open().await?;
                file.fork(local_branch.clone()).await?;

                (file.parent().await?, Cow::Owned(src_name))
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

                (src_dir, Cow::Borrowed(src_name))
            }
        };

        drop(src_joint_dir);

        // Get the entry here before we release the lock to the directory. This way the entry shall
        // contain version vector that we want to delete and if someone updates the entry between
        // now and when the entry is actually to be removed, the concurrent updates shall remain.
        let src_entry = src_dir.lookup(&src_name)?.clone_data();

        let dst_joint_dir = self.cd(&dst_dir_path).await?;

        let dst_vv = match dst_joint_dir.lookup_unique(dst_name) {
            Err(Error::EntryNotFound) => {
                // Even if there is no regular entry, there still may be tombstones and so the
                // destination version vector must be "happened after" those.
                dst_joint_dir
                    .merge_entry_version_vectors(dst_name)
                    .merged(src_entry.version_vector())
                    .incremented(*local_branch.id())
            }
            Ok(_) => return Err(Error::EntryExists),
            Err(e) => return Err(e),
        };

        drop(dst_joint_dir);

        let mut dst_dir = local_branch
            .ensure_directory_exists(dst_dir_path.as_ref())
            .await?;
        src_dir
            .move_entry(&src_name, src_entry, &mut dst_dir, dst_name, dst_vv)
            .await?;

        Ok(())
    }

    /// Returns the local branch or `Error::PermissionDenied` if this repo doesn't have at least
    /// read access.
    pub fn local_branch(&self) -> Result<Branch> {
        self.shared.local_branch()
    }

    // Returns the branch corresponding to the given id.
    // Currently test only.
    #[cfg(test)]
    pub(crate) fn get_branch(&self, id: PublicKey) -> Result<Branch> {
        self.shared.get_branch(id)
    }

    /// Subscribe to event notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.vault.event_tx.subscribe()
    }

    /// Gets the access mode this repository is opened in.
    pub fn access_mode(&self) -> AccessMode {
        self.shared.secrets.access_mode()
    }

    /// Gets the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    pub async fn sync_progress(&self) -> Result<Progress> {
        self.shared.vault.sync_progress().await
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
                Err(error @ Error::Store(store::Error::BranchNotFound))
                    if branch.id() == local_branch.id() =>
                {
                    tracing::trace!(
                        branch_id = ?branch.id(),
                        ?error,
                        "failed to open root directory on local branch"
                    );
                    // Local branch doesn't exist yet in the store. This is safe to ignore.
                    continue;
                }
                Err(error @ Error::Store(store::Error::BlockNotFound)) => {
                    tracing::trace!(
                        branch_id = ?branch.id(),
                        ?error,
                        "failed to open root directory"
                    );
                    // Some branch root blocks may not have been loaded across the network yet.
                    // This is safe to ignore.
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        branch_id = ?branch.id(),
                        ?error,
                        "failed to open root directory"
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

        for branch in branches {
            let print = print.indent();
            let local = if branch.id() == &self.shared.this_writer_id {
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
    pub async fn count_blocks(&self) -> Result<usize> {
        self.shared.vault.count_blocks().await
    }

    fn db(&self) -> &db::Pool {
        self.shared.vault.store().raw()
    }
}

pub struct RepositoryHandle {
    pub(crate) vault: Vault,
}

struct Shared {
    vault: Vault,
    this_writer_id: PublicKey,
    secrets: AccessSecrets,
    branch_shared: BranchShared,
}

impl Shared {
    pub fn local_branch(&self) -> Result<Branch> {
        self.get_branch(self.this_writer_id)
    }

    pub fn get_branch(&self, id: PublicKey) -> Result<Branch> {
        let keys = self.secrets.keys().ok_or(Error::PermissionDenied)?;

        // Only the local branch is writable.
        let keys = if id == self.this_writer_id {
            keys
        } else {
            keys.read_only()
        };

        Ok(Branch::new(
            id,
            self.vault.store().clone(),
            keys,
            self.branch_shared.clone(),
            self.vault.event_tx.clone(),
        ))
    }

    pub async fn load_branches(&self) -> Result<Vec<Branch>> {
        self.vault
            .store()
            .acquire_read()
            .await?
            .load_root_nodes()
            .err_into()
            .and_then(|root_node| future::ready(self.get_branch(root_node.proof.writer_id)))
            .try_collect()
            .await
    }
}

// TODO: Writer IDs are currently practically just UUIDs with no real security (any replica with a
// write access may impersonate any other replica).
fn generate_writer_id() -> sign::PublicKey {
    let writer_id = sign::SecretKey::random();
    PublicKey::from(&writer_id)
}

async fn generate_and_store_writer_id(
    tx: &mut db::WriteTransaction,
    device_id: &DeviceId,
    local_key: Option<&cipher::SecretKey>,
) -> Result<sign::PublicKey> {
    let writer_id = generate_writer_id();
    metadata::set_writer_id(tx, &writer_id, local_key).await?;
    metadata::set_device_id(tx, device_id).await?;
    Ok(writer_id)
}

async fn report_sync_progress(vault: Vault) {
    let mut prev_progress = Progress { value: 0, total: 0 };
    let mut event_rx = ThrottleReceiver::new(vault.event_tx.subscribe(), Duration::from_secs(1));

    loop {
        match event_rx.recv().await {
            Ok(_) | Err(RecvError::Lagged(_)) => (),
            Err(RecvError::Closed) => break,
        }

        let next_progress = match vault.sync_progress().await {
            Ok(progress) => progress,
            Err(error) => {
                tracing::error!("failed to retrieve sync progress: {:?}", error);
                continue;
            }
        };

        if next_progress != prev_progress {
            prev_progress = next_progress;
            tracing::debug!(
                "sync progress: {} bytes ({:.1})",
                prev_progress * BLOCK_SIZE as u64,
                prev_progress.percent()
            );
        }
    }
}
