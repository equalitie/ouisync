mod id;
#[cfg(test)]
mod tests;
mod worker;

pub(crate) use self::id::LocalId;
pub use self::id::RepositoryId;

use self::worker::{Worker, WorkerHandle};
use crate::{
    access_control::{Access, AccessMode, AccessSecrets, LocalSecret},
    block::{BlockTracker, BLOCK_SIZE},
    branch::Branch,
    crypto::{
        cipher,
        sign::{self, PublicKey},
        Password,
    },
    db,
    debug::DebugPrinter,
    device_id::DeviceId,
    directory::{Directory, EntryType, MissingBlockStrategy},
    error::{Error, Result},
    event::Event,
    file::{File, FileCache},
    index::{BranchData, Index},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    metadata, path,
    progress::Progress,
    scoped_task::{self, ScopedJoinHandle},
    store::{BlockRequestMode, Store},
    sync::broadcast::ThrottleReceiver,
};
use camino::Utf8Path;
use std::{fmt, path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    task,
};
use tracing::{instrument, instrument::Instrument, Span};

const EVENT_CHANNEL_CAPACITY: usize = 256;

pub struct RepositoryDb {
    pool: db::Pool,
    span: Span,
}

impl RepositoryDb {
    pub async fn create(store: impl AsRef<Path>) -> Result<Self> {
        let span = tracing::info_span!("repository", db = %store.as_ref().display());
        let pool = db::create(store).await?;
        Ok(Self { pool, span })
    }

    pub async fn password_to_key(&self, password: Password) -> Result<cipher::SecretKey> {
        let mut tx = self.pool.begin().await?;
        let key = metadata::password_to_key(&mut tx, &password).await?;
        tx.commit().await?;
        Ok(key)
    }
}

pub struct Repository {
    shared: Arc<Shared>,
    worker_handle: WorkerHandle,
    _progress_reporter_handle: ScopedJoinHandle<()>,
}

impl Repository {
    /// Creates a new repository.
    pub async fn create(db: RepositoryDb, device_id: DeviceId, access: Access) -> Result<Self> {
        Self::create_in(db.pool, device_id, access, db.span).await
    }

    /// Creates a new repository in an already opened database.
    pub(crate) async fn create_in(
        pool: db::Pool,
        device_id: DeviceId,
        access: Access,
        span: Span,
    ) -> Result<Self> {
        let mut tx = pool.begin().await?;

        let this_writer_id =
            generate_writer_id(&mut tx, &device_id, access.local_write_key()).await?;

        metadata::initialize_access_secrets(&mut tx, &access).await?;

        tx.commit().await?;

        Self::new(pool, this_writer_id, access.secrets(), span).await
    }

    /// Opens an existing repository.
    ///
    /// # Arguments
    ///
    /// * `local_secret` - A user provided secret to encrypt the access secrets. If not provided,
    ///                    the repository will be opened as a blind replica.
    pub async fn open(
        store: impl AsRef<Path>,
        device_id: DeviceId,
        local_secret: Option<LocalSecret>,
    ) -> Result<Self> {
        Self::open_with_mode(store, device_id, local_secret, AccessMode::Write).await
    }

    /// Opens an existing repository with the provided access mode. This allows to reduce the
    /// access mode the repository was created with.
    pub async fn open_with_mode(
        store: impl AsRef<Path>,
        device_id: DeviceId,
        local_secret: Option<LocalSecret>,
        max_access_mode: AccessMode,
    ) -> Result<Self> {
        let span = tracing::info_span!("repository", db = %store.as_ref().display());
        let pool = db::open(store).await?;

        Self::open_in(pool, device_id, local_secret, max_access_mode, span).await
    }

