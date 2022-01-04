mod id;

pub use self::id::RepositoryId;

use crate::{
    access_control::{AccessSecrets, MasterSecret},
    block,
    branch::Branch,
    crypto::sign::{self, PublicKey},
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
use futures_util::{future, stream::FuturesUnordered, StreamExt};
use log::Level;
use rand::{rngs::OsRng, Rng};
use std::{
    collections::{hash_map::Entry, HashMap},
    iter,
    sync::Arc,
};
use tokio::{select, sync::Mutex};

pub struct Repository {
    shared: Arc<Shared>,
    _merge_handle: Option<ScopedJoinHandle<()>>,
}

impl Repository {
    /// Creates a new repository.
    pub async fn create(
        store: &db::Store,
        _this_replica_id: ReplicaId,
        master_secret: MasterSecret,
        access_secrets: AccessSecrets,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = create_db(store).await?;
        let mut tx = pool.begin().await?;

        let master_key = metadata::secret_to_key(master_secret, &mut tx).await?;

        // TODO: we should be storing the SK in the db, not the PK.
        let this_writer_id: sign::SecretKey = OsRng.gen();
        let this_writer_id = PublicKey::from(&this_writer_id);
        metadata::set_writer_id(&this_writer_id, &master_key, &mut tx).await?;

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
        _this_replica_id: ReplicaId,
        master_secret: Option<MasterSecret>,
        enable_merger: bool,
    ) -> Result<Self> {
        let pool = db::open(store).await?;

        let master_key = if let Some(master_secret) = master_secret {
            Some(metadata::secret_to_key(master_secret, &pool).await?)
        } else {
            None
        };

        let this_writer_id = metadata::get_writer_id(master_key.as_ref(), &pool).await?;

        let access_secrets = if let Some(master_key) = master_key {
            metadata::get_access_secrets(&master_key, &pool).await?
        } else {
            let id = metadata::get_repository_id(&pool).await?;
            AccessSecrets::Blind { id }
        };

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
        let local_branch = self.local_branch().await?;

        use std::borrow::Cow;

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
                    .open(MissingVersionStrategy::Skip, &local_branch)
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
                &local_branch,
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
        let local_branch = self.local_branch().await?;
        let branches = self.shared.branches().await?;
        let mut dirs = Vec::with_capacity(branches.len());

        for branch in branches {
            let dir = if branch.id() == local_branch.id() {
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
}

impl Drop for Repository {
    fn drop(&mut self) {
        self.shared.index.close()
    }
}

/// Creates the repository database.
pub(crate) async fn create_db(store: &db::Store) -> Result<db::Pool> {
    let pool = db::open_or_create(store).await?;

    block::init(&pool).await?;
    index::init(&pool).await?;
    store::init(&pool).await?;
    metadata::init(&pool).await?;

    Ok(pool)
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
                let branch = Branch::new(self.index.pool.clone(), data.clone(), keys);
                entry.insert(branch.clone());
                Ok(branch)
            }
        }
    }
}

// The merge algorithm.
struct Merger {
    shared: Arc<Shared>,
    local_branch: Branch,
    tasks: FuturesUnordered<ScopedJoinHandle<PublicKey>>,
    states: HashMap<PublicKey, MergeState>,
}

impl Merger {
    fn new(shared: Arc<Shared>, local_branch: Branch) -> Self {
        Self {
            shared,
            local_branch,
            tasks: FuturesUnordered::new(),
            states: HashMap::new(),
        }
    }

    async fn run(mut self) {
        let mut rx = self.shared.index.subscribe();

        loop {
            select! {
                branch_id = rx.recv() => {
                    if let Ok(branch_id) = branch_id {
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

    async fn handle_branch_changed(&mut self, branch_id: PublicKey) {
        if branch_id == *self.local_branch.id() {
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

    async fn handle_task_finished(&mut self, branch_id: PublicKey) {
        if let Some(MergeState::Pending) = self.states.remove(&branch_id) {
            self.spawn_task(branch_id).await
        }
    }

    async fn spawn_task(&mut self, remote_id: PublicKey) {
        let local = self.local_branch.clone();

        let remote = if let Ok(remote) = self.shared.branch(&remote_id).await {
            remote
        } else {
            // branch removed in the meantime - ignore.
            return;
        };

        let handle = scoped_task::spawn(async move {
            if local.data().root().await.versions > remote.data().root().await.versions {
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
    let remote_root = remote.open_root().await?;
    JointDirectory::new(local.clone(), iter::once(remote_root))
        .merge()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::SeekFrom;

    use super::*;
    use crate::{db, index::RootNode};
    use assert_matches::assert_matches;
    use tokio::time::{sleep, Duration};

    #[tokio::test(flavor = "multi_thread")]
    async fn root_directory_always_exists() {
        let writer_id = rand::random();
        let repo = Repository::create(
            &db::Store::Memory,
            writer_id,
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
        .await
        .unwrap();
        let _ = repo.open_directory("/").await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge() {
        let local_id = rand::random();
        let repo = Repository::create(
            &db::Store::Memory,
            local_id,
            MasterSecret::random(),
            AccessSecrets::random_write(),
            true,
        )
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

        let remote_branch = repo.shared.branch(&remote_id).await.unwrap();
        let remote_root = remote_branch.open_or_create_root().await.unwrap();

        let local_branch = repo.local_branch().await.unwrap();
        let local_root = local_branch.open_or_create_root().await.unwrap();

        let mut file = remote_root
            .create_file("test.txt".to_owned(), &remote_branch)
            .await
            .unwrap();
        file.write(b"hello", &remote_branch).await.unwrap();
        file.flush().await.unwrap();

        let mut rx = repo.subscribe();

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

            rx.recv().await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recreate_previously_deleted_file() {
        let local_id = rand::random();
        let repo = Repository::create(
            &db::Store::Memory,
            local_id,
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
        .await
        .unwrap();

        let local_branch = repo.local_branch().await.unwrap();

        // Create file
        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write(b"foo", &local_branch).await.unwrap();
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
        file.write(b"bar", &local_branch).await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        // Read it back and check the content
        let content = read_file(&repo, "test.txt").await;
        assert_eq!(content, b"bar");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recreate_previously_deleted_directory() {
        let local_id = rand::random();
        let repo = Repository::create(
            &db::Store::Memory,
            local_id,
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
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

    // This one used to deadlock
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_read_and_create_dir() {
        let writer_id = rand::random();
        let repo = Repository::create(
            &db::Store::Memory,
            writer_id,
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
        .await
        .unwrap();

        let path = "/dir";
        let repo = Arc::new(repo);

        let _watch_dog = scoped_task::spawn(async {
            sleep(Duration::from_millis(5 * 1000)).await;
            panic!("timed out");
        });

        // The deadlock here happened because the reader lock when opening the directory is
        // acquired in the opposite order to the writer lock acqurired from flushing. I.e. the
        // reader lock acquires `/` and then `/dir`, but flushing acquires `/dir` first and
        // then `/`.
        let create_dir = scoped_task::spawn({
            let repo = repo.clone();
            async move {
                let dir = repo.create_directory(path).await.unwrap();
                dir.flush(None).await.unwrap();
            }
        });

        let open_dir = scoped_task::spawn({
            let repo = repo.clone();
            async move {
                for _ in 1..10 {
                    if let Ok(dir) = repo.open_directory(path).await {
                        dir.read().await.entries().count();
                        return;
                    }
                    // Sometimes opening the directory may outrace its creation,
                    // so continue to retry again.
                }
                panic!("Failed to open the directory after multiple attempts");
            }
        });

        create_dir.await.unwrap();
        open_dir.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn append_to_file() {
        let repo = Repository::create(
            &db::Store::Memory,
            rand::random(),
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
        .await
        .unwrap();

        let local_branch = repo.local_branch().await.unwrap();
        let mut file = repo.create_file("foo.txt").await.unwrap();
        file.write(b"foo", &local_branch).await.unwrap();
        file.flush().await.unwrap();

        let mut file = repo.open_file("foo.txt").await.unwrap();
        file.seek(SeekFrom::End(0)).await.unwrap();
        file.write(b"bar", &local_branch).await.unwrap();
        file.flush().await.unwrap();

        let mut file = repo.open_file("foo.txt").await.unwrap();
        let content = file.read_to_end().await.unwrap();
        assert_eq!(content, b"foobar");
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
