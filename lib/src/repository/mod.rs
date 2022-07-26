mod id;
#[cfg(test)]
mod tests;
mod utils;
mod worker;

pub(crate) use self::id::LocalId;
pub use self::id::RepositoryId;

use self::worker::{Worker, WorkerHandle};
use crate::{
    access_control::{AccessMode, AccessSecrets, MasterSecret},
    blob::BlobCache,
    block::{BlockTracker, BLOCK_SIZE},
    branch::Branch,
    crypto::{
        cipher,
        sign::{self, PublicKey},
    },
    db,
    debug::DebugPrinter,
    device_id::DeviceId,
    directory::{Directory, EntryType},
    error::{Error, Result},
    event::BranchChangedReceiver,
    file::File,
    index::{BranchData, Index, Proof, EMPTY_INNER_HASH},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    metadata, path,
    progress::Progress,
    state_monitor::{MonitoredValue, StateMonitor},
    store::Store,
    sync::broadcast::ThrottleReceiver,
    version_vector::VersionVector,
};
use camino::Utf8Path;
use std::{fmt, sync::Arc, time::Duration};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    task,
};
use tracing::{instrument, instrument::Instrument};

const EVENT_CHANNEL_CAPACITY: usize = 256;

pub struct Repository {
    shared: Arc<Shared>,
    worker_handle: WorkerHandle,
}

impl Repository {
    /// Creates a new repository.
    #[instrument(level = "info", skip_all, fields(store), err)]
    pub async fn create(
        store: &db::Store,
        device_id: DeviceId,
        master_secret: MasterSecret,
        access_secrets: AccessSecrets,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        let pool = db::create(store).await?;
        Self::create_in(
            pool,
            device_id,
            master_secret,
            access_secrets,
            enable_merger,
            parent_monitor,
        )
        .await
    }

    /// Creates a new repository in an already opened database.
    pub(crate) async fn create_in(
        pool: db::Pool,
        device_id: DeviceId,
        master_secret: MasterSecret,
        access_secrets: AccessSecrets,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        let mut tx = pool.begin().await?;

        let master_key = metadata::secret_to_key(master_secret, &mut tx).await?;
        let this_writer_id = generate_writer_id(&device_id, &master_key, &mut tx).await?;
        metadata::set_access_secrets(&access_secrets, &master_key, &mut tx).await?;

        tx.commit().await?;

        Self::new(
            pool,
            this_writer_id,
            access_secrets,
            enable_merger,
            parent_monitor,
        )
        .await
    }

    /// Opens an existing repository.
    ///
    /// # Arguments
    ///
    /// * `master_secret` - A user provided secret to encrypt the access secrets. If not provided,
    ///                     the repository will be opened as a blind replica.
    #[instrument(level = "info", skip_all, fields(store), err)]
    pub async fn open(
        store: &db::Store,
        device_id: DeviceId,
        master_secret: Option<MasterSecret>,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        Self::open_with_mode(
            store,
            device_id,
            master_secret,
            AccessMode::Write,
            enable_merger,
            parent_monitor,
        )
        .await
    }

    /// Opens an existing repository with the provided access mode. This allows to reduce the
    /// access mode the repository was created with.
    #[instrument(level = "info", skip_all, fields(store, max_access_mode), err)]
    pub async fn open_with_mode(
        store: &db::Store,
        device_id: DeviceId,
        master_secret: Option<MasterSecret>,
        max_access_mode: AccessMode,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        let pool = db::open(store).await?;
        Self::open_in(
            pool,
            device_id,
            master_secret,
            max_access_mode,
            enable_merger,
            parent_monitor,
        )
        .await
    }