    /// Opens an existing repository in an already opened database.
    pub(crate) async fn open_in(
        pool: db::Pool,
        device_id: DeviceId,
        local_secret: Option<LocalSecret>,
        // Allows to reduce the access mode (e.g. open in read-only mode even if the local secret
        // would give us write access otherwise). Currently used only in tests.
        max_access_mode: AccessMode,
        span: Span,
    ) -> Result<Self> {
        let mut tx = pool.begin().await?;

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
                generate_writer_id(&mut tx, &device_id, local_key.as_ref()).await?
            }
        } else {
            PublicKey::from(&sign::SecretKey::random())
        };

        tx.commit().await?;

        let access_secrets = access_secrets.with_mode(max_access_mode);

        Self::new(pool, this_writer_id, access_secrets, span).await
    }

    async fn new(
        pool: db::Pool,
        this_writer_id: PublicKey,
        secrets: AccessSecrets,
        span: Span,
    ) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let index = Index::new(pool, *secrets.id(), event_tx.clone());

        let block_request_mode = if secrets.can_read() {
            BlockRequestMode::Lazy
        } else {
            BlockRequestMode::Greedy
        };

        span.in_scope(|| tracing::trace!(access = ?secrets.access_mode()));

        let store = Store {
            index,
            block_tracker: BlockTracker::new(),
            block_request_mode,
            local_id: LocalId::new(),
            span: span.clone(),
        };

        let shared = Arc::new(Shared {
            store,
            this_writer_id,
            secrets,
            file_cache: Arc::new(FileCache::new(event_tx)),
        });

        let local_branch = if shared.secrets.can_write() {
            shared.local_branch().ok()
        } else {
            None
        };

        let (worker, worker_handle) = Worker::new(shared.clone(), local_branch);
        task::spawn(worker.run().instrument(span.clone()));

        let _progress_reporter_handle =
            scoped_task::spawn(report_sync_progress(shared.store.clone()).instrument(span));

        Ok(Self {
            shared,
            worker_handle,
            _progress_reporter_handle,
        })
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

        let mut tx = self.db().begin().await?;
        metadata::set_access(&mut tx, access).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_read_access(
        &self,
        local_read_secret: Option<LocalSecret>,
        secrets: Option<AccessSecrets>,
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

        let mut tx = self.db().begin().await?;

        let local_read_key = if let Some(secret) = local_read_secret {
            Some(metadata::secret_to_key(&mut tx, secret).await?)
        } else {
            None
        };

        metadata::set_read_key(&mut tx, secrets.id(), read_key, local_read_key.as_ref()).await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn set_write_access(
        &self,
        local_write_secret: Option<LocalSecret>,
        secrets: Option<AccessSecrets>,
    ) -> Result<()> {
        let secrets = match secrets.as_ref() {
            Some(secrets) => secrets,
            None => self.secrets(),
        };

        if secrets.id() != self.shared.secrets.id() {
            return Err(Error::PermissionDenied);
        }

        let write_key = match secrets.write_key() {
            Some(write_key) => write_key,
            None => return Err(Error::PermissionDenied),
        };

        let mut tx = self.db().begin().await?;

        let local_write_key = if let Some(secret) = local_write_secret {
            Some(metadata::secret_to_key(&mut tx, secret).await?)
        } else {
            None
        };

        metadata::set_write_key(&mut tx, write_key, local_write_key.as_ref()).await?;

        tx.commit().await?;

        Ok(())
    }

    /// After running this command, the user won't be able to obtain read access to the repository
    /// using their local read secret.
    pub async fn remove_read_key(&self) -> Result<()> {
        let mut tx = self.db().begin().await?;
        metadata::remove_read_key(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// After running this command, the user won't be able to obtain write access to the repository
    /// using their local write secret.
    pub async fn remove_write_key(&self) -> Result<()> {
        let mut tx = self.db().begin().await?;
        metadata::remove_write_key(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub fn secrets(&self) -> &AccessSecrets {
        &self.shared.secrets
    }

    pub fn store(&self) -> &Store {
        &self.shared.store
    }

    pub fn db(&self) -> &db::Pool {
        self.shared.store.db()
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
    #[instrument(skip(path), fields(path = %path.as_ref()), err)]
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
    #[instrument(skip(path), fields(path = %path.as_ref()), err)]
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
    #[instrument(skip(path), fields(path = %path.as_ref()), err(Debug))]
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.cd(path).await
    }

    /// Creates a new file at the given path.
    #[instrument(skip(path), fields(path = %path.as_ref()), err(Debug))]
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let file = self
            .local_branch()?
            .ensure_file_exists(path.as_ref())
            .await?;

        Ok(file)
    }

    /// Creates a new directory at the given path.
    #[instrument(skip(path), fields(path = %path.as_ref()), err(Debug))]
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let dir = self
            .local_branch()?
            .ensure_directory_exists(path.as_ref())
            .await?;

        Ok(dir)
    }

    /// Removes the file or directory (must be empty) and flushes its parent directory.
    #[instrument(skip(path), fields(path = %path.as_ref()), err(Debug))]
    pub async fn remove_entry<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.cd(parent).await?;
        parent.remove_entry(name).await?;

        Ok(())
    }

    /// Removes the file or directory (including its content) and flushes its parent directory.
    #[instrument(skip(path), fields(path = %path.as_ref()), err)]
    pub async fn remove_entry_recursively<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.cd(parent).await?;
        parent.remove_entry_recursively(name).await?;

        Ok(())
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    #[instrument(
        skip(src_dir_path, dst_dir_path),
        fields(src_dir_path = %src_dir_path.as_ref(), dst_dir_path = %dst_dir_path.as_ref()),
        err
    )]
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
                    .open_with(MissingVersionStrategy::Skip, MissingBlockStrategy::Fail)
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
    pub(crate) fn get_branch(&self, remote_id: PublicKey) -> Result<Branch> {
        self.shared
            .inflate(self.shared.store.index.get_branch(remote_id))
    }

    /// Subscribe to event notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.store.index.subscribe()
    }

    /// Gets the access mode this repository is opened in.
    pub fn access_mode(&self) -> AccessMode {
        self.shared.secrets.access_mode()
    }

    /// Gets the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    pub async fn sync_progress(&self) -> Result<Progress> {
        self.shared.store.sync_progress().await
    }

    /// Force the background worker to run one job and wait for it to complete, returning its result.
    ///
    /// It's usually not necessary to call this method because the worker runs automatically in the
    /// background. It might still e.g. be usefull for testing/debugging.
    pub async fn force_work(&self) -> Result<()> {
        self.worker_handle.work().await
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn root(&self) -> Result<JointDirectory> {
        let local_branch = self.local_branch()?;
        let branches = self.shared.load_branches().await?;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = match branch.open_root(MissingBlockStrategy::Fallback).await {
                Ok(dir) => dir,
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => {
                    // Some branch roots may not have been loaded across the network yet. We'll
                    // ignore those.
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        "failed to open root directory in branch {:?}: {:?}",
                        branch.id(),
                        error
                    );
                    return Err(error);
                }
            };

            dirs.push(dir);
        }

        Ok(JointDirectory::new(Some(local_branch), dirs))
    }

    async fn cd<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.root().await?.cd(path).await
    }

    /// Close all db connections held by this repository. After this function returns, any
    /// subsequent operation on this repository that requires to access the db returns an error.
    pub async fn close(&self) -> Result<()> {
        self.worker_handle.shutdown().await;
        self.shared.store.db().close().await?;

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
        self.shared.store.index.debug_print(print).await;
    }

    /// Returns the total number of blocks in this repository. This is useful for diagnostics and
    /// tests.
    pub async fn count_blocks(&self) -> Result<usize> {
        self.shared.store.count_blocks().await
    }

    pub fn local_id(&self) -> LocalId {
        self.shared.store.local_id
    }
}

