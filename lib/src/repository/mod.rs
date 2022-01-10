mod id;
mod merger;
#[cfg(test)]
mod tests;

pub use self::id::RepositoryId;

use self::merger::Merger;
use crate::{
    access_control::{AccessMode, AccessSecrets, MasterSecret},
    block,
    branch::Branch,
    crypto::{
        cipher,
        sign::{self, PublicKey},
    },
    db,
    debug_printer::DebugPrinter,
    directory::{Directory, EntryType},
    error::{Error, Result},
    file::File,
    index::{self, BranchData, Index},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    metadata, path,
    replica_id::ReplicaId,
    scoped_task::{self, ScopedJoinHandle},
    store,
};
use camino::Utf8Path;
use futures_util::future;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use tokio::sync::Mutex;

pub struct Repository {
    shared: Arc<Shared>,
    _merge_handle: Option<ScopedJoinHandle<()>>,
}

impl Repository {
    /// Creates a new repository.
    pub async fn create(
        store: &db::Store,
        this_replica_id: ReplicaId,
        master_secret: MasterSecret,
        access_secrets: AccessSecrets,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = db::open_or_create(store).await?;
        Self::create_in(
            pool,
            this_replica_id,
            master_secret,
            access_secrets,
            enable_merger,
        )
        .await
    }

    /// Creates a new repository in an already opened database.
    pub(crate) async fn create_in(
        pool: db::Pool,
        this_replica_id: ReplicaId,
        master_secret: MasterSecret,
        access_secrets: AccessSecrets,
        enable_merger: bool,
    ) -> Result<Self> {
        let mut tx = pool.begin().await?;
        init_db(&mut tx).await?;

        let master_key = metadata::secret_to_key(master_secret, &mut tx).await?;
        let this_writer_id = generate_writer_id(&this_replica_id, &master_key, &mut tx).await?;
        metadata::set_access_secrets(&access_secrets, &master_key, &mut tx).await?;

        tx.commit().await?;

        let index = Index::load(pool, *access_secrets.id()).await?;

        if access_secrets.can_write() {
            index.create_branch(this_writer_id).await?;
        }

        Self::new(index, this_writer_id, access_secrets, enable_merger).await
    }

    /// Opens an existing repository.
    ///
    /// # Arguments
    ///
    /// * `master_secret` - A user provided secret to encrypt the access secrets. If not provided,
    ///                     the repository will be opened as a blind replica.
    pub async fn open(
        store: &db::Store,
        this_replica_id: ReplicaId,
        master_secret: Option<MasterSecret>,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = db::open(store).await?;
        Self::open_in(
            pool,
            this_replica_id,
            master_secret,
            AccessMode::Write,
            enable_merger,
        )
        .await
    }

    /// Opens an existing repository in an already opened database.
    pub(crate) async fn open_in(
        pool: db::Pool,
        this_replica_id: ReplicaId,
        master_secret: Option<MasterSecret>,
        // Allows to reduce the access mode (e.g. open in read-only mode even if the master secret
        // would give us write access otherwise). Currently used only in tests.
        max_access_mode: AccessMode,
        enable_merger: bool,
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

            if metadata::check_replica_id(&this_replica_id, &mut conn).await? {
                metadata::get_writer_id(master_key, &mut conn).await?
            } else {
                // Replica id changed. Must generate new writer id.
                generate_writer_id(&this_replica_id, master_key, &mut conn).await?
            }
        } else {
            PublicKey::from(&sign::SecretKey::random())
        };

        drop(conn);

        let access_secrets = access_secrets.with_mode(max_access_mode);
        let index = Index::load(pool, *access_secrets.id()).await?;