    /// Opens an existing repository in an already opened database.
    pub(crate) async fn open_in(
        pool: db::Pool,
        device_id: DeviceId,
        master_secret: Option<MasterSecret>,
        // Allows to reduce the access mode (e.g. open in read-only mode even if the master secret
        // would give us write access otherwise). Currently used only in tests.
        max_access_mode: AccessMode,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let master_key = if let Some(master_secret) = master_secret {
            Some(metadata::secret_to_key(master_secret, &mut conn).await?)
        } else {
            None
        };

        let access_secrets = if let Some(master_key) = &master_key {
            metadata::get_access_secrets(master_key, &mut conn).await?
        } else {
            let id = metadata::get_repository_id(&mut conn).await?;
            AccessSecrets::Blind { id }
        };

        // If we are writer, load the writer id from the db, otherwise use a dummy random one.
        let this_writer_id = if access_secrets.can_write() {
            // This unwrap is ok because the fact that we have write access means the access
            // secrets must have been successfully decrypted and thus we must have a valid
            // master secret.
            let master_key = master_key.as_ref().unwrap();

            if metadata::check_device_id(&device_id, &mut conn).await? {
                metadata::get_writer_id(master_key, &mut conn).await?
            } else {
                // Replica id changed. Must generate new writer id.
                generate_writer_id(&device_id, master_key, &mut conn).await?
            }
        } else {
            PublicKey::from(&sign::SecretKey::random())
        };

        drop(conn);

        let access_secrets = access_secrets.with_mode(max_access_mode);

        Self::new(
            pool,
            this_writer_id,
            access_secrets,
            enable_merger,
            parent_monitor,
        )
        .await
    }

    async fn new(
        pool: db::Pool,
        this_writer_id: PublicKey,
        secrets: AccessSecrets,
        enable_merger: bool,
        parent_monitor: &StateMonitor,
    ) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let index = Index::load(pool, *secrets.id(), event_tx.clone()).await?;

        // Lazy block downloading requires at least read access because it needs to be able to
        // traverse the repository in order to enumarate reachable blocks.
        let block_tracker = if secrets.can_read() {
            BlockTracker::lazy()
        } else {
            BlockTracker::greedy()
        };

        let monitor = parent_monitor.make_child(hex::encode(index.repository_id().as_ref()));

        let monitored = Arc::new(MonitoredValues {
            index_requests_inflight: monitor.make_value("index_requests_inflight".into(), 0),
            block_requests_inflight: monitor.make_value("block_requests_inflight".into(), 0),
            total_request_cummulative: monitor.make_value("total_request_cummulative".into(), 0),
        });

        let local_id = LocalId::new();
        tracing::info!(%local_id);

        let store = Store {
            monitored: Arc::downgrade(&monitored),
            index,
            block_tracker,
            local_id,
        };

        let shared = Arc::new(Shared {
            store,
            this_writer_id,
            secrets,
            blob_cache: Arc::new(BlobCache::new(event_tx)),
            _monitored: monitored,
        });

        let local_branch = if shared.secrets.can_write() && enable_merger {
            match shared.get_or_create_local_branch().await {
                Ok(branch) => Some(branch),
                Err(Error::PermissionDenied) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };

        let (worker, worker_handle) = Worker::new(shared.clone(), local_branch);
        let worker_span =
            tracing::debug_span!(parent: None, "worker", local_id = %shared.store.local_id);
        task::spawn(worker.run().instrument(worker_span));

        task::spawn(report_sync_progress(shared.store.clone()));

        Ok(Self {
            shared,
            worker_handle,
        })
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
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        let mut conn = self.db().acquire().await?;

        self.cd(&mut conn, parent)
            .await?
            .lookup_unique(name)?
            .file()?
            .open(&mut conn)
            .await
    }

    /// Open a specific version of the file at the given path.
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn open_file_version<P: AsRef<Utf8Path>>(
        &self,
        path: P,
        branch_id: &PublicKey,
    ) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        let mut conn = self.db().acquire().await?;