impl fmt::Debug for Repository {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Repository")
            .field("local_id", &format_args!("{}", self.shared.store.local_id))
            .finish_non_exhaustive()
    }
}

struct Shared {
    store: Store,
    this_writer_id: PublicKey,
    secrets: AccessSecrets,
    // Cache for open files to track multiple instances of the same file.
    file_cache: Arc<FileCache>,
}

impl Shared {
    pub fn local_branch(&self) -> Result<Branch> {
        self.inflate(self.store.index.get_branch(self.this_writer_id))
    }

    pub async fn load_branches(&self) -> Result<Vec<Branch>> {
        self.store
            .index
            .load_branches()
            .await?
            .into_iter()
            .map(|data| self.inflate(data))
            .collect()
    }

    // Create `Branch` wrapping the given `data`.
    fn inflate(&self, data: BranchData) -> Result<Branch> {
        let keys = self.secrets.keys().ok_or(Error::PermissionDenied)?;

        // Only the local branch is writable.
        let keys = if *data.id() == self.this_writer_id {
            keys
        } else {
            keys.read_only()
        };

        Ok(Branch::new(
            self.store.db().clone(),
            data,
            keys,
            self.file_cache.clone(),
        ))
    }
}

async fn generate_writer_id(
    tx: &mut db::Transaction,
    device_id: &DeviceId,
    local_key: Option<&cipher::SecretKey>,
) -> Result<sign::PublicKey> {
    // TODO: we should be storing the SK in the db, not the PK.
    let writer_id = sign::SecretKey::random();
    let writer_id = PublicKey::from(&writer_id);
    metadata::set_writer_id(tx, &writer_id, device_id, local_key).await?;

    Ok(writer_id)
}

async fn report_sync_progress(store: Store) {
    let mut prev_progress = Progress { value: 0, total: 0 };
    let mut event_rx = ThrottleReceiver::new(store.index.subscribe(), Duration::from_secs(1));

    loop {
        match event_rx.recv().await {
            Ok(_) | Err(RecvError::Lagged(_)) => (),
            Err(RecvError::Closed) => break,
        }

        let next_progress = match store.sync_progress().await {
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