        Self::new(index, this_writer_id, access_secrets, enable_merger).await
    }

    async fn new(
        index: Index,
        this_writer_id: PublicKey,
        secrets: AccessSecrets,
        enable_merger: bool,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            index,
            this_writer_id,
            secrets,
            branches: Mutex::new(HashMap::new()),
        });

        let local_branch = if enable_merger {
            match shared.local_branch().await {
                Ok(branch) => Some(branch),
                Err(Error::PermissionDenied) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };

        let merge_handle = local_branch.map(|local_branch| {
            scoped_task::spawn(Merger::new(shared.clone(), local_branch).run())
        });

        Ok(Self {
            shared,
            _merge_handle: merge_handle,
        })
    }

    pub fn secrets(&self) -> &AccessSecrets {
        &self.shared.secrets
    }

    pub fn index(&self) -> &Index {
        &self.shared.index
    }

    /// Looks up an entry by its path. The path must be relative to the repository root.
    /// If the entry exists, returns its `JointEntryType`, otherwise returns `EntryNotFound`.
    pub async fn lookup_type<P: AsRef<Utf8Path>>(&self, path: P) -> Result<EntryType> {
        match path::decompose(path.as_ref()) {
            Some((parent, name)) => {
                let parent = self.open_directory(parent).await?;
                let parent = parent.read().await;
                Ok(parent.lookup_unique(name)?.entry_type())
            }
            None => Ok(EntryType::Directory),
        }
    }

    /// Opens a file at the given path (relative to the repository root)
    pub async fn open_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::EntryIsDirectory)?;

        self.open_directory(parent)
            .await?
            .read()
            .await
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

        self.open_directory(parent)
            .await?
            .read()
            .await
            .lookup_version(name, branch_id)?
            .open()
            .await
    }

    /// Opens a directory at the given path (relative to the repository root)
    pub async fn open_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<JointDirectory> {
        self.joint_root().await?.cd(path).await
    }

    /// Creates a new file at the given path.
    pub async fn create_file<P: AsRef<Utf8Path>>(&self, path: P) -> Result<File> {
        let local_branch = self.local_branch().await?;
        local_branch.ensure_file_exists(path.as_ref()).await
    }

    /// Creates a new directory at the given path.
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        let local_branch = self.local_branch().await?;
        local_branch.ensure_directory_exists(path.as_ref()).await
    }

    /// Removes the file or directory (must be empty) and flushes its parent directory.
    pub async fn remove_entry<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.open_directory(parent).await?;

        // TODO: fork the parent dir

        parent.remove_entry(name).await?;
        parent.flush().await
    }

    /// Removes the file or directory (including its content) and flushes its parent directory.
    pub async fn remove_entry_recursively<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.open_directory(parent).await?;

        // TODO: fork the parent dir

        parent.remove_entry_recursively(name).await?;
        parent.flush().await
    }

    /// Moves (renames) an entry from the source path to the destination path.
    /// If both source and destination refer to the same entry, this is a no-op.
    /// Returns the parent directories of both `src` and `dst`.
    pub async fn move_entry<S: AsRef<Utf8Path>, D: AsRef<Utf8Path>>(
        &self,
        src_dir_path: S,
        src_name: &str,
        dst_dir_path: D,
        dst_name: &str,
    ) -> Result<()> {
        use std::borrow::Cow;

        let local_branch = self.local_branch().await?;

        let src_joint_dir = self.open_directory(src_dir_path).await?;
        let src_joint_dir_r = src_joint_dir.read().await;

        let (src_dir, src_name, src_author) = match src_joint_dir_r.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();
                let src_author = *entry.author();

                let mut file = entry.open().await?;
                file.fork(&local_branch).await?;

                (file.parent(), Cow::Owned(src_name), src_author)
            }
            JointEntryRef::Directory(entry) => {
                let dir_to_move = entry
                    .open(MissingVersionStrategy::Skip)
                    .await?
                    .merge()
                    .await?;

                let src_dir = dir_to_move
                    .parent()
                    .await
                    .ok_or(Error::OperationNotSupported /* can't move root */)?;

                (src_dir, Cow::Borrowed(src_name), *local_branch.id())
            }
        };

        // Get the entry here before we release the lock to the directory. This way the entry shall
        // contain version vector that we want to delete and if someone updates the entry between
        // now and when the entry is actually to be removed, the concurrent updates shall remain.
        let src_entry = src_dir
            .read()
            .await
            .lookup_version(&src_name, &src_author)?
            .clone_data();

        drop(src_joint_dir_r);
        drop(src_joint_dir);

        let dst_joint_dir = self.open_directory(&dst_dir_path).await?;
        let dst_joint_reader = dst_joint_dir.read().await;

        let dst_vv = match dst_joint_reader.lookup_unique(dst_name) {
            Err(Error::EntryNotFound) => {
                // Even if there is no regular entry, there still may be tombstones and so the
                // destination version vector must be "happened after" those.
                dst_joint_reader
                    .merge_version_vectors(dst_name)
                    .incremented(*local_branch.id())
            }
            Ok(_) => return Err(Error::EntryExists),
            Err(e) => return Err(e),
        };

        drop(dst_joint_reader);
        drop(dst_joint_dir);

        let dst_dir = self.create_directory(dst_dir_path).await?;

        src_dir
            .move_entry(
                &src_name,
                &src_author,
                src_entry,
                &dst_dir,
                dst_name,
                dst_vv,
            )
            .await?;

        src_dir.flush(None).await?;

        if src_dir != dst_dir {
            dst_dir.flush(None).await?;
        }

        Ok(())
    }

    /// Returns the local branch if it exists.
    pub async fn local_branch(&self) -> Result<Branch> {
        self.shared.local_branch().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub(crate) fn subscribe(&self) -> async_broadcast::Receiver<PublicKey> {
        self.shared.index.subscribe()
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn joint_root(&self) -> Result<JointDirectory> {
        let local_branch = self.local_branch().await.ok();
        let branches = self.shared.branches().await?;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = if Some(branch.id()) == local_branch.as_ref().map(Branch::id) {
                branch.open_or_create_root().await.map_err(|error| {
                    log::error!(
                        "failed to open root directory on the local branch: {:?}",
                        error
                    );
                    error
                })?
            } else {
                match branch.open_root().await {
                    Ok(dir) => dir,
                    Err(Error::EntryNotFound | Error::BlockNotFound(_)) => {
                        // Some branch roots may not have been loaded across the network yet. We'll
                        // ignore those.
                        continue;
                    }
                    Err(error) => {
                        log::error!(
                            "failed to open root directory on a remote branch {:?}: {:?}",
                            branch.id(),
                            error
                        );
                        return Err(error);
                    }
                }
            };

            dirs.push(dir);
        }

        Ok(JointDirectory::new(local_branch, dirs))
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"Repository");
        let branches = self.shared.branches.lock().await;
        for (writer_id, branch) in &*branches {
            let print = print.indent();
            let local = if *writer_id == self.shared.this_writer_id {
                " (local)"
            } else {
                ""
            };
            print.display(&format_args!(
                "Branch ID: {:?}{}, root block ID:{:?}",
                writer_id,
                local,
                branch.root_block_id().await
            ));
            let print = print.indent();
            print.display(&format_args!(
                "/, vv: {:?}",
                branch.data().root().await.versions
            ));
            branch.debug_print(print.indent()).await;
        }
    }

    // Create remote branch in this repository and returns it.
    // FOR TESTS ONLY!
    #[cfg(test)]
    pub(crate) async fn create_remote_branch(&self, remote_id: PublicKey) -> Result<Branch> {
        use crate::index::RootNode;

        let remote_node = RootNode::load_latest_or_create(
            &mut self.index().pool.acquire().await.unwrap(),
            &remote_id,
        )
        .await?;

        self.index()
            .update_remote_branch(remote_id, remote_node)
            .await?;

        self.shared.branch(&remote_id).await
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        self.shared.index.close()
    }
}