        self.cd(&mut conn, parent)
            .await?
            .lookup_version(name, branch_id)?
            .open(&mut conn)
            .await
    }

    /// Opens a directory at the given path (relative to the repository root)
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        let mut conn = self.db().acquire().await?;
        self.cd(&mut conn, path).await
    }

    /// Creates a new file at the given path.
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let local_branch = self.get_or_create_local_branch().await?;
        let mut conn = self.db().acquire().await?;
        local_branch
            .ensure_file_exists(&mut conn, path.as_ref())
            .await
    }

    /// Creates a new directory at the given path.
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let local_branch = self.get_or_create_local_branch().await?;
        let mut conn = self.db().acquire().await?;
        local_branch
            .ensure_directory_exists(&mut conn, path.as_ref())
            .await
    }

    /// Removes the file or directory (must be empty) and flushes its parent directory.
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn remove_entry<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;

        self.get_or_create_local_branch().await?;

        let mut conn = self.db().acquire().await?;
        let mut parent = self.cd(&mut conn, parent).await?;
        parent.remove_entry(&mut conn, name).await
    }

    /// Removes the file or directory (including its content) and flushes its parent directory.
    #[instrument(level = "info", skip(path), fields(path = %path.as_ref()), err)]
    pub async fn remove_entry_recursively<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;

        let mut conn = self.db().acquire().await?;
        let mut parent = self.cd(&mut conn, parent).await?;
        parent.remove_entry_recursively(&mut conn, name).await
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    #[instrument(
        level = "info",
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

        let local_branch = self.get_or_create_local_branch().await?;
        let mut conn = self.db().acquire().await?;

        let src_joint_dir = self.cd(&mut conn, src_dir_path).await?;

        let (mut src_dir, src_name) = match src_joint_dir.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();

                let mut file = entry.open(&mut conn).await?;
                file.fork(&mut conn, local_branch.clone()).await?;

                (file.parent(&mut conn).await?, Cow::Owned(src_name))
            }
            JointEntryRef::Directory(entry) => {
                let mut dir_to_move = entry.open(&mut conn, MissingVersionStrategy::Skip).await?;

                // `merge` takes `Pool`, not `Connection` (to avoid holding the connection for too
                // long) and so to avoid deadlock we need to temporarily release `conn`...
                drop(conn);

                let dir_to_move = dir_to_move.merge(self.db()).await?;

                // ...and acquire it back again once `merge` is done.
                conn = self.db().acquire().await?;

                let src_dir = dir_to_move
                    .parent(&mut conn)
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

        let dst_joint_dir = self.cd(&mut conn, &dst_dir_path).await?;

        let dst_vv = match dst_joint_dir.lookup_unique(dst_name) {
            Err(Error::EntryNotFound) => {
                // Even if there is no regular entry, there still may be tombstones and so the
                // destination version vector must be "happened after" those.
                dst_joint_dir
                    .merge_entry_version_vectors(dst_name)
                    .incremented(*local_branch.id())
            }
            Ok(_) => return Err(Error::EntryExists),
            Err(e) => return Err(e),
        };

        drop(dst_joint_dir);

        let mut dst_dir = local_branch
            .ensure_directory_exists(&mut conn, dst_dir_path.as_ref())
            .await?;
        src_dir
            .move_entry(
                &mut conn,
                &src_name,
                src_entry,
                &mut dst_dir,
                dst_name,
                dst_vv,
            )
            .await
    }

    /// Returns the local branch if it exists.
    pub fn local_branch(&self) -> Option<Branch> {
        self.shared.local_branch()
    }

    /// Returns the local branch if it exists or create it otherwise. The repository must have
    /// write access, otherwise returns PermissionDenied error.
    pub async fn get_or_create_local_branch(&self) -> Result<Branch> {
        self.shared.get_or_create_local_branch().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> BranchChangedReceiver {
        BranchChangedReceiver::new(self.shared.store.index.subscribe())
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

    /// Force the garbage collection to run and wait for it to complete, returning its result.
    ///
    /// It's usually not necessary to call this method because the gc runs automatically in the
    /// background.
    pub async fn force_garbage_collection(&self) -> Result<()> {
        self.worker_handle.collect().await
    }

    /// Force the merge to run and wait for it to complete, returning its result.
    ///
    /// It's usually not necessary to call this method because the merger runs automatically in the
    /// background.
    pub async fn force_merge(&self) -> Result<()> {
        self.worker_handle.merge().await
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn root(&self, conn: &mut db::Connection) -> Result<JointDirectory> {
        // If we have only blind access we can cut this short. Also this check is necessary to
        // distinguish *empty* repository with blind access from one with read access.
        if !self.shared.secrets.can_read() {
            return Err(Error::PermissionDenied);
        }

        let local_branch = self.local_branch();
        let branches = self.shared.collect_branches()?;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = match branch.open_root(conn).await {
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

        Ok(JointDirectory::new(local_branch, dirs))
    }

    async fn cd<P: AsRef<Utf8Path>>(
        &self,
        conn: &mut db::Connection,
        path: P,
    ) -> Result<JointDirectory> {
        self.root(conn).await?.cd(conn, path).await
    }

    /// Close all db connections held by this repository. After this function returns, any
    /// subsequent operation on this repository that requires to access the db returns an error.
    pub async fn close(&self) {
        self.shared.store.db().close().await;
    }

    pub async fn debug_print_root(&self) {
        self.debug_print(DebugPrinter::new()).await
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"Repository");

        let mut conn = match self.db().acquire().await {
            Ok(conn) => conn,
            Err(error) => {
                print.display(&format_args!(
                    "failed to acquire db connection: {:?}",
                    error
                ));
                return;
            }
        };

        let branches = match self.shared.collect_branches() {
            Ok(branches) => branches,
            Err(error) => {
                print.display(&format_args!("failed to collect branches: {:?}", error));
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
                branch.root_block_id(&mut conn).await
            ));
            let print = print.indent();
            print.display(&format_args!(
                "/, vv: {:?}",
                branch.version_vector(&mut conn).await.unwrap_or_default()
            ));
            branch.debug_print(&mut conn, print.indent()).await;
        }

        drop(conn);

        print.display(&"Index");
        let print = print.indent();
        self.shared.store.index.debug_print(print).await;
    }

    /// Returns the total number of blocks in this repository. This is useful for diagnostics and
    /// tests.
    pub async fn count_blocks(&self) -> Result<usize> {
        self.shared.store.count_blocks().await
    }

    // Create remote branch in this repository and returns it.
    // FOR TESTS ONLY!
    #[cfg(test)]
    pub(crate) async fn create_remote_branch(&self, remote_id: PublicKey) -> Result<Branch> {
        let write_keys = self.secrets().write_keys().ok_or(Error::PermissionDenied)?;
        let proof = Proof::new(
            remote_id,
            VersionVector::new(),
            *EMPTY_INNER_HASH,
            write_keys,
        );
        let branch = self.shared.store.index.create_branch(proof).await?;

        self.shared.inflate(branch)
    }
}

impl fmt::Debug for Repository {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Repository")
            .field("local_id", &format_args!("{}", self.shared.store.local_id))
            .finish_non_exhaustive()
    }
}

