use crate::{
    block,
    branch::Branch,
    crypto::Cryptor,
    db,
    debug_printer::DebugPrinter,
    directory::{Directory, EntryType},
    error::{Error, Result},
    file::File,
    index::{self, BranchData, Index, Subscription},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    path,
    scoped_task::{self, ScopedJoinHandle},
    share_token::ShareToken,
    store, ReplicaId,
};
use camino::Utf8Path;
use futures_util::{future, stream::FuturesUnordered, StreamExt};
use log::Level;
use sqlx::Row;
use std::{collections::HashMap, iter, str::FromStr, sync::Arc};
use tokio::{select, sync::Mutex};

/// Size of repository ID in bytes.
pub const REPOSITORY_ID_SIZE: usize = 16;

define_random_id! {
    /// Unique id of a repository.
    pub struct RepositoryId([u8; REPOSITORY_ID_SIZE]);
}

derive_sqlx_traits_for_u8_array_wrapper!(RepositoryId);

impl FromStr for RepositoryId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; REPOSITORY_ID_SIZE];
        hex::decode_to_slice(s, &mut bytes)?;

        Ok(Self(bytes))
    }
}

pub struct Repository {
    shared: Arc<Shared>,
    _merge_handle: Option<ScopedJoinHandle<()>>,
}

impl Repository {
    /// Opens an existing repository or creates a new one if it doesn't exists yet.
    pub async fn open(
        store: &db::Store,
        this_replica_id: ReplicaId,
        cryptor: Cryptor,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = open_db(store).await?;
        let index = Index::load(pool, this_replica_id).await?;

        let shared = Arc::new(Shared {
            index,
            cryptor,
            branches: Mutex::new(HashMap::new()),
        });

        let merge_handle = if enable_merger {
            Some(scoped_task::spawn(Merger::new(shared.clone()).run()))
        } else {
            None
        };

        Ok(Self {
            shared,
            _merge_handle: merge_handle,
        })
    }

    /// Get the id of this repository or `None` if no id was assigned yet. A repository gets an id
    /// the first time it is either shared ([`Self::share`]) or a accepts a share
    /// ([`Self::accept`]).
    pub async fn get_id(&self) -> Result<Option<RepositoryId>> {
        get_id(self.db_pool()).await
    }

    /// Shares this repository with other replicas. The returned share token should be transmitted
    /// to other replica who may choose to accept it by calling [`Self::accept`]. After the token
    /// is accepted and the replicas connect, the repositories will start syncing.
    ///
    /// If neither `share` nor `accept` was called on this repository before, a random repository
    /// id is created and assigned to this repository. Subsequent calls to `share` will then use
    /// the same repository id.
    pub async fn share(&self) -> Result<ShareToken> {
        let mut tx = self.db_pool().begin().await?;
        let id = if let Some(id) = get_id(&mut tx).await? {
            id
        } else {
            let id = rand::random();
            set_id(&mut tx, &id).await?;
            id
        };
        tx.commit().await?;

        Ok(ShareToken::new(id))
    }

    /// Accept a share token from other replica.
    ///
    /// If neither `accept` nor `share` was called on this repository before, the repository id
    /// from the token is assigned to this repository. Subsequent calls to `accept` will fail with
    /// [`Error::EntryExists`] unless the passed token has the same id as this repository in which
    /// case they are no-op.
    pub async fn accept(&self, token: &ShareToken) -> Result<()> {
        if set_id(self.db_pool(), &token.id).await? {
            Ok(())
        } else {
            Err(Error::EntryExists)
        }
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.shared.index.this_replica_id()
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
        branch_id: &ReplicaId,
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
        self.local_branch()
            .await
            .ensure_file_exists(path.as_ref())
            .await
    }

    /// Creates a new directory at the given path.
    pub async fn create_directory<P: AsRef<Utf8Path>>(&self, path: P) -> Result<Directory> {
        self.local_branch()
            .await
            .ensure_directory_exists(path.as_ref())
            .await
    }