/// Creates and initializes the repository database.
#[cfg(test)]
pub(crate) async fn create_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open_or_create(store).await?;
    init_db(&mut *pool.acquire().await?).await?;

    Ok(pool)
}

async fn init_db(conn: &mut db::Connection) -> Result<()> {
    use sqlx::Connection;

    let mut tx = conn.begin().await?;

    block::init(&mut tx).await?;
    index::init(&mut tx).await?;
    store::init(&mut tx).await?;
    metadata::init(&mut tx).await?;

    tx.commit().await?;

    Ok(())
}

struct Shared {
    index: Index,
    this_writer_id: PublicKey,
    secrets: AccessSecrets,
    // Cache for `Branch` instances to make them persistent over the lifetime of the program.
    branches: Mutex<HashMap<PublicKey, Branch>>,
}

impl Shared {
    pub async fn local_branch(&self) -> Result<Branch> {
        self.inflate(
            self.index
                .branches()
                .await
                .get(&self.this_writer_id)
                .ok_or(Error::PermissionDenied)?,
        )
        .await
    }

    pub async fn branch(&self, id: &PublicKey) -> Result<Branch> {
        self.inflate(
            self.index
                .branches()
                .await
                .get(id)
                .ok_or(Error::EntryNotFound)?,
        )
        .await
    }

    pub async fn branches(&self) -> Result<Vec<Branch>> {
        future::try_join_all(
            self.index
                .branches()
                .await
                .values()
                .map(|data| self.inflate(data)),
        )
        .await
    }

    // Create `Branch` wrapping the given `data`, reusing a previously cached one if it exists,
    // and putting it into the cache it it does not.
    async fn inflate(&self, data: &Arc<BranchData>) -> Result<Branch> {
        match self.branches.lock().await.entry(*data.id()) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let keys = self.secrets.keys().ok_or(Error::PermissionDenied)?;

                // Only the local branch is writable.
                let keys = if *data.id() == self.this_writer_id {
                    keys
                } else {
                    keys.read_only()
                };

                let branch = Branch::new(self.index.pool.clone(), data.clone(), keys);
                entry.insert(branch.clone());
                Ok(branch)
            }
        }
    }
}

async fn generate_writer_id(
    this_replica_id: &ReplicaId,
    master_key: &cipher::SecretKey,
    conn: &mut db::Connection,
) -> Result<sign::PublicKey> {
    // TODO: we should be storing the SK in the db, not the PK.
    let writer_id = sign::SecretKey::random();
    let writer_id = PublicKey::from(&writer_id);
    metadata::set_writer_id(&writer_id, this_replica_id, master_key, conn).await?;

    Ok(writer_id)
}