pub(crate) struct MonitoredValues {
    // This indicates how many requests for index nodes are currently in flight.  It is used by the
    // UI to indicate that the index is being synchronized.
    pub index_requests_inflight: MonitoredValue<usize>,
    pub block_requests_inflight: MonitoredValue<usize>,
    pub total_request_cummulative: MonitoredValue<usize>,
}

struct Shared {
    store: Store,
    this_writer_id: PublicKey,
    secrets: AccessSecrets,
    // Cache for open blobs to track multiple instances of the same blob.
    blob_cache: Arc<BlobCache>,
    _monitored: Arc<MonitoredValues>,
}

impl Shared {
    pub fn local_branch(&self) -> Option<Branch> {
        self.store
            .index
            .get_branch(&self.this_writer_id)
            .and_then(|data| self.inflate(data).ok())
    }

    pub async fn get_or_create_local_branch(&self) -> Result<Branch> {
        let data = if let Some(data) = self.store.index.get_branch(&self.this_writer_id) {
            data
        } else if let Some(write_keys) = self.secrets.write_keys() {
            let proof = Proof::new(
                self.this_writer_id,
                VersionVector::new(),
                *EMPTY_INNER_HASH,
                write_keys,
            );
            self.store.index.create_branch(proof).await?
        } else {
            return Err(Error::PermissionDenied);
        };

        self.inflate(data)
    }

    pub fn collect_branches(&self) -> Result<Vec<Branch>> {
        self.store
            .index
            .collect_branches()
            .into_iter()
            .map(|data| self.inflate(data))
            .collect()
    }

    // Create `Branch` wrapping the given `data`, reusing a previously cached one if it exists,
    // and putting it into the cache if it does not.
    fn inflate(&self, data: Arc<BranchData>) -> Result<Branch> {
        let keys = self.secrets.keys().ok_or(Error::PermissionDenied)?;

        // Only the local branch is writable.
        let keys = if *data.id() == self.this_writer_id {
            keys
        } else {
            keys.read_only()
        };

        Ok(Branch::new(data, keys, self.blob_cache.clone()))
    }
}

async fn generate_writer_id(
    device_id: &DeviceId,
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<sign::PublicKey> {
    // TODO: we should be storing the SK in the db, not the PK.
    let writer_id = sign::SecretKey::random();
    let writer_id = PublicKey::from(&writer_id);
    metadata::set_writer_id(&writer_id, device_id, master_key, conn).await?;

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
                repo = %store.local_id,
                "sync progress: {} bytes ({:.1})",
                prev_progress * BLOCK_SIZE as u64,
                prev_progress.percent()
            );
        }
    }
}