    /// Removes the file or directory (must be empty) and flushes its parent directory.
    pub async fn remove_entry<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.open_directory(parent).await?;
        parent.remove_entry(name).await?;
        parent.flush().await
    }

    /// Removes the file or directory (including its content) and flushes its parent directory.
    pub async fn remove_entry_recursively<P: AsRef<Utf8Path>>(&self, path: P) -> Result<()> {
        let (parent, name) = path::decompose(path.as_ref()).ok_or(Error::OperationNotSupported)?;
        let mut parent = self.open_directory(parent).await?;
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

        let src_joint_dir = self.open_directory(src_dir_path).await?;
        let src_joint_dir_r = src_joint_dir.read().await;

        let (src_dir, src_name, src_author) = match src_joint_dir_r.lookup_unique(src_name)? {
            JointEntryRef::File(entry) => {
                let src_name = entry.name().to_string();
                let src_author = *entry.author();

                let mut file = entry.open().await?;
                file.fork().await?;

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

                (src_dir, Cow::Borrowed(src_name), *self.this_replica_id())
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
                    .incremented(*self.this_replica_id())
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

    /// Returns the local branch
    pub async fn local_branch(&self) -> Branch {
        self.shared.local_branch().await
    }

    /// Return the branch with the specified id.
    pub async fn branch(&self, id: &ReplicaId) -> Option<Branch> {
        self.shared.branch(id).await
    }

    /// Returns all branches
    pub async fn branches(&self) -> Vec<Branch> {
        self.shared.branches().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub(crate) fn subscribe(&self) -> Subscription {
        self.shared.index.subscribe()
    }

    // Opens the root directory across all branches as JointDirectory.
    async fn joint_root(&self) -> Result<JointDirectory> {
        let branches = self.branches().await;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = if branch.id() == self.this_replica_id() {
                branch.open_or_create_root().await.map_err(|error| {
                    log::error!(
                        "failed to open root directory on the local branch: {:?}",
                        error
                    );
                    error
                })?
            } else {
                match branch.open_root(self.local_branch().await).await {
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

        Ok(JointDirectory::new(dirs).await)
    }

    pub(crate) fn index(&self) -> &Index {
        &self.shared.index
    }

    fn db_pool(&self) -> &db::Pool {
        &self.index().pool
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        print.display(&"Repository");
        let branches = self.shared.branches.lock().await;
        for (replica_id, branch) in &*branches {
            let print = print.indent();
            let local = if replica_id == self.this_replica_id() {
                " (local)"
            } else {
                ""
            };
            print.display(&format_args!(
                "Branch ID: {:?}{}, root block ID:{:?}",
                replica_id,
                local,
                branch.root_block_id().await
            ));
            let print = print.indent();
            print.display(&format_args!(
                "/, vv: {:?}",
                branch.data().root_version_vector().await
            ));
            branch.debug_print(print.indent()).await;
        }
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        self.shared.index.close()
    }
}

/// Opens or creates the repository database.
pub(crate) async fn open_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open(store).await?;

    block::init(&pool).await?;
    index::init(&pool).await?;
    store::init(&pool).await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(pool)
}

async fn get_id(db: impl db::Executor<'_>) -> Result<Option<RepositoryId>> {
    Ok(sqlx::query("SELECT value FROM metadata WHERE name = ?")
        .bind(metadata::ID)
        .map(|row| row.get(0))
        .fetch_optional(db)
        .await?)
}

async fn set_id(db: impl db::Executor<'_>, id: &RepositoryId) -> Result<bool> {
    Ok(
        sqlx::query("INSERT INTO metadata(name, value) VALUES (?, ?) ON CONFLICT DO NOTHING")
            .bind(metadata::ID)
            .bind(id)
            .execute(db)
            .await?
            .rows_affected()
            > 0,
    )
}

// Metadata keys
mod metadata {
    pub(super) const ID: &[u8] = b"id";
}

struct Shared {
    index: Index,
    cryptor: Cryptor,
    // Cache for `Branch` instances to make them persistent over the lifetime of the program.
    branches: Mutex<HashMap<ReplicaId, Branch>>,
}

impl Shared {
    pub async fn local_branch(&self) -> Branch {
        self.inflate(self.index.branches().await.local()).await
    }

    pub async fn branch(&self, id: &ReplicaId) -> Option<Branch> {
        Some(self.inflate(self.index.branches().await.get(id)?).await)
    }

    pub async fn branches(&self) -> Vec<Branch> {
        future::join_all(
            self.index
                .branches()
                .await
                .all()
                .map(|data| self.inflate(data)),
        )
        .await
    }

    // Create `Branch` wrapping the given `data`, reusing a previously cached one if it exists,
    // and putting it into the cache it it does not.
    async fn inflate(&self, data: &Arc<BranchData>) -> Branch {
        self.branches
            .lock()
            .await
            .entry(*data.id())
            .or_insert_with(|| {
                Branch::new(self.index.pool.clone(), data.clone(), self.cryptor.clone())
            })
            .clone()
    }
}

// The merge algorithm.
struct Merger {
    shared: Arc<Shared>,
    tasks: FuturesUnordered<ScopedJoinHandle<ReplicaId>>,
    states: HashMap<ReplicaId, MergeState>,
}

impl Merger {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            tasks: FuturesUnordered::new(),
            states: HashMap::new(),
        }
    }

    async fn run(mut self) {
        let mut rx = self.shared.index.subscribe();

        loop {
            select! {
                branch_id = rx.recv() => {
                    if let Some(branch_id) = branch_id {
                        self.handle_branch_changed(branch_id).await
                    } else {
                        break;
                    }
                }
                branch_id = self.tasks.next(), if !self.tasks.is_empty() => {
                    if let Some(Ok(branch_id)) = branch_id {
                        self.handle_task_finished(branch_id).await
                    }
                }
            }
        }
    }

    async fn handle_branch_changed(&mut self, branch_id: ReplicaId) {
        if branch_id == *self.shared.index.this_replica_id() {
            // local branch change - ignore.
            return;
        }

        if let Some(state) = self.states.get_mut(&branch_id) {
            // Merge of this branch is already ongoing - schedule to run it again after it's
            // finished.
            *state = MergeState::Pending;
            return;
        }

        self.spawn_task(branch_id).await
    }

    async fn handle_task_finished(&mut self, branch_id: ReplicaId) {
        if let Some(MergeState::Pending) = self.states.remove(&branch_id) {
            self.spawn_task(branch_id).await
        }
    }

    async fn spawn_task(&mut self, remote_id: ReplicaId) {
        let remote = if let Some(remote) = self.shared.branch(&remote_id).await {
            remote
        } else {
            // branch removed in the meantime - ignore.
            return;
        };

        let local = self.shared.local_branch().await;

        let handle = scoped_task::spawn(async move {
            if *local.data().root_version_vector().await
                > *remote.data().root_version_vector().await
            {
                log::debug!(
                    "merge with branch {:?} suppressed - local branch already up to date",
                    remote_id
                );
                return remote_id;
            }

            log::debug!("merge with branch {:?} started", remote_id);

            match merge_branches(local, remote).await {
                Ok(()) => {
                    log::info!("merge with branch {:?} complete", remote_id)
                }
                Err(error) => {
                    // `EntryNotFound` most likely means the remote snapshot is not fully
                    // downloaded yet and `BlockNotFound` means that a block is not downloaded yet.
                    // Both error are harmless because the merge will be attempted again on the
                    // next change notification. We reduce the log severity for them to avoid log
                    // spam.
                    let level = if matches!(error, Error::EntryNotFound | Error::BlockNotFound(_)) {
                        Level::Trace
                    } else {
                        Level::Error
                    };

                    log::log!(
                        level,
                        "merge with branch {:?} failed: {}",
                        remote_id,
                        error.verbose()
                    )
                }
            }

            remote_id
        });

        self.tasks.push(handle);
        self.states.insert(remote_id, MergeState::Ongoing);
    }
}

enum MergeState {
    // A merge is ongoing
    Ongoing,
    // A merge is ongoing and another one was already scheduled
    Pending,
}

async fn merge_branches(local: Branch, remote: Branch) -> Result<()> {
    let remote_root = remote.open_root(local.clone()).await?;
    JointDirectory::new(iter::once(remote_root))
        .await
        .merge()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, index::RootNode};
    use assert_matches::assert_matches;

    #[tokio::test(flavor = "multi_thread")]
    async fn root_directory_always_exists() {
        let replica_id = rand::random();
        let repo = Repository::open(&db::Store::Memory, replica_id, Cryptor::Null, false)
            .await
            .unwrap();
        let _ = repo.open_directory("/").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge() {
        let local_id = rand::random();
        let repo = Repository::open(&db::Store::Memory, local_id, Cryptor::Null, true)
            .await
            .unwrap();

        // Add another branch to the index. Eventually there might be a more high-level API for
        // this but for now we have to resort to this.
        let remote_id = rand::random();
        let remote_node = RootNode::load_latest_or_create(&repo.index().pool, &remote_id)
            .await
            .unwrap();
        repo.index()
            .update_remote_branch(remote_id, remote_node)
            .await
            .unwrap();

        let remote_branch = repo.branch(&remote_id).await.unwrap();
        let remote_root = remote_branch.open_or_create_root().await.unwrap();

        let local_branch = repo.local_branch().await;
        let local_root = local_branch.open_or_create_root().await.unwrap();

        let mut file = remote_root
            .create_file("test.txt".to_owned())
            .await
            .unwrap();
        file.write(b"hello").await.unwrap();
        file.flush().await.unwrap();

        let mut rx = local_branch.data().subscribe();

        loop {
            match local_root
                .read()
                .await
                .lookup_version("test.txt", &remote_id)
            {
                Ok(entry) => {
                    let content = entry
                        .file()
                        .unwrap()
                        .open()
                        .await
                        .unwrap()
                        .read_to_end()
                        .await
                        .unwrap();
                    assert_eq!(content, b"hello");
                    break;
                }
                Err(Error::EntryNotFound) => (),
                Err(error) => panic!("unexpected error: {:?}", error),
            }

            rx.changed().await.unwrap()
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recreate_previously_deleted_file() {
        let local_id = rand::random();
        let repo = Repository::open(&db::Store::Memory, local_id, Cryptor::Null, false)
            .await
            .unwrap();

        // Create file
        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write(b"foo").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        // Read it back and check the content
        let content = read_file(&repo, "test.txt").await;
        assert_eq!(content, b"foo");

        // Delete it and assert it's gone
        repo.remove_entry("test.txt").await.unwrap();
        assert_matches!(repo.open_file("test.txt").await, Err(Error::EntryNotFound));

        // Create a file with the same name but different content
        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write(b"bar").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        // Read it back and check the content
        let content = read_file(&repo, "test.txt").await;
        assert_eq!(content, b"bar");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recreate_previously_deleted_directory() {
        let local_id = rand::random();
        let repo = Repository::open(&db::Store::Memory, local_id, Cryptor::Null, false)
            .await
            .unwrap();

        // Create dir
        repo.create_directory("test")
            .await
            .unwrap()
            .flush(None)
            .await
            .unwrap();

        // Check it exists
        assert_matches!(repo.open_directory("test").await, Ok(_));

        // Delete it and assert it's gone
        repo.remove_entry("test").await.unwrap();
        assert_matches!(repo.open_directory("test").await, Err(Error::EntryNotFound));

        // Create another directory with the same name
        repo.create_directory("test")
            .await
            .unwrap()
            .flush(None)
            .await
            .unwrap();

        // Check it exists
        assert_matches!(repo.open_directory("test").await, Ok(_))
    }

    async fn read_file(repo: &Repository, path: impl AsRef<Utf8Path>) -> Vec<u8> {
        repo.open_file(path)
            .await
            .unwrap()
            .read_to_end()
            .await
            .unwrap()
    }
}
